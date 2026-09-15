/**
 * statusbar -- the one grammar REactor's footer blocks follow, shared as a
 * stateless library (ADR-0029).
 *
 * Every extension that writes a footer block composes it from here. The
 * words are the extension's own -- what its service is doing, which identity
 * is active -- and the shape is everyone's:
 *
 * - a block is a glyph in the anchor colour (`glyph`), then words in the
 *   text colour; counts and qualifiers go dim or muted;
 * - the tool count is the line's anchor: its key (`ANCHOR_KEY`) sorts first,
 *   so it is always the line's first block, and it never leads a separator;
 * - every other block leads with the dim `·` (`lead`) while the anchor is on
 *   the line, so blocks read as one separated line rather than strings pi
 *   happened to join with a space. A block never *ends* with a separator:
 *   the next block leads.
 * - a failure is `✗ <message>` in the error colour (`errorBlock`), whatever
 *   extension it came from.
 *
 * Stateless by contract: pi instantiates this module once per importer
 * (`moduleCache: false` -- ADR-0014's fact about its loader), which is
 * harmless exactly as long as the module holds no mutable state. No
 * module-level `let`, no caches, no counters, no events. Probe results flow
 * through the CLI and `cache.json` (ADR-0014); this module carries
 * presentation, never facts.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { getAgentDir } from "@earendil-works/pi-coding-agent";
import type { Theme, ThemeColor } from "@earendil-works/pi-coding-agent";

/**
 * The key of the anchor block -- the tool count, which sorts first on the
 * line and is the block every other block's separator hangs off.
 */
export const ANCHOR_KEY = "0-reactor";

/**
 * Whether the anchor block is on the line. Its existence is exactly the
 * toolbox flag: `toolbox: false` removes `tool-registry/` and with it the
 * anchor, so nothing leads and no separator dangles. Read fresh from the
 * shared preferences file (ADR-0016) at every call, like everything that
 * reads it.
 */
export function toolboxEnabled(): boolean {
	try {
		return JSON.parse(readFileSync(join(getAgentDir(), "reactor.json"), "utf8")).toolbox !== false;
	} catch {
		return true;
	}
}

/**
 * The separator a non-anchor block leads with while the anchor is on the
 * line. pi trims each status and joins the statuses with one space, so the
 * dot lands exactly between blocks -- never doubled, never dangling at the
 * head of a line whose anchor is absent, because a line without the anchor
 * gets an empty lead.
 */
export function lead(t: Theme): string {
	return toolboxEnabled() ? `${t.fg("dim", "·")} ` : "";
}

/**
 * A block's glyph, in the anchor colour: the one colour that says "this is
 * REactor talking", leaving the state colours -- green, red, yellow -- to
 * mean exactly one thing on the line.
 */
export function glyph(t: Theme, glyph: string): string {
	return t.fg("accent", glyph);
}

/**
 * A failure, in the one colour that means "do something". The message is the
 * extension's own; the mark and the colour are the family's.
 */
export function errorBlock(t: Theme, message: string): string {
	return t.fg("error", `✗ ${message}`);
}