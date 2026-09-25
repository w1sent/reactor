/**
 * guiview: the envelope contract's shared vocabulary, checked against both
 * consumers -- reactor-gui parses what this emits (ADR-0032, gui/SPEC.md §4).
 *
 * The assertions pin the *wire*: the marker line's shape, the mandatory
 * fallback lines, the event-command payload. reactor-gui's own Rust tests
 * (`gui/crates/reactor-gui/src/contract.rs`) mirror this fixture from the
 * other side -- the contract is the same bytes seen from both ends, and a
 * change here or there fails one of the two.
 *
 * This file imports the lib directly: `guiview.ts` imports nothing from pi
 * (it is env + JSON over strings, ADR-0029 stateless presentation), so plain
 * node -- no pi loader -- is a faithful reader of it. The extensions that
 * *use* it are tested through pi's own loader like every other extension.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import {
	buildPayload,
	buildWidgetLines,
	eventCommand,
	fallbackLines,
	GUI_ENV,
	isGuiMode,
	KEY_PREFIX,
	MARKER,
	parseView,
	VERSION,
	widgetKey,
} from "../../extensions/lib/guiview.ts";

const SELECTOR_PAYLOAD = {
	v: VERSION,
	view: "selector",
	title: "Tools",
	command: "/reactor-tools-event",
	placement: "overlay",
	table: {
		columns: [
			{ id: "tool", title: "Tool", width: 16 },
			{ id: "state", title: "", width: 4 },
		],
		rows: [
			{
				id: "bn",
				cells: { tool: { text: "bn" }, state: { text: "●", color: "success" } },
				actions: [{ id: "toggle", label: "Toggle", disabled: false }],
			},
		],
	},
	footer: "9 active · 12 catalogued",
};

test("the handshake is the environment reactor-gui set", () => {
	delete process.env[GUI_ENV];
	assert.equal(isGuiMode(), false, "TUI and plain-RPC clients do not set it");
	process.env[GUI_ENV] = "1";
	assert.equal(isGuiMode(), true);
	delete process.env[GUI_ENV];
});

test("an envelope's line 0 is the marker + JSON, the rest the fallback", () => {
	const payload = structuredClone(SELECTOR_PAYLOAD);
	const lines = buildWidgetLines(payload, ["bn   ●", "9 active · 12 catalogued"]);
	assert.match(lines[0], new RegExp(`^${MARKER} v${VERSION} \\{`));
	assert.equal(lines.length, 3, "marker + two fallback lines");
	// The JSON parses back to the same payload.
	const parsed = JSON.parse(lines[0].replace(`${MARKER} v${VERSION} `, ""));
	assert.equal(parsed.view, "selector");
	assert.equal(parsed.command, "/reactor-tools-event");
	// The fallback survives after the marker, for every other client.
	assert.deepEqual(lines.slice(1), ["bn   ●", "9 active · 12 catalogued"]);
});

test("a view without fallback lines is refused, not emitted", () => {
	// The fallback is what every other client renders; a payload without one
	// would hide data from text clients (gui/SPEC.md §4.2, ADR-0032).
	assert.throws(() => buildWidgetLines(structuredClone(SELECTOR_PAYLOAD), []), /fallback/);
});

test("parseView reads back exactly what buildWidgetLines wrote", () => {
	const lines = buildWidgetLines(structuredClone(SELECTOR_PAYLOAD), ["bn   ●"]);
	const parsed = parseView(widgetKey("selector"), lines);
	assert.ok(parsed);
	assert.equal(parsed.viewId, "selector");
	assert.equal(parsed.payload.command, "/reactor-tools-event");
	assert.equal(parsed.payload.placement, "overlay");
	assert.equal(parsed.payload.table.rows[0].cells.state.color, "success");
});

test("non-envelope widgets render as text, unchanged", () => {
	// No `reactor:` prefix, no marker, wrong version, no command: the plain
	// widget path is every other client's path, untouched (ADR-0032).
	assert.equal(parseView("some-extension-panel", ["just text"]), undefined);
	assert.equal(parseView(widgetKey("selector"), ["just text"]), undefined);
	assert.equal(parseView(widgetKey("selector"), []), undefined);
	assert.equal(
		parseView(widgetKey("x"), [`${MARKER} v${VERSION} not json`]),
		undefined,
	);
	// A future schema version is not parsed as v1 either -- the fallback
	// lines carry it until the GUI grows the schema.
	const future = buildWidgetLines(
		{ ...SELECTOR_PAYLOAD, v: VERSION + 1 },
		["fallback"],
	);
	assert.equal(parseView(widgetKey("fancy"), future), undefined);
});

test("event commands carry the view, the action and the row", () => {
	assert.equal(
		eventCommand("/reactor-tools-event", "selector", "toggle", "bn"),
		'/reactor-tools-event {"view":"selector","action":"toggle","row":"bn"}',
	);
	assert.equal(
		eventCommand("/reactor-tree", "tree", "switch"),
		'/reactor-tree {"view":"tree","action":"switch"}',
	);
});

test("fallback lines render a table aligned, a list one-per-item", () => {
	const lines = fallbackLines(structuredClone(SELECTOR_PAYLOAD));
	assert.ok(lines.length >= 3, "header, rule, row");
	assert.ok(lines[0].includes("Tool"));
	assert.ok(lines[2].includes("bn"));
	assert.ok(lines.at(-1).includes("9 active"), "the footer survives the fallback");

	const list = fallbackLines({
		v: VERSION,
		view: "guide",
		title: "Guide",
		command: "/guide-event",
		placement: "overlay",
		list: { items: [{ id: "p1", label: "Concept" }] },
		footer: "guide",
	});
	assert.ok(list.includes("Concept"));
});

test("the widget key is the reactor-prefixed view id", () => {
	assert.equal(widgetKey("selector"), "reactor:selector");
});