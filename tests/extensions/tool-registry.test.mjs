/**
 * tool-registry: does the agent get told the truth about this machine?
 *
 * Everything here runs against the real CLI behind a PATH shim (docs/adr/0012),
 * so a passing test means the extension works with the JSON `reactor` actually
 * emits, not with a transcription of it.
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
	CLI_ERROR,
	CLI_GARBAGE,
	CLI_MISSING,
	loadExtension,
	makeContext,
	needsPi,
	withFixture,
} from "./harness.mjs";

const EXT = "extensions/tool-registry/index.ts";

/** The last status the extension pushed, which is what the footer shows. */
const lastStatus = (calls) => calls.status.at(-1)?.value;

// ---------------------------------------------------------------------------
// The injection
// ---------------------------------------------------------------------------

test("the registry block is appended to the system prompt, not substituted for it", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);

		const result = await extension.handlers
			.get("before_agent_start")[0]({ systemPrompt: "ORIGINAL PROMPT" }, ctx);

		assert.ok(result.systemPrompt.startsWith("ORIGINAL PROMPT"));
		assert.match(result.systemPrompt, /unpacks alpha containers/);
		assert.match(result.systemPrompt, /diffs gamma firmware images/);
	}));

test("an absent tool is not advertised", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);

		// beta is active but not installed. Naming it would send the agent
		// looking for something that is not there, which is the exact failure
		// the registry exists to prevent.
		assert.doesNotMatch(result.systemPrompt, /traces beta processes/);
	}));

test("deactivating narrows the block", needsPi, () =>
	withFixture({ state: { toolsets: ["pair"] } }, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);

		assert.match(result.systemPrompt, /unpacks alpha containers/);
		assert.doesNotMatch(result.systemPrompt, /diffs gamma firmware images/);
		// Present counts the active tools; catalogued counts the whole file, so
		// the footer keeps saying how much is being held back.
		assert.equal(lastStatus(calls), "RE 1/3");
	}));

test("the status line reports presence against the whole catalogue", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);

		assert.equal(lastStatus(calls), "RE 2/3");
	}));

// ---------------------------------------------------------------------------
// Failure modes
// ---------------------------------------------------------------------------

test("a failed probe keeps the last good block", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		const handler = extension.handlers.get("before_agent_start")[0];

		const good = await handler({ systemPrompt: "P" }, ctx);
		fixture.mode = CLI_MISSING;
		const after = await handler({ systemPrompt: "P" }, ctx);

		// An unreachable CLI is not evidence the tools vanished. Emptying the
		// block would tell the agent something false.
		assert.equal(after.systemPrompt, good.systemPrompt);
	}));

test("with no good block ever, the prompt is left alone", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		fixture.mode = CLI_MISSING;

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "P" }, ctx);

		// Returning void, rather than the prompt unchanged, is what lets pi tell
		// "nothing to add" from "here is your prompt back".
		assert.equal(result, undefined);
	}));

test("a missing CLI is announced once, not every turn", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);
		fixture.mode = CLI_MISSING;
		const handler = extension.handlers.get("before_agent_start")[0];

		await handler({ systemPrompt: "" }, ctx);
		await handler({ systemPrompt: "" }, ctx);
		await handler({ systemPrompt: "" }, ctx);

		// REactor not being installed is a valid state for a pi session.
		const unavailable = calls.status.filter((s) => s.value === "reactor: CLI unavailable");
		assert.equal(unavailable.length, 1);
	}));

test("output that is not JSON says so instead of throwing", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);
		fixture.mode = CLI_GARBAGE;

		const result = await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);

		assert.equal(result, undefined);
		assert.equal(lastStatus(calls), "reactor: unreadable output");
	}));

test("an error payload is surfaced verbatim", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);
		fixture.mode = CLI_ERROR;

		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);

		assert.equal(lastStatus(calls), "reactor: probe exploded");
	}));

// ---------------------------------------------------------------------------
// Skills
// ---------------------------------------------------------------------------

test("session_start warms the cache so the first turn does not pay for it", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		const cache = path.join(fixture.dir, "cache.json");
		assert.equal(fs.existsSync(cache), false);

		await extension.handlers.get("session_start")[0]({}, ctx);

		assert.equal(fs.existsSync(cache), true);
	}));

test("resources_discover reads the cache rather than probing", needsPi, () =>
	withFixture({ skills: ["alpha"] }, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		const discover = extension.handlers.get("resources_discover")[0];

		// Cold, `--cached` can only report "unknown", and an unknown tool is not
		// a present one -- so there is nothing to contribute yet.
		assert.equal(await discover({}, ctx), undefined);

		await extension.handlers.get("session_start")[0]({}, ctx);

		const result = await discover({}, ctx);
		assert.deepEqual(result.skillPaths, [path.join(fixture.dir, "skills", "alpha")]);
	}));

test("a deactivated tool's skill is withdrawn", needsPi, () =>
	withFixture({ skills: ["alpha"], state: { toolsets: ["static"] } }, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		await extension.handlers.get("session_start")[0]({}, ctx);
		const discover = extension.handlers.get("resources_discover")[0];
		assert.equal((await discover({}, ctx)).skillPaths.length, 1);

		fixture.writeState({ toolsets: ["static"], tools: { enabled: [], disabled: ["alpha"] } });

		// Gating skills is the whole reason activation is worth having: a
		// deactivated tool must stop costing description tokens too.
		assert.equal(await discover({}, ctx), undefined);
	}));

