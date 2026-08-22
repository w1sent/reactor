/**
 * context-editor -- toggling visibility (landscape), editing the serialized
 * text (manual), and the fork-vs-current-branch apply choice.
 *
 * `newSession`'s `setup` and `ui.select`'s answers are both fakes added to
 * `harness.mjs` for this extension specifically -- see its doc comment.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { loadExtension, makeContext, makeTui, needsPi, press, withFixture } from "./harness.mjs";

const EXT = "extensions/context-editor/index.ts";

const lastNotify = (calls) => calls.notify.at(-1);

/** A fake `SessionEntry` of type "message", the shape buildContextEntries() returns. */
function msg(id, role, content) {
	return { type: "message", id, parentId: null, timestamp: "2020-01-01T00:00:00Z", message: { role, content } };
}
const text = (s) => [{ type: "text", text: s }];

const BRANCH = [
	msg("u1", "user", text("please find the license check")),
	msg("a1", "assistant", text("looking at sub_401000 now")),
];

test("registers one command and both hooks", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		assert.deepEqual([...extension.commands.keys()], ["context-editor"]);
		assert.deepEqual([...extension.handlers.keys()].sort(), ["context", "session_start"]);
	}));

test("the context hook is a no-op with nothing hidden", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture, { branch: BRANCH });
		await extension.handlers.get("session_start")[0]({}, ctx);

		const result = await extension.handlers.get("context")[0]({ messages: [] }, ctx);

		assert.equal(result, undefined);
	}));

test("with nothing in context yet, says so instead of opening anything", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { branch: [] });

		await extension.commands.get("context-editor").handler("", ctx);

		assert.match(lastNotify(calls).message, /nothing in context yet/);
	}));

test("landscape: hiding a row and applying to the current branch persists it, enforced by the context hook next time", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { branch: BRANCH, entries, selectAnswers: ["Apply to current branch (in place)"] });
		await extension.handlers.get("session_start")[0]({}, ctx);

		const handlerPromise = extension.commands.get("context-editor").handler("", ctx);
		// space (hide the first row) then "a" (apply)
		await press(calls.overlay, " ");
		await press(calls.overlay, "a");
		await handlerPromise;

		assert.match(lastNotify(calls).message, /1 entry hidden/);

		// Simulate a reload picking the persisted state back up.
		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2 } = makeContext(fixture, { branch: BRANCH, entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);
		const result = await second.extension.handlers.get("context")[0]({ messages: [] }, ctx2);

		assert.ok(result, "the context hook should now filter");
		assert.equal(result.messages.length, 1);
		assert.match(result.messages[0].content[0].text, /sub_401000/);
	}));

test("landscape: cancelling (esc) applies nothing and persists nothing", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { branch: BRANCH, entries });
		await extension.handlers.get("session_start")[0]({}, ctx);

		const handlerPromise = extension.commands.get("context-editor").handler("", ctx);
		await press(calls.overlay, "\x1b");
		await handlerPromise;

		assert.equal(calls.select.length, 0, "esc should never reach the apply-target prompt");
		assert.equal(entries.length, 0);
	}));

test("landscape: applying with nothing toggled reports no change without prompting for a target", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { branch: BRANCH, entries });
		await extension.handlers.get("session_start")[0]({}, ctx);

		const handlerPromise = extension.commands.get("context-editor").handler("", ctx);
		await press(calls.overlay, "a");
		await handlerPromise;

		assert.match(lastNotify(calls).message, /nothing changed/);
		assert.equal(calls.select.length, 0);
	}));

