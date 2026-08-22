/**
 * selector: does /reactor-tools write what it says it writes?
 *
 * The overlay is driven for real -- keystrokes in, rendered lines out -- with
 * pi's own component contract and the real CLI behind it (docs/adr/0012). What
 * is asserted is behaviour and invariants, never the exact layout: the layout is
 * expected to keep changing and a golden file would make every change look like
 * a regression.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import {
	CLI_MISSING,
	CLI_OK,
	GAMMA_VERSION,
	loadExtension,
	makeContext,
	makeTui,
	needsPi,
	piTui,
	plainTheme,
	press,
	waitFor,
	withFixture,
} from "./harness.mjs";

const EXT = "extensions/selector/index.ts";

const ESC = "\x1b";
const ENTER = "\r";
const DOWN = "\x1b[B";
const TAB = "\t";
const SPACE = " ";
const CTRL_R = "\x12";

/**
 * Open the overlay and hand it to `body`, then make sure it is closed.
 *
 * `guard` is shared between the two mocks: closing the overlay can both
 * `ctx.reload()` (a toggle changed) and touch `ctx`/`pi` again afterward (a
 * skill was read) in the same handler run, and pi's own runtime refuses any
 * `ctx`/`pi` use after `await ctx.reload()` resolves. If `index.ts` ever
 * reorders those two branches back to the wrong way round, this throws
 * `STALE_CTX_MESSAGE` instead of quietly passing.
 */
async function withOverlay(fixture, body, { mode = "tui", tui = makeTui() } = {}) {
	const { extension, entries, guard } = await loadExtension(EXT, fixture);
	const { ctx, calls } = makeContext(fixture, { mode, tui, guard });
	const running = extension.commands.get("reactor-tools").handler("", ctx);
	const overlay = await waitFor(() => calls.overlay, "the overlay to mount");
	const view = {
		...overlay,
		lines: (width = 120) => overlay.component.render(width),
		text: (width = 120) => overlay.component.render(width).join("\n"),
	};
	try {
		await body(view, calls, entries);
	} finally {
		if (calls.overlay.component.detailFor !== undefined) await press(overlay, ESC);
		overlay.component.handleInput(ESC);
		await running;
	}
	return { calls, entries };
}

// ---------------------------------------------------------------------------
// Preconditions
// ---------------------------------------------------------------------------

test("the keys this file types are the keys in force", needsPi, () => {
	// Everything below is written in literal escape sequences. If the developer
	// running the suite has rebound these, say so once here rather than failing
	// a dozen tests for reasons that look like selector bugs.
	const kb = piTui.getKeybindings();
	assert.ok(kb.matches(ESC, "tui.select.cancel"), "esc is not bound to cancel");
	assert.ok(kb.matches(ENTER, "tui.select.confirm"), "enter is not bound to confirm");
	assert.ok(kb.matches(DOWN, "tui.select.down"), "down-arrow is not bound to down");
});

// ---------------------------------------------------------------------------
// Opening
// ---------------------------------------------------------------------------

test("outside a TUI it points at the CLI instead of half-working", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		// rpc, not print: `ctx.hasUI` is true here, and gating on it was the bug.
		// `ctx.ui.custom` has nothing to mount into outside a terminal.
		const { ctx, calls } = makeContext(fixture, { mode: "rpc" });

		await extension.commands.get("reactor-tools").handler("", ctx);

		assert.equal(calls.custom.length, 0);
		assert.equal(calls.notify.at(-1).level, "info");
		assert.match(calls.notify.at(-1).message, /3 tool\(s\) active/);
		assert.match(calls.notify.at(-1).message, /reactor tools enable\|disable\|reset/);
	}));

test("an unreachable CLI opens nothing", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture);
		fixture.mode = CLI_MISSING;

		await extension.commands.get("reactor-tools").handler("", ctx);

		// Half a screen of "unknown" would be worse than not opening: someone
		// came here to make a decision and would make it on bad data.
		assert.equal(calls.custom.length, 0);
		assert.equal(calls.notify.at(-1).level, "error");
		assert.match(calls.notify.at(-1).message, /reactor doctor/);
	}));

