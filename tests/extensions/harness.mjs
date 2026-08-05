/**
 * Test harness for the pi extensions.
 *
 * The extension under test is loaded by *pi's own loader*, so `pi.exec`,
 * `registerCommand` and the rest are pi's implementations and the imports
 * resolve through pi's alias map -- the same module instances the running agent
 * gets (docs/adr/0012). Only the host side is faked: the context, its `ui`, and
 * the TUI/theme/done triple that `ctx.ui.custom` hands a component.
 *
 * `reactor` is not faked. A shim on PATH execs the real `bin/reactor` against a
 * throwaway REACTOR_CONFIG_DIR, or reproduces one named failure mode.
 */

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const REPO_ROOT = path.resolve(import.meta.dirname, "../..");

// ---------------------------------------------------------------------------
// Locating pi
// ---------------------------------------------------------------------------

/** pi's `dist/`, or undefined. Found through PATH, so it follows the symlink. */
function findPiDist() {
	if (process.env.REACTOR_PI_DIST) return process.env.REACTOR_PI_DIST;
	let resolved;
	try {
		resolved = execFileSync("sh", ["-c", "command -v pi"], { encoding: "utf8" }).trim();
	} catch {
		return undefined;
	}
	if (!resolved) return undefined;
	const dist = path.dirname(fs.realpathSync(resolved));
	return fs.existsSync(path.join(dist, "core/extensions/loader.js")) ? dist : undefined;
}

const PI_DIST = findPiDist();

/**
 * Spread into a `test()` options object. pi missing is not a REactor defect, so
 * these skip rather than fail -- but say why, so a skip is never mistaken for a
 * pass.
 */
export const needsPi = PI_DIST
	? {}
	: { skip: "pi is not installed (no `pi` on PATH, and REACTOR_PI_DIST is unset)" };

/** pi-tui, resolved exactly as pi's alias map resolves it for extensions. */
export const piTui = PI_DIST
	? await import(
			pathToFileURL(
				createRequire(path.join(PI_DIST, "core/extensions/loader.js")).resolve(
					"@earendil-works/pi-tui",
				),
			).href
		)
	: {};

// ---------------------------------------------------------------------------
// The fixture catalogue
// ---------------------------------------------------------------------------

/**
 * Three tools whose presence is the same on every machine: `sh` is on any POSIX
 * box, and nothing is named `reactor-absent-by-design`. Naming a real RE tool
 * here would make the suite pass or fail on what the developer happens to have
 * installed.
 */
export const FIXTURE_TOOLS = `
version = 1

[platform]
prefer = ["pacman", "brew"]

[platform.manager]
pacman = { binary = "pacman", os = "linux", sudo = true }
brew   = { binary = "brew", os = "darwin" }

[probe]
timeout = 5.0

[tool.alpha]
name   = "Alpha"
desc   = "unpacks alpha containers"
invoke = "sh"
detect = { binary = "sh" }
tags   = ["static", "shell"]
skill  = { source = "https://example.invalid/skills.git", path = "alpha" }

[tool.alpha.install]
pacman = "pacman -S alpha"
manual = "https://example.invalid/alpha"

[tool.beta]
name   = "Beta"
desc   = "traces beta processes at runtime"
invoke = "reactor-absent-by-design"
detect = { binary = "reactor-absent-by-design" }
tags   = ["dynamic"]

[tool.gamma]
name    = "Gamma"
desc    = "diffs gamma firmware images"
invoke  = "ls"
detect  = { binary = "ls" }
# Long on purpose: a version cut to fit reads as a different version, which is
# a bug the selector had. Nothing real is guaranteed to print a long one.
version = ["sh", "-c", "echo 'Gamma 10.1.1.8388'"]
tags    = ["static"]
`;

/** What `version` above resolves to, once the CLI has parsed the line. */
export const GAMMA_VERSION = "10.1.1.8388";

export const FIXTURE_TOOLSETS = `
version = 1

[toolset.everything]
desc = "the whole catalogue"
all  = true

[toolset.static]
desc = "static analysis only"
tags = ["static"]

[toolset.pair]
desc  = "alpha and beta"
tools = ["alpha", "beta"]
`;

