/**
 * status: does the footer say what is actually running?
 *
 * Driven against the real CLI (docs/adr/0012), over a fixture catalogue whose
 * service probes are `sh -c` one-liners, so "up", "down" and "declared but not
 * installed" are the same three states on every machine.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import {
	CLI_GARBAGE,
	CLI_MISSING,
	CLI_OK,
	loadExtension,
	makeContext,
	needsPi,
	piTui,
	SERVICE_TOOLS,
	withFixture,
} from "./harness.mjs";

const EXT = "extensions/status/index.ts";
const opts = { tools: SERVICE_TOOLS };

const lastStatus = (calls) => calls.status.at(-1)?.value;
const lastWidget = (calls) => calls.widgets.at(-1);

/** Run session_start, which is what first fills the footer. */
async function start(fixture, ctxOptions) {
	const loaded = await loadExtension(EXT, fixture);
	const made = makeContext(fixture, ctxOptions);
	await loaded.extension.handlers.get("session_start")[0]({}, made.ctx);
	return { ...loaded, ...made };
}

// ---------------------------------------------------------------------------
// The footer
// ---------------------------------------------------------------------------

test("the footer reports each service by what it is doing", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { calls } = await start(fixture);

		// A count is the interesting part when there is one; "up" is the
		// fallback for a service with nothing to count.
		assert.match(lastStatus(calls), /answering:2 devices/);
		assert.match(lastStatus(calls), /refusing:down/);
	}));

test("a service that is down is named first", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { calls } = await start(fixture);

		// Down is the state that means "do something", and the footer is read
		// at a glance from the left.
		assert.ok(
			lastStatus(calls).indexOf("refusing") < lastStatus(calls).indexOf("answering"),
			lastStatus(calls),
		);
	}));

/**
 * Two services and no more, so the footer stays itemised. Against the fuller
 * fixture an extra entry pushes the line past its budget and it collapses to
 * counts, which would hide the very thing this test is looking for.
 */
const ONE_PRESENT_ONE_ABSENT = `
version = 1

[probe]
timeout = 5.0

[tool.answering]
name    = "Answering"
desc    = "a service that answers"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo 'a device'"], label = "answering", count = { pattern = 'device$', noun = "device" } }

[tool.uninstalled]
name    = "Uninstalled"
desc    = "declares a service but is not here"
invoke  = "reactor-absent-by-design"
detect  = { binary = "reactor-absent-by-design" }
service = { probe = ["sh", "-c", "exit 0"], label = "uninstalled" }
`;

test("a tool that is not installed is left out of the footer", needsPi, () =>
	withFixture({ tools: ONE_PRESENT_ONE_ABSENT }, async (fixture) => {
		const { calls } = await start(fixture);

		// `uninstalled` declares a service probe, but there is nothing here to
		// run it against -- reporting it would be a status for a thing that
		// does not exist.
		assert.equal(lastStatus(calls), "answering:1 device");
	}));

/** Six services, each rendering ~12 characters, is well past the footer budget. */
const MANY_SERVICES =
	"version = 1\n\n[probe]\ntimeout = 5.0\n" +
	Array.from(
		{ length: 6 },
		(_, i) => `
[tool.s${i}]
name    = "S${i}"
desc    = "service ${i}"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo 'a device'; echo 'b device'"], label = "s${i}", count = { pattern = 'device$', noun = "device" } }
`,
	).join("");

test("the footer collapses to counts rather than truncating a number", needsPi, () =>
	withFixture({ tools: MANY_SERVICES }, async (fixture) => {
		const { calls } = await start(fixture);

		// A cut `s5:12 devices` reads as `s5:1 device`, which is worse than no
		// detail at all -- so past the budget the line stops itemising entirely.
		assert.equal(lastStatus(calls), "6 up");
		assert.ok(piTui.visibleWidth(lastStatus(calls)) <= 44);
	}));

