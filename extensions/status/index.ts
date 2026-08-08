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

import type {
	BeforeAgentStartEvent,
	ExtensionAPI,
	ExtensionContext,
	SessionStartEvent,
	Theme,
} from "@earendil-works/pi-coding-agent";
import type { Component, TUI } from "@earendil-works/pi-tui";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

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
 * How much footer the status line may take before it collapses to counts. The
 * footer is shared with the branch, the model and the registry's own entry, so
 * this is a guest in someone else's space.
 */
const MAX_STATUS_WIDTH = 44;

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

		last = payload;
		return payload;
	}

	/** Probe, then repaint whatever is currently on screen. */
	async function refresh(ctx: ExtensionContext, args: string[] = []): Promise<boolean> {
		const payload = (await fetchServices(ctx, args)) ?? last;
		if (ctx.mode !== "tui") return payload !== undefined;
		ctx.ui.setStatus(STATUS_KEY, payload ? statusLine(payload) : undefined);
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
			["refresh", "hide"]
				.filter((c) => c.startsWith(prefix))
				.map((c) => ({ value: c, label: c })),
		handler: async (args, ctx) => {
			const sub = args.trim();
			if (sub && sub !== "refresh" && sub !== "hide") {
				ctx.ui.notify(`reactor-status: unknown subcommand "${sub}" -- try refresh or hide`, "error");
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

/**
 * The footer line. Down services first: they are the ones that mean "do
 * something". Falls back to counts rather than being cut mid-word, because a
 * truncated `adb: 12 devices` reads as a different number.
 */
function statusLine(payload: ServicesPayload): string | undefined {
	const known = payload.services.filter((s) => s.status === "present");
	if (!known.length) return undefined;

	const ordered = [...known].sort((a, b) => rank(a) - rank(b));
	const full = ordered.map((s) => `${s.id}:${s.state === "up" ? (s.detail ?? "up") : s.state}`);
	const line = full.join(" · ");
	if (visibleWidth(line) <= MAX_STATUS_WIDTH) return line;

	const counts = { up: 0, down: 0, unknown: 0 };
	for (const s of known) counts[s.state]++;
	return Object.entries(counts)
		.filter(([, n]) => n > 0)
		.map(([k, n]) => `${n} ${k}`)
		.join(" · ");
}

function rank(s: ServiceRow): number {
	return s.state === "down" ? 0 : s.state === "unknown" ? 1 : 2;
}

/** The panel above the editor. Stateless: it renders the payload it was given. */
class ServicePanel implements Component {
	constructor(
		private theme: Theme,
		private payload: ServicesPayload | undefined,
	) {}

	invalidate(): void {}

	render(width: number): string[] {
		const t = this.theme;
		const services = this.payload?.services ?? [];
		if (!this.payload) {
			return [truncateToWidth(t.fg("error", "  reactor: no service data -- try `reactor doctor`"), width)];
		}
		if (!services.length) {
			return [truncateToWidth(t.fg("dim", "  no catalogued tool declares a service probe"), width)];
		}

		const idWidth = Math.max(...services.map((s) => visibleWidth(s.id)));
		// A label that repeats the id says nothing -- `adb  adb  2 devices`.
		// The column disappears entirely when no service has anything to add.
		const labelWidth = Math.max(...services.map((s) => visibleWidth(labelOf(s))));
		return [
			truncateToWidth(t.bold(t.fg("accent", "  running")), width),
			...services.map((s) => truncateToWidth(this.row(s, idWidth, labelWidth), width)),
		];
	}

	private row(s: ServiceRow, idWidth: number, labelWidth: number): string {
		const t = this.theme;
		// A tool that is not installed has no service state, and rendering it
		// as "down" would send someone looking for a thing to restart.
		const [glyph, colour, text] =
			s.status !== "present"
				? (["[ ]", "dim", "not installed"] as const)
				: s.state === "up"
					? (["[^]", "success", (s.detail ?? "up")] as const)
					: s.state === "down"
						? (["[v]", "error", "down"] as const)
						: (["[?]", "dim", "unknown"] as const);
		const label = labelWidth ? `${t.fg("muted", labelOf(s).padEnd(labelWidth))}  ` : "";
		return `  ${t.fg(colour, glyph)} ${s.id.padEnd(idWidth)}  ${label}${t.fg(colour, text)}`;
	}
}

function labelOf(s: ServiceRow): string {
	return s.label === s.id ? "" : s.label;
}
