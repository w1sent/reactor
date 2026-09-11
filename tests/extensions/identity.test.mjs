/**
 * identity: the persona block in the system prompt. Built-ins select by name,
 * the adhoc custom identity is written in-session (one-liner or external
 * editor) and can be saved as a named, reusable one in pi-identity.json;
 * "off" is an explicit state that overrides a configured global default.
 * With nothing selected, before_agent_start returns undefined and the system
 * prompt stays byte-identical.
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { loadExtension, makeContext, makeTui, needsPi, withFixture } from "./harness.mjs";

const EXT = "extensions/identity/index.ts";

const lastNotify = (calls) => calls.notify.at(-1);
const configPath = (fixture) => path.join(fixture.agentDir, "pi-identity.json");
const readConfig = (fixture) => JSON.parse(fs.readFileSync(configPath(fixture), "utf8"));

/** Load the extension and run session_start. */
async function started(fixture, ctxOptions = {}) {
	const loaded = await loadExtension(EXT, fixture);
	const made = makeContext(fixture, { entries: loaded.entries, ...ctxOptions });
	await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
	return { ...loaded, ...made };
}

// ---------------------------------------------------------------------------
// Shape, and off by default
// ---------------------------------------------------------------------------

test("registers its one command and no tools", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);

		assert.deepEqual([...extension.commands.keys()].sort(), ["identity"]);
		assert.deepEqual([...extension.tools.keys()], []);
	}));

test("before_agent_start injects nothing while nothing is selected", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.equal(result, undefined);
	}));

// ---------------------------------------------------------------------------
// Selecting built-in identities
// ---------------------------------------------------------------------------

test("a selected identity reaches the system prompt under its own header", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture);

		await extension.commands.get("identity").handler("forensics", ctx);
		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.ok(result.systemPrompt.startsWith("BASE"));
		assert.match(result.systemPrompt, /## Identity/);
		assert.match(result.systemPrompt, /forensic analyst/);
	}));

test("every documented built-in selects and injects its own text", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture);

		for (const [name, marker] of [
			["reverse-engineer", /reverse engineer/],
			["cyber-forensics", /systems where malware is suspected or known to have executed/],
			["forensics", /what a user did on a system/],
			["software-engineer", /software engineer/],
			["devops", /infrastructure engineer/],
			["publisher", /publisher/],
		]) {
			await extension.commands.get("identity").handler(name, ctx);
			const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
			assert.match(result.systemPrompt, marker, name);
			assert.doesNotMatch(result.systemPrompt, /## Session/);
		}
	}));

test("an unknown name warns and lists the available identities", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("identity").handler("penetration-tester", ctx);

		assert.equal(lastNotify(calls).level, "warning");
		assert.match(
			lastNotify(calls).message,
			/reverse-engineer, cyber-forensics, forensics, software-engineer, devops, publisher, custom/,
		);
	}));

test("/identity with no arg reports the active identity and the list", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("identity").handler("publisher", ctx);
		await extension.commands.get("identity").handler("", ctx);

		assert.match(lastNotify(calls).message, /identity: publisher/);
		assert.match(lastNotify(calls).message, /available: /);
	}));

test("/identity off is an explicit off, and it overrides a configured default", needsPi, () =>
	withFixture({}, async (fixture) => {
		fs.writeFileSync(configPath(fixture), JSON.stringify({ default: "devops" }));
		const { ctx, calls, extension } = await started(fixture);

		// The global default applies before the session says otherwise.
		const withDefault = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.match(withDefault.systemPrompt, /infrastructure engineer/);

		await extension.commands.get("identity").handler("off", ctx);
		const off = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.equal(off, undefined);
		assert.deepEqual(calls.status.at(-1), { key: "identity", value: undefined });
	}));

test("/identity show prints the text of the active or a named identity", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("identity").handler("show", ctx);
		assert.match(lastNotify(calls).message, /none active/);

		await extension.commands.get("identity").handler("publisher", ctx);
		await extension.commands.get("identity").handler("show", ctx);
		assert.match(lastNotify(calls).message, /\[publisher\]/);

		await extension.commands.get("identity").handler("show forensics", ctx);
		assert.match(lastNotify(calls).message, /\[forensics\]/);
	}));

// ---------------------------------------------------------------------------
// The adhoc custom identity
// ---------------------------------------------------------------------------

test("/identity write sets an adhoc custom identity and activates it", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("identity").handler("write Focus exclusively on firmware images and their boot chains.", ctx);
		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.ok(result.systemPrompt.startsWith("BASE"));
		assert.match(result.systemPrompt, /boot chains/);
		assert.deepEqual(calls.status.at(-1), { key: "identity", value: "identity: custom" });
	}));

test("selecting an empty custom identity warns instead of activating", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("identity").handler("custom", ctx);

		assert.equal(lastNotify(calls).level, "warning");
		assert.match(lastNotify(calls).message, /custom identity is empty/);
	}));

test("the custom identity survives a simulated reload", needsPi, () =>
	withFixture({}, async (fixture) => {
		const first = await loadExtension(EXT, fixture);
		const { ctx: ctx1 } = makeContext(fixture, { entries: first.entries });
		await first.extension.handlers.get("session_start")[0]({}, ctx1);
		await first.extension.commands.get("identity").handler("write triage ransomware samples only", ctx1);

		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2, calls } = makeContext(fixture, { entries: first.entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);

		assert.deepEqual(calls.status.at(-1), { key: "identity", value: "identity: custom" });
		const result = await second.extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx2);
		assert.match(result.systemPrompt, /ransomware samples only/);
	}));

// ---------------------------------------------------------------------------
// The external editor (tui only)
// ---------------------------------------------------------------------------