test("the overlay asks for the whole terminal", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { calls } = await withOverlay(fixture, async () => {});

		// pi's resolveOverlayLayout caps an overlay at min(80, availWidth)
		// unless it says otherwise, and 80 columns truncates the description
		// column on any real terminal. The list is a table; it wants the width.
		assert.equal(calls.custom[0].overlay, true);
		assert.equal(calls.custom[0].overlayOptions.width, "100%");
		assert.equal(calls.custom[0].overlayOptions.maxHeight, "100%");
	}));

test("closing without touching anything reloads nothing", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { calls } = await withOverlay(fixture, async () => {});

		assert.equal(calls.reloads, 0);
		assert.equal(fixture.readStateBytes(), undefined);
	}));

// ---------------------------------------------------------------------------
// Reading the list
// ---------------------------------------------------------------------------

test("the filter searches descriptions, not just names", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			for (const ch of "firmware") view.component.handleInput(ch);

			// Finding a tool by what it does is what a catalogue is for, and is
			// the specific reason pi's SettingsList was not reused (ADR-0011).
			const text = view.text();
			assert.match(text, /gamma/);
			assert.doesNotMatch(text, /unpacks alpha containers/);
		});
	}));

test("a filter that matches nothing says so", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			for (const ch of "zzz") view.component.handleInput(ch);

			assert.match(view.text(), /nothing matches "zzz"/);
		});
	}));

test("/ is the conventional search key: it resets rather than being typed", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			for (const ch of "firmware") view.component.handleInput(ch);
			assert.match(view.text(), /gamma/);

			view.component.handleInput("/");

			// Back to the unfiltered list -- not a literal search for a tool
			// whose id, name, description or tags contain a slash.
			assert.match(view.text(), /type to filter/);
			assert.match(view.text(), /alpha/);

			for (const ch of "firmware") view.component.handleInput(ch);
			assert.match(view.text(), /gamma/);
		});
	}));

test("tab switches pane and drops the filter with it", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			for (const ch of "firmware") view.component.handleInput(ch);
			view.component.handleInput(TAB);

			const text = view.text();
			// A query typed against tools means nothing against toolsets, and
			// carrying it over would show an empty pane for no stated reason.
			assert.match(text, /type to filter/);
			assert.match(text, /the whole catalogue/);
			assert.match(text, /static analysis only/);
		});
	}));

test("a version is never truncated into a different version", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			// The right-hand column is sized to the rows on screen precisely so
			// this cannot happen: `10.1.1` would be a lie, not an abbreviation.
			assert.ok(view.text().includes(GAMMA_VERSION), view.text());
		});
	}));

test("no rendered line is wider than the width it was given", needsPi, () =>
	withFixture({}, async (fixture) => {
		for (const rows of [14, 40]) {
			await withOverlay(
				fixture,
				async (view) => {
					for (const width of [40, 62, 80, 120, 200]) {
						for (const line of view.lines(width)) {
							assert.ok(
								piTui.visibleWidth(line) <= width,
								`${piTui.visibleWidth(line)} > ${width} (rows ${rows}): ${JSON.stringify(line)}`,
							);
						}
					}
				},
				{ tui: makeTui({ rows }) },
			);
		}
	}));

/** More tools than any terminal shows at once, so scrolling has to happen. */
const CROWDED_TOOLS = `
version = 1

[platform]
prefer = []

[probe]
timeout = 5.0

${Array.from(
	{ length: 40 },
	(_, i) => `[tool.t${String(i).padStart(2, "0")}]
name   = "Tool ${i}"
desc   = "does the ${i}th thing to a binary"
invoke = "ls"
detect = { binary = "ls" }
tags   = ["static"]
`,
).join("\n")}`;

