/**
 * scenario: does reactor_phase_complete actually advance, and restore across a reload?
 *
 * Uses a throwaway two-step scenario (`REACTOR_SCENARIOS_DIR`, ADR-0017)
 * rather than this package's own shipped `investigation` scenario for the state
 * machine, so assertions do not break because someone reworded a step's
 * prose. One test runs the real shipped scenario end to end and checks its
 * *shape* only, the way `TestShippedConfig` does for the catalogue.
 */

import assert from "node:assert/strict";
import path from "node:path";
import { test } from "node:test";
import { loadExtension, makeContext, needsPi, withFixture } from "./harness.mjs";

const EXT = "extensions/scenario/index.ts";
const REPO_ROOT = path.resolve(import.meta.dirname, "../..");

/** Two steps, the first naming a toolset the default fixture actually has. */
const DEMO = {
	demo: ["---\ntitle: One\ntoolset: static\n---\nDo the first thing.", "---\ntitle: Two\n---\nDo the second thing."],
};

const lastNotify = (calls) => calls.notify.at(-1);

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

test("list names every scenario directory", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor-scenario").handler("list", ctx);

		assert.match(lastNotify(calls).message, /demo/);
	}));

test("no scenarios directory is a normal empty answer, not a crash", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor-scenario").handler("list", ctx);

		assert.match(lastNotify(calls).message, /no scenarios/);
	}));

// ---------------------------------------------------------------------------
// start / status / stop
// ---------------------------------------------------------------------------

test("start sends the first step as a message and activates its toolset", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { extension, sent, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });

		await extension.commands.get("reactor-scenario").handler("start demo", ctx);

		assert.equal(sent.length, 1);
		assert.match(sent[0].content, /Phase 1\/2: One/);
		assert.match(sent[0].content, /Do the first thing/);
		assert.deepEqual(fixture.readState()?.toolsets, ["static"]);
		assert.match(lastNotify(calls).message, /started "demo" -- step 1\/2/);
	}));

test("status reports the current step", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });
		await extension.commands.get("reactor-scenario").handler("start demo", ctx);

		await extension.commands.get("reactor-scenario").handler("status", ctx);

		assert.match(lastNotify(calls).message, /"demo" -- step 1\/2: One/);
	}));

test("bare invocation is the same as status", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor-scenario").handler("", ctx);

		assert.match(lastNotify(calls).message, /no scenario active/);
	}));

test("start refuses a second scenario while one is running", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });
		await extension.commands.get("reactor-scenario").handler("start demo", ctx);

		await extension.commands.get("reactor-scenario").handler("start demo", ctx);

		assert.equal(lastNotify(calls).level, "error");
		assert.match(lastNotify(calls).message, /already running/);
	}));

test("start refuses an unknown scenario id", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor-scenario").handler("start nope", ctx);

		assert.equal(lastNotify(calls).level, "error");
		assert.match(lastNotify(calls).message, /unknown scenario/);
	}));

test("start with no id is refused", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor-scenario").handler("start", ctx);

		assert.equal(lastNotify(calls).level, "error");
	}));

test("stop clears an active scenario", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });
		await extension.commands.get("reactor-scenario").handler("start demo", ctx);

		await extension.commands.get("reactor-scenario").handler("stop", ctx);
		await extension.commands.get("reactor-scenario").handler("status", ctx);

		assert.match(lastNotify(calls).message, /no scenario active/);
	}));

test("stop with nothing active says so rather than erroring", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor-scenario").handler("stop", ctx);

		assert.equal(lastNotify(calls).level, "info");
	}));

// ---------------------------------------------------------------------------
// reactor_phase_complete
// ---------------------------------------------------------------------------

test("the tool advances one step and returns the next briefing as content", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture, { entries });
		await extension.commands.get("reactor-scenario").handler("start demo", ctx);
		const tool = extension.tools.get("reactor_phase_complete").definition;

		const result = await tool.execute("id", { summary: "first thing done" }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /Phase 2\/2: Two/);
		assert.deepEqual(result.details, { scenarioId: "demo", stepIndex: 1, summaries: ["first thing done"] });
	}));

