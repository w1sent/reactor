/**
 * goal-setting: the manifest in the system prompt, and `update_steps` gated
 * on "switch on AND a goal set" -- inactive in a session with no goal (the
 * default), force-off via `/manifest off` even with a goal (ADR-0024).
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { loadExtension, makeContext, needsPi, piAiCompat, withFixture } from "./harness.mjs";

const EXT = "extensions/goal-setting/index.ts";

const lastNotify = (calls) => calls.notify.at(-1);

/** Load the extension and run session_start, without setting anything. */
async function started(fixture, ctxOptions = {}) {
	const loaded = await loadExtension(EXT, fixture);
	const made = makeContext(fixture, { entries: loaded.entries, ...ctxOptions });
	await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
	return { ...loaded, ...made };
}

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

		assert.deepEqual([...extension.commands.keys()].sort(), ["derive", "frame", "goal", "guidelines", "manifest"]);
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

test("/manifest off pauses the whole extension -- block and tool both go quiet", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension } = await withGoal(fixture, "recover the key");
		const tool = extension.tools.get("update_steps").definition;
		await tool.execute("id", { steps: [{ summary: "halfway there", status: "done" }] }, undefined, undefined, ctx);

		await extension.commands.get("manifest").handler("off", ctx);
		assert.equal(await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx), undefined);
		const gate = await tool.execute("id", { steps: [{ summary: "x", status: "done" }] }, undefined, undefined, ctx);
		assert.match(gate.content[0].text, /goal-setting is off/);
	}));

test("/manifest on resumes with the data preserved -- nothing was lost by the pause", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withGoal(fixture, "recover the key");
		const tool = extension.tools.get("update_steps").definition;
		await tool.execute("id", { steps: [{ summary: "halfway there", status: "done" }] }, undefined, undefined, ctx);
		await extension.commands.get("guidelines").handler("be careful", ctx);

		await extension.commands.get("manifest").handler("off", ctx);
		await extension.commands.get("manifest").handler("on", ctx);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.match(result.systemPrompt, /Goal: recover the key/);
		assert.match(result.systemPrompt, /halfway there/);
		assert.match(result.systemPrompt, /be careful/);
		await extension.commands.get("frame").handler("", ctx);
		assert.match(lastNotify(calls).message, /switch: on/);
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
// Derive: a direct provider call through ctx.modelRegistry -- no chat
// message, no agent loop, no tools. The fake registry stands in for pi's
// own, which resolves auth (including models.json custom providers).
// ---------------------------------------------------------------------------


/** A fake ModelRegistry: records the contexts it was handed, returns a scripted answer. */
function fakeRegistry(respond) {
	const state = { calls: [], count: 0 };
	return {
		state,
		complete: async (model, context) => {
			state.calls.push({ model, context });
			state.count += 1;
			if (typeof respond === "function") return respond(context);
			return piAiCompat.fauxAssistantMessage(respond);
		},
	};
}

async function withDeriveRegistry(fixture, respond, model = { id: "test-model", provider: "test", contextWindow: 128_000 }) {
	const registry = fakeRegistry(respond);
	const made = await started(fixture, { model, modelRegistry: registry });
	return { ...made, registry };
}

test("derive with no model selected fails cleanly and changes nothing", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("derive").handler("all", ctx);

		assert.match(lastNotify(calls).message, /no model selected/);
		assert.equal(lastNotify(calls).level, "error");
		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.equal(result, undefined);
	}));

test("derive applies goal, guidelines and steps from one registry call", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, sentUserMessages, extension } = await withDeriveRegistry(
			fixture,
			JSON.stringify({
				goal: "recover the stolen key",
				guidelines: "work on copies only",
				steps: [
					{ summary: "image the disk", status: "done" },
					{ summary: "x".repeat(200), status: "one two three four five" },
				],
			}),
		);

		await extension.commands.get("derive").handler("all", ctx);

		// Applied straight to state -- no user message, no agent turn.
		assert.equal(sentUserMessages.length, 0);
		assert.match(lastNotify(calls).message, /derive: goal: recover the stolen key, guidelines, steps: 2/);
		await extension.commands.get("frame").handler("", ctx);
		assert.match(lastNotify(calls).message, /goal: recover the stolen key/);
		// Clamping: the long summary truncates to 79 chars + ellipsis, the status to 3 words.
		assert.match(lastNotify(calls).message, new RegExp(`x{79}…`));
		assert.match(lastNotify(calls).message, /\[one two three\]/);
	}));