/** How the PATH shim behaves. `ok` is the real CLI. */
export const CLI_OK = "ok";
/** Exit 1 with empty stdout -- what pi's exec reports for ENOENT too. */
export const CLI_MISSING = "missing";
/** Exit 0 with output that is not JSON. */
export const CLI_GARBAGE = "garbage";
/** Valid JSON carrying an `error` field, which the CLI emits for bad input. */
export const CLI_ERROR = "error";

/**
 * A temporary REACTOR_CONFIG_DIR plus a `reactor` shim on PATH.
 *
 * Both are installed into `process.env`, because the extension calls
 * `pi.exec("reactor", ...)` without an env of its own and so inherits ours.
 * That is also the lever for `mode`: the shim reads REACTOR_TEST_MODE at spawn
 * time, so a test can change the CLI's behaviour between calls.
 */
export class Fixture {
	constructor({ tools = FIXTURE_TOOLS, toolsets = FIXTURE_TOOLSETS, state, skills = [] } = {}) {
		this.dir = fs.mkdtempSync(path.join(os.tmpdir(), "reactor-ext-"));
		fs.writeFileSync(path.join(this.dir, "tools.toml"), tools);
		fs.writeFileSync(path.join(this.dir, "toolsets.toml"), toolsets);
		if (state) this.writeState(state);
		for (const id of skills) {
			const d = path.join(this.dir, "skills", id);
			fs.mkdirSync(d, { recursive: true });
			fs.writeFileSync(
				path.join(d, "SKILL.md"),
				`---\nname: ${id}\ndescription: fixture skill\n---\n\nbody line one\nbody line two\n`,
			);
			fs.writeFileSync(
				path.join(d, ".reactor-skill.json"),
				JSON.stringify({ commit: "0".repeat(40), fetched_at: "2020-01-01T00:00:00Z" }),
			);
		}

		this.bin = path.join(this.dir, "bin");
		fs.mkdirSync(this.bin);
		const shim = path.join(this.bin, "reactor");
		fs.writeFileSync(
			shim,
			[
				"#!/bin/sh",
				'case "${REACTOR_TEST_MODE:-ok}" in',
				"  missing) exit 1 ;;",
				"  garbage) printf 'this is not json\\n' ;;",
				`  error)   printf '{"error":"probe exploded"}\\n' ;;`,
				`  *) exec ${JSON.stringify(path.join(REPO_ROOT, "bin", "reactor"))} "$@" ;;`,
				"esac",
				"",
			].join("\n"),
		);
		fs.chmodSync(shim, 0o755);

		this._saved = { PATH: process.env.PATH, dir: process.env.REACTOR_CONFIG_DIR };
		process.env.PATH = `${this.bin}${path.delimiter}${process.env.PATH}`;
		process.env.REACTOR_CONFIG_DIR = this.dir;
		this.mode = CLI_OK;
	}

	/** Which CLI behaviour the next spawn gets. */
	set mode(value) {
		process.env.REACTOR_TEST_MODE = value;
	}

	writeState(state) {
		fs.writeFileSync(path.join(this.dir, "state.json"), `${JSON.stringify(state, null, 2)}\n`);
	}

	/** Raw bytes, so a test can assert a write left the file *identical*. */
	readStateBytes() {
		try {
			return fs.readFileSync(path.join(this.dir, "state.json"), "utf8");
		} catch {
			return undefined;
		}
	}

	readState() {
		const raw = this.readStateBytes();
		return raw === undefined ? undefined : JSON.parse(raw);
	}

	cleanup() {
		process.env.PATH = this._saved.PATH;
		if (this._saved.dir === undefined) delete process.env.REACTOR_CONFIG_DIR;
		else process.env.REACTOR_CONFIG_DIR = this._saved.dir;
		delete process.env.REACTOR_TEST_MODE;
		fs.rmSync(this.dir, { recursive: true, force: true });
	}
}

/** A fixture that cleans up even when the body throws. */
export async function withFixture(options, body) {
	const fixture = new Fixture(options);
	try {
		return await body(fixture);
	} finally {
		fixture.cleanup();
	}
}

// ---------------------------------------------------------------------------
// Loading the extension
// ---------------------------------------------------------------------------

