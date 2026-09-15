/**
 * auto-continue: after a successful automatic compaction that left the turn
 * ended (willRetry false), send the continuation message at agent_settled --
 * and only then. Overflow recovery retries by itself (pi's own
 * agent.continue()), failed compactions never shrank the context, and manual
 * /compact is housekeeping: none of those continue. The runaway guard pauses
 * after maxConsecutive continuations of the same message and lifts on any
 * other prompt (ADR-0025).
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { loadExtension, makeContext, needsPi, recordingTheme, withFixture } from "./harness.mjs";

const EXT = "extensions/auto-continue/index.ts";

const lastNotify = (calls) => calls.notify.at(-1);
const lastSent = (sent) => sent.at(-1);

const compactEvent = (reason = "threshold", willRetry = false) => ({
	type: "session_compact",
	compactionEntry: {},
	fromExtension: false,
	reason,
	willRetry,
});

async function loaded(fixture, ctxOptions = {}) {
	const loaded = await loadExtension(EXT, fixture);
	const made = makeContext(fixture, { entries: loaded.entries, ...ctxOptions });
	await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
	return { ...loaded, ...made };
}

/** Load the extension, run session_start, and switch it on. */
async function enabled(fixture, ctxOptions = {}) {
	const made = await loaded(fixture, ctxOptions);
	await made.extension.commands.get("auto-continue").handler("on", made.ctx);
	return made;
}

// ---------------------------------------------------------------------------
// Shape, and off by default
// ---------------------------------------------------------------------------

test("registers its one command and no tools", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);

		assert.deepEqual([...extension.commands.keys()].sort(), ["auto-continue"]);
		assert.deepEqual([...extension.tools.keys()], []);
	}));

test("an automatic compaction sends nothing while disabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await loaded(fixture);

		await extension.handlers.get("session_compact")[0](compactEvent(), ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(sentUserMessages.length, 0);
	}));

// ---------------------------------------------------------------------------
// The trigger
// ---------------------------------------------------------------------------

test("a threshold compaction continues at agent_settled while enabled", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		await extension.handlers.get("session_compact")[0](compactEvent("threshold", false), ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(sentUserMessages.length, 1);
		assert.equal(lastSent(sentUserMessages).content, "continue");
	}));

test("an overflow compaction with a preserved response continues too", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		await extension.handlers.get("session_compact")[0](compactEvent("overflow", false), ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(lastSent(sentUserMessages).content, "continue");
	}));

test("overflow recovery (willRetry) is left to pi's own retry -- no continuation", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		await extension.handlers.get("session_compact")[0](compactEvent("overflow", true), ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(sentUserMessages.length, 0);
	}));

test("a manual /compact is housekeeping, not an interruption", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		await extension.handlers.get("session_compact")[0](compactEvent("manual", false), ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(sentUserMessages.length, 0);
	}));

test("a failed compaction never shrunk the context, so it never continues", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		// No session_compact -- compaction failed or was aborted instead.
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(sentUserMessages.length, 0);
	}));

test("a pre-prompt compaction is not a continuation: before_agent_start clears the pending flag", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		// Compaction ran before the user's prompt (the pre-prompt check)...
		await extension.handlers.get("session_compact")[0](compactEvent("threshold", false), ctx);
		// ...then the user's turn started.
		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE", prompt: "check the crash" }, ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(sentUserMessages.length, 0);
	}));

// ---------------------------------------------------------------------------
// The runaway guard
// ---------------------------------------------------------------------------

/** maxConsecutive continuations in a row, then a pause and no more sends. */
test("consecutive continuations pause after the limit, with one notice", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, sentUserMessages, extension } = await enabled(fixture);

		for (let i = 0; i < 12; i++) {
			await extension.handlers.get("session_compact")[0](compactEvent(), ctx);
			await extension.handlers.get("agent_settled")[0]({}, ctx);
		}

		assert.equal(sentUserMessages.length, 10, "maxConsecutive sends, then pause");
		const notices = calls.notify.filter((n) => /paused after/.test(n.message));
		assert.equal(notices.length, 1, "the pause notice is shown once, not every settle");
	}));

