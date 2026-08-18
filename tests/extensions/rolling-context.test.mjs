/**
 * rolling-context: does the fade actually fade, and stay off until asked?
 *
 * Off by default and independent of everything else this package ships
 * (ADR-0019) -- most tests here start by turning it on via `/rolling on`,
 * the same way a real session would, rather than reaching into module state.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { loadExtension, makeContext, needsPi, withFixture } from "./harness.mjs";

const EXT = "extensions/rolling-context/index.ts";

const lastNotify = (calls) => calls.notify.at(-1);

/** A fake `SessionEntry` of type "message", the shape `getBranch()` returns. */
function msg(role, text) {
	return { type: "message", message: { role, content: [{ type: "text", text }] } };
}

/** Load the extension and switch it on, returning everything a test needs. */
async function enabled(fixture, ctxOptions = {}) {
	const loaded = await loadExtension(EXT, fixture);
	const made = makeContext(fixture, { entries: loaded.entries, ...ctxOptions });
	await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
	await loaded.extension.commands.get("rolling").handler("on", made.ctx);
	return { ...loaded, ...made };
}

// ---------------------------------------------------------------------------
// Off by default
// ---------------------------------------------------------------------------

test("registers every command and tool the README documents", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);

		assert.deepEqual(
			[...extension.commands.keys()].sort(),
			["frame", "goal", "guidelines", "rolling"],
		);
		assert.deepEqual(
			[...extension.tools.keys()].sort(),
			["history_index", "history_read", "history_search", "update_steps"],
		);
	}));

test("the context hook is a no-op while disabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		const messages = [msg("user", "hello").message];

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		assert.equal(result, undefined);
	}));

test("a tool call while disabled says so instead of doing anything", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		const tool = extension.tools.get("update_steps").definition;

		const result = await tool.execute("id", { steps: [{ summary: "x", status: "done" }] }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /disabled.*\/rolling on/);
	}));

// ---------------------------------------------------------------------------
// /rolling, /goal, /guidelines, /frame
// ---------------------------------------------------------------------------

test("/rolling with no argument toggles", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });

		await extension.commands.get("rolling").handler("", ctx);
		assert.match(lastNotify(calls).message, /enabled/);

		await extension.commands.get("rolling").handler("", ctx);
		assert.match(lastNotify(calls).message, /disabled/);
	}));

test("/rolling on and /rolling off set state explicitly rather than toggling", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });

		await extension.commands.get("rolling").handler("on", ctx);
		await extension.commands.get("rolling").handler("on", ctx);
		assert.match(lastNotify(calls).message, /enabled/);

		await extension.commands.get("rolling").handler("off", ctx);
		assert.match(lastNotify(calls).message, /disabled/);
	}));

test("/rolling sets the footer status", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });

		await extension.commands.get("rolling").handler("on", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "rolling-context", value: "rolling: on" });

		await extension.commands.get("rolling").handler("off", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "rolling-context", value: undefined });
	}));

test("/goal requires text and is reflected in /frame", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await enabled(fixture);

		await extension.commands.get("goal").handler("", ctx);
		assert.equal(lastNotify(calls).level, "warning");

		await extension.commands.get("goal").handler("find the crash", ctx);
		await extension.commands.get("frame").handler("", ctx);
		assert.match(lastNotify(calls).message, /goal: find the crash/);
	}));

test("/guidelines requires text and reaches the system prompt while enabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture);

		await extension.commands.get("guidelines").handler("never touch prod", ctx);
		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.ok(result.systemPrompt.startsWith("BASE"));
		assert.match(result.systemPrompt, /never touch prod/);
	}));

test("before_agent_start does nothing while disabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.equal(result, undefined);
	}));

test("/frame reports zero steps and disabled state before anything is set", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("frame").handler("", ctx);

		assert.match(lastNotify(calls).message, /goal: \(none\)/);
		assert.match(lastNotify(calls).message, /steps \(0\/20\)/);
		assert.match(lastNotify(calls).message, /enabled: false/);
	}));