test("no installed service means no footer entry at all", needsPi, () =>
	withFixture({ tools: "version = 1\n" }, async (fixture) => {
		const { calls } = await start(fixture);

		// An empty catalogue is a real state -- before `scripts/install.py`
		// seeds anything -- and an empty label in the footer is clutter.
		assert.equal(lastStatus(calls), undefined);
	}));

// ---------------------------------------------------------------------------
// hiddenServices (ADR-0016)
// ---------------------------------------------------------------------------

test("a hidden service is left out of the footer, the others are not", needsPi, () =>
	withFixture({ ...opts, agentSettings: { hiddenServices: ["answering"] } }, async (fixture) => {
		const { calls } = await start(fixture);

		assert.doesNotMatch(lastStatus(calls), /answering/);
		assert.match(lastStatus(calls), /refusing:down/);
	}));

test("a hidden service is left out of the panel too", needsPi, () =>
	withFixture({ ...opts, agentSettings: { hiddenServices: ["answering"] } }, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		await extension.commands.get("reactor-status").handler("", ctx);

		const panel = lastWidget(calls).lines(80).join("\n");
		assert.doesNotMatch(panel, /answering/);
		assert.match(panel, /refusing.*down/);
	}));

test("an empty hiddenServices hides nothing, same as no reactor.json at all", needsPi, () =>
	withFixture({ ...opts, agentSettings: { hiddenServices: [] } }, async (fixture) => {
		const { calls } = await start(fixture);

		assert.match(lastStatus(calls), /answering:2 devices/);
	}));

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

test("the command toggles a panel above the editor", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		const command = extension.commands.get("reactor-status");

		await command.handler("", ctx);
		const widget = lastWidget(calls);
		assert.equal(widget.key, "reactor-status");
		assert.equal(widget.options.placement, "aboveEditor");
		const panel = widget.lines(80).join("\n");
		assert.match(panel, /answering.*2 devices/);
		assert.match(panel, /refusing.*down/);
		assert.match(panel, /uninstalled.*not installed/);

		await command.handler("", ctx);
		assert.equal(lastWidget(calls).cleared, true);
	}));

test("hide clears the panel whether or not it was up", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);

		await extension.commands.get("reactor-status").handler("hide", ctx);

		assert.equal(lastWidget(calls).cleared, true);
	}));

test("refresh leaves the panel up rather than toggling it away", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		const command = extension.commands.get("reactor-status");
		await command.handler("", ctx);

		await command.handler("refresh", ctx);

		// You asked to look at fresh data; hiding it would be perverse.
		assert.equal(lastWidget(calls).cleared, false);
	}));

test("the panel repaints on the next turn while it is up", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		await extension.commands.get("reactor-status").handler("", ctx);
		const before = calls.widgets.length;

		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);

		assert.ok(calls.widgets.length > before, "a turn did not repaint the panel");
	}));

test("no panel is drawn while it is hidden", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		const before = calls.widgets.length;

		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);

		assert.equal(calls.widgets.length, before);
	}));

test("no rendered panel line is wider than the width it was given", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		await extension.commands.get("reactor-status").handler("", ctx);

		for (const width of [20, 40, 80, 200]) {
			for (const line of lastWidget(calls).lines(width)) {
				assert.ok(piTui.visibleWidth(line) <= width, `${JSON.stringify(line)} at ${width}`);
			}
		}
	}));

// ---------------------------------------------------------------------------
// Failure modes
// ---------------------------------------------------------------------------

test("a failed probe keeps the last good reading", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		const good = lastStatus(calls);
		fixture.mode = CLI_MISSING;

		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);

		// A CLI that did not answer is not evidence that everything went down.
		assert.equal(lastStatus(calls), good);
	}));

test("output that is not JSON does not throw", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		const good = lastStatus(calls);
		fixture.mode = CLI_GARBAGE;

		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);

		assert.equal(lastStatus(calls), good);
	}));

