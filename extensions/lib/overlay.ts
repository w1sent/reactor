/**
 * overlay -- the frame every REactor popup sits in: a square border in the
 * theme's border colour, the title set into the top rule, the key hints set
 * into the bottom one, and the content on a tinted background -- so an
 * overlay reads as an overlay at a glance instead of as more session output.
 *
 * Stateless presentation code (ADR-0029): pure over (theme, width, args).
 * The tint is the theme's own selectedBg, so both colour modes carry it.
 */

import type { Theme } from "@earendil-works/pi-coding-agent";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

export interface FramedLines {
	title: string;
	lines: string[];
	hint: string;
}

/**
 * Frame `lines` for a popup of `width` columns. The content is computed by
 * the caller at width - 4 (the two side borders and their margins); anything
 * longer is truncated there, never clipped through the border.
 */
export function frame(t: Theme, width: number, opts: FramedLines): string[] {
	// The content region between the borders and their margins.
	const inner = Math.max(8, width - 4);
	const rule = (label: string, open: string, close: string): string => {
		const dashes = "─".repeat(Math.max(0, inner - 1 - visibleWidth(label)));
		return truncateToWidth(
			`${t.fg("border", open + "─ ")}${t.fg("accent", label)}${t.fg("border", " " + dashes + close)}`,
			width,
		);
	};
	return [
		rule(opts.title, "┌", "┐"),
		...opts.lines.map((line) => {
			const content = `${truncateToWidth(line, inner)}${" ".repeat(Math.max(0, inner - visibleWidth(line)))}`;
			return `${t.fg("border", "│")}${t.bg("selectedBg", ` ${content} `)}${t.fg("border", "│")}`;
		}),
		rule(opts.hint, "└", "┘"),
	];
}