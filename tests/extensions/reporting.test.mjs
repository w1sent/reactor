/**
 * reporting: does the folder probe actually detect a change, does level 1
 * nag and level 2 self-dispatch at the right moments, and does maxReverts
 * actually stop it?
 *
 * Two things this suite cannot exercise, by construction of the mock host
 * (see extensions/README.md): pi's real command dispatch (a call recorded in
 * `sentUserMessages` never actually reaches `/reactor-report-enforce` here --
 * the command handler below is exercised directly instead) and real
 * session-tree navigation (`ctx.navigateTree` only records the call). Both
 * are covered by `scripts/check-in-pi.mjs` against a real `pi --mode rpc`
 * process.
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import { test } from "node:test";
import path from "node:path";
import { loadExtension, makeContext, needsPi, withFixture } from "./harness.mjs";

const EXT = "extensions/reporting/index.ts";

const lastNotify = (calls) => calls.notify.at(-1);

/** A fake `SessionEntry` of type "message", with the `id` the real one carries and this extension's command reads. */
function msg(role, text, id) {
	return { type: "message", id, message: { role, content: [{ type: "text", text }] } };
}

function writeReportFile(fixture, relPath, content) {
	const full = path.join(fixture.dir, "report", relPath);
	fs.mkdirSync(path.dirname(full), { recursive: true });
	fs.writeFileSync(full, content);
}

function readGlobalConfig(fixture) {
	return JSON.parse(fs.readFileSync(path.join(fixture.agentDir, "pi-reactor-reporting.json"), "utf8"));
}

/** Load, start the session, and enable at the given level (default 0 -- on, no tracking). */
async function enabled(fixture, level, ctxOptions = {}) {
	const loaded = await loadExtension(EXT, fixture);
	const made = makeContext(fixture, { entries: loaded.entries, ...ctxOptions });
	await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
	await loaded.extension.commands.get("report").handler(level === undefined ? "on" : `level ${level}`, made.ctx);
	const tick = () => loaded.extension.handlers.get("tool_execution_end")[0]({}, made.ctx);
	return { ...loaded, ...made, tick };
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

test("registers exactly the two commands and no tools", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);

		assert.deepEqual([...extension.commands.keys()].sort(), ["reactor-report-enforce", "report"]);
		assert.equal(extension.tools.size, 0);
	}));

// ---------------------------------------------------------------------------
// Off by default; the system-prompt block
// ---------------------------------------------------------------------------

test("before_agent_start does nothing while disabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);

		const result = await extension.handlers.get("before_agent_start")[0](
			{ prompt: "hi", systemPrompt: "BASE" },
			ctx,
		);

		assert.equal(result, undefined);
	}));

test("before_agent_start appends the reporting block once enabled, naming the configured folder", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx } = await enabled(fixture);

		const result = await extension.handlers.get("before_agent_start")[0](
			{ prompt: "investigate the packer", systemPrompt: "BASE" },
			ctx,
		);

		assert.ok(result.systemPrompt.startsWith("BASE"));
		assert.match(result.systemPrompt, /## Reporting mode/);
		assert.match(result.systemPrompt, /`report\/`/);
		assert.match(result.systemPrompt, /reactor-reporting/);
	}));

test("a configured templatePath is named in the block", needsPi, () =>
	withFixture({}, async (fixture) => {
		fs.mkdirSync(fixture.agentDir, { recursive: true });
		fs.writeFileSync(
			path.join(fixture.agentDir, "pi-reactor-reporting.json"),
			JSON.stringify({ templatePath: "docs/my-template.md" }),
		);
		const { extension, ctx } = await enabled(fixture);

		const result = await extension.handlers.get("before_agent_start")[0](
			{ prompt: "hi", systemPrompt: "BASE" },
			ctx,
		);

		assert.match(result.systemPrompt, /docs\/my-template\.md/);
	}));

// ---------------------------------------------------------------------------
// Status indicator
// ---------------------------------------------------------------------------

test("the footer shows reporting mode at each level, and clears when off", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, calls } = await enabled(fixture, 0);
		assert.deepEqual(calls.status.at(-1), { key: "reactor-reporting", value: "reporting mode" });

		await extension.commands.get("report").handler("level 1", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "reactor-reporting", value: "reporting mode · low" });

		await extension.commands.get("report").handler("level 2", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "reactor-reporting", value: "reporting mode · strict" });

		await extension.commands.get("report").handler("off", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "reactor-reporting", value: undefined });
	}));

// ---------------------------------------------------------------------------
// The folder probe: baseline, increment, reset
// ---------------------------------------------------------------------------