// ---------------------------------------------------------------------------
// update_steps
// ---------------------------------------------------------------------------

test("update_steps overwrites the list and reports the count", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture);
		const tool = extension.tools.get("update_steps").definition;

		const result = await tool.execute(
			"id",
			{ steps: [{ summary: "found entry point", status: "done" }, { summary: "mapping imports", status: "in progress" }] },
			undefined,
			undefined,
			ctx,
		);

		assert.match(result.content[0].text, /^Steps updated: 2 step\(s\)\.$/);
	}));

test("update_steps clamps an over-long summary and an over-long status", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await enabled(fixture);
		const tool = extension.tools.get("update_steps").definition;

		await tool.execute(
			"id",
			{ steps: [{ summary: "x".repeat(200), status: "one two three four five" }] },
			undefined,
			undefined,
			ctx,
		);
		await extension.commands.get("frame").handler("", ctx);

		const frame = lastNotify(calls).message;
		// maxDescription defaults to 80: 79 chars plus the truncation ellipsis.
		assert.match(frame, new RegExp(`x{79}…`));
		// statusWords defaults to 3.
		assert.match(frame, /\[one two three\]/);
	}));

test("update_steps warns once the soft limit is exceeded", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture);
		const tool = extension.tools.get("update_steps").definition;
		const steps = Array.from({ length: 21 }, (_, i) => ({ summary: `step ${i}`, status: "todo" }));

		const result = await tool.execute("id", { steps }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /WARNING.*21.*exceeds the soft limit \(20\)/s);
	}));

test("update_steps under the soft limit carries no warning", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture);
		const tool = extension.tools.get("update_steps").definition;

		const result = await tool.execute("id", { steps: [{ summary: "one", status: "todo" }] }, undefined, undefined, ctx);

		assert.doesNotMatch(result.content[0].text, /WARNING/);
	}));

// ---------------------------------------------------------------------------
// history_index / history_search / history_read
// ---------------------------------------------------------------------------

const HISTORY = [msg("user", "please find the license check"), msg("assistant", "looking at sub_401000 now")];

test("history_index lists one entry per message", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_index").definition;

		const result = await tool.execute("id", {}, undefined, undefined, ctx);

		// +1: `enabled()` itself ran `/rolling on`, which appended its own
		// (contentless) state entry onto the same branch -- a real session
		// would carry that too, so this counts it rather than special-casing
		// the fixture setup out of the branch.
		assert.equal(result.details.totalEntries, HISTORY.length + 1);
		assert.match(result.content[0].text, /license check/);
		assert.match(result.content[0].text, /sub_401000/);
	}));

test("history_search finds a hit and shows context lines", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_search").definition;

		const result = await tool.execute("id", { query: "sub_401000" }, undefined, undefined, ctx);

		assert.equal(result.details.hitCount, 1);
		assert.match(result.content[0].text, /sub_401000/);
	}));

test("history_search with no hits says so plainly", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_search").definition;

		const result = await tool.execute("id", { query: "definitely not present" }, undefined, undefined, ctx);

		assert.equal(result.content[0].text, "No matches.");
	}));

test("history_read returns a line range", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_read").definition;

		const result = await tool.execute("id", { startLine: 0, endLine: 0 }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /^0 \|/);
	}));

test("history_read out of range says so", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_read").definition;

		const result = await tool.execute("id", { startLine: 500, endLine: 600 }, undefined, undefined, ctx);

		assert.equal(result.content[0].text, "Out of range.");
	}));

// ---------------------------------------------------------------------------
// The fade (the "context" hook)
// ---------------------------------------------------------------------------

