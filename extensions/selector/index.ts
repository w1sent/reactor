/**
 * selector -- choose what the agent is told about.
 *
 * The registry extension answers "what is on this machine". This one answers
 * "which of it is worth mentioning", which is a curation problem: a block
 * advertising two dozen tools when six are relevant is noise, and noise
 * degrades tool selection ([ADR-0007](../../docs/adr/0007-deactivation-is-soft.md)).
 *
 * One overlay, two panes -- tools and toolsets, switched with Tab. Any
 * printable key filters the current pane from the first keystroke (id, name,
 * description and tags all match); "/" is the conventional shortcut for that
 * same behaviour and resets the query rather than being typed into it, which
 * matters once the catalogue is too long to page through by eye. Space
 * toggles, Enter inspects, and every mutation is a `reactor` call: activation
 * is derived from the toolsets rather than stored, so only the CLI knows what
 * the smallest correct edit to `state.json` is
 * ([ADR-0011](../../docs/adr/0011-selector-edits-overrides-not-outcomes.md)).
 * This file computes nothing about the catalogue; it renders what it is told
 * and forwards keystrokes.
 */

import { getAgentDir } from "@earendil-works/pi-coding-agent";
import type {
	ExtensionAPI,
	ExtensionCommandContext,
	Theme,
} from "@earendil-works/pi-coding-agent";
import type { Component, TUI } from "@earendil-works/pi-tui";
import { fuzzyFilter, getKeybindings, Text, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";
import { readFileSync } from "node:fs";
import { join } from "node:path";

/** Shape of `reactor tools list --format json`. Part of REactor's contract. */
interface ToolRow {
	id: string;
	name: string;
	desc: string;
	tags: string[];
	status: "present" | "absent" | "unknown";
	version: string | null;
	active: boolean;
	/** What state.json says about this tool alone, ignoring the toolsets. */
	override: "on" | "off" | null;
	skill: { fetched: boolean; dir?: string } | null;
}

interface ToolsetRow {
	id: string;
	desc: string;
	active: boolean;
	tools: string[];
}

/** Shape of `reactor state` and of every enable/disable/reset response. */
interface StatePayload {
	state: { toolsets: string[]; tools: { enabled: string[]; disabled: string[] } };
	active: string[];
	path?: string;
	scope?: string;
	error?: string;
}

const EXEC_TIMEOUT_MS = 20_000;

/** Rows the list gives up to the header, the hint line and breathing room. */
const CHROME_LINES = 8;

/** Bounds on the visible rows when the terminal height is unknown or extreme. */
const MIN_VISIBLE = 5;
const MAX_VISIBLE = 30;

/** How much of a skill body a collapsed session entry shows. */
const COLLAPSED_LINES = 12;

type Pane = "tools" | "toolsets";

/** What the overlay hands back to the command handler when it closes. */
interface Outcome {
	/** Whether anything was written, and so whether resources need reloading. */
	changed: boolean;
	/** A tool whose fetched skill the user asked to read. */
	skill?: string;
}

/**
 * `false` in `<agent dir>/reactor.json` (normally `~/.pi/agent/reactor.json`)
 * hides this whole extension, as if it were never loaded (ADR-0016) -- the
 * same file and the same flag `tool-registry/` checks, since `/reactor-tools`
 * is the other half of "the toolbox". Read once at registration; `/reactor-toolbox`
 * (registered in `tool-registry/`, unconditionally, so it survives being off)
 * reloads after writing this, so a flip through that command takes effect at
 * once. A hand-edit of the file still needs a manual `/reload`.
 */
function toolboxEnabled(): boolean {
	try {
		const doc = JSON.parse(readFileSync(join(getAgentDir(), "reactor.json"), "utf8"));
		return doc.toolbox !== false;
	} catch {
		return true;
	}
}

export default function selector(pi: ExtensionAPI) {
	if (!toolboxEnabled()) return;

	async function reactor<T>(ctx: ExtensionCommandContext, args: string[]): Promise<T | undefined> {
		// pi.exec resolves rather than throwing, including on ENOENT, so a
		// missing CLI arrives as code 1 with empty stdout. Branch on the output.
		const result = await pi.exec("reactor", [...args, "--format", "json"], {
			timeout: EXEC_TIMEOUT_MS,
			cwd: ctx.cwd,
		});
		if (result.killed || !result.stdout.trim()) return undefined;
		try {
			const payload = JSON.parse(result.stdout) as T & { error?: string };
			return payload.error ? undefined : payload;
		} catch {
			return undefined;
		}
	}

	/** Raw human-readable output, for panes that just show what the CLI prints. */
	async function reactorText(ctx: ExtensionCommandContext, args: string[]): Promise<string> {
		const result = await pi.exec("reactor", args, { timeout: EXEC_TIMEOUT_MS, cwd: ctx.cwd });
		const out = `${result.stdout}${result.stderr}`.trimEnd();
		return out || "reactor: no output";
	}

	/**
	 * Detail and skill bodies are for the person at the keyboard, so they go in
	 * as session entries rather than messages: `convertToLlm` turns a custom
	 * message into a *user* message, and the model has the registry block
	 * already (ADR-0011).
	 */
	pi.registerEntryRenderer<{ title: string; body: string }>(
		"reactor-detail",
		(entry, options, theme) => {
			const data = entry.data;
			if (!data) return undefined;
			const lines = data.body.split("\n");
			// A skill is a document, not a status line. Collapsed is the
			// default view, so it shows the head and says how much it is hiding.
			const shown =
				options.expanded || lines.length <= COLLAPSED_LINES
					? lines
					: [...lines.slice(0, COLLAPSED_LINES), `… ${lines.length - COLLAPSED_LINES} more lines`];
			return new Text(
				`${theme.bold(theme.fg("accent", data.title))}\n${theme.fg("toolOutput", shown.join("\n"))}`,
				1,
				1,
			);
		},
	);

	pi.registerCommand("reactor-tools", {
		description: "REactor: choose which tools the agent is told about",
		handler: async (_args, ctx) => {
			if (ctx.mode !== "tui") {
				// `ctx.ui.custom` mounts a terminal component, so this is
				// narrower than hasUI, which is also true for RPC. The CLI is
				// the whole backend anyway: point at it rather than half-work.
				const state = await reactor<StatePayload>(ctx, ["state"]);
				ctx.ui.notify(
					state
						? `reactor: ${state.active.length} tool(s) active -- use \`reactor tools list\` and ` +
							"`reactor tools enable|disable|reset <id>` outside the TUI"
						: "reactor: could not reach the CLI -- try `reactor doctor`",
					state ? "info" : "error",
				);
				return;
			}

			// A live probe, not `--cached`: opening the selector is a deliberate
			// act, so it can afford the cold path once rather than showing a
			// screen of "unknown" to someone who came here to decide something.
			const [tools, toolsets, state] = await Promise.all([
				reactor<{ tools: ToolRow[] }>(ctx, ["tools", "list"]),
				reactor<{ toolsets: ToolsetRow[] }>(ctx, ["toolsets", "list"]),
				reactor<StatePayload>(ctx, ["state"]),
			]);
			if (!tools || !toolsets || !state) {
				ctx.ui.notify("reactor: could not reach the CLI -- try `reactor doctor`", "error");
				return;
			}

			// `?? {}` because a component can also be torn down by pi rather
			// than by its own `done`, and that arrives as an unresolved value.
			const outcome = (await ctx.ui.custom<Outcome>(
				(tui, theme, _keybindings, done) =>
					new SelectorOverlay(tui, theme, done, tools.tools, toolsets.toolsets, state, {
						mutate: (args) => reactor<StatePayload>(ctx, args),
						detail: (id) => reactorText(ctx, ["tools", "show", id]),
					}),
				// Without this an overlay is capped at 80 columns
				// (`resolveOverlayLayout`: `Math.min(80, availWidth)`), which
				// truncates the description column on any real terminal. The
				// list is a table, so it wants the whole width.
				{ overlay: true, overlayOptions: { width: "100%", maxHeight: "100%" } },
			)) ?? { changed: false };

			if (outcome.skill) {
				pi.appendEntry("reactor-detail", {
					title: `skill: ${outcome.skill}`,
					body: await reactorText(ctx, ["skills", "show", outcome.skill]),
				});
			}
			if (outcome.changed) {
				// Skills are gated on the same activation state, so a write has
				// to re-run resources_discover. The registry block needs no
				// invalidation: it is recomputed from state.json every turn.
				// Last: pi invalidates this ctx (and the closed-over `pi`) the
				// moment this resolves, so nothing may follow it -- see the
				// skill branch above, not below.
				await ctx.reload();
			}
		},
	});
}

interface Backend {
	mutate(args: string[]): Promise<StatePayload | undefined>;
	detail(id: string): Promise<string>;
}

/**
 * The overlay. Holds no catalogue knowledge -- `active` and `override` come
 * from the CLI on load and are replaced wholesale by each mutation's response,
 * which carries the recomputed activation (ADR-0011).
 */
class SelectorOverlay implements Component {
	private pane: Pane = "tools";
	private query = "";
	private index = 0;
	private changed = false;
	private detailFor: string | undefined;
	private detailBody = "";
	private notice = "";
	private busy = false;

	constructor(
		private tui: TUI,
		private theme: Theme,
		private done: (result: Outcome) => void,
		private tools: ToolRow[],
		private toolsets: ToolsetRow[],
		private state: StatePayload,
		private backend: Backend,
	) {}

	// -- data ---------------------------------------------------------------

	/** Fuzzy over everything a person might remember: id, name, purpose, tags. */
	private rows(): (ToolRow | ToolsetRow)[] {
		const items = this.pane === "tools" ? this.tools : this.toolsets;
		if (!this.query) return items;
		return fuzzyFilter(items, this.query, (item) =>
			"tags" in item
				? `${item.id} ${item.name} ${item.desc} ${item.tags.join(" ")}`
				: `${item.id} ${item.desc}`,
		);
	}

	private current(): ToolRow | ToolsetRow | undefined {
		return this.rows()[this.index];
	}

	/** Replace the derived state from a mutation's response. */
	private absorb(payload: StatePayload): void {
		this.state = { ...this.state, ...payload };
		const active = new Set(payload.active);
		const on = new Set(payload.state.tools.enabled);
		const off = new Set(payload.state.tools.disabled);
		for (const t of this.tools) {
			t.active = active.has(t.id);
			t.override = off.has(t.id) ? "off" : on.has(t.id) ? "on" : null;
		}
		const sets = new Set(payload.state.toolsets);
		for (const s of this.toolsets) s.active = sets.has(s.id);
		this.changed = true;
	}

	private async run(args: string[], failure: string): Promise<void> {
		this.busy = true;
		this.tui.requestRender();
		const payload = await this.backend.mutate(args);
		this.busy = false;
		if (payload) {
			this.absorb(payload);
			this.notice = "";
		} else {
			this.notice = failure;
		}
		this.tui.requestRender();
	}

	// -- input --------------------------------------------------------------

	handleInput(data: string): void {
		const kb = getKeybindings();
		// A notice belongs to the keystroke that produced it. Anything else the
		// user does has answered it.
		this.notice = "";

		if (this.detailFor !== undefined) {
			if (kb.matches(data, "tui.select.cancel")) {
				this.detailFor = undefined;
			} else if (data === "s") {
				const tool = this.tools.find((t) => t.id === this.detailFor);
				if (tool?.skill?.fetched) {
					this.done({ changed: this.changed, skill: tool.id });
					return;
				}
				this.notice = "no fetched skill for this tool";
			}
			this.tui.requestRender();
			return;
		}

		const rows = this.rows();
		if (kb.matches(data, "tui.select.cancel")) {
			this.done({ changed: this.changed });
			return;
		}
		if (kb.matches(data, "tui.select.up")) {
			this.index = this.index === 0 ? Math.max(0, rows.length - 1) : this.index - 1;
		} else if (kb.matches(data, "tui.select.down")) {
			this.index = this.index >= rows.length - 1 ? 0 : this.index + 1;
		} else if (data === "\t") {
			this.pane = this.pane === "tools" ? "toolsets" : "tools";
			this.query = "";
			this.index = 0;
		} else if (data === " ") {
			void this.toggle();
		} else if (data === "\x12") {
			// Ctrl+R. Not a printable character, so it cannot collide with the
			// filter, which every letter key feeds.
			void this.reset();
		} else if (kb.matches(data, "tui.select.confirm")) {
			void this.inspect();
		} else if (data === "\x7f" || data === "\b") {
			this.query = this.query.slice(0, -1);
			this.index = 0;
		} else if (data === "/") {
			// The conventional search key (less, vim, fzf) and, not coincidentally,
			// what the header already renders the query as: `/query`. Typing is
			// filtering from the first keystroke regardless, so this is not a mode
			// switch -- it is a fast way back to a bare prompt without holding
			// backspace, and a discoverable answer to "how do I search" for anyone
			// who has not noticed that every other key already does.
			this.query = "";
			this.index = 0;
		} else if (/^[\x20-\x7e]$/.test(data)) {
			// Space is the toggle and "/" resets, so neither reaches here; every
			// other printable character is filter text.
			this.query += data;
			this.index = 0;
		}
		this.tui.requestRender();
	}

	private async toggle(): Promise<void> {
		const item = this.current();
		if (!item || this.busy) return;
		const verb = item.active ? "disable" : "enable";
		const group = this.pane === "tools" ? "tools" : "toolsets";
		await this.run([group, verb, item.id], `could not ${verb} ${item.id}`);
	}

	private async reset(): Promise<void> {
		if (this.pane !== "tools" || this.busy) return;
		const item = this.current();
		if (!item) return;
		if (!("tags" in item) || item.override === null) {
			this.notice = "nothing to reset -- this tool has no override";
			return;
		}
		await this.run(["tools", "reset", item.id], `could not reset ${item.id}`);
	}

	private async inspect(): Promise<void> {
		const item = this.current();
		if (!item || this.pane !== "tools") return;
		this.detailFor = item.id;
		this.detailBody = "";
		this.busy = true;
		this.tui.requestRender();
		this.detailBody = await this.backend.detail(item.id);
		this.busy = false;
		this.tui.requestRender();
	}

	// -- rendering ----------------------------------------------------------

	invalidate(): void {}

	/**
	 * render() is handed a width but never a height, so the terminal is asked
	 * directly. The overlay is sized to the screen in both directions -- a
	 * fixed row count leaves half a tall terminal empty and overflows a short
	 * one, and 24 catalogue entries are worth seeing at once when they fit.
	 */
	private visibleRows(): number {
		const rows = this.tui.terminal?.rows ?? 0;
		if (!rows) return MIN_VISIBLE * 2;
		return Math.max(MIN_VISIBLE, Math.min(MAX_VISIBLE, rows - CHROME_LINES));
	}

	render(width: number): string[] {
		const t = this.theme;
		if (this.detailFor !== undefined) {
			const hasSkill = this.tools.find((x) => x.id === this.detailFor)?.skill?.fetched;
			const lines = [
				t.bold(t.fg("accent", `  ${this.detailFor}`)),
				"",
				...(this.busy ? ["  ..."] : this.detailBody.split("\n").map((l) => `  ${l}`)),
				"",
				...(this.notice ? [t.fg("error", `  ${this.notice}`)] : []),
				t.fg("dim", hasSkill ? "  s read the fetched skill · esc back" : "  esc back"),
			];
			return lines.map((l) => truncateToWidth(l, width));
		}

		const rows = this.rows();
		this.index = Math.min(this.index, Math.max(0, rows.length - 1));
		const lines = [this.header(width), ""];

		if (rows.length === 0) {
			lines.push(t.fg("muted", `  nothing matches "${this.query}"`));
		} else {
			const visible = this.visibleRows();
			const start = Math.max(0, Math.min(this.index - Math.floor(visible / 2), rows.length - visible));
			const end = Math.min(start + visible, rows.length);
			// Both columns are sized to the rows on screen. The right one has to
			// be: a version cut to fit reads as a *different* version.
			const idWidth = Math.min(16, Math.max(...rows.map((r) => visibleWidth(r.id))));
			const rightWidth = Math.min(20, Math.max(...rows.map((r) => visibleWidth(this.rightText(r)))));
			for (let i = start; i < end; i++) {
				lines.push(
					truncateToWidth(this.row(rows[i], i === this.index, idWidth, rightWidth, width), width),
				);
			}
			if (start > 0 || end < rows.length) {
				lines.push(t.fg("dim", `  (${this.index + 1}/${rows.length})`));
			}
		}

		lines.push("");
		if (this.notice) lines.push(truncateToWidth(t.fg("error", `  ${this.notice}`), width));
		lines.push(truncateToWidth(t.fg("dim", `  ${this.hint()}`), width));
		return lines;
	}

	private header(width: number): string {
		const t = this.theme;
		const sets = this.state.state.toolsets;
		const base = sets.length ? sets.join(", ") : "everything (no toolset selected)";
		const left = `${this.pane === "tools" ? "tools" : "toolsets"}  ${this.state.active.length} active  ·  base: ${base}`;
		const query = this.query ? t.fg("accent", `/${this.query}`) : t.fg("dim", "/type to filter");
		const gap = Math.max(1, width - visibleWidth(left) - visibleWidth(query) - 4);
		return truncateToWidth(`  ${t.bold(left)}${" ".repeat(gap)}${query}`, width);
	}

	/**
	 * The right-hand column. A tool can be active and absent -- activation says
	 * what to mention, detection says what is here, and this is the second one.
	 */
	private rightText(item: ToolRow | ToolsetRow): string {
		if (!("tags" in item)) return `${item.tools.length} tools`;
		return item.status === "present" ? (item.version ?? "present") : item.status;
	}

	private row(
		item: ToolRow | ToolsetRow,
		selected: boolean,
		idWidth: number,
		rightWidth: number,
		width: number,
	): string {
		const t = this.theme;
		const prefix = selected ? t.fg("accent", "→ ") : "  ";
		const id = item.id.padEnd(idWidth);
		const label = selected ? t.fg("accent", id) : id;

		const right = this.rightText(item);
		const state = truncateToWidth(right, rightWidth, "");
		// One column short of the full width on purpose: writing the last cell
		// of a line makes some terminals wrap it.
		const descWidth = Math.max(8, width - visibleWidth(prefix) - 4 - idWidth - 5 - rightWidth);
		const desc = truncateToWidth(item.desc, descWidth, "…", true);
		const box = "tags" in item ? this.box(item.active, item.override) : this.box(item.active, null);
		const dim = "tags" in item && !item.active;
		return (
			`${prefix}${box} ${label}  ${t.fg(dim ? "dim" : "muted", desc)}  ` +
			`${t.fg(right === "absent" ? "dim" : "text", state)}`
		);
	}

	/**
	 * Four glyphs, because there are four states worth telling apart: on and
	 * off, each of them either inherited from the toolsets or pinned in
	 * state.json. Without the distinction a toolset switch looks arbitrary.
	 */
	private box(active: boolean, override: "on" | "off" | null): string {
		const t = this.theme;
		if (override === "on") return t.fg("accent", "[+]");
		if (override === "off") return t.fg("accent", "[-]");
		return active ? t.fg("success", "[x]") : t.fg("dim", "[ ]");
	}

	private hint(): string {
		const where = this.pane === "tools" ? "toolsets" : "tools";
		return this.pane === "tools"
			? `space toggle · enter inspect · ctrl-r unpin · / reset search · tab ${where} · esc close   [+]/[-] pinned`
			: `space toggle · / reset search · tab ${where} · esc close`;
	}
}
