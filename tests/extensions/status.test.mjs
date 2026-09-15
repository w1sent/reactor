/**
 * status: does the statusbar say what is actually running, and does it read
 * at a glance?
 *
 * Driven against the real CLI (docs/adr/0012), over a fixture catalogue whose
 * service probes are `sh -c` one-liners, so "up", "down" and "declared but not
 * installed" are the same three states on every machine.
 *
 * The display contract under test:
 * - every service is one block -- state glyph, id, state words -- separated
 *   from the next by a dim middot; the glyph and the words carry the state
 *   colour (green up, red down, dim for unknown and not installed), so a
 *   problem is spottable without reading;
 * - a service's id is coloured from a rotation that never carries state
 *   meaning, and the footer and the panel give the same id the same colour;
 * - a footer too narrow for the whole line sheds detail first, then names,
 *   then itemises counts -- a number is never cut into a different number;
 * - the panel wraps instead of truncating: the label column goes first, then
 *   label and state words stack under the service id;
 * - mute/unmute, the toggle lifecycle and the failure modes keep working.
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
	recordingTheme,
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

/**
 * The colour a rendering asked for `text`, read off the recording theme.
 * Compared after trimming on both sides: the panel pads its id column before
 * it colours it, and the separator is coloured with its spaces attached.
 * `undefined` means the text was rendered uncoloured, which is its own
 * assertion.
 */
const colouredAs = (rec, text) => rec.fgCalls.find((c) => c.text.trim() === text.trim())?.color;

// ---------------------------------------------------------------------------
// The footer
// ---------------------------------------------------------------------------

test("the footer reports each service by what it is doing", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { calls } = await start(fixture);

		// A count is the interesting part when there is one; "up" is the
		// fallback for a service with nothing to count.
		assert.match(lastStatus(calls), /✗ refusing down/);
		assert.match(lastStatus(calls), /● answering 2 devices/);
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

test("the footer separates its blocks instead of running them together", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { calls } = await start(fixture);

		assert.match(lastStatus(calls), /down · ●/);
	}));

/**
 * Two services and no more, so the footer stays itemised. Against the fuller
 * fixture an extra entry pushes the line past its budget and it starts
 * shedding, which would hide the very thing this test is looking for.
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
		assert.equal(lastStatus(calls), "· ● answering 1 device");
	}));

/** Six services, each rendering ~14 characters, is well past the footer budget. */
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

test("a footer too wide for the details sheds them and keeps every name", needsPi, () =>
	withFixture({ tools: MANY_SERVICES }, async (fixture) => {
		const { calls } = await start(fixture);

		// A cut `s5 2 devices` reads as `s5 1 device`, which is worse than no
		// detail at all -- so past the budget the line stops itemising detail
		// before it stops itemising services. Every id is still there, one
		// block each, state glyph included.
		assert.equal(lastStatus(calls), "· ● s0 · ● s1 · ● s2 · ● s3 · ● s4 · ● s5");
	}));

test("a very narrow footer collapses to counts, worst first", needsPi, () =>
	withFixture({ tools: MIXED_STATES }, async (fixture) => {
		process.env.COLUMNS = "30";
		try {
			const { calls } = await start(fixture);

			// Names alone no longer fit, so the footer falls back to counts --
			// and the order is the blocks' order: down, then the rest.
			assert.equal(lastStatus(calls), "· 1 down · 3 up");
			assert.ok(piTui.visibleWidth(lastStatus(calls)) <= 24);
		} finally {
			delete process.env.COLUMNS;
		}
	}));