test("the first check establishes a baseline rather than crediting content already there", needsPi, () =>
	withFixture({}, async (fixture) => {
		writeReportFile(fixture, "existing.md", "written before reporting mode ever looked");
		const { extension, ctx, calls, tick } = await enabled(fixture, 1);

		await tick();
		await extension.commands.get("report").handler("status", ctx);

		assert.match(lastNotify(calls).message, /0\/8 step/);
	}));

test("steps increment while the folder is unchanged, and reset the moment it changes", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, calls, tick } = await enabled(fixture, 1);

		await tick(); // baseline
		await tick(); // 1
		await tick(); // 2
		await extension.commands.get("report").handler("status", ctx);
		assert.match(lastNotify(calls).message, /2\/8 step/);

		writeReportFile(fixture, "findings.md", "the license check is at 0x4012a0");
		await tick(); // saw the change -> reset
		await extension.commands.get("report").handler("status", ctx);
		assert.match(lastNotify(calls).message, /0\/8 step/);
	}));

test("a changed file (not just a new one) also resets the counter", needsPi, () =>
	withFixture({}, async (fixture) => {
		writeReportFile(fixture, "findings.md", "short");
		const { extension, ctx, calls, tick } = await enabled(fixture, 1);
		await tick(); // baseline includes findings.md as it is now
		await tick(); // 1

		writeReportFile(fixture, "findings.md", "considerably longer than before, a real edit");
		await tick(); // size changed -> reset

		await extension.commands.get("report").handler("status", ctx);
		assert.match(lastNotify(calls).message, /0\/8 step/);
	}));

test("/report reset clears the counters by hand", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, calls, tick } = await enabled(fixture, 1);
		for (let i = 0; i < 5; i++) await tick();

		await extension.commands.get("report").handler("reset", ctx);
		await extension.commands.get("report").handler("status", ctx);

		assert.match(lastNotify(calls).message, /0\/8 step/);
	}));

// ---------------------------------------------------------------------------
// Level 0: block only, never nags or reverts
// ---------------------------------------------------------------------------

test("level 0 never nags and never self-dispatches, however many steps pass", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, sentUserMessages, tick } = await enabled(fixture, 0);
		for (let i = 0; i < 20; i++) await tick();

		const nagResult = await extension.handlers.get("context")[0]({ messages: [] }, ctx);
		assert.equal(nagResult, undefined);

		await extension.handlers.get("agent_settled")[0]({}, ctx);
		assert.equal(sentUserMessages.length, 0);
	}));

// ---------------------------------------------------------------------------
// Level 1: nag mid-loop once the threshold is crossed
// ---------------------------------------------------------------------------

test("level 1 leaves the context hook alone before the threshold", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, tick } = await enabled(fixture, 1);
		for (let i = 0; i < 5; i++) await tick(); // stepsSinceChange = 4, under the default threshold of 8

		const result = await extension.handlers.get("context")[0]({ messages: [{ role: "user" }] }, ctx);

		assert.equal(result, undefined);
	}));

test("level 1 appends a nag every context call once over threshold, and stops once documented", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, tick } = await enabled(fixture, 1);
		for (let i = 0; i < 9; i++) await tick(); // baseline + 8 -> at the default threshold

		const messages = [{ role: "user" }];
		const result = await extension.handlers.get("context")[0]({ messages }, ctx);
		assert.equal(result.messages.length, 2);
		assert.equal(result.messages[0], messages[0]);
		assert.match(result.messages[1].content, /reactor-reporting/);
		assert.equal(result.messages[1].role, "custom");

		// Still over threshold on the very next call too -- keeps nagging, not once-and-done.
		const again = await extension.handlers.get("context")[0]({ messages }, ctx);
		assert.equal(again.messages.length, 2);

		writeReportFile(fixture, "findings.md", "documented now");
		await tick(); // resets

		const after = await extension.handlers.get("context")[0]({ messages }, ctx);
		assert.equal(after, undefined);
	}));

// ---------------------------------------------------------------------------
// Level 2: self-dispatch on agent_settled, and the maxReverts fallback
// ---------------------------------------------------------------------------

test("level 2 does not self-dispatch before the threshold", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, sentUserMessages, tick } = await enabled(fixture, 2);
		await tick(); // baseline only

		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(sentUserMessages.length, 0);
	}));

test("level 2 self-dispatches /reactor-report-enforce once agent_settled sees the threshold crossed", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, sentUserMessages, tick } = await enabled(fixture, 2);
		for (let i = 0; i < 9; i++) await tick();

		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.deepEqual(sentUserMessages, [
			{ content: "/reactor-report-enforce", options: { expandPromptTemplates: true } },
		]);
	}));