test("the list is sized to the terminal, not to a constant", needsPi, () =>
	withFixture({ tools: CROWDED_TOOLS }, async (fixture) => {
		const count = async (rows) => {
			let n = 0;
			await withOverlay(
				fixture,
				async (view) => {
					n = view.lines(120).filter((l) => /\[[x +-]\]/.test(l)).length;
				},
				{ tui: makeTui({ rows }) },
			);
			return n;
		};

		// render() is handed a width but never a height, so the component asks
		// the terminal directly. A fixed row count leaves half a tall window
		// empty and overflows a short one.
		const short = await count(14);
		const tall = await count(40);
		assert.ok(short < tall, `short ${short} !< tall ${tall}`);
		assert.ok(short <= 14 && tall <= 40, `${short}/${tall} rows do not fit`);
	}));

test("a list that does not fit says where you are in it", needsPi, () =>
	withFixture({ tools: CROWDED_TOOLS }, async (fixture) => {
		await withOverlay(
			fixture,
			async (view) => {
				assert.match(view.text(), /\(1\/40\)/);
				view.component.handleInput(DOWN);
				assert.match(view.text(), /\(2\/40\)/);
			},
			{ tui: makeTui({ rows: 20 }) },
		);
	}));

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

test("toggling a toolset writes it and reloads once on close", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { calls } = await withOverlay(fixture, async (view, live) => {
			view.component.handleInput(TAB);
			await press(view, SPACE);

			assert.deepEqual(fixture.readState().toolsets, ["everything"]);
			// Nothing is reloaded mid-overlay: resources are re-gated once, on
			// the way out, however many keys were pressed.
			assert.equal(live.reloads, 0);
		});

		assert.equal(calls.reloads, 1);
	}));

test("the row redraws from the CLI's answer, not from a guess", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			view.component.handleInput(TAB);
			assert.match(view.text(), /\[ \] everything/);

			await press(view, SPACE);

			// `reactor toolsets enable` returns the recomputed activation, and
			// the overlay applies that rather than re-deriving anything.
			assert.match(view.text(), /\[x\] everything/);
		});
	}));

test("toggling a tool off and back on leaves state.json identical", needsPi, () =>
	withFixture(
		{ state: { toolsets: ["static"], tools: { enabled: [], disabled: [] } } },
		async (fixture) => {
			await withOverlay(fixture, async (view) => {
				// One round trip first, so the file is in the CLI's own
				// formatting and the byte comparison below is about content.
				await press(view, SPACE);
				await press(view, SPACE);
				const before = fixture.readStateBytes();

				await press(view, SPACE);
				assert.deepEqual(fixture.readState().tools.disabled, ["alpha"]);
				await press(view, SPACE);

				// ADR-0011: a toggle is "make the smallest edit that produces
				// this outcome", so looking around costs nothing.
				assert.equal(fixture.readStateBytes(), before);
				assert.deepEqual(fixture.readState().tools, { enabled: [], disabled: [] });
			});
		},
	));

test("an override is shown as an override, not just as on", needsPi, () =>
	withFixture(
		{ state: { toolsets: ["static"], tools: { enabled: ["beta"], disabled: [] } } },
		async (fixture) => {
			await withOverlay(fixture, async (view) => {
				// Without the distinction, a later toolset switch that drops
				// beta looks arbitrary -- the file, not the toolset, decides it.
				assert.match(view.text(), /\[x\] alpha/);
				assert.match(view.text(), /\[\+\] beta/);
			});
		},
	));

test("ctrl-r drops an override and the row falls back to the toolsets", needsPi, () =>
	withFixture(
		{ state: { toolsets: ["static"], tools: { enabled: ["beta"], disabled: [] } } },
		async (fixture) => {
			await withOverlay(fixture, async (view) => {
				view.component.handleInput(DOWN);
				await press(view, CTRL_R);

				assert.deepEqual(fixture.readState().tools.enabled, []);
				// static does not select beta, so dropping the pin turns it off.
				assert.match(view.text(), /\[ \] beta/);
			});
		},
	));

test("ctrl-r on a tool with no override writes nothing and says why", needsPi, () =>
	withFixture(
		{ state: { toolsets: ["static"], tools: { enabled: [], disabled: [] } } },
		async (fixture) => {
			await withOverlay(fixture, async (view) => {
				const before = fixture.readStateBytes();
				await press(view, CTRL_R);

				assert.match(view.text(), /nothing to reset/);
				assert.equal(fixture.readStateBytes(), before);
			});
		},
	));