test("an unfetched skill contributes no path", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture);
		await extension.handlers.get("session_start")[0]({}, ctx);

		// alpha declares a skill in the catalogue but nothing has fetched it.
		assert.equal(await extension.handlers.get("resources_discover")[0]({}, ctx), undefined);
	}));

// ---------------------------------------------------------------------------
// The /reactor command
// ---------------------------------------------------------------------------

test("/reactor show displays the block without reloading resources", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, sent, guard } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { guard });

		await extension.commands.get("reactor").handler("", ctx);

		assert.equal(sent.length, 1);
		assert.equal(sent[0].customType, "reactor-registry");
		assert.match(sent[0].content, /unpacks alpha containers/);
		assert.equal(calls.reloads, 0);
	}));

test("/reactor refresh sends the block, then reloads last", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, sent, guard } = await loadExtension(EXT, fixture);
		// Shared `guard`: if the handler touched `ctx` or `pi` after
		// `await ctx.reload()`, this throws the same error pi's own runtime
		// does instead of quietly succeeding against a mock that never goes
		// stale.
		const { ctx, calls } = makeContext(fixture, { guard });

		await extension.commands.get("reactor").handler("refresh", ctx);

		// A refresh that changed what is present has changed which skills are
		// eligible, so discovery has to run again.
		assert.equal(calls.reloads, 1);
		assert.equal(sent.length, 1);
	}));

test("an unknown subcommand is refused without touching the CLI", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, sent } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor").handler("refesh", ctx);

		assert.equal(sent.length, 0);
		assert.equal(calls.notify.at(-1).level, "error");
		assert.match(calls.notify.at(-1).message, /unknown subcommand "refesh"/);
	}));

test("/reactor points at the doctor when the CLI is unreachable", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, sent } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);
		fixture.mode = CLI_MISSING;

		await extension.commands.get("reactor").handler("", ctx);

		assert.equal(sent.length, 0);
		assert.equal(calls.notify.at(-1).level, "error");
		assert.match(calls.notify.at(-1).message, /reactor doctor/);
	}));

test("argument completion offers both subcommands and filters by prefix", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const complete = extension.commands.get("reactor").getArgumentCompletions;

		assert.deepEqual(
			complete("").map((c) => c.value),
			["refresh", "show"],
		);
		assert.deepEqual(
			complete("re").map((c) => c.value),
			["refresh"],
		);
	}));

// ---------------------------------------------------------------------------
// The toolbox toggle (ADR-0016)
// ---------------------------------------------------------------------------

test("toolbox: false in reactor.json registers no handler and only the toggle command", needsPi, () =>
	withFixture({ agentSettings: { toolbox: false } }, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);

		// reactor-toolbox survives being off -- it is the only way back on.
		assert.deepEqual([...extension.commands.keys()], ["reactor-toolbox"]);
		assert.equal(extension.handlers.size, 0);
	}));

test("an unreadable reactor.json is treated as toolbox: true", needsPi, () =>
	withFixture({}, async (fixture) => {
		// No agentSettings written -- the fixture's agent dir carries no
		// reactor.json at all, the same as a machine that never set one.
		const { extension } = await loadExtension(EXT, fixture);

		assert.ok(extension.commands.has("reactor"));
	}));

test("reactor-toolbox with no argument reports the current state", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor-toolbox").handler("", ctx);

		assert.match(calls.notify.at(-1).message, /toolbox is on/);
	}));

test("reactor-toolbox off writes reactor.json and reloads last, not touching ctx after", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, guard } = await loadExtension(EXT, fixture);
		// Shared `guard`: catches exactly the crash a real user hit --
		// `ctx.ui.notify` called after `await ctx.reload()`, which pi's own
		// runtime refuses with STALE_CTX_MESSAGE.
		const { ctx, calls } = makeContext(fixture, { guard });

		await extension.commands.get("reactor-toolbox").handler("off", ctx);

		assert.deepEqual(fixture.readAgentSettings(), { toolbox: false });
		assert.equal(calls.reloads, 1);
		assert.match(calls.notify.at(-1).message, /toolbox is now off/);
	}));

test("reactor-toolbox on works from a session where the toolbox is off", needsPi, () =>
	withFixture({ agentSettings: { toolbox: false } }, async (fixture) => {
		const { extension, guard } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { guard });

		// The command that lives outside the gate is exactly the one that
		// needs to work while the gate is shut.
		await extension.commands.get("reactor-toolbox").handler("on", ctx);

		assert.deepEqual(fixture.readAgentSettings(), { toolbox: true });
		assert.match(calls.notify.at(-1).message, /toolbox is now on/);
	}));

test("reactor-toolbox off does not clobber hiddenServices already on disk", needsPi, () =>
	withFixture({ agentSettings: { hiddenServices: ["adb"] } }, async (fixture) => {
		const { extension, guard } = await loadExtension(EXT, fixture);
		const { ctx } = makeContext(fixture, { guard });

		await extension.commands.get("reactor-toolbox").handler("off", ctx);

		assert.deepEqual(fixture.readAgentSettings(), { hiddenServices: ["adb"], toolbox: false });
	}));

test("reactor-toolbox rejects an argument that is not on or off", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor-toolbox").handler("maybe", ctx);

		assert.equal(fixture.readAgentSettings(), undefined);
		assert.equal(calls.notify.at(-1).level, "error");
	}));

test("reactor-toolbox argument completion offers on and off", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const complete = extension.commands.get("reactor-toolbox").getArgumentCompletions;

		assert.deepEqual(
			complete("").map((c) => c.value),
			["on", "off"],
		);
	}));
