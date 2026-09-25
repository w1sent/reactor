/**
 * gui-bridge: the commands reactor-gui needs that pi's RPC itself does not
 * carry (ADR-0032, gui/SPEC.md §4.6).
 *
 * `/reactor-tree <entryId>` moves the active branch through
 * `ctx.navigateTree` -- the handler pi wires into extension command contexts
 * in every mode (docs/pi-api-notes.md, "navigateTree is exposed to extension
 * commands in RPC mode"). What is asserted: the bridge navigates with the
 * entry id it was given, refuses an empty one, and registers exactly one
 * command -- it is a bridge, not a feature.
 */

import assert from "node:assert/strict";
import { test } from "node:test";
import { loadExtension, makeContext, needsPi, withFixture } from "./harness.mjs";

const EXT = "extensions/gui-bridge/index.ts";
const TREE_COMMAND = "reactor-tree";

test(
	"gui-bridge registers exactly the tree command",
	needsPi,
	async () => {
		await withFixture({}, async (fixture) => {
			const { extension } = await loadExtension(EXT, fixture);
			assert.ok(extension.commands.has(TREE_COMMAND));
			assert.equal(extension.commands.size, 1);
			assert.match(
				extension.commands.get(TREE_COMMAND).description,
				/move the active branch/,
			);
		});
	},
);

test(
	"/reactor-tree <entryId> navigates with the id it was given",
	needsPi,
	async () => {
		await withFixture({}, async (fixture) => {
			const { extension } = await loadExtension(EXT, fixture);
			const { ctx, calls } = await import("./harness.mjs").then((h) =>
				h.makeContext(fixture, { mode: "rpc" }),
			);
			await extension.commands.get(TREE_COMMAND).handler("abc123", ctx);
			assert.deepEqual(calls.navigateTree, [{ targetId: "abc123", options: undefined }]);
		});
	},
);

test(
	"/reactor-tree without an id says so and navigates nowhere",
	needsPi,
	async () => {
		await withFixture({}, async (fixture) => {
			const { extension } = await loadExtension(EXT, fixture);
			const { ctx, calls } = await import("./harness.mjs").then((h) =>
				h.makeContext(fixture, { mode: "rpc" }),
			);
			await extension.commands.get(TREE_COMMAND).handler("", ctx);
			assert.deepEqual(calls.navigateTree, [], "no navigation without an id");
			assert.equal(calls.notify.length, 1);
			assert.match(calls.notify[0].message, /entry id is required/);
			assert.equal(calls.notify[0].level, "error");
		});
	},
);

test(
	"the bridge works in TUI mode too -- the same handler, both hosts",
	needsPi,
	async () => {
		await withFixture({}, async (fixture) => {
			const { extension } = await loadExtension(EXT, fixture);
			const { ctx, calls } = await import("./harness.mjs").then((h) =>
				h.makeContext(fixture, { mode: "tui" }),
			);
			await extension.commands.get(TREE_COMMAND).handler("leaf-9", ctx);
			assert.deepEqual(calls.navigateTree, [{ targetId: "leaf-9", options: undefined }]);
		});
	},
);