test("a notice belongs to the keystroke that produced it", needsPi, () =>
	withFixture(
		{ state: { toolsets: ["static"], tools: { enabled: [], disabled: [] } } },
		async (fixture) => {
			await withOverlay(fixture, async (view) => {
				await press(view, CTRL_R);
				assert.match(view.text(), /nothing to reset/);

				view.component.handleInput(DOWN);

				// Anything else the user does has answered the notice.
				assert.doesNotMatch(view.text(), /nothing to reset/);
			});
		},
	));

test("a failed write is reported and changes nothing on screen", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { calls } = await withOverlay(fixture, async (view) => {
			assert.match(view.text(), /\[x\] alpha/);
			fixture.mode = CLI_MISSING;

			await press(view, SPACE);

			assert.match(view.text(), /could not disable alpha/);
			// The row must not flip on a write that did not happen.
			assert.match(view.text(), /\[x\] alpha/);
			fixture.mode = CLI_OK;
		});

		// Nothing was written, so nothing needs re-gating.
		assert.equal(calls.reloads, 0);
	}));

// ---------------------------------------------------------------------------
// Inspecting
// ---------------------------------------------------------------------------

test("enter opens the CLI's own detail view, esc goes back", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			await press(view, ENTER);

			const detail = view.text();
			assert.match(detail, /alpha/);
			assert.match(detail, /unpacks alpha containers/);
			assert.match(detail, /esc back/);

			await press(view, ESC);
			assert.match(view.text(), /diffs gamma firmware images/);
		});
	}));

test("a fetched skill is offered, and read as a session entry", needsPi, () =>
	withFixture({ skills: ["alpha"] }, async (fixture) => {
		const { calls, entries } = await withOverlay(fixture, async (view) => {
			await press(view, ENTER);
			assert.match(view.text(), /s read the fetched skill/);
			view.component.handleInput("s");
		});

		// A skill body is for the person at the keyboard: `convertToLlm` turns a
		// custom *message* into a user message, and the model already has the
		// registry block.
		assert.equal(entries.length, 1);
		assert.equal(entries[0].customType, "reactor-detail");
		assert.equal(entries[0].data.title, "skill: alpha");
		assert.match(entries[0].data.body, /body line one/);
		assert.equal(calls.reloads, 0);
	}));

test("a tool with no fetched skill says so and stays open", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { entries } = await withOverlay(fixture, async (view) => {
			await press(view, ENTER);

			// alpha declares a skill in the catalogue; nothing has fetched it.
			assert.doesNotMatch(view.text(), /s read the fetched skill/);
			view.component.handleInput("s");
			assert.match(view.text(), /no fetched skill/);
		});

		assert.equal(entries.length, 0);
	}));

test("a long detail body is collapsed with a count of what is hidden", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const renderer = extension.entryRenderers.get("reactor-detail");
		const body = Array.from({ length: 30 }, (_, i) => `line ${i + 1}`).join("\n");
		const entry = { data: { title: "skill: alpha", body } };

		const collapsed = renderer(entry, { expanded: false }, plainTheme).render(80).join("\n");
		const expanded = renderer(entry, { expanded: true }, plainTheme).render(80).join("\n");

		// A skill is a document, not a status line, so the collapsed view has to
		// admit how much of it is missing.
		assert.match(collapsed, /line 12/);
		assert.doesNotMatch(collapsed, /line 13/);
		assert.match(collapsed, /18 more lines/);
		assert.match(expanded, /line 30/);
	}));

// ---------------------------------------------------------------------------
// The toolbox toggle (ADR-0016)
// ---------------------------------------------------------------------------

test("toolbox: false in reactor.json registers no command at all", needsPi, () =>
	withFixture({ agentSettings: { toolbox: false } }, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);

		assert.equal(extension.commands.size, 0);
		assert.equal(extension.entryRenderers.size, 0);
	}));