test("completing the last step ends the scenario", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });
		await extension.commands.get("reactor-scenario").handler("start demo", ctx);
		const tool = extension.tools.get("reactor_phase_complete").definition;
		await tool.execute("id1", { summary: "first" }, undefined, undefined, ctx);

		const result = await tool.execute("id2", { summary: "second" }, undefined, undefined, ctx);
		await extension.commands.get("reactor-scenario").handler("status", ctx);

		assert.match(result.content[0].text, /scenario "demo" complete -- 2 phase\(s\) done/);
		assert.equal(result.details, undefined);
		assert.match(lastNotify(calls).message, /no scenario active/);
	}));

test("calling the tool with no scenario active is a normal answer, not an error", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		const tool = extension.tools.get("reactor_phase_complete").definition;

		const result = await tool.execute("id", { summary: "x" }, undefined, undefined, ctx);

		assert.match(result.content[0].text, /no scenario is active/);
	}));

// ---------------------------------------------------------------------------
// next -- the manual override (ADR-0009)
// ---------------------------------------------------------------------------

test("next advances without a model tool call, and sends the briefing as a message", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { extension, sent, entries } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture, { entries });
		await extension.commands.get("reactor-scenario").handler("start demo", ctx);

		await extension.commands.get("reactor-scenario").handler("next done with one", ctx);

		assert.equal(sent.length, 2);
		assert.match(sent[1].content, /Phase 2\/2: Two/);
	}));

test("next with no summary text still advances", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { extension, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });
		await extension.commands.get("reactor-scenario").handler("start demo", ctx);

		await extension.commands.get("reactor-scenario").handler("next", ctx);
		await extension.commands.get("reactor-scenario").handler("status", ctx);

		assert.match(lastNotify(calls).message, /"demo" -- step 2\/2: Two/);
	}));

test("next requires an active scenario", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor-scenario").handler("next", ctx);

		assert.equal(lastNotify(calls).level, "error");
	}));

// ---------------------------------------------------------------------------
// Persistence across a reload (ADR-0009)
// ---------------------------------------------------------------------------

test("state survives a simulated reload via session_start", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const first = await loadExtension(EXT, fixture);
		const { ctx: ctx1 } = makeContext(fixture, { entries: first.entries });
		await first.extension.commands.get("reactor-scenario").handler("start demo", ctx1);
		const tool = first.extension.tools.get("reactor_phase_complete").definition;
		await tool.execute("id", { summary: "first" }, undefined, undefined, ctx1);

		// A fresh module instance -- what a `/reload` or a resumed session
		// gets -- reading the same entries the first instance appended.
		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2, calls } = makeContext(fixture, { entries: first.entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);
		await second.extension.commands.get("reactor-scenario").handler("status", ctx2);

		assert.match(lastNotify(calls).message, /"demo" -- step 2\/2: Two/);
	}));

test("a stop persisted before reload restores to no scenario active", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const first = await loadExtension(EXT, fixture);
		const { ctx: ctx1 } = makeContext(fixture, { entries: first.entries });
		await first.extension.commands.get("reactor-scenario").handler("start demo", ctx1);
		await first.extension.commands.get("reactor-scenario").handler("stop", ctx1);

		const second = await loadExtension(EXT, fixture);
		const { ctx: ctx2, calls } = makeContext(fixture, { entries: first.entries });
		await second.extension.handlers.get("session_start")[0]({}, ctx2);
		await second.extension.commands.get("reactor-scenario").handler("status", ctx2);

		assert.match(lastNotify(calls).message, /no scenario active/);
	}));

// ---------------------------------------------------------------------------
// The shipped scenario -- shape only, like TestShippedConfig
// ---------------------------------------------------------------------------

