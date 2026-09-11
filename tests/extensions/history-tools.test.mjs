/**
 * history-tools: line-addressed recovery over the session history, working
 * by default in any session -- independent of rolling-context's fade
 * (ADR-0024). `/history-tools off` is the user's lever when the agent
 * overuses them, and it rides its own per-session `custom` entry so it
 * survives a reload.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { loadExtension, makeContext, needsPi, withFixture } from "./harness.mjs";

const EXT = "extensions/history-tools/index.ts";

const lastNotify = (calls) => calls.notify.at(-1);

/** A fake `SessionEntry` of type "message", the shape `getBranch()` returns. */
function msg(role, text) {
	return { type: "message", message: { role, content: [{ type: "text", text }] } };
}

const HISTORY = [msg("user", "please find the license check"), msg("assistant", "looking at sub_401000 now")];

/** Load the extension and run session_start -- the tools work with no enabling. */
async function started(fixture, ctxOptions = {}) {
	const loaded = await loadExtension(EXT, fixture);
	const made = makeContext(fixture, { entries: loaded.entries, ...ctxOptions });
	await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
	return { ...loaded, ...made };
}

// ---------------------------------------------------------------------------
// Shape, and on by default
// ---------------------------------------------------------------------------

test("registers every command and tool the README documents", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);

		assert.deepEqual([...extension.commands.keys()].sort(), ["history-tools"]);
		assert.deepEqual([...extension.tools.keys()].sort(), ["history_index", "history_read", "history_search"]);
	}));

test("the tools work in a fresh session with no enabling step", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_index").definition;

		const result = await tool.execute("id", {}, undefined, undefined, ctx);

		assert.equal(result.details.totalEntries, HISTORY.length);
	}));

// ---------------------------------------------------------------------------
// index / search / read
// ---------------------------------------------------------------------------

test("history_index lists one entry per message", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_index").definition;

		const result = await tool.execute("id", {}, undefined, undefined, ctx);

		assert.match(result.content[0].text, /license check/);
		assert.match(result.content[0].text, /sub_401000/);
	}));

test("history_search finds a hit and shows context lines", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_search").definition;

		const result = await tool.execute("id", { query: "sub_401000" }, undefined, undefined, ctx);

		assert.equal(result.details.hitCount, 1);
		assert.match(result.content[0].text, /sub_401000/);
	}));

test("history_search with no hits says so plainly", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_search").definition;

		const result = await tool.execute("id", { query: "definitely not present" }, undefined, undefined, ctx);

		assert.equal(result.content[0].text, "No matches.");
	}));

test("history_read returns a line range", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_read").definition;

		const result = await tool.execute("id", { startLine: 0, endLine: 0 }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /^0 \|/);
	}));

test("history_read out of range says so", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture, { branch: HISTORY });
		const tool = extension.tools.get("history_read").definition;

		const result = await tool.execute("id", { startLine: 500, endLine: 600 }, undefined, undefined, ctx);

		assert.equal(result.content[0].text, "Out of range.");
	}));

// ---------------------------------------------------------------------------
// The per-session toggle
// ---------------------------------------------------------------------------

test("/history-tools with no argument toggles", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });
		await extension.handlers.get("session_start")[0]({}, ctx);

		await extension.commands.get("history-tools").handler("", ctx);
		assert.match(lastNotify(calls).message, /disabled/);

		await extension.commands.get("history-tools").handler("", ctx);
		assert.match(lastNotify(calls).message, /enabled/);
	}));

test("a disabled history tool says so instead of doing anything", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture, { branch: HISTORY });

		await extension.commands.get("history-tools").handler("off", ctx);
		const tool = extension.tools.get("history_search").definition;

		const result = await tool.execute("id", { query: "anything" }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /disabled.*\/history-tools on/s);
	}));

test("/history-tools off persists across a simulated reload", needsPi, () =>
	withFixture({}, async (fixture) => {
		const first = await loadExtension(EXT, fixture);
		const { ctx: ctx1 } = makeContext(fixture, { entries: first.entries });
		await first.extension.handlers.get("session_start")[0]({}, ctx1);
		await first.extension.commands.get("history-tools").handler("off", ctx1);

		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2 } = makeContext(fixture, { entries: first.entries, branch: HISTORY });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);
		const tool = second.extension.tools.get("history_read").definition;

		const result = await tool.execute("id", { startLine: 0, endLine: 0 }, undefined, undefined, ctx2);

		assert.match(result.content[0].text, /disabled/);
	}));

test("/history-tools on turns them back on explicitly", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await started(fixture, { branch: HISTORY });

		await extension.commands.get("history-tools").handler("off", ctx);
		await extension.commands.get("history-tools").handler("on", ctx);
		const tool = extension.tools.get("history_index").definition;

		const result = await tool.execute("id", {}, undefined, undefined, ctx);

		// +2: the two toggle writes themselves ride the branch as (contentless)
		// custom entries, and the index counts entries, not just content -- a
		// real session carries those too, so this counts them rather than
		// special-casing the fixture setup out of the branch (the same stance
		// the pre-split suite took for its own state writes).
		assert.equal(result.details.totalEntries, HISTORY.length + 2);
	}));