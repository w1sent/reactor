#!/usr/bin/env node
/**
 * Verify the *installed* REactor, the way a real user's pi run loads it.
 *
 * `scripts/check-in-pi.mjs` loads every extension with explicit `-e` flags
 * pointed at this checkout -- it exercises the files, but not pi's package
 * discovery. This script does the opposite: it runs `pi --mode rpc` with no
 * `-e` at all, against a throwaway agent dir whose `settings.json` carries
 * this repo as a package exactly the way `pi install <path>` records it, and
 * proves the package scan picks up each extension by asserting on the UI
 * events its commands emit.
 *
 * The assertions are positive, not just "no crash": an unknown command in RPC
 * mode falls through to the model and still answers `success:true`, so a bare
 * response proves nothing. What only a registered extension command can do is
 * emit its own `extension_ui_request` (the `ctx.ui.notify`/`setStatus` each
 * handler here calls), so each command is checked for its event and a bogus
 * command runs first as the negative control. Nothing here needs an API key.
 *
 * Usage:  node scripts/install-check.mjs
 * Exits non-zero if any expected UI event is missing or any `extension_error`
 * appears. Spawns one real pi process (~3s); not part of `npm test`, same
 * reasoning as `check-in-pi.mjs`.
 */

import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const REPO_ROOT = path.resolve(import.meta.dirname, "..");

const cwd = fs.mkdtempSync(path.join(os.tmpdir(), "reactor-install-check-cwd-"));
const agentDir = fs.mkdtempSync(path.join(os.tmpdir(), "reactor-install-check-agent-"));
fs.writeFileSync(
	path.join(agentDir, "settings.json"),
	JSON.stringify({ packages: [REPO_ROOT] }),
);

const child = spawn("pi", ["--mode", "rpc", "--no-session", "--no-context-files"], {
	cwd,
	env: { ...process.env, PI_CODING_AGENT_DIR: agentDir },
	stdio: ["pipe", "pipe", "inherit"],
});

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
			/* non-JSON startup lines are not ours */
		}
	}
});

let nextId = 0;
async function send(message) {
	const id = String(nextId++);
	const before = events.length;
	child.stdin.write(`${JSON.stringify({ type: "prompt", message, id })}\n`);
	const deadline = Date.now() + 15_000;
	while (!events.some((e) => e.type === "response" && e.id === id)) {
		if (Date.now() > deadline) throw new Error(`timed out waiting for a response to ${message}`);
		await new Promise((r) => setTimeout(r, 20));
	}
	await new Promise((r) => setTimeout(r, 250));
	return events.slice(events.findIndex((e) => e.type === "response" && e.id === id) - (events.length));
}

/**
 * The UI event each command must produce. `notify`/`setStatus` carry their
 * payload under different RPC field names per pi build, so the check is for
 * the method + key, not the text -- the negative control is what rules out a
 * silent fall-through.
 */
const CASES = [
	{ message: "/definitely-not-a-command", expect: null, why: "negative control: a bogus command must emit no extension notify" },
	{ message: "/goal install-check", expect: { method: "notify" }, why: "goal-setting/ is registered (ADR-0024)" },
	{ message: "/history-tools off", expect: { method: "notify" }, why: "history-tools/ is registered (ADR-0024)" },
	{ message: "/auto-continue", expect: { method: "notify" }, why: "auto-continue/ is registered (ADR-0025)" },
	{ message: "/rolling on", expect: { method: "setStatus", statusKey: "rolling-context" }, why: "the fade is registered and toggles" },
	{ message: "/rolling off", expect: { method: "setStatus", statusKey: "rolling-context" }, why: "the fade toggles off cleanly" },
];

await new Promise((resolve) => setTimeout(resolve, 1200)); // let extension load settle

let failed = false;
for (const { message, expect, why } of CASES) {
	const id = String(nextId++);
	const before = events.length;
	child.stdin.write(`${JSON.stringify({ type: "prompt", message, id })}\n`);
	const deadline = Date.now() + 15_000;
	while (!events.some((e) => e.type === "response" && e.id === id)) {
		if (Date.now() > deadline) throw new Error(`timed out waiting for a response to ${message}`);
		await new Promise((r) => setTimeout(r, 20));
	}
	await new Promise((r) => setTimeout(r, 250));
	const ui = events.slice(before).filter((e) => e.type === "extension_ui_request");
	const hit =
		expect === null
			? !ui.some((e) => e.method === "notify")
			: ui.some((e) => e.method === expect.method && (expect.statusKey === undefined || e.statusKey === expect.statusKey));
	console.log(`  ${message} -> ${hit ? "ok" : "MISSING EXPECTED EVENT"}`);
	if (!hit) {
		failed = true;
		console.error(`    ! ${why}`);
	}
}
await new Promise((r) => setTimeout(r, 300));
child.kill();

const errors = events.filter((e) => e.type === "extension_error");
for (const e of errors) console.error(`  [${e.extensionPath}] ${e.error}`);
fs.rmSync(cwd, { recursive: true, force: true });
fs.rmSync(agentDir, { recursive: true, force: true });

if (failed || errors.length) {
	console.error(`\nFAIL -- ${errors.length} extension_error(s), check the case output above`);
	process.exit(1);
}
console.log("\nok -- the installed package discovered and loaded the split extensions (ADR-0024)");