test("asking for a panel with no CLI says so and leaves nothing behind", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await loadExtension(EXT, fixture).then(async (l) => ({
			...l,
			...makeContext(fixture),
		}));
		fixture.mode = CLI_MISSING;

		await extension.commands.get("reactor-status").handler("", ctx);

		assert.equal(calls.notify.at(-1).level, "error");
		assert.match(calls.notify.at(-1).message, /reactor doctor/);
		assert.equal(lastWidget(calls).cleared, true);
		fixture.mode = CLI_OK;
	}));

test("an unknown subcommand is refused without touching the CLI", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		const before = calls.widgets.length;

		await extension.commands.get("reactor-status").handler("refesh", ctx);

		assert.equal(calls.notify.at(-1).level, "error");
		assert.match(calls.notify.at(-1).message, /unknown subcommand "refesh"/);
		assert.equal(calls.widgets.length, before);
	}));

// ---------------------------------------------------------------------------
// Outside a TUI
// ---------------------------------------------------------------------------

test("outside a TUI it answers in one line and draws nothing", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		// rpc, not print: hasUI is true here, and there is still no terminal
		// for setWidget to draw into.
		const { ctx, calls } = makeContext(fixture, { mode: "rpc" });

		await extension.commands.get("reactor-status").handler("", ctx);

		assert.equal(calls.widgets.length, 0);
		assert.equal(calls.status.length, 0);
		assert.equal(calls.notify.at(-1).level, "info");
		assert.match(calls.notify.at(-1).message, /answering: 2 devices/);
		assert.match(calls.notify.at(-1).message, /uninstalled: not installed/);
	}));

test("a turn outside a TUI touches no UI at all", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { mode: "print" });

		await extension.handlers.get("session_start")[0]({}, ctx);
		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);

		assert.deepEqual([calls.status.length, calls.widgets.length, calls.notify.length], [0, 0, 0]);
	}));

test("argument completion offers every subcommand and filters by prefix", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const complete = extension.commands.get("reactor-status").getArgumentCompletions;

		assert.deepEqual(
			complete("").map((c) => c.value),
			["refresh", "hide", "mute", "unmute"],
		);
		assert.deepEqual(
			complete("h").map((c) => c.value),
			["hide"],
		);
	}));

// ---------------------------------------------------------------------------
// mute / unmute (ADR-0016)
// ---------------------------------------------------------------------------

test("mute writes hiddenServices and the next refresh drops the row", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);

		await extension.commands.get("reactor-status").handler("mute answering", ctx);

		assert.deepEqual(fixture.readAgentSettings(), { hiddenServices: ["answering"] });
		assert.match(calls.notify.at(-1).message, /answering muted/);

		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);
		assert.doesNotMatch(lastStatus(calls), /answering/);
	}));

test("unmute reverses it", needsPi, () =>
	withFixture({ ...opts, agentSettings: { hiddenServices: ["answering"] } }, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		assert.doesNotMatch(lastStatus(calls), /answering/);

		await extension.commands.get("reactor-status").handler("unmute answering", ctx);

		assert.deepEqual(fixture.readAgentSettings(), { hiddenServices: [] });
		await extension.handlers.get("before_agent_start")[0]({ systemPrompt: "" }, ctx);
		assert.match(lastStatus(calls), /answering:2 devices/);
	}));

test("mute repaints an already-open panel immediately", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		await extension.commands.get("reactor-status").handler("", ctx);
		assert.match(lastWidget(calls).lines(80).join("\n"), /answering/);

		await extension.commands.get("reactor-status").handler("mute answering", ctx);

		assert.doesNotMatch(lastWidget(calls).lines(80).join("\n"), /answering/);
	}));

test("mute with no id is refused", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);

		await extension.commands.get("reactor-status").handler("mute", ctx);

		assert.equal(calls.notify.at(-1).level, "error");
		assert.equal(fixture.readAgentSettings(), undefined);
	}));

test("mute and unmute work outside a TUI too, since it is a preference edit", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx } = await start(fixture, { mode: "print" });

		await extension.commands.get("reactor-status").handler("mute answering", ctx);

		assert.deepEqual(fixture.readAgentSettings(), { hiddenServices: ["answering"] });
	}));
