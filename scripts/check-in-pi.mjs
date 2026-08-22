#!/usr/bin/env node
/**
 * Drive a real `pi --mode rpc` process, with every REactor extension loaded
 * the way pi actually loads them, and watch for `extension_error` events.
 *
 * `tests/extensions/*.test.mjs` runs through pi's own *loader* against the
 * real CLI (ADR-0012) -- that catches most things, but it is still a mock
 * host underneath: `ctx`/`pi` are fakes this repo maintains, and until
 * `harness.mjs` grew a `guard` (see ADR-0016's fix for `/reactor-toolbox
 * off`'s stale-ctx crash), nothing simulated what happens to a captured
 * `ctx` after `await ctx.reload()`. This script closes that gap by not
 * mocking the host at all: it is the actual pi runtime, given the actual
 * extension files, over the documented RPC protocol (`docs/rpc.md`). A
 * `/name` prompt runs the extension command directly -- no LLM call, no API
 * key needed -- and a thrown error surfaces as `extension_error` on stdout
 * instead of a caught assertion.
 *
 * Usage:
 *   scripts/check-in-pi.mjs                       # the default smoke set
 *   scripts/check-in-pi.mjs "/reactor-status mute adb" "/reactor-status"
 *
 * Every run gets a fresh, throwaway PI_CODING_AGENT_DIR, REACTOR_CONFIG_DIR
 * (so it falls back to this repo's own shipped tools.toml/toolsets.toml, per
 * bin/reactor's own fallback -- no seeding needed) and cwd, isolated from
 * whatever is on the machine running this. `bin/reactor` from this checkout
 * goes on PATH ahead of anything else there.
 *
 * Exits non-zero, and prints every `extension_error` in full, if any command
 * made an extension throw. Otherwise exits 0. Not part of `npm test`: it
 * spawns a real pi process per run (~2-3s for the default set) and needs
 * `pi` on PATH, the same reasoning that keeps `verify-recipes.py` out of the
 * fast suite.
 */

import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const EXTENSIONS = ["tool-registry", "selector", "status", "scenario", "rolling-context", "context-editor"];

/**
 * Covers the command each extension registers, including the two ordering
 * fixes this script exists because of: `/reactor-toolbox off` (had the
 * crash) and `/reactor refresh` (had the same bug, latent). `/reactor-tools`
 * degrades to "point at the CLI" in RPC mode (`ctx.ui.custom` is `undefined`
 * there per docs/rpc.md) rather than opening -- still worth a line, since
 * that degrade path is itself extension code that can throw. The
 * `rolling-context/` tail turns it on, exercises `/goal`/`/frame` and a real
 * `context` fade, then off again -- unrelated to the toolbox and worth
 * checking on its own account (ADR-0019). `context-editor/`'s two commands
 * degrade the same way `/reactor-tools` does over RPC -- both its landscape
 * overlay and its external-editor launch need a real terminal (`ctx.mode
 * === "tui"`), so they just notify instead, which is still extension code
 * worth exercising for a throw.
 */
const DEFAULT_COMMANDS = [
	"/reactor",
	"/reactor refresh",
	"/reactor-toolbox",
	"/reactor-toolbox off",
	"/reactor-toolbox on",
	"/reactor-tools",
	"/reactor-status",
	"/reactor-status mute adb",
	"/reactor-status unmute adb",
	"/reactor-scenario list",
	"/reactor-scenario start triage",
	"/reactor-scenario status",
	"/reactor-scenario next smoke test",
	"/reactor-scenario stop",
	"/rolling on",
	"/goal check the crash",
	"/frame",
	"/rolling off",
	"/context-editor",
	"/context-editor manual",
];

const commands = process.argv.slice(2);
const toRun = commands.length ? commands : DEFAULT_COMMANDS;

const agentDir = fs.mkdtempSync(path.join(os.tmpdir(), "reactor-pi-check-agent-"));
const reactorDir = fs.mkdtempSync(path.join(os.tmpdir(), "reactor-pi-check-config-"));
const cwd = fs.mkdtempSync(path.join(os.tmpdir(), "reactor-pi-check-cwd-"));

const env = {
	...process.env,
	PI_CODING_AGENT_DIR: agentDir,
	REACTOR_CONFIG_DIR: reactorDir,
	PATH: `${path.join(REPO_ROOT, "bin")}${path.delimiter}${process.env.PATH ?? ""}`,
};

function cleanup() {
	for (const dir of [agentDir, reactorDir, cwd]) fs.rmSync(dir, { recursive: true, force: true });
}

const child = spawn(
	"pi",
	[
		"--mode",
		"rpc",
		"--no-session",
		"--no-context-files",
		...EXTENSIONS.flatMap((n) => ["-e", path.join(REPO_ROOT, "extensions", n, "index.ts")]),
	],
	{ cwd, env, stdio: ["pipe", "pipe", "inherit"] },
);

const events = [];
let buf = "";
child.stdout.on("data", (chunk) => {
	buf += chunk.toString();
	let idx;
	while ((idx = buf.indexOf("\n")) !== -1) {
		const line = buf.slice(0, idx);
		buf = buf.slice(idx + 1);
		if (!line.trim()) continue;
		try {
			events.push(JSON.parse(line));
		} catch {
			// A non-JSON line from pi itself (a startup banner, say) is not
			// this script's business; only extension_error is.
		}
	}
});

let nextId = 0;

/**
 * Send one command and wait for its own `response`, by id, before returning
 * -- real usage (a person, or an agent working through one command result
 * before issuing the next) never has two extension commands in flight on the
 * same extension instance at once. Sending the next command before this one
 * settles can race a `ctx.reload()` from either command against the other's
 * still-suspended handler, which pi's own runtime treats as a *different*
 * bug (an extension instance reloaded out from under a concurrent caller)
 * from the ordering-within-one-handler bug this script was written for; it
 * is a real risk on its own, just not the one `-c` overlap testing wants.
 */
async function sendAndWait(message, timeoutMs = 15_000) {
	const id = String(nextId++);
	const deadline = Date.now() + timeoutMs;
	child.stdin.write(`${JSON.stringify({ type: "prompt", message, id })}\n`);
	while (!events.some((e) => e.type === "response" && e.id === id)) {
		if (Date.now() > deadline) throw new Error(`timed out waiting for a response to ${JSON.stringify(message)}`);
		await new Promise((resolve) => setTimeout(resolve, 20));
	}
	// The response confirms accept/dispatch, not necessarily that every
	// async continuation inside the handler (a reload, a sendMessage) has
	// settled -- a short grace period catches those without going back to a
	// blind fixed delay for the whole run.
	await new Promise((resolve) => setTimeout(resolve, 150));
}

await new Promise((resolve) => setTimeout(resolve, 800)); // let extension load settle

for (const cmd of toRun) {
	console.log(`  ${cmd}`);
	await sendAndWait(cmd);
}
await new Promise((resolve) => setTimeout(resolve, 300));
child.kill();
cleanup();

const errors = events.filter((e) => e.type === "extension_error");
if (errors.length) {
	console.error(`\n${errors.length} extension_error event(s):\n`);
	for (const e of errors) console.error(`  [${e.extensionPath}] ${e.error}\n`);
	process.exit(1);
}
console.log(`\nok -- ${toRun.length} command(s), no extension_error`);
