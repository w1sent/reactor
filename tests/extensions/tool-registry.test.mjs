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
		const { extension, sent } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

		await extension.commands.get("reactor").handler("", ctx);

		assert.equal(sent.length, 1);
		assert.equal(sent[0].customType, "reactor-registry");
		assert.match(sent[0].content, /unpacks alpha containers/);
		assert.equal(calls.reloads, 0);
	}));

test("/reactor refresh re-runs resource discovery", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, sent } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);

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