test("maxReverts stops the reverting and warns once, falling back to nagging", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, calls, sentUserMessages, tick } = await enabled(fixture, 2);
		for (let i = 0; i < 9; i++) await tick();

		// Default maxReverts is 3: three consecutive settles self-dispatch, the
		// fourth gives up and warns instead.
		await extension.handlers.get("agent_settled")[0]({}, ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);
		assert.equal(sentUserMessages.length, 3);

		await extension.handlers.get("agent_settled")[0]({}, ctx);
		assert.equal(sentUserMessages.length, 3, "the fourth settle must not self-dispatch again");
		assert.match(lastNotify(calls).message, /gave up reverting/);
		assert.equal(lastNotify(calls).level, "warning");

		// And it only warns once, not on every subsequent settle.
		const notifyCountBefore = calls.notify.length;
		await extension.handlers.get("agent_settled")[0]({}, ctx);
		assert.equal(calls.notify.length, notifyCountBefore);

		// Nagging is still live underneath -- level 2 is level 1 plus reverts.
		const nag = await extension.handlers.get("context")[0]({ messages: [{ role: "user" }] }, ctx);
		assert.equal(nag.messages.length, 2);
	}));

// ---------------------------------------------------------------------------
// /reactor-report-enforce itself
// ---------------------------------------------------------------------------

test("/reactor-report-enforce navigates to the last user entry and re-sends the demanded prompt", needsPi, () =>
	withFixture({}, async (fixture) => {
		const branch = [msg("user", "investigate the packer", "u1"), msg("assistant", "looking now", "a1")];
		const { extension, entries, sentUserMessages } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries, branch });
		await extension.handlers.get("session_start")[0]({}, ctx);
		await extension.commands.get("report").handler("on", ctx);
		await extension.handlers.get("before_agent_start")[0]({ prompt: "investigate the packer", systemPrompt: "BASE" }, ctx);

		await extension.commands.get("reactor-report-enforce").handler("", ctx);

		assert.equal(calls.navigateTree.length, 1);
		assert.equal(calls.navigateTree[0].targetId, "u1");
		assert.match(calls.navigateTree[0].options.label, /reverted/);

		assert.equal(sentUserMessages.length, 1);
		assert.match(sentUserMessages[0].content, /^investigate the packer/);
		assert.match(sentUserMessages[0].content, /reactor-reporting/);
		assert.match(sentUserMessages[0].content, /report\//);
		assert.deepEqual(sentUserMessages[0].options, { expandPromptTemplates: true });
	}));

test("/reactor-report-enforce does nothing when there is no user entry to revert to", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture); // empty branch

		await extension.commands.get("reactor-report-enforce").handler("", ctx);

		assert.equal(calls.navigateTree.length, 0);
	}));

// ---------------------------------------------------------------------------
// /report on|off|level|status|folder|reset
// ---------------------------------------------------------------------------

test("/report with no argument reports status", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });
		await extension.handlers.get("session_start")[0]({}, ctx);

		await extension.commands.get("report").handler("", ctx);

		assert.match(lastNotify(calls).message, /off/);
	}));

test("/report level rejects anything outside 0, 1, 2", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });
		await extension.handlers.get("session_start")[0]({}, ctx);

		await extension.commands.get("report").handler("level 5", ctx);

		assert.equal(lastNotify(calls).level, "error");
	}));

test("/report folder patches the global config and re-baselines", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, calls, tick } = await enabled(fixture, 1);
		await tick(); // baseline against the default "report" folder
		await tick(); // 1 step in

		await extension.commands.get("report").handler("folder scratch-report", ctx);

		assert.equal(readGlobalConfig(fixture).folder, "scratch-report");
		await extension.commands.get("report").handler("status", ctx);
		assert.match(lastNotify(calls).message, /scratch-report/);
		// Re-baselined against the new folder: this next check just establishes
		// it again rather than counting as a step.
		await tick();
		await extension.commands.get("report").handler("status", ctx);
		assert.match(lastNotify(calls).message, /0\/8 step/);
	}));

test("global config defaults apply when the file is absent", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, ctx, calls } = await enabled(fixture, 1);

		await extension.commands.get("report").handler("status", ctx);

		assert.match(lastNotify(calls).message, /folder "report"/);
		assert.match(lastNotify(calls).message, /0\/8 step/);
	}));

// ---------------------------------------------------------------------------
// State survives a reload
// ---------------------------------------------------------------------------

test("enabled/level survive a simulated reload", needsPi, () =>
	withFixture({}, async (fixture) => {
		const first = await loadExtension(EXT, fixture);
		const { ctx: ctx1 } = makeContext(fixture, { entries: first.entries });
		await first.extension.handlers.get("session_start")[0]({}, ctx1);
		await first.extension.commands.get("report").handler("level 2", ctx1);

		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2, calls } = makeContext(fixture, { entries: first.entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);

		assert.deepEqual(calls.status.at(-1), { key: "reactor-reporting", value: "reporting mode · strict" });
	}));
