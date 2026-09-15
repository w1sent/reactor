/**
 * status -- what is running, as opposed to what is installed.
 *
 * The registry answers "is Binary Ninja on this machine". This answers "is a
 * session open right now", which is a different question with a different
 * shelf life: presence changes when someone installs something, service state
 * changes when they plug in a phone.
 *
 * That volatility is why this is a separate extension and not another line in
 * `tool-registry`. The registry block is injected into the system prompt and
 * must be byte-stable across turns (ADR-0006); this is a footer and a panel for
 * the person at the keyboard, and it is *expected* to change under them.
 *
 * It shares nothing with the registry extension but `cache.json`, which both
 * reach through the CLI and neither one owns (ADR-0014). There is no shared
 * module and no event between them -- pi loads each extension with its own jiti
 * instance, so a shared module would be instantiated twice and quietly diverge.
 */

import { getAgentDir } from "@earendil-works/pi-coding-agent";
import type {
	BeforeAgentStartEvent,
	ExtensionAPI,
	ExtensionContext,
	SessionStartEvent,
	Theme,
	ThemeColor,
} from "@earendil-works/pi-coding-agent";
import type { Component, TUI } from "@earendil-works/pi-tui";
import { truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { lead } from "../lib/statusbar.ts";
import { join } from "node:path";

/** Shape of `reactor services --format json`. Part of REactor's contract. */
interface ServiceRow {
	id: string;
	name: string;
	label: string;
	state: "up" | "down" | "unknown";
	/** A count, or nothing. Deliberately coarse -- see `_service_detail`. */
	detail: string | null;
	status: "present" | "absent" | "unknown";
	active: boolean;
}

interface ServicesPayload {
	schema: number;
	services: ServiceRow[];
	summary: Record<string, number>;
	error?: string;
}

/** Same backstop as the registry: a wedged CLI costs one slow turn, not a hung one. */
const EXEC_TIMEOUT_MS = 20_000;

const STATUS_KEY = "reactor-status";
const WIDGET_KEY = "reactor-status";

/**
 * How much of the footer's status line REactor may use before it starts
 * shedding detail. The line is shared with the other extensions' entries
 * (identity, auto-continue, ...), whose length no API exposes, so this is a
 * fixed allowance carved off the real terminal width, never the whole width.
 */
const FOOTER_RESERVE = 24;

/** Icon, colour and words for one service's state. */
interface StateDeco {
	glyph: string;
	colour: ThemeColor;
	text: string;
}

const REACTOR_JSON = () => join(getAgentDir(), "reactor.json");

/** Whatever is on disk already, so a write can patch one field without
 * clobbering the other (ADR-0016 covers both `hiddenServices` and `toolbox`
 * in one file). Unreadable or absent both read as "nothing set yet". */
function readReactorJson(): Record<string, unknown> {
	try {
		return JSON.parse(readFileSync(REACTOR_JSON(), "utf8"));
	} catch {
		return {};
	}
}

function writeReactorJson(patch: Record<string, unknown>): void {
	const dir = getAgentDir();
	mkdirSync(dir, { recursive: true });
	writeFileSync(REACTOR_JSON(), `${JSON.stringify({ ...readReactorJson(), ...patch }, null, 2)}\n`);
}

/**
 * Catalogue ids to omit from the footer and the panel -- `hiddenServices` in
 * `<agent dir>/reactor.json` (normally `~/.pi/agent/reactor.json`), the same
 * file `tool-registry/` and `selector/` check for `toolbox` (ADR-0016). A
 * list of ids rather than a bespoke flag per known service, so a future
 * service-backed tool needs no code change here to be hideable.
 *
 * Unlike `toolbox`, this is read fresh on every refresh rather than once at
 * registration: nothing here decides whether to register at all, so there is
 * no reason to make a live edit wait for `/reload`.
 */
function hiddenServices(): Set<string> {
	const ids = readReactorJson().hiddenServices;
	return Array.isArray(ids) ? new Set(ids.filter((id): id is string => typeof id === "string")) : new Set();
}

/** `mute`/`unmute` write this back, letting `hiddenServices()` pick it up on
 * the very next probe -- no reload needed, unlike the toolbox toggle. */
function setServiceHidden(id: string, hidden: boolean): void {
	const ids = hiddenServices();
	if (hidden) ids.add(id);
	else ids.delete(id);
	writeReactorJson({ hiddenServices: [...ids] });
}

export default function status(pi: ExtensionAPI) {
	/**
	 * Last good payload, for the same reason the registry keeps one: a CLI that
	 * did not answer is not evidence that everything went down.
	 */
	let last: ServicesPayload | undefined;
	let shown = false;
	let warned = false;

	async function fetchServices(
		ctx: ExtensionContext,
		args: string[] = [],
	): Promise<ServicesPayload | undefined> {
		// No flags by default: the CLI's TTLs already decide when a service is
		// stale (30s) and when detection is (300s). Re-implementing that
		// schedule here would be the second copy ADR-0005 exists to prevent.
		const result = await pi.exec("reactor", ["services", "--format", "json", ...args], {
			timeout: EXEC_TIMEOUT_MS,
			cwd: ctx.cwd,
		});

		// pi.exec resolves rather than throwing, including on ENOENT, so a
		// missing CLI arrives as code 1 with empty stdout -- same as a crash.
		if (result.killed || !result.stdout.trim()) {
			if (!warned) {
				warned = true;
				ctx.ui.setStatus(STATUS_KEY, undefined);
			}
			return undefined;
		}
		let payload: ServicesPayload;
		try {
			payload = JSON.parse(result.stdout) as ServicesPayload;
		} catch {
			return undefined;
		}
		if (payload.error) return undefined;

		const hidden = hiddenServices();
		if (hidden.size) payload = { ...payload, services: payload.services.filter((s) => !hidden.has(s.id)) };

		last = payload;
		return payload;
	}

	/** Probe, then repaint whatever is currently on screen. */
	async function refresh(ctx: ExtensionContext, args: string[] = []): Promise<boolean> {
		const payload = (await fetchServices(ctx, args)) ?? last;
		if (ctx.mode !== "tui") return payload !== undefined;
		// Colours come from the live theme, re-read per refresh: a `/theme`
		// switch shows up on the next turn without a reload. The lead is the
		// dim separator that ties the line to the anchor block before it.
		ctx.ui.setStatus(
			STATUS_KEY,
			payload ? statusLine(payload, ctx.ui.theme, lead(ctx.ui.theme)) : undefined,
		);
		if (shown) paint(ctx, payload);
		return payload !== undefined;
	}

	function paint(ctx: ExtensionContext, payload: ServicesPayload | undefined): void {
		// The widget is replaced rather than mutated: the panel holds no state
		// worth preserving across a refresh, and a factory closing over fresh
		// data is less to get wrong than a component with a setter.
		ctx.ui.setWidget(
			WIDGET_KEY,
			(_tui: TUI, theme: Theme) => new ServicePanel(theme, payload),
			{ placement: "aboveEditor" },
		);
	}

	pi.on("session_start", async (_event: SessionStartEvent, ctx: ExtensionContext) => {
		await refresh(ctx);
	});

	/**
	 * Once per user turn, which is the right grain rather than a compromise:
	 * the state that matters is the state at the moment the agent acts, and
	 * that moment is this one. No timer, no background probe (ADR-0014).
	 */
	pi.on("before_agent_start", async (_event: BeforeAgentStartEvent, ctx: ExtensionContext) => {
		await refresh(ctx);
	});

	pi.registerCommand("reactor-status", {
		description: "REactor: show what is running -- BN sessions, devices, captures",
		getArgumentCompletions: (prefix: string) =>
			["refresh", "hide", "mute", "unmute"]
				.filter((c) => c.startsWith(prefix))
				.map((c) => ({ value: c, label: c })),
		handler: async (args, ctx) => {
			const [sub, ...rest] = args.trim().split(/\s+/).filter(Boolean);

			// mute/unmute (ADR-0016) work in every mode, not just tui: muting a
			// service is a preference edit, not a display concern, so there is
			// no reason to require a terminal for it.
			if (sub === "mute" || sub === "unmute") {
				const id = rest.join(" ");
				if (!id) {
					ctx.ui.notify(`reactor-status: ${sub} needs a service id, e.g. \`${sub} adb\``, "error");
					return;
				}
				setServiceHidden(id, sub === "mute");
				ctx.ui.notify(`reactor: ${id} ${sub === "mute" ? "muted" : "unmuted"}`, "info");
				if (shown) await refresh(ctx, ["--refresh"]);
				return;
			}

			if (sub && sub !== "refresh" && sub !== "hide") {
				ctx.ui.notify(
					`reactor-status: unknown subcommand "${sub}" -- try refresh, hide, mute <id> or unmute <id>`,
					"error",
				);
				return;
			}

			if (ctx.mode !== "tui") {
				// setWidget needs a terminal to draw into. Outside one, the
				// answer is still worth having, so say it in one line.
				const payload = (await fetchServices(ctx)) ?? last;
				ctx.ui.notify(
					payload
						? payload.services.length
							? payload.services.map(oneLine).join("; ")
							: "reactor: no catalogued tool declares a service probe"
						: "reactor: could not reach the CLI -- try `reactor doctor`",
					payload ? "info" : "error",
				);
				return;
			}

			if (sub === "hide") {
				shown = false;
				ctx.ui.setWidget(WIDGET_KEY, undefined);
				return;
			}
			// A bare invocation toggles, because the panel is a thing you glance
			// at and dismiss. `refresh` always leaves it up: you asked to look.
			shown = sub === "refresh" ? true : !shown;
			if (!shown) {
				ctx.ui.setWidget(WIDGET_KEY, undefined);
				return;
			}
			if (!(await refresh(ctx, sub === "refresh" ? ["--refresh"] : []))) {
				shown = false;
				ctx.ui.setWidget(WIDGET_KEY, undefined);
				ctx.ui.notify("reactor: could not reach the CLI -- try `reactor doctor`", "error");
			}
		},
	});
}

/** `bn: up`, `adb: 2 devices`. The detail is the interesting part when there is one. */
function oneLine(s: ServiceRow): string {
	if (s.status !== "present") return `${s.id}: not installed`;
	return `${s.id}: ${s.state === "up" ? (s.detail ?? "up") : s.state}`;
}

// ---------------------------------------------------------------------------
// The shared vocabulary of the two views
// ---------------------------------------------------------------------------

/**
 * What one service's state looks like: a glyph and words coloured by state.
 * Green means running, red means "do something", dim means "nothing to act
 * on" -- an unanswered probe and an uninstalled tool alike, because neither
 * is a fault, they are just absences of an answer. The glyphs are the same
 * marks pi's own UI reaches for (dots, ballot marks), so the statusbar reads
 * as part of the TUI rather than as a guest with its own alphabet.
 */
function stateDeco(s: ServiceRow): StateDeco {
	if (s.status !== "present") return { glyph: "○", colour: "dim", text: "not installed" };
	if (s.state === "up") return { glyph: "●", colour: "success", text: s.detail ?? "up" };
	if (s.state === "down") return { glyph: "✗", colour: "error", text: "down" };
	return { glyph: "?", colour: "dim", text: "unknown" };
}

/**
 * Colours that name a service rather than judge it. `success`, `error` and
 * `warning` are deliberately absent from the rotation: on the glyph and the
 * state words they must keep meaning exactly one thing. A rotation of five,
 * handed out by sorted id, so `bn` and `adb` disagree in the footer and the
 * panel alike, and an id keeps its colour for as long as the set of services
 * does -- muted or newly installed services reshuffle it, which is the price
 * of never colliding.
 */
const IDENTITY_COLOURS = ["accent", "mdLink", "thinkingHigh", "thinkingXhigh", "syntaxType"] as const;

function identityColours(services: ServiceRow[]): Map<string, ThemeColor> {
	const map = new Map<string, ThemeColor>();
	[...services]
		.sort((a, b) => a.id.localeCompare(b.id))
		.forEach((s, i) => map.set(s.id, IDENTITY_COLOURS[i % IDENTITY_COLOURS.length]));
	return map;
}

// ---------------------------------------------------------------------------
// The footer line
// ---------------------------------------------------------------------------

/**
 * The budget behind the ladder: the real terminal width minus a fixed
 * allowance for the other extensions' entries that share the line, whose
 * length no API exposes. The width is pi-tui's own chain (`stdout.columns
 * || $COLUMNS || 80`, terminal.js), re-read on every refresh, so a resize
 * lands with the next turn like any other state change.
 */
function footerBudget(): number {
	const columns = process.stdout.columns || Number(process.env.COLUMNS) || 0;
	return Math.max(FOOTER_RESERVE, (columns || 80) - FOOTER_RESERVE);
}

/** One footer block: state glyph, service id, state words. */
function footerBlock(s: ServiceRow, colours: Map<string, ThemeColor>, t: Theme): string {
	const { glyph, colour, text } = stateDeco(s);
	return `${t.fg(colour, glyph)} ${t.fg(colours.get(s.id) ?? "text", s.id)} ${t.fg(colour, text)}`;
}

/**
 * The footer line. Down services first: they are the ones that mean "do
 * something". Blocks are separated by a dim middot, the separator pi's own
 * selectors use -- and while the anchor (the tool count) is on the line, the
 * whole line leads with one, so the anchor and the services read as blocks
 * of the same line, not two unrelated strings pi happened to join with a
 * space.
 *
 * Three rungs, each width-checked against `footerBudget()`, so a narrow
 * window sheds detail before pi's own truncation can ever cut a number into
 * a different number: full blocks, then names only, then counts.
 */
function statusLine(payload: ServicesPayload, t: Theme, lead: string): string | undefined {
	const known = payload.services.filter((s) => s.status === "present");
	if (!known.length) return undefined;

	const ordered = [...known].sort((a, b) => rank(a) - rank(b));
	const colours = identityColours(payload.services);
	const joiner = t.fg("dim", " · ");
	const budget = footerBudget();

	const blocks = lead + ordered.map((s) => footerBlock(s, colours, t)).join(joiner);
	if (visibleWidth(blocks) <= budget) return blocks;

	// Drop the details, keep every name: a count that vanished because the
	// window was narrow is a guessable loss; a truncated `12 devices` reads
	// as `1` and is a lie.
	const names = lead + ordered
		.map((s) => {
			const { glyph, colour } = stateDeco(s);
			return `${t.fg(colour, glyph)} ${t.fg(colours.get(s.id) ?? "text", s.id)}`;
		})
		.join(joiner);
	if (visibleWidth(names) <= budget) return names;

	// Counts, worst first -- the same order the blocks above use. Short by
	// construction, so this rung is the floor: below it, pi's ellipsis would
	// at worst shave the last word, and there is no number left to misread.
	const counts: Record<ServiceRow["state"], number> = { down: 0, unknown: 0, up: 0 };
	for (const s of known) counts[s.state]++;
	const countColour: Record<ServiceRow["state"], ThemeColor> = {
		down: "error",
		unknown: "dim",
		up: "success",
	};
	return (
		lead +
		(["down", "unknown", "up"] as const)
			.map((state) => [state, counts[state]] as const)
			.filter(([, n]) => n > 0)
			.map(([state, n]) => t.fg(countColour[state], `${n} ${state}`))
			.join(joiner)
	);
}
function rank(s: ServiceRow): number {
	return s.state === "down" ? 0 : s.state === "unknown" ? 1 : 2;
}

// ---------------------------------------------------------------------------
// The panel above the editor
// ---------------------------------------------------------------------------

/**
 * The panel above the editor. Stateless: it renders the payload it was
 * given. A row too wide for its window sheds the label column first, then
 * stacks label and state words onto continuation lines indented under the
 * service id -- it wraps rather than truncates, so nothing a probe said can
 * be cut into a different reading.
 */
class ServicePanel implements Component {
	constructor(
		private theme: Theme,
		private payload: ServicesPayload | undefined,
	) {}

	invalidate(): void {}

	render(width: number): string[] {
		const t = this.theme;
		if (!this.payload) {
			return [truncateToWidth(t.fg("error", "  ✗ reactor: no service data -- try `reactor doctor`"), width)];
		}
		// A tool that is not installed has no service to show -- reporting it
		// would be a status for a thing that does not exist, which is the same
		// reason the footer leaves it out.
		const services = this.payload.services.filter((s) => s.status === "present");
		if (!services.length) {
			const declared = this.payload.services.length > 0;
			return [
				truncateToWidth(
					t.fg(
						"dim",
						declared ? "  nothing that declares a service is installed" : "  no catalogued tool declares a service probe",
					),
					width,
				),
			];
		}

		// The colour map spans the whole payload, not the rows: the footer and
		// the panel must hand the same id the same colour, whether or not it is
		// installed and therefore shown.
		const colours = identityColours(this.payload.services);
		const idWidth = Math.max(...services.map((s) => visibleWidth(s.id)));
		// A label that repeats the id says nothing -- `adb  adb  2 devices`.
		// The column disappears entirely when no service has anything to add.
		const labelWidth = Math.max(0, ...services.map((s) => visibleWidth(labelOf(s))));
		return [
			truncateToWidth(t.bold(t.fg("accent", "  services")), width),
			...services.flatMap((s) => this.rowLines(s, colours, idWidth, labelWidth, width)),
		];
	}

	private rowLines(
		s: ServiceRow,
		colours: Map<string, ThemeColor>,
		idWidth: number,
		labelWidth: number,
		width: number,
	): string[] {
		const t = this.theme;
		const { glyph, colour, text } = stateDeco(s);
		const head = `  ${t.fg(colour, glyph)} ${t.fg(colours.get(s.id) ?? "text", s.id.padEnd(idWidth))}`;
		const stateText = t.fg(colour, text);

		// The table line: icon, id, label column, state words.
		if (labelWidth) {
			const line = `${head}  ${t.fg("muted", labelOf(s).padEnd(labelWidth))}  ${stateText}`;
			if (visibleWidth(line) <= width) return [line];
		}
		// Without the label column, the state words still fit beside the id.
		const compact = `${head}  ${stateText}`;
		if (visibleWidth(compact) <= width) return [compact];

		// Stacked: whatever is left wraps under the id. The label keeps its
		// own colour, the state words keep theirs, and wrapTextWithAnsi keeps
		// both across the break.
		const rest = labelOf(s) ? `${t.fg("muted", labelOf(s))}  ${stateText}` : stateText;
		const lines = [truncateToWidth(head, width)];
		for (const wrapped of wrapTextWithAnsi(rest, Math.max(8, width - 4))) {
			lines.push(truncateToWidth(`    ${wrapped}`, width));
		}
		return lines;
	}
}

function labelOf(s: ServiceRow): string {
	return s.label === s.id ? "" : s.label;
}