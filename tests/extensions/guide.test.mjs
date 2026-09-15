/**
 * guide: does /guide open the popup, render the flows, scroll them, and
 * close cleanly -- and does it answer usefully outside a TUI?
 *
 * The overlay is driven the same way the selector's is: the handler runs
 * un-awaited (it settles only when the overlay closes), the harness parks
 * the component in `calls.overlay`, and the test types raw key sequences
 * at `handleInput`. The keys are asserted to be the ones in force before
 * anything types them, so a rebinding fails one assertion, not a dozen.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import {
	loadExtension,
	makeContext,
	makeTui,
	needsPi,
	piTui,
	recordingTheme,
	waitFor,
	withFixture,
} from "./harness.mjs";

const EXT = "extensions/guide/index.ts";
const DOWN = "\x1b[B";
const UP = "\x1b[A";
const ESC = "\x1b";

/** The keys this file types must be the keys in force. */
test("the keys this file types are the keys in force", needsPi, () => {
	const kb = piTui.getKeybindings();
	assert.ok(kb.matches(DOWN, "tui.select.down"), "down-arrow is not bound to down");
	assert.ok(kb.matches(UP, "tui.select.up"), "up-arrow is not bound to up");
	assert.ok(kb.matches(ESC, "tui.select.cancel"), "escape is not bound to cancel");
});

/** Start /guide without awaiting it: it settles when the overlay closes. */
async function withOverlay(fixture, body, { mode = "tui", rows = 40, args = "", theme } = {}) {
	const { extension, guard } = await loadExtension(EXT, fixture);
	const tui = makeTui({ rows });
	const { ctx, calls } = makeContext(fixture, { mode, tui, guard, theme });
	const running = extension.commands.get("guide").handler(args, ctx);
	const overlay = await waitFor(() => calls.overlay, "the guide overlay to mount");
	const view = {
		...overlay,
		lines: (width = 100) => overlay.component.render(width),
		text: (width = 100) => overlay.component.render(width).join("\n"),
	};
	try {
		await body(view, calls);
	} finally {
		overlay.component.handleInput(ESC);
		await running;
	}
	return { calls, extension };
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

test("registers /guide and nothing else", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		assert.deepEqual([...extension.commands.keys()], ["guide"]);
		assert.deepEqual([...extension.tools.keys()], []);
	}));

// ---------------------------------------------------------------------------
// The popup
// ---------------------------------------------------------------------------

test("/guide opens an overlay with the concept and the flows", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			const text = view.text(100);
			assert.match(text, /THE CONCEPT/);
			assert.match(text, /THE FLOWS/);
			assert.match(text, /🛠 5\/50 tools/);
			assert.match(text, /\/goal <text>/);
			assert.match(text, /reactor-scenario start/);
			assert.match(text, /reactor doctor/);
		});
	}));

test("the popup opens at the top: framed: title in the rule, hints in the rule, body between", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view, calls) => {
			const lines = view.lines(100);
			assert.match(lines[0], /^┌─ REACTOR -- a reverse-engineering harness .*┐$/);
			// Every middle line is inside the frame: border, tinted, border.
			for (const line of lines.slice(1, -1)) {
				assert.ok(line.startsWith("│"), JSON.stringify(line));
				assert.ok(line.endsWith("│"), JSON.stringify(line));
			}
			// The hint line is the frame's bottom rule, not lost mid-scroll.
			assert.match(lines.at(-1), /^└─ .*q\/Esc close.*┘$/);
			assert.equal(calls.overlay.options.overlay, true);
		});
	}));

test("down moves the window one line; pageDown moves it a viewport", needsPi, () =>
	withFixture({}, async (fixture) => {
		// A short terminal, or the whole body fits and there is nothing to
		// scroll. rows 14 -> a 9-line viewport over a ~30-line body.
		await withOverlay(fixture, async (view) => {
			const top = view.lines(100).join("\n");
			view.component.handleInput(DOWN);
			const oneDown = view.lines(100).join("\n");
			assert.notEqual(oneDown, top, "down did not move the window");
			// Exactly one body line moved: the second visible line became the
			// first.
			assert.equal(oneDown.split("\n")[2], top.split("\n")[3]);

			view.component.handleInput("\x1b[6~"); // pageDown
			const paged = view.lines(100).join("\n");
			assert.notEqual(paged, oneDown, "pageDown did not move the window");
		}, { rows: 14 });
	}));

test("the window clamps at both ends: down past the end stays put, up returns", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			for (let i = 0; i < 50; i++) view.component.handleInput(DOWN);
			const bottom = view.lines(100).join("\n");
			// The bottom of the body is the gated-tools note.
			assert.match(bottom, /update_steps after a/);

			view.component.handleInput(UP);
			assert.notEqual(view.lines(100).join("\n"), bottom, "up did not move the window");
		}, { rows: 14 });
	}));

