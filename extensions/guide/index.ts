/**
 * guide -- /guide for the person at the keyboard: what REactor is, what the
 * footer is telling them, and the flows they will actually type in pi.
 *
 * Three depths, one command:
 * - `/guide`            the overview: the concept, the footer, the flows;
 * - `/guide tools`      the index -- every tool, one line each;
 * - `/guide <name>`     the detail page for one tool, aliases included
 *                       (`/guide manifest` opens the manifest page).
 * An unknown name falls back to the index with a notice, so a typo never
 * dead-ends.
 *
 * The overlay is a scrollable window over the requested page: up/down by
 * line, PgUp/PgDn by page, q/Esc to close. Nothing here is agent-facing: the
 * model never sees any of it. The module carries no state and touches no CLI.
 */

import type { ExtensionAPI, ExtensionContext, Theme } from "@earendil-works/pi-coding-agent";
import type { Component, TUI } from "@earendil-works/pi-tui";
import { getKeybindings, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { frame } from "../lib/overlay.ts";

/** The key hints, the frame's last line. */
const HINTS_TEXT = "↑/↓ line · PgUp/PgDn page · q/Esc close";

interface PageLine {
	kind: "header" | "flow" | "text";
	/** For a `flow` line: the emphasized token -- a command or glyph. */
	a?: string;
	/** For a `flow` line: the explanation that follows it. */
	b?: string;
	/** For a `text` line: the whole line. */
	text?: string;
}

interface Page {
	title: string;
	lines: PageLine[];
}

/** The overview: the concept, the footer, the flows, and where to go next. */
const OVERVIEW: Page = {
	title: "REACTOR -- a reverse-engineering harness for pi",
	lines: [
		{ kind: "header", text: "THE CONCEPT" },
		{ kind: "text", text: "REactor is a reverse-engineering harness for pi: a catalogue of RE tools" },
		{ kind: "text", text: "(tools.toml), a `reactor` CLI that probes what is installed and running," },
		{ kind: "text", text: "and extensions that surface that to the agent (a registry in its system" },
		{ kind: "text", text: "prompt) and to you (the footer, panels and the commands below). Nothing" },
		{ kind: "text", text: "here blocks the agent -- it is advertising and steering, never enforcing." },
		{ kind: "text", text: "" },
		{ kind: "header", text: "WHAT YOU SEE" },
		{ kind: "flow", a: "🛠 5/50 tools", b: "how much of the catalogue is installed -- the footer's anchor" },
		{ kind: "flow", a: "✗ bn · ● adb", b: "services that are actually running, right now" },
		{ kind: "flow", a: "◎ goal · 3 steps", b: "the session manifest: your goal, the agent's steps" },
		{ kind: "text", text: "" },
		{ kind: "header", text: "THE FLOWS" },
		{ kind: "flow", a: "run pi in the dir", b: "the agent already knows what is installed -- /reactor shows it" },
		{ kind: "flow", a: "/reactor-tools", b: "choose which tools and toolsets it is told about" },
		{ kind: "flow", a: "/reactor-status", b: "watch services live; refresh; mute what is noise" },
		{
			kind: "flow",
			a: "/goal <text>",
			b: "anchor the session -- update_steps wakes up and the agent maintains the steps",
		},
		{ kind: "flow", a: "/reactor-scenario start", b: "run a guided analysis; the agent works phase by phase" },
		{ kind: "flow", a: "/rolling · /auto-continue", b: "fade old context, keep going after compaction" },
		{ kind: "flow", a: "/identity", b: "switch the persona (reverse-engineer, forensics, ...)" },
		{ kind: "flow", a: "/report on", b: "make the agent document findings as it goes" },
		{ kind: "text", text: "" },
		{ kind: "header", text: "WHEN SOMETHING LOOKS OFF" },
		{ kind: "flow", a: "/reactor-status refresh", b: "re-probe the services" },
		{ kind: "flow", a: "/reactor refresh", b: "re-detect the catalogue" },
		{ kind: "flow", a: "reactor doctor", b: "diagnose the CLI itself" },
		{ kind: "flow", a: "/guide", b: "this popup" },
		{ kind: "text", text: "" },
		{ kind: "text", text: "The agent's tools appear when they become relevant: update_steps after a" },
		{ kind: "text", text: "/goal, reactor_phase_complete inside a scenario." },
		{ kind: "flow", a: "/guide tools", b: "the index of every tool -- /guide <name> opens its page" },
	],
};

/** The index: every tool, one line each, pointing at its page. */
const TOOLS_PAGE: Page = {
	title: "THE TOOLS",
	lines: [
		{ kind: "header", text: "THE COMMANDS YOU TYPE" },
		{ kind: "flow", a: "/reactor", b: "the registry as the agent sees it; refresh -- /guide reactor" },
		{ kind: "flow", a: "/reactor-toolbox", b: "the whole toolbox on or off" },
		{ kind: "flow", a: "/reactor-tools", b: "choose tools and toolsets (the selector)" },
		{ kind: "flow", a: "/reactor-status", b: "services, the panel, mute -- /guide reactor-status" },
		{
			kind: "flow",
			a: "/reactor-scenario",
			b: "guided analyses, phase by phase -- /guide reactor-scenario",
		},
		{ kind: "flow", a: "/goal · /guidelines · /manifest · /frame · /derive", b: "the manifest -- /guide goal" },
		{ kind: "flow", a: "/identity", b: "the persona -- /guide identity" },
		{ kind: "flow", a: "/report", b: "document-as-you-go -- /guide report" },
		{ kind: "flow", a: "/rolling", b: "the fade -- /guide rolling" },
		{ kind: "flow", a: "/auto-continue", b: "continue after compaction -- /guide auto-continue" },
		{ kind: "flow", a: "/context-editor", b: "fork with a trimmed context -- /guide context-editor" },
		{ kind: "text", text: "" },
		{ kind: "header", text: "THE AGENT'S TOOLS" },
		{ kind: "flow", a: "update_steps", b: "maintain the manifest's steps (after /goal) -- /guide update_steps" },
		{
			kind: "flow",
			a: "reactor_phase_complete",
			b: "advance the scenario's phase -- /guide reactor_phase_complete",
		},
		{ kind: "flow", a: "history_search · history_read", b: "recover faded history" },
	],
};

const PAGES: Record<string, Page> = {
	tools: TOOLS_PAGE,
	reactor: {
		title: "THE REGISTRY & THE TOOLBOX",
		lines: [
			{ kind: "header", text: "WHAT IT IS" },
			{
				kind: "text",
				text: "The registry is the block in the agent's system prompt: one line per",
			},
			{ kind: "text", text: "present, active tool -- what exists, how to invoke it. It is rebuilt" },
			{ kind: "text", text: "from the catalogue every turn; you never edit it by hand." },
			{ kind: "header", text: "THE COMMANDS" },
			{ kind: "flow", a: "/reactor", b: "show the block exactly as the agent sees it" },
			{ kind: "flow", a: "/reactor refresh", b: "drop the cache, re-probe everything" },
			{ kind: "flow", a: "/reactor-toolbox", b: "report whether the toolbox is on" },
			{ kind: "flow", a: "/reactor-toolbox off", b: "hide the whole toolbox, then reload" },
			{ kind: "header", text: "THE FOOTER" },
			{ kind: "text", text: "🛠 N/M tools anchors the statusbar line; the other blocks hang off it." },
		],
	},
	"reactor-tools": {
		title: "THE TOOL SELECTOR",
		lines: [
			{
				kind: "text",
				text: "A full-width overlay over the catalogue: pick which tools and",
			},
			{
				kind: "text",
				text: "toolsets the agent is told about. Deactivation is soft -- nothing is",
			},
			{ kind: "text", text: "blocked; a deactivated tool can still be used, it just stops being" },
			{ kind: "text", text: "advertised in the registry." },
			{ kind: "header", text: "THE KEYS" },
			{ kind: "flow", a: "↑/↓", b: "move · / searches · Tab switches tools ⇄ toolsets" },
			{ kind: "flow", a: "space", b: "toggle · Enter inspects · ctrl+r resets overrides" },
			{ kind: "text", text: "Outside a TUI: `reactor tools list|enable|disable|reset <id>`." },
		],
	},
	"reactor-status": {
		title: "THE STATUS PANEL",
		lines: [
			{
				kind: "text",
				text: "What is running right now -- BN sessions, adb devices -- in a panel",
			},
			{ kind: "text", text: "above the editor, with the footer's services line beneath it." },
			{ kind: "header", text: "THE COMMANDS" },
			{ kind: "flow", a: "/reactor-status", b: "open the panel (again closes it)" },
			{ kind: "flow", a: "/reactor-status refresh", b: "re-probe the services now" },
			{ kind: "flow", a: "/reactor-status hide", b: "close it" },
			{ kind: "flow", a: "/reactor-status mute <id>", b: "leave a service out until unmute" },
			{ kind: "header", text: "READING IT" },
			{ kind: "text", text: "✗ means down -- do something. ● means up. ? and ○ mean nothing to" },
			{ kind: "text", text: "act on: an unanswered probe and an uninstalled tool." },
		],
	},
	"reactor-scenario": {
		title: "SCENARIOS",
		lines: [
			{
				kind: "text",
				text: "A guided analysis: an ordered set of phase briefings -- scoping,",
			},
			{ kind: "text", text: "acquisition, triage, static, dynamic, ... The agent works one phase" },
			{ kind: "text", text: "at a time and calls reactor_phase_complete to receive the next" },
			{ kind: "text", text: "briefing. Phases are the scenario's own stages; the manifest's steps" },
			{ kind: "text", text: "are a different thing." },
			{ kind: "header", text: "THE COMMANDS" },
			{ kind: "flow", a: "/reactor-scenario list", b: "what scenarios exist" },
			{ kind: "flow", a: "/reactor-scenario start <id>", b: "begin -- the first briefing arrives" },
			{ kind: "flow", a: "/reactor-scenario status", b: "where you are" },
			{ kind: "flow", a: "/reactor-scenario next [summary]", b: "advance by hand" },
			{ kind: "flow", a: "/reactor-scenario stop", b: "end it" },
			{ kind: "text", text: "The tool is advertised while a scenario runs, withdrawn when it ends." },
		],
	},
	goal: {
		title: "THE MANIFEST",
		lines: [
			{ kind: "text", text: "/goal <text> gives the session its anchor. The goal goes into the" },
			{ kind: "text", text: "agent's system prompt; update_steps wakes up and the agent maintains" },
			{ kind: "text", text: "its own steps; the goal row appears above the footer." },
			{ kind: "header", text: "THE COMMANDS" },
			{ kind: "flow", a: "/goal <text>", b: "set it -- this is what activates the steps tool" },
			{ kind: "flow", a: "/goal clear", b: "remove it (the steps survive)" },
			{ kind: "flow", a: "/guidelines <text>", b: "standing rules, into the system prompt" },
			{ kind: "flow", a: "/manifest off|on", b: "pause or resume the whole thing" },
			{ kind: "flow", a: "/manifest clear", b: "reset: goal, guidelines, steps gone" },
			{ kind: "flow", a: "/frame", b: "review the manifest" },
			{ kind: "flow", a: "/derive [scope]", b: "derive goal, guidelines or steps from the session" },
			{ kind: "text", text: "The agent's tool for the steps is update_steps -- see /guide update_steps." },
		],
	},
	identity: {
		title: "THE PERSONA",
		lines: [
			{ kind: "text", text: "/identity switches the agent's working persona: reverse-engineer," },
			{ kind: "text", text: "cyber-forensics, forensics, software-engineer, infrastructure," },
			{ kind: "text", text: "publisher -- or your own saved text. The persona block goes into the" },
			{ kind: "text", text: "system prompt." },
			{ kind: "header", text: "THE COMMANDS" },
			{ kind: "flow", a: "/identity <name>", b: "switch to a built-in or saved persona" },
			{ kind: "flow", a: "/identity off", b: "none active" },
			{ kind: "flow", a: "/identity write <text> · editor", b: "an adhoc custom persona" },
			{ kind: "flow", a: "/identity save <name> · delete <name>", b: "manage saved ones" },
		],
	},
	report: {
		title: "REPORTING MODE",
		lines: [
			{ kind: "text", text: "/report on makes the agent document findings as it works, citing" },
			{ kind: "text", text: "where in the target each finding came from. Levels: low nags once" },
			{ kind: "text", text: "per turn; strict reverts a turn that ignored the requirement." },
			{ kind: "header", text: "THE COMMANDS" },
			{ kind: "flow", a: "/report on", b: "switch it on at the default level" },
			{ kind: "flow", a: "/report level 1|2", b: "low or strict" },
			{ kind: "flow", a: "/report status", b: "where things stand" },
			{ kind: "flow", a: "/report off", b: "back to plain" },
		],
	},
	rolling: {
		title: "THE FADE",
		lines: [
			{ kind: "text", text: "/rolling on replaces pi's own compaction with the fade: the newest" },
			{ kind: "text", text: "messages that fit the budget go to the model, older ones are left" },
			{ kind: "text", text: "out -- but the session file keeps everything, and the agent can" },
			{ kind: "text", text: "recover it with the history tools." },
			{ kind: "header", text: "THE COMMANDS" },
			{ kind: "flow", a: "/rolling on|off", b: "the fade replaces pi's compaction" },
			{ kind: "flow", a: "history_search · history_read", b: "the agent recovers faded history" },
		],
	},
	"auto-continue": {
		title: "AUTO-CONTINUE",
		lines: [
			{ kind: "text", text: "After an automatic compaction ends the agent's turn, this continues" },
			{ kind: "text", text: "it, bounded by a maximum number of continues -- and never into pi's" },
			{ kind: "text", text: "own retry machinery." },
			{ kind: "header", text: "THE COMMANDS" },
			{ kind: "flow", a: "/auto-continue on|off", b: "the bounded continue" },
		],
	},
	"context-editor": {
		title: "THE CONTEXT EDITOR",
		lines: [
			{ kind: "text", text: "Edit what the agent sees: a landscape view to toggle entries out of" },
			{ kind: "text", text: "the context, or manual mode to edit it in $EDITOR. Forking applies" },
			{ kind: "text", text: "the edit as a fresh branch; a filter hides recurring noise on the" },
			{ kind: "text", text: "current branch." },
			{ kind: "header", text: "THE COMMAND" },
			{ kind: "flow", a: "/context-editor", b: "landscape view" },
			{ kind: "flow", a: "/context-editor manual", b: "edit in $EDITOR" },
		],
	},
	update_steps: {
		title: "UPDATE STEPS (the agent's tool)",
		lines: [
			{ kind: "text", text: "The agent's tool for maintaining the manifest's steps: conceptual" },
			{ kind: "text", text: "summaries with 3-word statuses, overwriting the whole list each" },
			{ kind: "text", text: "call. It is advertised only while a goal is set and the switch is" },
			{ kind: "text", text: "on; reached for without one, it returns instructions instead. You" },
			{ kind: "text", text: "set the goal; the agent maintains the steps." },
		],
	},
	reactor_phase_complete: {
		title: "REACTOR PHASE COMPLETE (the agent's tool)",
		lines: [
			{ kind: "text", text: "Advances the running scenario: marks the current phase complete and" },
			{ kind: "text", text: "returns the next phase's briefing as the tool result. It is" },
			{ kind: "text", text: "advertised only while a scenario runs. You can advance by hand with" },
			{ kind: "text", text: "/reactor-scenario next." },
		],
	},
};

/** Names that open another tool's page: the command belongs to that system. */
const ALIASES: Record<string, string> = {
	manifest: "goal",
	guidelines: "goal",
	frame: "goal",
	derive: "goal",
	scenario: "reactor-scenario",
	toolbox: "reactor",
};

/** The scrollable window over the requested page: title + body slice + hints. */
class GuideOverlay implements Component {
	private offset = 0;
	/** Set by every render; input handling wraps to the same width. */
	private lastWidth = 100;

	constructor(
		private tui: TUI,
		private theme: Theme,
		private done: () => void,
		private page: Page,
		private notice: string | undefined,
	) {}

	invalidate(): void {}

	private viewport(): number {
		// The overlay clips at the terminal height; reserve room for the
		// frame's own title and hints.
		return Math.max(6, (this.tui.terminal.rows ?? 40) - 5);
	}

	private body(width: number): string[] {
		const t = this.theme;
		return [
			...(this.notice ? [t.fg("warning", `  ${this.notice}`), ""] : []),
			...this.page.lines.flatMap((line) => {
				const rendered =
					line.kind === "header"
						? t.bold(t.fg("accent", `  ${line.text ?? ""}`))
						: line.kind === "flow"
							? `  ${t.fg("accent", line.a ?? "")}${line.b ? `  ${t.fg("muted", line.b)}` : ""}`
							: line.text
								? `  ${t.fg("muted", line.text)}`
								: "";
			return wrapTextWithAnsi(rendered, Math.max(20, width - 1));
		}),
		];
	}

	render(width: number): string[] {
		this.lastWidth = width;
		const inner = Math.max(8, width - 4);
		const body = this.body(inner);
		const height = this.viewport();
		const maxOffset = Math.max(0, body.length - height);
		this.offset = Math.min(Math.max(0, this.offset), maxOffset);
		const shown = body.slice(this.offset, this.offset + height);
		const position = maxOffset > 0 ? `  (${this.offset + 1}–${Math.min(body.length, this.offset + height)}/${body.length})` : "";
		return frame(this.theme, width, {
			title: this.page.title,
			lines: shown,
			hint: `${HINTS_TEXT}${position}`,
		});
	}

	handleInput(data: string): void {
		const kb = getKeybindings();
		const height = this.viewport();
		const maxOffset = Math.max(0, this.body(Math.max(8, this.lastWidth - 4)).length - height);
		if (kb.matches(data, "tui.select.cancel") || kb.matches(data, "tui.select.confirm") || data === "q") {
			this.done();
			return;
		}
		if (kb.matches(data, "tui.select.up")) {
			this.offset = Math.max(0, this.offset - 1);
		} else if (kb.matches(data, "tui.select.down")) {
			this.offset = Math.min(maxOffset, this.offset + 1);
		} else if (kb.matches(data, "tui.select.pageUp")) {
			this.offset = Math.max(0, this.offset - height);
		} else if (kb.matches(data, "tui.select.pageDown")) {
			this.offset = Math.min(maxOffset, this.offset + height);
		} else {
			return;
		}
		this.tui.requestRender();
	}
}

export default function guide(pi: ExtensionAPI) {
	/** Page names for /guide <name>, the index included. */
	const pageNames = () => Object.keys(PAGES);

	pi.registerCommand("guide", {
		description: "REactor: what this is, and the flows to drive it (/guide tools lists the tools)",
		getArgumentCompletions: (prefix: string) => {
			const prefixText = prefix.trim().toLowerCase();
			return pageNames()
				.filter((name) => name.startsWith(prefixText))
				.map((name) => ({ value: name, label: name }));
		},
		handler: async (args: string, ctx: ExtensionContext) => {
			const name = (args ?? "").trim().toLowerCase().replace(/^\//, "");
			const resolved = name ? PAGES[name] ?? PAGES[ALIASES[name] ?? ""] : undefined;
			// No argument opens the overview; an unknown name falls back to the
			// index with a notice, so a typo never dead-ends.
			const page = name ? (resolved ?? PAGES.tools) : OVERVIEW;
			const notice = name && !resolved ? `no guide page for "${name}" -- the index follows` : undefined;

			if (ctx.mode !== "tui") {
				// The popup needs a terminal, like every overlay. Outside one,
				// the flows still fit one line each -- say the short version.
				ctx.ui.notify(
					"reactor guide: /reactor (what the agent is told) · /reactor-tools (choose tools) · " +
						"/reactor-status (what is running) · /goal (manifest) · /reactor-scenario (phases) · " +
						"/rolling, /auto-continue, /identity, /report",
					"info",
				);
				return;
			}
			await ctx.ui.custom<void>(
				(tui, theme, _keybindings, done) => new GuideOverlay(tui, theme, done, page, notice),
				// Same reason as the selector: the default cap is 80 columns,
				// which is narrow for wrapped prose.
				{ overlay: true, overlayOptions: { width: "100%", maxHeight: "100%" } },
			);
		},
	});
}