/** Four ~500-token messages -- enough to force a drop under a small window. */
const BIG_BRANCH = [
	msg("user", "A".repeat(2000)),
	msg("assistant", "B".repeat(2000)),
	msg("user", "C".repeat(2000)),
	msg("assistant", "D".repeat(2000)),
];

test("a small context window drops the oldest messages and notifies", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await enabled(fixture, { branch: BIG_BRANCH, model: { contextWindow: 4000 } });
		const messages = BIG_BRANCH.map((b) => b.message);

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		assert.ok(result.messages.length < messages.length + 1, "nothing was dropped");
		assert.equal(result.messages[0].customType, "pi-rolling-context");
		assert.match(lastNotify(calls).message, /dropped \d+ message\(s\)/);
	}));

test("the manifest carries the goal and steps, not just the line marker", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture, { branch: BIG_BRANCH, model: { contextWindow: 4000 } });
		await extension.commands.get("goal").handler("find the license check", ctx);
		const tool = extension.tools.get("update_steps").definition;
		await tool.execute("id", { steps: [{ summary: "found sub_401000", status: "done" }] }, undefined, undefined, ctx);
		const messages = BIG_BRANCH.map((b) => b.message);

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		assert.match(result.messages[0].content, /find the license check/);
		assert.match(result.messages[0].content, /found sub_401000/);
	}));

test("a window with room to spare drops nothing and does not notify", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await enabled(fixture, { branch: BIG_BRANCH, model: { contextWindow: 500_000 } });
		const messages = BIG_BRANCH.map((b) => b.message);

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		// +1 for the prepended manifest; every original message survives.
		assert.equal(result.messages.length, messages.length + 1);
		assert.doesNotMatch(lastNotify(calls)?.message ?? "", /dropped/);
	}));

test("the newest message always survives even under a near-zero budget", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture, { branch: BIG_BRANCH, model: { contextWindow: 1 } });
		const messages = BIG_BRANCH.map((b) => b.message);

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		// manifest + at least the current turn's message.
		assert.ok(result.messages.length >= 2);
	}));

// ---------------------------------------------------------------------------
// Compaction is suppressed while enabled, except when manual
// ---------------------------------------------------------------------------

test("automatic compaction is cancelled while enabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture);

		const result = await extension.handlers.get("session_before_compact")[0]({ reason: "threshold" }, ctx);

		assert.deepEqual(result, { cancel: true });
	}));

test("a manual /compact is allowed through even while enabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture);

		const result = await extension.handlers.get("session_before_compact")[0]({ reason: "manual" }, ctx);

		assert.equal(result, undefined);
	}));

test("compaction is untouched while disabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);

		const result = await extension.handlers.get("session_before_compact")[0]({ reason: "threshold" }, ctx);

		assert.equal(result, undefined);
	}));

// ---------------------------------------------------------------------------
// State survives a reload (session_start restores from the branch)
// ---------------------------------------------------------------------------

test("goal, guidelines and steps survive a simulated reload", needsPi, () =>
	withFixture({}, async (fixture) => {
		const first = await loadExtension(EXT, fixture);
		const { ctx: ctx1 } = makeContext(fixture, { entries: first.entries });
		await first.extension.handlers.get("session_start")[0]({}, ctx1);
		await first.extension.commands.get("rolling").handler("on", ctx1);
		await first.extension.commands.get("goal").handler("recover the key", ctx1);
		const tool = first.extension.tools.get("update_steps").definition;
		await tool.execute("id", { steps: [{ summary: "found the vault", status: "in progress" }] }, undefined, undefined, ctx1);

		// A fresh module instance -- what a `/reload` or a resumed session
		// gets -- reading the same entries the first instance appended.
		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2, calls } = makeContext(fixture, { entries: first.entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);
		await second.extension.commands.get("frame").handler("", ctx2);

		const frame = lastNotify(calls).message;
		assert.match(frame, /goal: recover the key/);
		assert.match(frame, /found the vault/);
		assert.match(frame, /enabled: true/);
	}));
