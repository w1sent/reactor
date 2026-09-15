/**
 * rolling-context: does the fade actually fade, and stay off until asked?
 *
 * Since the split (ADR-0024) this extension is only the fade: no manifest,
 * no tools. The manifest lives in goal-setting/ (system prompt), the
 * recovery tools in history-tools/; the fade's token accounting includes
 * them for free because `ctx.getSystemPrompt()` returns the chained prompt.
 *
 * Off by default and independent of everything else this package ships
 * (ADR-0019) -- most tests here start by turning it on via `/rolling on`,
 * the same way a real session would, rather than reaching into module state.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { loadExtension, makeContext, needsPi, recordingTheme, withFixture } from "./harness.mjs";

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
// Shape, and off by default
// ---------------------------------------------------------------------------

test("registers its one command and no tools", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);

		assert.deepEqual([...extension.commands.keys()].sort(), ["rolling"]);
		assert.deepEqual([...extension.tools.keys()], []);
	}));

test("the context hook is a no-op while disabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		const messages = [msg("user", "hello").message];

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		assert.equal(result, undefined);
	}));

test("before_agent_start injects nothing while disabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.equal(result, undefined);
	}));

// ---------------------------------------------------------------------------
// /rolling
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
		const rec = recordingTheme;
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries, theme: rec.theme });

		await extension.commands.get("rolling").handler("on", ctx);
		await extension.commands.get("rolling").handler("on", ctx);
		assert.match(lastNotify(calls).message, /enabled/);

		await extension.commands.get("rolling").handler("off", ctx);
		assert.match(lastNotify(calls).message, /disabled/);
	}));

test("/rolling sets the footer status", needsPi, () =>
	withFixture({}, async (fixture) => {
		const rec = recordingTheme;
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries, theme: rec.theme });

		await extension.commands.get("rolling").handler("on", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "rolling-context", value: "· ⋯ rolling" });
		assert.equal(rec.fgCalls.find((c) => c.text === "⋯")?.color, "accent");

		await extension.commands.get("rolling").handler("off", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "rolling-context", value: undefined });
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

test("a small context window drops the oldest messages, keeps a suffix, and notifies", needsPi, () =>
	withFixture({}, async (fixture) => {
		// Window picked so the *soft* budget forces a drop while staying well
		// above the default 16384-token reserve -- the hard ceiling (window -
		// reserve) has to leave comfortable room, or this is testing the
		// hard-boundary archive path below instead of a plain soft drop.
		const { ctx, calls, extension } = await enabled(fixture, { branch: BIG_BRANCH, model: { contextWindow: 18_000 } });
		const messages = BIG_BRANCH.map((b) => b.message);

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		assert.ok(result.messages.length < messages.length, "nothing was dropped");
		// What is kept is a contiguous suffix of what arrived -- the fade
		// prepends nothing since the split (ADR-0024); the manifest lives in
		// the system prompt now.
		assert.deepEqual(result.messages, messages.slice(-result.messages.length));
		assert.match(lastNotify(calls).message, /dropped \d+ message\(s\)/);
	}));

test("a cut never orphans a tool result from its tool call", needsPi, () =>
	withFixture({}, async (fixture) => {
		// Same shape a real multi-step tool turn takes: assistant-with-toolCall
		// immediately followed by its toolResult. A positional slice that does
		// not respect this boundary would send the toolResult alone and the
		// backend would reject the request (bug #2/#3).
		const branch = [
			msg("user", "A".repeat(2000)),
			{ type: "message", message: { role: "assistant", content: [{ type: "toolCall", id: "1", name: "read", arguments: {} }] } },
			{ type: "message", message: { role: "toolResult", toolCallId: "1", content: [{ type: "text", text: "B".repeat(2000) }] } },
			msg("assistant", "C".repeat(2000)),
		];
		const { ctx, extension } = await enabled(fixture, { branch, model: { contextWindow: 18_000 } });
		const messages = branch.map((b) => b.message);

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		if (result.messages.length > 0) {
			assert.notEqual(result.messages[0].role, "toolResult", "kept window must not start on an orphaned tool result");
		}
	}));

test("a turn far larger than the whole hard budget is archived, not overflowed or silently dropped", needsPi, () =>
	withFixture({}, async (fixture) => {
		// The default reserve (16384) alone exceeds this window, so the hard
		// ceiling (window - reserve) clamps to 0: nothing at all fits without
		// archiving. This is the scenario the old code silently overflowed --
		// it kept the newest message whole regardless of the ceiling, which is
		// exactly how a request ends up rejected by the backend (bug #2).
		const { ctx, calls, extension } = await enabled(fixture, { branch: BIG_BRANCH, model: { contextWindow: 4000 } });
		const messages = BIG_BRANCH.map((b) => b.message);

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		assert.equal(result.messages.length, 1, "the newest message is archived, never dropped outright");
		assert.match(result.messages[0].content[0].text, /archived/);
		assert.ok(result.messages[0].content[0].text.length < 2000, "content was actually shrunk, not sent whole");
		assert.match(lastNotify(calls).message, /exceeded the hard context limit/);
	}));

test("a window with room to spare drops nothing and does not notify", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await enabled(fixture, { branch: BIG_BRANCH, model: { contextWindow: 500_000 } });
		const messages = BIG_BRANCH.map((b) => b.message);

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		assert.deepEqual(result.messages, messages);
		assert.doesNotMatch(lastNotify(calls)?.message ?? "", /dropped/);
	}));

test("the newest message always survives even under a near-zero budget", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture, { branch: BIG_BRANCH, model: { contextWindow: 1 } });
		const messages = BIG_BRANCH.map((b) => b.message);

		const result = await extension.handlers.get("context")[0]({ messages }, ctx);

		assert.ok(result.messages.length >= 1);
	}));

test("the system prompt's own size is subtracted from the fade's budget", needsPi, () =>
	withFixture({}, async (fixture) => {
		// The manifest block now lives in the system prompt (goal-setting/,
		// ADR-0024); the fade accounts for it because ctx.getSystemPrompt()
		// returns the chained prompt. A large fake system prompt must shrink
		// the kept window compared with an empty one, all else equal.
		const messages = BIG_BRANCH.map((b) => b.message);
		const { ctx, extension } = await enabled(fixture, { branch: BIG_BRANCH, model: { contextWindow: 18_000 } });
		const short = await extension.handlers.get("context")[0]({ messages }, ctx);

		const big = await extension.handlers.get("context")[0](
			{ messages },
			makeContext(fixture, { branch: BIG_BRANCH, model: { contextWindow: 18_000 }, systemPrompt: "X".repeat(4000) }).ctx,
		);

		assert.ok(short.messages.length > big.messages.length, "a larger system prompt must leave room for fewer messages");
	}));

// ---------------------------------------------------------------------------
// The guidance block (before_agent_start while enabled)
// ---------------------------------------------------------------------------

test("fading guidance reaches the system prompt while enabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.ok(result.systemPrompt.startsWith("BASE"));
		assert.match(result.systemPrompt, /## Rolling Context/);
		assert.match(result.systemPrompt, /still exist in the session file/);
	}));

// ---------------------------------------------------------------------------
// Compaction is suppressed while enabled, except when manual
// ---------------------------------------------------------------------------

test("threshold compaction is cancelled while enabled -- the fade already covers it", needsPi, () =>
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

test("overflow recovery is never cancelled -- it is pi's last resort after a real backend rejection", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await enabled(fixture);

		const result = await extension.handlers.get("session_before_compact")[0]({ reason: "overflow" }, ctx);

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

test("the enabled toggle survives a simulated reload", needsPi, () =>
	withFixture({}, async (fixture) => {
		const first = await loadExtension(EXT, fixture);
		const { ctx: ctx1 } = makeContext(fixture, { entries: first.entries });
		await first.extension.handlers.get("session_start")[0]({}, ctx1);
		await first.extension.commands.get("rolling").handler("on", ctx1);

		// A fresh module instance -- what a `/reload` or a resumed session
		// gets -- reading the same entries the first instance appended.
		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2, calls } = makeContext(fixture, { entries: first.entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);

		assert.deepEqual(calls.status.at(-1), { key: "rolling-context", value: "· ⋯ rolling" });
		const messages = [msg("user", "hello").message];
		const result = await second.extension.handlers.get("context")[0]({ messages }, ctx2);
		assert.deepEqual(result.messages, messages);
	}));