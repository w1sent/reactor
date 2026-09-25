/**
 * gui-bridge -- the reactor-gui side of pi's RPC surface: the commands the
 * GUI needs that RPC itself does not carry (ADR-0032, gui/SPEC.md §4.6).
 *
 * pi's RPC has no `navigate_tree` command (the switch ends at get_commands;
 * docs/pi-api-notes.md), but it wires `ctx.navigateTree` into extension
 * command contexts in every mode -- so `/reactor-tree <entryId>` moves the
 * active branch of the session tree for the GUI's tree panel. In the TUI the
 * builtin `/tree` already covers the need; this command is harmless there and
 * works too, because the TUI host wires the same handler.
 *
 * The command is transparent: other RPC clients never see it unless they
 * invoke it, and it registers nothing else -- it is a bridge, not a feature.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/** The command the GUI's tree panel invokes; the payload is the entry id. */
export const TREE_COMMAND = "reactor-tree";

export default function guiBridge(pi: ExtensionAPI) {
	pi.registerCommand(TREE_COMMAND, {
		description: "REactor: move the active branch to a session entry (/reactor-tree <entryId>)",
		handler: async (args, ctx) => {
			const entryId = args.trim();
			if (!entryId) {
				ctx.ui.notify(`/${TREE_COMMAND}: an entry id is required`, "error");
				return;
			}
			// Navigation rejects while an agent response, compaction, or another
			// navigation is active (docs/pi-api-notes.md) -- pi reports the
			// error through the extension error path, and the GUI disables the
			// affordance while streaming, so this is the quiet path, not a
			// fight.
			await ctx.navigateTree(entryId);
		},
	});
}