test("the shipped investigation scenario is a real, orderable, seventeen-step chain", needsPi, () =>
	withFixture({}, async (fixture) => {
		// Overrides the fixture's own isolated (empty) scenarios dir with the
		// real one this package ships, for this test only.
		process.env.REACTOR_SCENARIOS_DIR = path.join(REPO_ROOT, "prompts", "scenarios");
		const { extension, sent, entries } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { entries });

		await extension.commands.get("reactor-scenario").handler("list", ctx);
		assert.match(lastNotify(calls).message, /investigation/);

		await extension.commands.get("reactor-scenario").handler("start investigation", ctx);
		assert.match(sent[0].content, /Phase 1\/17:/);

		const tool = extension.tools.get("reactor_phase_complete").definition;
		let result;
		for (let i = 0; i < 16; i++) {
			result = await tool.execute(`id${i}`, { summary: `step ${i + 1} done` }, undefined, undefined, ctx);
			// Every real step declares its own title -- never the "step N"
			// placeholder a file with no frontmatter would fall back to.
			assert.doesNotMatch(result.content[0].text, /Phase \d+\/17: step \d+/);
		}
		result = await tool.execute("id16", { summary: "tooling delivered" }, undefined, undefined, ctx);
		assert.match(result.content[0].text, /scenario "investigation" complete -- 17 phase\(s\) done/);
	}));

// ---------------------------------------------------------------------------
// Tool advertisement: visible exactly while a scenario runs (ADR-0030)
// ---------------------------------------------------------------------------

test("a fresh session withdraws the tool; starting a scenario advertises it", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const { activeToolWrites, ctx, extension } = await loadExtension(EXT, fixture).then(async (l) => {
			const made = makeContext(fixture);
			await l.extension.handlers.get("session_start")[0]({}, made.ctx);
			return { ...l, ...made };
		});

		assert.ok(!activeToolWrites.at(-1).includes("reactor_phase_complete"));

		await extension.commands.get("reactor-scenario").handler("start demo", ctx);
		assert.ok(activeToolWrites.at(-1).includes("reactor_phase_complete"));
	}));

test("stopping the scenario withdraws the tool", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const loaded = await loadExtension(EXT, fixture);
		const made = makeContext(fixture);
		await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
		await loaded.extension.commands.get("reactor-scenario").handler("start demo", made.ctx);
		const before = loaded.activeToolWrites.length;

		await loaded.extension.commands.get("reactor-scenario").handler("stop", made.ctx);

		assert.ok(loaded.activeToolWrites.length > before);
		assert.ok(!loaded.activeToolWrites.at(-1).includes("reactor_phase_complete"));
	}));

test("completing the last phase withdraws the tool", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const loaded = await loadExtension(EXT, fixture);
		const made = makeContext(fixture);
		await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
		await loaded.extension.commands.get("reactor-scenario").handler("start demo", made.ctx);
		const tool = loaded.extension.tools.get("reactor_phase_complete").definition;

		await tool.execute("id", { summary: "done one" }, undefined, undefined, made.ctx);
		await tool.execute("id", { summary: "done two" }, undefined, undefined, made.ctx);

		assert.ok(!loaded.activeToolWrites.at(-1).includes("reactor_phase_complete"));
	}));

test("an advance that leaves the scenario running writes nothing", needsPi, () =>
	withFixture({ scenarios: DEMO }, async (fixture) => {
		const loaded = await loadExtension(EXT, fixture);
		const made = makeContext(fixture);
		await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
		await loaded.extension.commands.get("reactor-scenario").handler("start demo", made.ctx);
		const before = loaded.activeToolWrites.length;
		const tool = loaded.extension.tools.get("reactor_phase_complete").definition;

		await tool.execute("id", { summary: "done one" }, undefined, undefined, made.ctx);

		// The scenario still runs; the tool is still wanted. Same visibility,
		// no write, no prompt rebuild.
		assert.equal(loaded.activeToolWrites.length, before);
	}));