/**
 * Load one extension the way pi does. Returns pi's own Extension record --
 * `handlers`, `commands`, `entryRenderers` -- plus the runtime side-effects the
 * extension produced, which are the only things it can do that a test cannot
 * see by calling it.
 */
export async function loadExtension(relativePath, fixture) {
	const loader = await import(
		pathToFileURL(path.join(PI_DIST, "core/extensions/loader.js")).href
	);
	const runtime = loader.createExtensionRuntime();
	const sent = [];
	const entries = [];
	runtime.sendMessage = (message) => sent.push(message);
	runtime.appendEntry = (customType, data) => entries.push({ customType, data });

	// Cleared because pi memoises by path, and two tests loading the same file
	// must not share one extension's closed-over `last` payload.
	loader.clearExtensionCache();

	const events = { on() {}, off() {}, emit() {} };
	const { extensions, errors } = await loader.loadExtensions(
		[path.join(REPO_ROOT, relativePath)],
		fixture.dir,
		events,
		runtime,
	);
	if (errors.length) throw new Error(errors.map((e) => e.error ?? e).join("; "));
	return { extension: extensions[0], sent, entries };
}

// ---------------------------------------------------------------------------
// Faking the host
// ---------------------------------------------------------------------------

/**
 * A theme whose every method is identity. Real themes emit ANSI, which would
 * put escape sequences in the middle of every width assertion -- and width is
 * most of what there is to assert about a list that must not overflow.
 */
export const plainTheme = {
	fg: (_color, text) => text,
	bg: (_color, text) => text,
	bold: (text) => text,
	dim: (text) => text,
	italic: (text) => text,
	underline: (text) => text,
	strikethrough: (text) => text,
	color: (_name) => (text) => text,
};

export function makeTui({ rows = 40, columns = 120 } = {}) {
	const tui = {
		renders: 0,
		terminal: { rows, columns },
		requestRender() {
			tui.renders++;
		},
	};
	return tui;
}

/**
 * A fake ExtensionContext, plus a record of everything the extension did to it.
 *
 * `ui.custom` is the interesting one: it builds the component the way pi does
 * and parks the returned promise until the component calls `done`, so a test
 * can reach in, drive `handleInput`, and then await the handler.
 */
export function makeContext(fixture, { mode = "tui", tui = makeTui() } = {}) {
	const calls = { status: [], notify: [], reloads: 0, custom: [], overlay: undefined };
	const ctx = {
		cwd: fixture.dir,
		mode,
		hasUI: mode === "tui" || mode === "rpc",
		ui: {
			setStatus: (key, value) => calls.status.push({ key, value }),
			clearStatus: (key) => calls.status.push({ key, value: undefined }),
			notify: (message, level) => calls.notify.push({ message, level }),
			custom: (factory, options) => {
				calls.custom.push(options);
				if (mode !== "tui") {
					// pi has no terminal to mount a component into here, so the
					// call never settles in production. Throwing turns "the
					// extension opened an overlay it should not have" from a
					// hung test run into a failing assertion.
					throw new Error(`ctx.ui.custom called in mode "${mode}"`);
				}
				return new Promise((resolve) => {
					const component = factory(tui, plainTheme, piTui.getKeybindings?.(), resolve);
					calls.overlay = { component, tui, options, render: (w) => component.render(w) };
				});
			},
		},
		reload: async () => {
			calls.reloads++;
		},
	};
	return { ctx, calls };
}

/**
 * Send a keystroke and wait for whatever it started.
 *
 * `handleInput` is synchronous but launches its `reactor` calls with `void`, so
 * there is nothing to await on the return. It does set `busy` before it yields,
 * though -- TypeScript's `private` is a compile-time fiction and the field is
 * plainly there at runtime -- which makes "no longer busy" the honest signal
 * that the keystroke is finished.
 */
export async function press(overlay, key) {
	overlay.component.handleInput(key);
	await waitFor(() => !overlay.component.busy, `${JSON.stringify(key)} to settle`);
}

/** Resolve once `predicate()` is truthy, or throw. For awaiting an overlay. */
export async function waitFor(predicate, what = "condition", timeoutMs = 30_000) {
	const deadline = Date.now() + timeoutMs;
	while (!predicate()) {
		if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
		await new Promise((r) => setTimeout(r, 5));
	}
	return predicate();
}
