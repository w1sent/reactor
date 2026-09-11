/**
 * goal-setting: the manifest in the system prompt, and `update_steps` gated
 * on "switch on AND a goal set" -- inactive in a session with no goal (the
 * default), force-off via `/manifest off` even with a goal (ADR-0024).
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { loadExtension, makeContext, needsPi, withFixture } from "./harness.mjs";

const EXT = "extensions/goal-setting/index.ts";

const lastNotify = (calls) => calls.notify.at(-1);

/** Load the extension, run session_start, and set a goal. */
async function withGoal(fixture, goal = "find the crash") {
	const loaded = await loadExtension(EXT, fixture);
	const made = makeContext(fixture, { entries: loaded.entries });
	await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
	await loaded.extension.commands.get("goal").handler(goal, made.ctx);
	return { ...loaded, ...made };
}

// ---------------------------------------------------------------------------
// Shape
// ---------------------------------------------------------------------------

test("registers every command and tool the README documents", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);

		assert.deepEqual([...extension.commands.keys()].sort(), ["frame", "goal", "guidelines", "manifest"]);
		assert.deepEqual([...extension.tools.keys()].sort(), ["update_steps"]);
	}));

// ---------------------------------------------------------------------------
// The update_steps gate
// ---------------------------------------------------------------------------

test("update_steps is inactive in a fresh session, where no goal is set", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		const tool = extension.tools.get("update_steps").definition;

		const result = await tool.execute("id", { steps: [{ summary: "x", status: "done" }] }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /inactive until a session goal is set/);
		assert.match(result.content[0].text, /\/goal <text>/);
	}));

test("update_steps activates once a goal is set", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await withGoal(fixture);
		const tool = extension.tools.get("update_steps").definition;

		const result = await tool.execute("id", { steps: [{ summary: "found entry point", status: "done" }] }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /^Steps updated: 1 step\(s\)\.$/);
	}));

test("/manifest off deactivates update_steps even with a goal set", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await withGoal(fixture);
		const tool = extension.tools.get("update_steps").definition;

		await extension.commands.get("manifest").handler("off", ctx);
		const result = await tool.execute("id", { steps: [{ summary: "x", status: "done" }] }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /goal-setting is off.*\/manifest on/s);
	}));

test("the switch gates only update_steps -- the manifest block still flows with a goal set", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await withGoal(fixture);

		await extension.commands.get("manifest").handler("off", ctx);
		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.ok(result.systemPrompt.startsWith("BASE"));
		assert.match(result.systemPrompt, /## Session Manifest/);
	}));

// ---------------------------------------------------------------------------
// /manifest, /goal, /guidelines, /frame
// ---------------------------------------------------------------------------

test("/manifest with no argument toggles", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });

		await extension.commands.get("manifest").handler("", ctx);
		assert.match(lastNotify(calls).message, /disabled/);

		await extension.commands.get("manifest").handler("", ctx);
		assert.match(lastNotify(calls).message, /enabled/);
	}));

test("/manifest on and /manifest off set state explicitly rather than toggling", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });

		await extension.commands.get("manifest").handler("on", ctx);
		await extension.commands.get("manifest").handler("on", ctx);
		assert.match(lastNotify(calls).message, /enabled/);

		await extension.commands.get("manifest").handler("off", ctx);
		assert.match(lastNotify(calls).message, /disabled/);
	}));

test("/goal requires text and is reflected in /frame", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withGoal(fixture);

		await extension.commands.get("goal").handler("", ctx);
		assert.equal(lastNotify(calls).level, "warning");

		await extension.commands.get("goal").handler("find the license check", ctx);
		await extension.commands.get("frame").handler("", ctx);
		assert.match(lastNotify(calls).message, /goal: find the license check/);
	}));

test("/guidelines requires text and reaches the system prompt once a goal exists", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await withGoal(fixture);

		await extension.commands.get("guidelines").handler("never touch prod", ctx);
		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.ok(result.systemPrompt.startsWith("BASE"));
		assert.match(result.systemPrompt, /never touch prod/);
	}));

// ---------------------------------------------------------------------------
// The system-prompt block
// ---------------------------------------------------------------------------

test("before_agent_start injects nothing while there is no goal", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.equal(result, undefined);
	}));

test("the manifest block carries the goal and the steps", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await withGoal(fixture, "find the license check");
		const tool = extension.tools.get("update_steps").definition;
		await tool.execute("id", { steps: [{ summary: "found sub_401000", status: "done" }] }, undefined, undefined, ctx);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.match(result.systemPrompt, /## Session Manifest/);
		assert.match(result.systemPrompt, /Goal: find the license check/);
		assert.match(result.systemPrompt, /1\. \[done\] found sub_401000/);
	}));

test("a session with guidelines but no goal still gets the guidelines section", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture, { entries });
		await extension.handlers.get("session_start")[0]({}, ctx);
		await extension.commands.get("guidelines").handler("never touch prod", ctx);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);

		assert.ok(result.systemPrompt.startsWith("BASE"));
		assert.match(result.systemPrompt, /## Session Guidelines/);
		assert.doesNotMatch(result.systemPrompt, /## Session Manifest/);
	}));

// ---------------------------------------------------------------------------
// update_steps behaviour
// ---------------------------------------------------------------------------

test("update_steps clamps an over-long summary and an over-long status", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withGoal(fixture);
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
		const { ctx, extension } = await withGoal(fixture);
		const tool = extension.tools.get("update_steps").definition;
		const steps = Array.from({ length: 21 }, (_, i) => ({ summary: `step ${i}`, status: "todo" }));

		const result = await tool.execute("id", { steps }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /WARNING.*21.*exceeds the soft limit \(20\)/s);
	}));

test("update_steps under the soft limit carries no warning", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await withGoal(fixture);
		const tool = extension.tools.get("update_steps").definition;

		const result = await tool.execute("id", { steps: [{ summary: "one", status: "todo" }] }, undefined, undefined, ctx);

		assert.doesNotMatch(result.content[0].text, /WARNING/);
	}));

// ---------------------------------------------------------------------------
// State survives a reload (session_start restores from the branch)
// ---------------------------------------------------------------------------

test("switch, goal, guidelines and steps survive a simulated reload", needsPi, () =>
	withFixture({}, async (fixture) => {
		const first = await loadExtension(EXT, fixture);
		const { ctx: ctx1 } = makeContext(fixture, { entries: first.entries });
		await first.extension.handlers.get("session_start")[0]({}, ctx1);
		await first.extension.commands.get("goal").handler("recover the key", ctx1);
		await first.extension.commands.get("guidelines").handler("be careful", ctx1);
		const tool = first.extension.tools.get("update_steps").definition;
		await tool.execute("id", { steps: [{ summary: "found the vault", status: "in progress" }] }, undefined, undefined, ctx1);
		await first.extension.commands.get("manifest").handler("off", ctx1);

		// A fresh module instance -- what a `/reload` or a resumed session
		// gets -- reading the same entries the first instance appended.
		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2, calls } = makeContext(fixture, { entries: first.entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);
		await second.extension.commands.get("frame").handler("", ctx2);

		const frame = lastNotify(calls).message;
		assert.match(frame, /goal: recover the key/);
		assert.match(frame, /guidelines: be careful/);
		assert.match(frame, /found the vault/);
		assert.match(frame, /switch: off/);

		// ...and the gate follows the restored switch state, not the default.
		const tool2 = second.extension.tools.get("update_steps").definition;
		const result = await tool2.execute("id", { steps: [{ summary: "x", status: "done" }] }, undefined, undefined, ctx2);
		assert.match(result.content[0].text, /goal-setting is off/);
	}));