test("the pause lifts and the count resets on a real prompt", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		for (let i = 0; i < 10; i++) {
			await extension.handlers.get("session_compact")[0](compactEvent(), ctx);
			await extension.handlers.get("agent_settled")[0]({}, ctx);
		}
		// A real prompt passes through before_agent_start...
		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE", prompt: "actually, also check the heap" }, ctx);
		// ...and the next compaction continues again.
		await extension.handlers.get("session_compact")[0](compactEvent(), ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(sentUserMessages.length, 11);
		assert.equal(lastSent(sentUserMessages).content, "continue");
	}));

test("the extension's own continuations do not reset their own count", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		for (let i = 0; i < 5; i++) {
			await extension.handlers.get("session_compact")[0](compactEvent(), ctx);
			await extension.handlers.get("agent_settled")[0]({}, ctx);
			// The next turn starts with exactly the continuation message.
			await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE", prompt: "continue" }, ctx);
		}

		assert.equal(sentUserMessages.length, 5);
	}));

test("the continuation message is configurable", needsPi, () =>
	withFixture({}, async (fixture) => {
		fs.writeFileSync(
			path.join(fixture.agentDir, "pi-auto-continue.json"),
			JSON.stringify({ message: "continue where you left off" }),
		);
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		await extension.handlers.get("session_compact")[0](compactEvent(), ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(lastSent(sentUserMessages).content, "continue where you left off");
	}));

// ---------------------------------------------------------------------------
// Toggle and persistence
// ---------------------------------------------------------------------------

test("/auto-continue with no argument toggles", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });
		await extension.handlers.get("session_start")[0]({}, ctx);

		await extension.commands.get("auto-continue").handler("", ctx);
		assert.match(lastNotify(calls).message, /enabled/);

		await extension.commands.get("auto-continue").handler("", ctx);
		assert.match(lastNotify(calls).message, /disabled/);
	}));

test("/auto-continue sets the footer status and state explicitly", needsPi, () =>
	withFixture({}, async (fixture) => {
		const rec = recordingTheme;
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries, theme: rec.theme });
		await extension.handlers.get("session_start")[0]({}, ctx);

		await extension.commands.get("auto-continue").handler("on", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "auto-continue", value: "· ↻ auto-continue" });
		assert.equal(rec.fgCalls.find((c) => c.text === "↻")?.color, "accent");

		await extension.commands.get("auto-continue").handler("off", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "auto-continue", value: undefined });
	}));

test("turning it off mid-run suppresses an already-pending continuation", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		await extension.handlers.get("session_compact")[0](compactEvent(), ctx);
		await extension.commands.get("auto-continue").handler("off", ctx);
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(sentUserMessages.length, 0);
	}));

test("the toggle survives a simulated reload", needsPi, () =>
	withFixture({}, async (fixture) => {
		const first = await loadExtension(EXT, fixture);
		const { ctx: ctx1 } = makeContext(fixture, { entries: first.entries });
		await first.extension.handlers.get("session_start")[0]({}, ctx1);
		await first.extension.commands.get("auto-continue").handler("on", ctx1);

		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2, calls } = makeContext(fixture, { entries: first.entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);
		await second.extension.handlers.get("session_compact")[0](compactEvent(), ctx2);
		await second.extension.handlers.get("agent_settled")[0]({}, ctx2);

		assert.equal(second.sentUserMessages.length, 1);
		assert.deepEqual(calls.status.at(-1), { key: "auto-continue", value: "· ↻ auto-continue" });
	}));

test("a fresh load never carries a stale pending flag into a normal settle", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, sentUserMessages, extension } = await enabled(fixture);

		// No compaction happened in this turn at all.
		await extension.handlers.get("agent_settled")[0]({}, ctx);

		assert.equal(sentUserMessages.length, 0);
	}));