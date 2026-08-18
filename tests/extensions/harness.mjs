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

/**
 * A catalogue whose point is its service probes: one answering with a count,
 * one refusing, one declaring a service on a tool that is not installed, and
 * one with no service at all. Every probe is a `sh -c`, so the states are the
 * same on every machine.
 */
export const SERVICE_TOOLS = `
version = 1

[probe]
timeout = 5.0

[tool.answering]
name    = "Answering"
desc    = "a service that answers"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo 'a device'; echo 'b device'"], label = "answering", count = { pattern = 'device$', noun = "device" } }

[tool.refusing]
name    = "Refusing"
desc    = "a service that is not running"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "exit 3"], label = "refusing" }

[tool.uninstalled]
name    = "Uninstalled"
desc    = "declares a service but is not here"
invoke  = "reactor-absent-by-design"
detect  = { binary = "reactor-absent-by-design" }
service = { probe = ["sh", "-c", "exit 0"], label = "uninstalled" }

[tool.plain]
name   = "Plain"
desc   = "no service at all"
invoke = "sh"
detect = { binary = "sh" }
`;

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
 * A temporary REACTOR_CONFIG_DIR plus a `reactor` shim on PATH, plus a
 * temporary pi agent dir (`PI_CODING_AGENT_DIR`) for `reactor.json`
 * (ADR-0016).
 *
 * All three are installed into `process.env`: the extension calls
 * `pi.exec("reactor", ...)` and `getAgentDir()` without an env of its own, so
 * it inherits ours. The agent dir is always overridden, even when no test
 * writes `reactor.json` into it -- otherwise a run on a machine that happens
 * to have `~/.pi/agent/reactor.json` would read that file instead of getting
 * defaults, the same isolation `REACTOR_CONFIG_DIR` already gives the
 * catalogue. `mode` is the other lever: the shim reads REACTOR_TEST_MODE at
 * spawn time, so a test can change the CLI's behaviour between calls.
 */
export class Fixture {
	constructor({
		tools = FIXTURE_TOOLS,
		toolsets = FIXTURE_TOOLSETS,
		state,
		skills = [],
		agentSettings,
		scenarios,
	} = {}) {
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

		this.agentDir = fs.mkdtempSync(path.join(os.tmpdir(), "reactor-agent-"));
		if (agentSettings !== undefined) this.writeAgentSettings(agentSettings);

		// Isolates extensions/scenario/ from this package's own shipped
		// prompts/scenarios/ (ADR-0017) the same way REACTOR_CONFIG_DIR isolates
		// the CLI from ~/.pi/reactor/ -- a test's assertions should not break
		// because someone reworded a step's prose.
		this.scenariosDir = fs.mkdtempSync(path.join(os.tmpdir(), "reactor-scenarios-"));
		if (scenarios !== undefined) this.writeScenarios(scenarios);

		this._saved = {
			PATH: process.env.PATH,
			dir: process.env.REACTOR_CONFIG_DIR,
			agentDir: process.env.PI_CODING_AGENT_DIR,
			scenariosDir: process.env.REACTOR_SCENARIOS_DIR,
		};
		process.env.PATH = `${this.bin}${path.delimiter}${process.env.PATH}`;
		process.env.REACTOR_CONFIG_DIR = this.dir;
		process.env.PI_CODING_AGENT_DIR = this.agentDir;
		process.env.REACTOR_SCENARIOS_DIR = this.scenariosDir;
		this.mode = CLI_OK;
	}

	/** Which CLI behaviour the next spawn gets. */
	set mode(value) {
		process.env.REACTOR_TEST_MODE = value;
	}

	/** `<agent dir>/reactor.json` -- `toolbox` and `hiddenServices` (ADR-0016). */
	writeAgentSettings(settings) {
		fs.writeFileSync(path.join(this.agentDir, "reactor.json"), JSON.stringify(settings));
	}

	/** What a command's write left behind, or undefined if it wrote nothing. */
	readAgentSettings() {
		try {
			return JSON.parse(fs.readFileSync(path.join(this.agentDir, "reactor.json"), "utf8"));
		} catch {
			return undefined;
		}
	}