test("the footer colours the counts by the state they count", needsPi, () =>
	withFixture({ tools: MIXED_STATES }, async (fixture) => {
		const rec = recordingTheme;
		rec.fgCalls.length = 0;
		process.env.COLUMNS = "30";
		try {
			const { calls } = await start(fixture, { theme: rec.theme });

			assert.match(lastStatus(calls), /^· 1 down · 3 up$/);
			assert.equal(colouredAs(rec, "1 down"), "error");
			assert.equal(colouredAs(rec, "3 up"), "success");
			assert.equal(colouredAs(rec, " · "), "dim");
			// The lead ties the counts to the anchor block before them.
			assert.equal(colouredAs(rec, "·"), "dim");
		} finally {
			delete process.env.COLUMNS;
		}
	}));

/** Four states in one catalogue: one refusing, three answering. */
const MIXED_STATES = `
version = 1

[probe]
timeout = 5.0

[tool.blocked]
name    = "Blocked"
desc    = "a service that is not running"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "exit 3"], label = "blocked" }

[tool.up1]
name    = "Up1"
desc    = "a service that answers"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo 'a device'"], label = "up1", count = { pattern = 'device$', noun = "device" } }

[tool.up2]
name    = "Up2"
desc    = "a service that answers"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo 'a device'"], label = "up2", count = { pattern = 'device$', noun = "device" } }

[tool.up3]
name    = "Up3"
desc    = "a service that answers"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo 'a device'"], label = "up3", count = { pattern = 'device$', noun = "device" } }
`;

test("the footer never truncates a detail count, at any width", needsPi, () =>
	withFixture(opts, async (fixture) => {
		// The ladder is a promise about every width, not about the widths the
		// developer happened to test by hand: at each of these, the line must
		// fit the budget it was given, and any detail it shows must be whole.
		for (const columns of [24, 30, 40, 56, 80, 120, 200]) {
			process.env.COLUMNS = String(columns);
			try {
				const { calls } = await start(fixture);
				const status = lastStatus(calls) ?? "";
				assert.ok(
					piTui.visibleWidth(status) <= Math.max(24, columns - 24),
					`${JSON.stringify(status)} wider than a ${columns}-column footer's budget`,
				);
				// "2 devices" is the answer; "2 d…" is a different answer.
				if (status.includes("device")) assert.match(status, /2 devices/, status);
			} finally {
				delete process.env.COLUMNS;
			}
		}
	}));

test("no installed service means no footer entry at all", needsPi, () =>
	withFixture({ tools: "version = 1\n" }, async (fixture) => {
		const { calls } = await start(fixture);

		// An empty catalogue is a real state -- before `scripts/install.py`
		// seeds anything -- and an empty label in the footer is clutter.
		assert.equal(lastStatus(calls), undefined);
	}));

// ---------------------------------------------------------------------------
// Colour: state apart from identity
// ---------------------------------------------------------------------------

test("the footer colours state and service apart", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const rec = recordingTheme;
		rec.fgCalls.length = 0;
		// Wide on purpose: the full form, every block coloured, nothing shed.
		process.env.COLUMNS = "200";
		try {
			const { calls } = await start(fixture, { theme: rec.theme });

			assert.match(lastStatus(calls), /· ✗ refusing down · ● answering 2 devices/);
			// The state words and their glyph share the state's colour...
			assert.equal(colouredAs(rec, "✗"), "error");
			assert.equal(colouredAs(rec, "down"), "error");
			assert.equal(colouredAs(rec, "●"), "success");
			assert.equal(colouredAs(rec, "2 devices"), "success");
			// ...and the ids get different colours from each other, drawn from
			// the rotation that never means "something is wrong".
			const answering = colouredAs(rec, "answering");
			const refusing = colouredAs(rec, "refusing");
			assert.notEqual(answering, refusing);
			const rotation = ["accent", "mdLink", "thinkingHigh", "thinkingXhigh", "syntaxType"];
			for (const colour of [answering, refusing]) {
				assert.ok(rotation.includes(colour), `${colour} is not an identity colour`);
			}
		} finally {
			delete process.env.COLUMNS;
		}
	}));