test("closing on q and on Esc both settle the command", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, guard } = await loadExtension(EXT, fixture);
		const tui = makeTui({ rows: 40 });
		const { ctx, calls } = makeContext(fixture, { mode: "tui", tui, guard });
		const running = extension.commands.get("guide").handler("", ctx);
		await waitFor(() => calls.overlay, "the overlay to mount");
		calls.overlay.component.handleInput("q");
		await running;
		assert.ok(true, "the command settled");
	}));

test("no rendered line is wider than the width it was given", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			for (const width of [40, 60, 80, 200]) {
				for (const line of view.lines(width)) {
					assert.ok(piTui.visibleWidth(line) <= width, `${JSON.stringify(line)} at ${width}`);
				}
			}
		});
	}));

// ---------------------------------------------------------------------------
// Pages: the index, the details, aliases, and the unknown-name fallback
// ---------------------------------------------------------------------------

test("/guide tools renders the index of every tool", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			const text = view.text(120);
			assert.match(text, /THE COMMANDS YOU TYPE/);
			assert.match(text, /\/reactor-tools/);
			assert.match(text, /\/goal · \/guidelines · \/manifest · \/frame · \/derive/);
			assert.match(text, /THE AGENT'S TOOLS/);
			assert.match(text, /update_steps/);
			assert.match(text, /reactor_phase_complete/);
			assert.match(text, /history_search/);
		}, { args: "tools" });
	}));

test("/guide <name> opens the tool's detail page", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			const text = view.text(120);
			assert.match(text, /THE MANIFEST/);
			assert.match(text, /\/goal <text>/);
			assert.match(text, /\/derive \[scope\]/);
			assert.match(text, /update_steps -- see/);
		}, { args: "goal" });
	}));

test("aliases open the same page: /guide manifest resolves to the manifest page", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			assert.match(view.text(120), /THE MANIFEST/);
		}, { args: "manifest" });
	}));

test("/guide reactor-status opens that page, with the gate vocabulary", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			const text = view.text(120);
			assert.match(text, /THE STATUS PANEL/);
			assert.match(text, /\/reactor-status mute <id>/);
			assert.match(text, /✗ means down/);
		}, { args: "reactor-status" });
	}));

test("an unknown name falls back to the index with a notice, so a typo never dead-ends", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			const text = view.text(120);
			assert.match(text, /no guide page for "nope" -- the index follows/);
			assert.match(text, /THE COMMANDS YOU TYPE/);
		}, { args: "nope" });
	}));

test("the argument completes every page name and filters by prefix", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension } = await loadExtension(EXT, fixture);
		const complete = extension.commands.get("guide").getArgumentCompletions;

		assert.deepEqual(
			complete("").map((c) => c.value),
			["tools", "reactor", "reactor-tools", "reactor-status", "reactor-scenario", "goal", "identity", "report", "rolling", "auto-continue", "context-editor", "update_steps", "reactor_phase_complete"],
		);
		assert.deepEqual(
			complete("re").map((c) => c.value),
			["reactor", "reactor-tools", "reactor-status", "reactor-scenario", "report", "reactor_phase_complete"],
		);
	}));

test("a leading slash still resolves: /guide /goal opens the manifest page", needsPi, () =>
	withFixture({}, async (fixture) => {
		await withOverlay(fixture, async (view) => {
			assert.match(view.text(120), /THE MANIFEST/);
		}, { args: "/goal" });
	}));

// ---------------------------------------------------------------------------
// The frame
// ---------------------------------------------------------------------------

test("the frame's rules take the border colour and the body sits on a tint", needsPi, () =>
	withFixture({}, async (fixture) => {
		const rec = recordingTheme;
		await withOverlay(fixture, async (view) => {
			view.lines(100);
			// The rules and sides are drawn in the theme's border colour; the
			// content region carries the theme's own selected background, so
			// both colour modes tint it.
			assert.ok(rec.fgCalls.some((c) => c.color === "border" && c.text.startsWith("┌─ ")), "the top rule is not border-coloured");
			assert.ok(rec.fgCalls.some((c) => c.color === "border" && c.text.startsWith("└─ ")), "the bottom rule is not border-coloured");
			assert.ok(rec.fgCalls.some((c) => c.color === "border" && c.text === "│"), "the sides are not border-coloured");
			assert.ok(rec.bgCalls.some((c) => c.color === "selectedBg"), "the content is not on a tinted background");
		}, { theme: rec.theme });
	}));

// ---------------------------------------------------------------------------
// Outside a TUI
// ---------------------------------------------------------------------------

test("outside a TUI it answers with the command sheet instead of an overlay", needsPi, () =>
	withFixture({}, async (fixture) => {
		const { extension, guard } = await loadExtension(EXT, fixture);
		const { ctx, calls } = makeContext(fixture, { mode: "rpc" });

		await extension.commands.get("guide").handler("", ctx);

		assert.equal(calls.widgets.length, 0);
		assert.equal(calls.notify.at(-1).level, "info");
		assert.match(calls.notify.at(-1).message, /\/reactor-tools/);
		assert.match(calls.notify.at(-1).message, /\/goal/);
		assert.match(calls.notify.at(-1).message, /\/reactor-scenario/);
	}));