test("the derive call carries the derivation task and the session tail", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, extension, registry } = await withDeriveRegistry(fixture, JSON.stringify({ goal: "g" }));
		await extension.commands.get("goal").handler("seeded goal", ctx);

		await extension.commands.get("derive").handler("all", ctx);

		const context = registry.state.calls[0].context;
		assert.match(context.systemPrompt, /session manifest/);
		assert.match(context.messages[0].content, /session transcript \(tail\)/);
	}));

test("derive scopes apply only their own part", needsPi, () =>
	withFixture({}, async (fixture) => {
		const full = JSON.stringify({
			goal: "derived goal",
			guidelines: "derived guidelines",
			steps: [{ summary: "derived step", status: "todo" }],
		});
		const { ctx, calls, extension } = await withDeriveRegistry(fixture, full);

		await extension.commands.get("derive").handler("goal", ctx);
		assert.match(lastNotify(calls).message, /derive: goal: derived goal/);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.match(result.systemPrompt, /derived goal/);
		assert.doesNotMatch(result.systemPrompt, /derived guidelines/);
		assert.doesNotMatch(result.systemPrompt, /derived step/);
	}));

test("derive tolerates fenced or prose-wrapped JSON", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withDeriveRegistry(
			fixture,
			'Here is the manifest:\n```json\n{"goal": "wrapped goal"}\n```\nDone.',
		);

		await extension.commands.get("derive").handler("goal", ctx);

		assert.match(lastNotify(calls).message, /derive: goal: wrapped goal/);
	}));

test("derive with an unparseable response applies nothing", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withDeriveRegistry(fixture, "I cannot help with that.");

		await extension.commands.get("derive").handler("all", ctx);

		assert.match(lastNotify(calls).message, /not the requested JSON/);
		assert.equal(lastNotify(calls).level, "warning");
	}));

test("derive surfaces registry error responses", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withDeriveRegistry(
			fixture,
			() => piAiCompat.fauxAssistantMessage("x", { stopReason: "error", errorMessage: "No API key for provider: ollama" }),
		);

		await extension.commands.get("derive").handler("goal", ctx);

		assert.equal(lastNotify(calls).level, "error");
		assert.match(lastNotify(calls).message, /derive failed.*No API key for provider: ollama/s);
	}));

test("derive without a model registry fails cleanly", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture, { model: { id: "m", provider: "p", contextWindow: 1000 } });

		await extension.commands.get("derive").handler("all", ctx);

		assert.match(lastNotify(calls).message, /no model registry/);
		assert.equal(lastNotify(calls).level, "error");
	}));

test("derive is one call at a time", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension, registry } = await withDeriveRegistry(fixture, JSON.stringify({ goal: "one" }));

		const p1 = extension.commands.get("derive").handler("all", ctx);
		const p2 = extension.commands.get("derive").handler("all", ctx);
		await Promise.all([p1, p2]);

		assert.ok(
			calls.notify.some((n) => /already running/.test(n.message)),
			"the second concurrent call must see the busy flag",
		);
		assert.equal(registry.state.count, 1, "the busy guard prevented a second provider call");
	}));

test("derive subcommands complete", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await started(fixture);

		const items = await extension.commands.get("derive").getArgumentCompletions("");
		assert.deepEqual(
			items.map((i) => i.value).sort(),
			["all", "goal", "guidelines", "steps"],
		);
		assert.deepEqual(
			(await extension.commands.get("derive").getArgumentCompletions("g")).map((i) => i.value).sort(),
			["goal", "guidelines"],
		);
	}));

// ---------------------------------------------------------------------------
// Clear and reset
// ---------------------------------------------------------------------------

test("/manifest clear resets goal, guidelines and steps, and stops the block", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withGoal(fixture, "recover the key");
		const tool = extension.tools.get("update_steps").definition;
		await tool.execute("id", { steps: [{ summary: "halfway there", status: "done" }] }, undefined, undefined, ctx);
		await extension.commands.get("guidelines").handler("be careful", ctx);

		await extension.commands.get("manifest").handler("clear", ctx);
		assert.match(lastNotify(calls).message, /manifest cleared/);

		// No goal -> no block, and update_steps is gated again.
		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.equal(result, undefined);
		const gate = await tool.execute("id", { steps: [{ summary: "x", status: "done" }] }, undefined, undefined, ctx);
		assert.match(gate.content[0].text, /inactive until a session goal is set/);
		await extension.commands.get("frame").handler("", ctx);
		assert.match(lastNotify(calls).message, /goal: \(none\)/);
		assert.match(lastNotify(calls).message, /\(none\)/);
	}));