test("no service borrows a state colour for its name", needsPi, () =>
	withFixture({ tools: MANY_SERVICES }, async (fixture) => {
		const rec = recordingTheme;
		rec.fgCalls.length = 0;
		// Six services walk the whole rotation, fifth slot included.
		process.env.COLUMNS = "200";
		try {
			const { calls } = await start(fixture, { theme: rec.theme });

			assert.match(lastStatus(calls), /● s0 2 devices/);
			const stateColours = ["success", "error", "warning"];
			for (let i = 0; i < 6; i++) {
				const colour = colouredAs(rec, `s${i}`);
				assert.ok(colour, `s${i} was rendered uncoloured`);
				assert.ok(!stateColours.includes(colour), `${colour} must not name a service`);
			}
		} finally {
			delete process.env.COLUMNS;
		}
	}));

// ---------------------------------------------------------------------------
// hiddenServices (ADR-0016)
// ---------------------------------------------------------------------------

test("a hidden service is left out of the footer, the others are not", needsPi, () =>
	withFixture({ ...opts, agentSettings: { hiddenServices: ["answering"] } }, async (fixture) => {
		const { calls } = await start(fixture);

		assert.doesNotMatch(lastStatus(calls), /answering/);
		assert.match(lastStatus(calls), /✗ refusing down/);
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

		assert.match(lastStatus(calls), /● answering 2 devices/);
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
		// A tool that is not installed has no service to show -- the same
		// reason the footer leaves it out.
		assert.doesNotMatch(panel, /uninstalled/);
		assert.doesNotMatch(panel, /not installed/);

		await command.handler("", ctx);
		assert.equal(lastWidget(calls).cleared, true);
	}));

test("the panel gives a service the colour the footer gave it", needsPi, () =>
	withFixture({ tools: SPLIT_ALPHABET }, async (fixture) => {
		const rec = recordingTheme;
		const { extension, ctx, calls } = await start(fixture, { theme: rec.theme });

		// `z-here` sorts after a tool that is not installed, so a colour map
		// built over the *present* services alone would hand it a different
		// slot than the panel's map, which spans the whole payload. They must
		// agree, or the same service changes colour between the two views.
		const footerColour = colouredAs(rec, "z-here");

		await extension.commands.get("reactor-status").handler("", ctx);
		rec.fgCalls.length = 0;
		const panel = lastWidget(calls).lines(80).join("\n");

		assert.match(panel, /z-here/);
		assert.equal(colouredAs(rec, "z-here"), footerColour);
	}));

test("the panel colours a label that says more than the id does", needsPi, () =>
	withFixture({ tools: LABELLED_SERVICE }, async (fixture) => {
		const rec = recordingTheme;
		const { extension, ctx, calls } = await start(fixture, { theme: rec.theme });
		await extension.commands.get("reactor-status").handler("", ctx);

		const panel = lastWidget(calls).lines(80).join("\n");
		assert.match(panel, /● bn\s+BN session\s+up/);
		// The label is context, not status: context reads quiet.
		assert.equal(colouredAs(rec, "BN session"), "muted");
		assert.equal(colouredAs(rec, "up"), "success");
	}));

