/**
 * statusbar: the grammar every REactor footer block follows, checked across
 * the whole family (ADR-0029).
 *
 * Every extension is loaded through pi's own loader (docs/adr/0012) against
 * the shared fixture catalogue, driven through the two events every
 * extension gets, and any footer status it sets is checked against the
 * vocabulary in `extensions/lib/statusbar.ts`:
 *
 * - the anchor block (`0-reactor`, the tool count) never leads a separator;
 * - every other block leads with the dim `·` while the toolbox is on, and
 *   none does when it is off, so no separator ever dangles;
 * - no block ends with a separator: the next block leads;
 * - the block's glyph took the anchor colour, and a block is not blank.
 *
 * A new extension is picked up by discovery the moment it sets a status on
 * the standard events; a block that stops following the grammar fails here
 * instead of quietly reading wrong.
 */

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
	FIXTURE_TOOLS,
	loadExtension,
	makeContext,
	needsPi,
	piTui,
	recordingTheme,
	SERVICE_TOOLS,
	withFixture,
} from "./harness.mjs";

/**
 * The anchor's key, mirrored from `extensions/lib/statusbar.ts` -- the test
 * file does not import extension sources (ADR-0012: nothing outside pi's
 * loader resolves the package aliases), and tool-registry's own test pins
 * the extension to the lib's constant.
 */
const ANCHOR_KEY = "0-reactor";

/** Every extension directory in the package, the lib directory excluded. */
function extensionFiles() {
	const dir = path.join(import.meta.dirname, "..", "..", "extensions");
	return fs
		.readdirSync(dir, { withFileTypes: true })
		.filter((e) => e.isDirectory() && e.name !== "lib")
		.map((e) => `extensions/${e.name}/index.ts`)
		.filter((rel) => fs.existsSync(path.join(import.meta.dirname, "..", "..", rel)));
}

/** The grammar of one footer block, read off the rendered line. */
function assertGrammar(value, { toolboxOn, isAnchor }, where) {
	assert.ok(value, `${where}: an empty footer block is not a block`);
	// No trailing separator: the next block leads.
	assert.ok(!/[·]\s*$/.test(value), `${where}: a block must not end with a separator: ${JSON.stringify(value)}`);
	// The lead: present while the anchor is on the line, absent without it.
	const leads = value.startsWith("· ");
	if (isAnchor) {
		assert.ok(!leads, `${where}: the anchor never leads a separator: ${JSON.stringify(value)}`);
	} else if (toolboxOn) {
		assert.ok(leads, `${where}: a non-anchor block leads with the separator: ${JSON.stringify(value)}`);
	} else {
		assert.ok(!leads, `${where}: with the anchor off, no separator may dangle: ${JSON.stringify(value)}`);
	}
}

async function collectStatuses(fixture, ctxOptions) {
	const rec = recordingTheme;
	const seen = [];
	for (const file of extensionFiles()) {
		const loaded = await loadExtension(file, fixture);
		const made = makeContext(fixture, { ...ctxOptions, theme: rec.theme });
		// Not every extension subscribes to both events; drive whichever exist.
		for (const event of ["session_start", "before_agent_start"]) {
			const handler = loaded.extension.handlers.get(event)?.[0];
			if (!handler) continue;
			await handler(event === "before_agent_start" ? { systemPrompt: "" } : {}, made.ctx);
		}
		for (const call of made.calls.status) {
			if (call.value !== undefined) seen.push({ file, key: call.key, value: call.value });
		}
	}
	return { rec, seen };
}

test("every footer block hangs off the anchor while it is on the line", needsPi, () =>
	withFixture({ tools: SERVICE_TOOLS }, async (fixture) => {
		const { rec, seen } = await collectStatuses(fixture, {});
		assert.ok(seen.length > 0, "no footer block was set at all");
		for (const { file, key, value } of seen) {
			assertGrammar(value, { toolboxOn: true, isAnchor: key === ANCHOR_KEY }, file);
		}
		// The vocabulary's colours, and nothing else: anchor accent for
		// glyphs, the services' identity rotation, state colours on the
		// services line, dim separators, muted qualifiers, red errors.
		const allowed = ["accent", "dim", "muted", "warning", "error", "success", "mdLink", "thinkingHigh", "thinkingXhigh", "syntaxType"];
		for (const call of rec.fgCalls) {
			assert.ok(allowed.includes(call.color), `unexpected colour ${call.color} for ${JSON.stringify(call.text)}`);
		}
	}));

test("with the toolbox off, no block leads and the anchor is gone", needsPi, () =>
	withFixture({ tools: SERVICE_TOOLS, agentSettings: { toolbox: false } }, async (fixture) => {
		const { rec, seen } = await collectStatuses(fixture, {});

		// The anchor is gone with the toolbox.
		assert.ok(!seen.some((s) => s.key === ANCHOR_KEY), "the anchor must be off with the toolbox");
		for (const { key, value } of seen) {
			assertGrammar(value, { toolboxOn: false, isAnchor: key === ANCHOR_KEY }, key);
		}
	}));

test("no footer block is wider than the width it would share", needsPi, () =>
	withFixture({ tools: SERVICE_TOOLS }, async (fixture) => {
		const { seen } = await collectStatuses(fixture, {});
		for (const { value } of seen) {
			assert.ok(piTui.visibleWidth(value) <= 60, `${JSON.stringify(value)} is wider than a footer block's budget`);
		}
	}));

// The known command-triggered statuses, driven explicitly: their statuses
// appear only through their own commands, not through the two lifecycle
// events the sweep above drives.

const DRIVEN = [
	{ file: "extensions/identity/index.ts", command: "identity", arg: "publisher" },
	{ file: "extensions/auto-continue/index.ts", command: "auto-continue", arg: "on" },
	{ file: "extensions/rolling-context/index.ts", command: "rolling", arg: "on" },
	{ file: "extensions/reporting/index.ts", command: "report", arg: "level 2" },
];

test("command-triggered blocks follow the same grammar, anchored or not", needsPi, async () => {
	const rec = recordingTheme;
	for (const toolboxOn of [true, false]) {
		await withFixture({ agentSettings: toolboxOn ? undefined : { toolbox: false } }, async (fixture) => {
			for (const { file, command, arg } of DRIVEN) {
				const loaded = await loadExtension(file, fixture);
				const made = makeContext(fixture, { theme: rec.theme });
				await loaded.extension.commands.get(command).handler(arg, made.ctx);

				const value = made.calls.status.at(-1)?.value;
				assert.ok(value, `${file}: the ${command} command set no footer block`);
				assertGrammar(value, { toolboxOn, isAnchor: false }, file);
			}
		});
	}
});