	/**
	 * `{ "scenario-id": ["<step 1 file contents>", "<step 2 ...>", ...] }` --
	 * each string is a whole step file, frontmatter included, written as
	 * `01.md`, `02.md`, ... so filename order is step order (ADR-0017).
	 */
	writeScenarios(scenarios) {
		for (const [id, steps] of Object.entries(scenarios)) {
			const dir = path.join(this.scenariosDir, id);
			fs.mkdirSync(dir, { recursive: true });
			steps.forEach((content, i) => {
				fs.writeFileSync(path.join(dir, `${String(i + 1).padStart(2, "0")}.md`), content);
			});
		}
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
		if (this._saved.agentDir === undefined) delete process.env.PI_CODING_AGENT_DIR;
		else process.env.PI_CODING_AGENT_DIR = this._saved.agentDir;
		if (this._saved.scenariosDir === undefined) delete process.env.REACTOR_SCENARIOS_DIR;
		else process.env.REACTOR_SCENARIOS_DIR = this._saved.scenariosDir;
		delete process.env.REACTOR_TEST_MODE;
		fs.rmSync(this.dir, { recursive: true, force: true });
		fs.rmSync(this.agentDir, { recursive: true, force: true });
		fs.rmSync(this.scenariosDir, { recursive: true, force: true });
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
 * pi's own wording (`loader.js`'s `invalidate`), reused here so a test
 * failure reads exactly like the crash a user actually hits, not a
 * paraphrase of it.
 */
export const STALE_CTX_MESSAGE =
	"This extension ctx is stale after session replacement or reload. Do not use a captured pi or command ctx after ctx.reload().";

/**
 * Load one extension the way pi does. Returns pi's own Extension record --
 * `handlers`, `commands`, `entryRenderers` -- plus the runtime side-effects the
 * extension produced, which are the only things it can do that a test cannot
 * see by calling it, plus a `guard` a paired `makeContext()` can share so
 * `ctx.reload()` poisons both at once (see `makeContext`).
 */
export async function loadExtension(relativePath, fixture, { guard = { stale: false } } = {}) {
	const loader = await import(
		pathToFileURL(path.join(PI_DIST, "core/extensions/loader.js")).href
	);
	const runtime = loader.createExtensionRuntime();
	const sent = [];
	const entries = [];
	runtime.sendMessage = (message) => {
		if (guard.stale) throw new Error(STALE_CTX_MESSAGE);
		sent.push(message);
	};
	runtime.appendEntry = (customType, data) => {
		if (guard.stale) throw new Error(STALE_CTX_MESSAGE);
		entries.push({ customType, data });
	};

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
	return { extension: extensions[0], sent, entries, guard };
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
 *
 * `entries` wires `ctx.sessionManager.getEntries()` to the *same* array
 * `loadExtension()` hands back -- pass it through so a test can call
 * `pi.appendEntry` (via the extension) and then simulate a reload by reading
 * it back through `sessionManager`, the way `scenario/` restores state on
 * `session_start`. Defaults to empty, since most extensions never read it.
 *
 * `guard` -- pass `loadExtension()`'s returned `guard` here too, and
 * `ctx.reload()` poisons the *whole* `ctx`, the same way pi's own runtime
 * invalidates a captured ctx/pi after `await ctx.reload()`: any further
 * property access throws `STALE_CTX_MESSAGE`. A handler that reads or calls
 * anything on `ctx` after reloading fails the test instead of quietly
 * "working" against a mock that never actually goes stale -- this is the
 * only reason `reactor-toolbox off`'s stale-ctx crash didn't show up here
 * first. Omit `guard` for a test that doesn't touch reload at all.
 */
export function makeContext(fixture, { mode = "tui", tui = makeTui(), entries = [], guard = { stale: false } } = {}) {
	const calls = { status: [], notify: [], reloads: 0, custom: [], overlay: undefined, widgets: [] };
	const raw = {
		cwd: fixture.dir,
		mode,
		hasUI: mode === "tui" || mode === "rpc",
		sessionManager: {
			getEntries: () => entries.map((e) => ({ type: "custom", customType: e.customType, data: e.data })),
		},
		ui: {
			setStatus: (key, value) => calls.status.push({ key, value }),
			clearStatus: (key) => calls.status.push({ key, value: undefined }),
			notify: (message, level) => calls.notify.push({ message, level }),
			// Widgets take either a string array or a component factory, so the
			// fake normalises both into lines a test can read.
			setWidget: (key, content, options) => {
				const lines =
					typeof content === "function"
						? (width = 100) => content(tui, plainTheme).render(width)
						: content === undefined
							? undefined
							: () => content;
				calls.widgets.push({ key, options, lines, cleared: content === undefined });
			},
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
			guard.stale = true;
		},
	};
	const ctx = new Proxy(raw, {
		get(target, prop, receiver) {
			if (guard.stale) throw new Error(STALE_CTX_MESSAGE);
			return Reflect.get(target, prop, receiver);
		},
	});
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