test("landscape: hiding an assistant's tool call also hides its tool result, and vice versa", needsPi, () =>
	withFixture({}, async (fixture) => {
		const branch = [
			msg("u1", "user", text("read this file")),
			{ type: "message", id: "a1", parentId: null, timestamp: "2020-01-01T00:00:00Z", message: { role: "assistant", content: [{ type: "toolCall", id: "call1", name: "read", arguments: {} }] } },
			{ type: "message", id: "r1", parentId: null, timestamp: "2020-01-01T00:00:00Z", message: { role: "toolResult", toolCallId: "call1", content: text("file contents here") } },
		];
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { branch, entries, selectAnswers: ["Apply to current branch (in place)"] });
		await extension.handlers.get("session_start")[0]({}, ctx);

		const handlerPromise = extension.commands.get("context-editor").handler("", ctx);
		// rows: [user, assistant(toolCall), toolResult] -- move to row 1 (assistant) and hide it.
		await press(calls.overlay, "\x1b[B"); // down arrow-ish; keybindings resolve the real binding, see below
		await press(calls.overlay, " ");
		await press(calls.overlay, "a");
		await handlerPromise;

		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2 } = makeContext(fixture, { branch, entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);
		const result = await second.extension.handlers.get("context")[0]({ messages: [] }, ctx2);

		// Only the user message should survive -- both the assistant's tool call
		// and its result must be dropped together, never one without the other.
		assert.ok(result);
		assert.equal(result.messages.length, 1);
		assert.equal(result.messages[0].role, "user");
	}));

test("fork: replays only the kept entries into a brand-new session, leaving the current one untouched", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { branch: BRANCH, entries, selectAnswers: ["Fork branch with new context (default)"] });
		await extension.handlers.get("session_start")[0]({}, ctx);

		const handlerPromise = extension.commands.get("context-editor").handler("", ctx);
		await press(calls.overlay, " "); // hide the user row
		await press(calls.overlay, "a");
		await handlerPromise;

		assert.equal(calls.newSession.length, 1);
		assert.equal(calls.newSession[0].appended.length, 1);
		assert.equal(calls.newSession[0].appended[0].role, "assistant");
		// Nothing persisted on the current branch -- fork touches a new session only.
		assert.equal(entries.length, 0);
	}));

test("manual: a deleted block hides that entry; the rest survive", needsPi, () =>
	withFixture({}, async (fixture) => {
		const fs = await import("node:fs");
		// A fake $EDITOR: deletes the first "=== [id] ... ===" block (header
		// through its trailing blank line) and leaves everything else as the
		// real serialized text handed to it, so the parse/apply half is
		// exercised the same way a person deleting a block in vi would.
		const script = `${fixture.dir}/fake-editor.mjs`;
		fs.writeFileSync(
			script,
			`import fs from "node:fs";\n` +
				`const path = process.argv[2];\n` +
				`const lines = fs.readFileSync(path, "utf8").split("\\n");\n` +
				`const out = [];\n` +
				`let skipping = false;\n` +
				`for (const line of lines) {\n` +
				`  if (/^=== \\[u1\\]/.test(line)) { skipping = true; continue; }\n` +
				`  if (skipping && line === "") { skipping = false; continue; }\n` +
				`  if (skipping) continue;\n` +
				`  out.push(line);\n` +
				`}\n` +
				`fs.writeFileSync(path, out.join("\\n"));\n`,
		);

		const { extension, entries } = await loadExtension(EXT, fixture);
		const tui = makeTui();
		const { ctx, calls } = makeContext(fixture, { tui, branch: BRANCH, entries, selectAnswers: ["Apply to current branch (in place)"] });
		await extension.handlers.get("session_start")[0]({}, ctx);

		const savedEditor = process.env.EDITOR;
		process.env.EDITOR = `node ${script}`;
		try {
			await extension.commands.get("context-editor").handler("manual", ctx);
		} finally {
			if (savedEditor === undefined) delete process.env.EDITOR;
			else process.env.EDITOR = savedEditor;
		}

		// The bug this regression-tests: pi's TUI holds the terminal in raw
		// mode for its own keystrokes, so spawning the external editor with
		// stdio: "inherit" without suspending that first means both read the
		// same stdin at once and the editor never sees a keypress. Manual mode
		// must stop the TUI before launching the editor and restart it after.
		assert.equal(tui.stops, 1, "the TUI must be suspended before the external editor gets the terminal");
		assert.equal(tui.starts, 1, "and resumed once it exits");

		assert.equal(calls.select.length, 1, "manual mode still asks fork-vs-branch when something changed");
		assert.match(lastNotify(calls).message, /1 entry hidden/);

		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2 } = makeContext(fixture, { branch: BRANCH, entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);
		const result = await second.extension.handlers.get("context")[0]({ messages: [] }, ctx2);

		assert.ok(result);
		assert.equal(result.messages.length, 1);
		assert.equal(result.messages[0].role, "assistant");
	}));