test("/identity editor degrades outside the terminal", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture, { mode: "rpc" });

		await extension.commands.get("identity").handler("editor", ctx);

		assert.equal(lastNotify(calls).level, "warning");
		assert.match(lastNotify(calls).message, /needs a terminal/);
	}));

test("/identity editor runs the external editor with the TUI suspended", needsPi, () =>
	withFixture({}, async (fixture) => {
		const fs = await import("node:fs");
		// A fake $EDITOR: appends a line to whatever it is handed, the way a
		// person finishing a thought in vi would.
		const script = `${fixture.dir}/fake-editor.mjs`;
		fs.writeFileSync(
			script,
			`import fs from "node:fs";\n` +
				`const path = process.argv[2];\n` +
				`fs.writeFileSync(path, fs.readFileSync(path, "utf8") + "Focus: firmware boot chains.\\n");\n`,
		);

		const { extension, entries } = await loadExtension(EXT, fixture);
		const tui = makeTui();
		const { ctx, calls } = makeContext(fixture, { tui, entries });
		await extension.handlers.get("session_start")[0]({}, ctx);

		const savedEditor = process.env.EDITOR;
		process.env.EDITOR = `node ${script}`;
		try {
			await extension.commands.get("identity").handler("editor", ctx);
		} finally {
			if (savedEditor === undefined) delete process.env.EDITOR;
			else process.env.EDITOR = savedEditor;
		}

		// The TUI holds the terminal in raw mode; the editor needs it
		// exclusively (same regression class context-editor/ fixed).
		assert.equal(tui.stops, 1, "the TUI must be suspended before the external editor gets the terminal");
		assert.equal(tui.starts, 1, "and resumed once it exits");

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.match(result.systemPrompt, /firmware boot chains/);
		assert.deepEqual(calls.status.at(-1), { key: "identity", value: "identity: custom" });
	}));

// ---------------------------------------------------------------------------
// Saving, deleting
// ---------------------------------------------------------------------------

test("/identity save persists the adhoc text as a named, reusable identity", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("identity").handler("write triage ransomware samples only", ctx);
		await extension.commands.get("identity").handler("save ransomware", ctx);
		await extension.commands.get("identity").handler("off", ctx);
		await extension.commands.get("identity").handler("ransomware", ctx);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.match(result.systemPrompt, /ransomware samples only/);
		assert.match(lastNotify(calls).message, /identity: ransomware/);
	}));

test("/identity save writes the user map to pi-identity.json, preserving the default", needsPi, () =>
	withFixture({}, async (fixture) => {
		fs.writeFileSync(configPath(fixture), JSON.stringify({ default: "publisher" }));
		const { ctx, extension } = await started(fixture);

		await extension.commands.get("identity").handler("write focus on chain-of-custody prose", ctx);
		await extension.commands.get("identity").handler("save custody", ctx);

		const saved = JSON.parse(fs.readFileSync(configPath(fixture), "utf8"));
		assert.equal(saved.default, "publisher");
		assert.match(saved.user.custody, /chain-of-custody prose/);
	}));

test("/identity save warns without an adhoc custom identity active", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("identity").handler("save nope", ctx);

		assert.equal(lastNotify(calls).level, "warning");
		assert.match(lastNotify(calls).message, /adhoc custom/);
	}));

test("/identity save overwrites an existing saved identity and says so", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("identity").handler("write first draft", ctx);
		await extension.commands.get("identity").handler("save mine", ctx);
		await extension.commands.get("identity").handler("write second draft", ctx);
		await extension.commands.get("identity").handler("save mine", ctx);

		assert.match(lastNotify(calls).message, /overwrote/);
		const saved = JSON.parse(fs.readFileSync(configPath(fixture), "utf8"));
		assert.equal(saved.user.mine, "second draft");
	}));

test("/identity delete removes a saved identity but never a built-in", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("identity").handler("write triage only", ctx);
		await extension.commands.get("identity").handler("save triage", ctx);
		await extension.commands.get("identity").handler("delete triage", ctx);
		assert.equal(JSON.parse(fs.readFileSync(configPath(fixture), "utf8")).user.triage, undefined);

		await extension.commands.get("identity").handler("delete triage", ctx);
		assert.match(lastNotify(calls).message, /no saved identity/);

		await extension.commands.get("identity").handler("delete forensics", ctx);
		assert.match(lastNotify(calls).message, /built in/);
	}));

test("the global default applies when the session has not chosen one", needsPi, () =>
	withFixture({}, async (fixture) => {
		fs.writeFileSync(configPath(fixture), JSON.stringify({ default: "publisher" }));
		const { ctx, calls, extension } = await started(fixture);

		assert.deepEqual(calls.status.at(-1), { key: "identity", value: "identity: publisher" });
		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.match(result.systemPrompt, /publisher/);
	}));

test("a hand-saved identity cannot shadow a built-in", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		// Saving under a built-in's name is refused -- built-in and user name
		// spaces stay disjoint, so nothing can be silently shadowed.
		await extension.commands.get("identity").handler("write pretend builtin", ctx);
		await extension.commands.get("identity").handler("save publisher", ctx);

		assert.equal(lastNotify(calls).level, "warning");
		assert.match(lastNotify(calls).message, /built in -- pick another name/);
		const saved = (() => {
			try {
				return JSON.parse(fs.readFileSync(configPath(fixture), "utf8"));
			} catch {
				return undefined; // the refused save wrote nothing at all
			}
		})();
		assert.equal(saved?.user?.publisher, undefined);

		// And the built-in still resolves to its own text.
		await extension.commands.get("identity").handler("publisher", ctx);
		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.match(result.systemPrompt, /turning technical findings into documents/);
		assert.doesNotMatch(result.systemPrompt, /pretend builtin/);
	}));