test("/goal clear removes only the goal; guidelines keep flowing", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withGoal(fixture, "recover the key");
		await extension.commands.get("guidelines").handler("never touch prod", ctx);

		await extension.commands.get("goal").handler("clear", ctx);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.doesNotMatch(result.systemPrompt, /## Session Manifest/);
		assert.match(result.systemPrompt, /never touch prod/);
	}));

test("/guidelines clear removes only the guidelines", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withGoal(fixture, "recover the key");
		await extension.commands.get("guidelines").handler("never touch prod", ctx);

		await extension.commands.get("guidelines").handler("clear", ctx);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "BASE" }, ctx);
		assert.match(result.systemPrompt, /## Session Manifest/);
		assert.doesNotMatch(result.systemPrompt, /never touch prod/);
	}));

test("/manifest completions offer on, off and clear", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await started(fixture);

		const items = await extension.commands.get("manifest").getArgumentCompletions("");
		assert.deepEqual(
			items.map((i) => i.value).sort(),
			["clear", "off", "on"],
		);
		assert.match(items.find((i) => i.value === "clear").description, /clear goal, guidelines and steps/);
	}));

test("/manifest with an unknown argument warns", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("manifest").handler("maybe", ctx);

		assert.equal(lastNotify(calls).level, "warning");
		assert.match(lastNotify(calls).message, /unknown argument/);
	}));

// ---------------------------------------------------------------------------
// Footer indicator: on only while active AND set
// ---------------------------------------------------------------------------

const lastStatus = (calls) => calls.status.at(-1);

test("setting a goal shows a footer indicator with a truncated snippet", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("goal").handler("recover the stolen certificate key", ctx);

		assert.deepEqual(calls.status.at(-1), { key: "goal-setting", value: "goal: recover the stolen certificate key" });
	}));

test("the indicator truncates a long goal", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("goal").handler("y".repeat(120), ctx);

		assert.deepEqual(calls.status.at(-1), { key: "goal-setting", value: `goal: ${"y".repeat(47)}…` });
	}));

test("clearing everything clears the footer; a fresh session shows nothing", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withGoal(fixture, "recover the key");
		await extension.commands.get("goal").handler("clear", ctx);

		assert.deepEqual(calls.status.at(-1), { key: "goal-setting", value: undefined });
	}));

test("guidelines-only sessions show the manifest indicator", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await started(fixture);

		await extension.commands.get("guidelines").handler("never touch prod", ctx);

		assert.deepEqual(calls.status.at(-1), { key: "goal-setting", value: "manifest: set" });
		await extension.commands.get("guidelines").handler("clear", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "goal-setting", value: undefined });
	}));

test("a paused extension shows nothing in the footer, even with content", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withGoal(fixture, "recover the key");

		await extension.commands.get("manifest").handler("off", ctx);
		assert.deepEqual(calls.status.at(-1), { key: "goal-setting", value: undefined });

		await extension.commands.get("manifest").handler("on", ctx);
		assert.match(calls.status.at(-1).value, /goal: recover the key/);
	}));

test("/manifest clear clears the footer", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withGoal(fixture, "recover the key");

		await extension.commands.get("manifest").handler("clear", ctx);

		assert.deepEqual(calls.status.at(-1), { key: "goal-setting", value: undefined });
	}));

test("derive updates the footer with the derived goal", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { ctx, calls, extension } = await withDeriveRegistry(fixture, JSON.stringify({ goal: "derived from session" }));

		await extension.commands.get("derive").handler("goal", ctx);

		assert.deepEqual(calls.status.at(-1), { key: "goal-setting", value: "goal: derived from session" });
	}));

test("session_start restores the footer from the session state", needsPi, () =>
	withFixture({}, async (fixture) => {
		const first = await loadExtension(EXT, fixture);
		const { ctx: ctx1 } = makeContext(fixture, { entries: first.entries });
		await first.extension.handlers.get("session_start")[0]({}, ctx1);
		await first.extension.commands.get("goal").handler("survives a reload", ctx1);

		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2, calls } = makeContext(fixture, { entries: first.entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);

		assert.deepEqual(calls.status.at(-1), { key: "goal-setting", value: "goal: survives a reload" });
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