test("the panel wraps what the window cannot hold", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const { extension, ctx, calls } = await start(fixture);
		await extension.commands.get("reactor-status").handler("", ctx);
		const widget = lastWidget(calls);

		const wide = widget.lines(120);
		const narrow = widget.lines(20);

		// Nothing is dropped on the way down: every service keeps its glyph,
		// its id and its state words, spread over more lines instead.
		assert.ok(narrow.length > wide.length, `${narrow.length} lines at 20, ${wide.length} at 120`);
		for (const id of ["answering", "refusing"]) {
			assert.match(narrow.join("\n"), new RegExp(id));
		}
		// The stacked lines are indented under the service id, which is what
		// makes a wrapped row still read as one row.
		assert.match(narrow.join("\n"), /^    \S/m);
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
// States the happy path does not hit
// ---------------------------------------------------------------------------

test("an unknown probe is its own quiet state, between down and up", needsPi, () =>
	withFixture({ tools: SLOWPOKE_SERVICE }, async (fixture) => {
		const { calls } = await start(fixture);

		// Unknown ranks after down and before up: it may need attention, but
		// "do something" is what down says. The glyph is a question, and the
		// words stay dim in both views -- a slow daemon is not a fault.
		assert.equal(lastStatus(calls), "· ? slowpoke unknown · ● answering 2 devices");

		const { extension, ctx, calls: panelCalls } = await start(fixture);
		await extension.commands.get("reactor-status").handler("", ctx);
		const panel = lastWidget(panelCalls).lines(80).join("\n");
		assert.match(panel, /\? slowpoke\s+unknown/);
	}));

test("a service-backed tool that is not installed is not shown", needsPi, () =>
	withFixture(opts, async (fixture) => {
		const rec = recordingTheme;
		const { extension, ctx, calls } = await start(fixture, { theme: rec.theme });
		await extension.commands.get("reactor-status").handler("", ctx);

		const panel = lastWidget(calls).lines(80).join("\n");
		// The catalogue declares three services; one of them is not installed,
		// and the panel is not the place for a status about a thing that does
		// not exist.
		assert.doesNotMatch(panel, /uninstalled/);
		assert.match(panel, /● answering/);
		assert.match(panel, /✗ refusing/);
	}));

test("a panel whose every service is absent explains itself, quietly", needsPi, () =>
	withFixture(
		{ tools: ONE_PRESENT_ONE_ABSENT, agentSettings: { hiddenServices: ["answering"] } },
		async (fixture) => {
			// `answering` is muted, so the only service left is the uninstalled
			// one; hiding that must not leave a bare header over nothing.
			const { extension, ctx, calls } = await start(fixture);
			await extension.commands.get("reactor-status").handler("", ctx);

			const lines = lastWidget(calls).lines(80);
			assert.deepEqual(lines, ["  nothing that declares a service is installed"]);
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
		assert.match(lastStatus(calls), /● answering 2 devices/);
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

// ---------------------------------------------------------------------------
// Fixtures for the rarer shapes
// ---------------------------------------------------------------------------

/**
 * One service-backed tool that is not installed, sorted before one that is:
 * the absent id occupies a rotation slot whether or not the footer shows it,
 * which is what pins the two views to one colour map.
 */
const SPLIT_ALPHABET = `
version = 1

[probe]
timeout = 5.0

[tool.a-away]
name    = "A-away"
desc    = "declares a service but is not here"
invoke  = "reactor-absent-by-design"
detect  = { binary = "reactor-absent-by-design" }
service = { probe = ["sh", "-c", "exit 0"], label = "a-away" }

[tool.z-here]
name    = "Z-here"
desc    = "a service that answers"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo 'a device'"], label = "z-here", count = { pattern = 'device$', noun = "device" } }
`;

/** One service whose probe answers, labelled with something the id does not say. */
const LABELLED_SERVICE = `
version = 1

[probe]
timeout = 5.0

[tool.bn]
name    = "Binary Ninja"
desc    = "a decompiler with a live session"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo healthy"], label = "BN session" }
`;

/**
 * A probe that cannot answer inside the catalogue's timeout: `sh -c sleep 2`
 * against a 0.2s budget, so the CLI records `unknown` -- the state of a
 * daemon that did not answer in time, distinct from one that answered no.
 */
const SLOWPOKE_SERVICE = `
version = 1

[probe]
timeout = 0.2

[tool.slowpoke]
name    = "Slowpoke"
desc    = "a probe that does not answer in time"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "sleep 2"], label = "slowpoke" }

[tool.answering]
name    = "Answering"
desc    = "a service that answers"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo 'a device'; echo 'b device'"], label = "answering", count = { pattern = 'device$', noun = "device" } }
`;