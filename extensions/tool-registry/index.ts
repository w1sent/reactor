/**
 * tool-registry -- tell the agent what RE tooling exists on this machine.
 *
 * The problem this solves: a model that does not know Binary Ninja is running
 * does not go looking for it. It writes a Python ELF parser instead. Usage is
 * documented by tool authors; *presence* is not knowable from inside a session.
 *
 * So on every user turn this appends a rendered registry block to the system
 * prompt -- one line per present, active tool -- and answers pi's
 * resources_discover with the skill directories of those same tools.
 *
 * It contains no catalogue logic. Every fact comes from `reactor ... --format
 * json`, which is REactor's single implementation of catalogue semantics
 * (ADR-0005); the block itself is rendered by the CLI so that the determinism
 * the design depends on is tested in one language (ADR-0006).
 */

import { getAgentDir } from "@earendil-works/pi-coding-agent";
import type {
	BeforeAgentStartEvent,
	BeforeAgentStartEventResult,
	ExtensionAPI,
	ExtensionContext,
	ResourcesDiscoverEvent,
	ResourcesDiscoverResult,
	SessionStartEvent,
	Theme,
} from "@earendil-works/pi-coding-agent";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { ANCHOR_KEY, errorBlock, glyph } from "../lib/statusbar.ts";
import { join } from "node:path";

/** Shape of `reactor registry --format json`. Part of REactor's contract. */
interface RegistryPayload {
	schema: number;
	block: string;
	skillPaths: string[];
	summary: { present: number; absent: number; unknown: number; catalogued: number };
	error?: string;
}

/**
 * Backstop only: the CLI already bounds every individual probe. This exists so
 * that a `reactor` wedged for a reason the CLI cannot see -- a stuck NFS mount
 * under PATH, a paused container -- costs one slow turn rather than a hung one.
 */
const EXEC_TIMEOUT_MS = 20_000;

/**
 * The key is the anchor of the whole statusbar line: it sorts before every
 * other REactor status, so the tool count is the first block, and the other
 * blocks' leading separators (the dim `·` they carry while the toolbox is
 * on) always land between blocks instead of dangling at the line's head.
 */
const STATUS_KEY = ANCHOR_KEY;

const REACTOR_JSON = () => join(getAgentDir(), "reactor.json");

/** Whatever is on disk already, so a write can patch one field without
 * clobbering the other (ADR-0016 covers both `toolbox` and `hiddenServices`
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
 * `false` in `<agent dir>/reactor.json` (normally `~/.pi/agent/reactor.json`)
 * hides this whole extension, as if it were never loaded (ADR-0016). Read
 * once at registration -- there is no `ctx` yet to react to the file
 * changing later, so a flip only takes effect immediately when it comes
 * through `/reactor-toolbox` below, which reloads; a hand-edit of the file
 * needs a manual `/reload`.
 */
function toolboxEnabled(): boolean {
	return readReactorJson().toolbox !== false;
}

export default function toolRegistry(pi: ExtensionAPI) {
	/**
	 * The on/off switch itself, registered unconditionally -- if this lived
	 * behind the `toolboxEnabled()` gate below, turning the toolbox off would
	 * remove the only command that can turn it back on. `/reactor` cannot
	 * carry this either, for the same reason: it is gated too.
	 */
	pi.registerCommand("reactor-toolbox", {
		description: "REactor: turn tool-registry and selector on or off",
		getArgumentCompletions: (prefix: string) =>
			["on", "off"].filter((c) => c.startsWith(prefix)).map((c) => ({ value: c, label: c })),
		handler: async (args, ctx) => {
			const sub = args.trim();
			if (!sub) {
				ctx.ui.notify(`reactor: toolbox is ${toolboxEnabled() ? "on" : "off"}`, "info");
				return;
			}
			if (sub !== "on" && sub !== "off") {
				ctx.ui.notify(`reactor-toolbox: unknown argument "${sub}" -- try on or off`, "error");
				return;
			}
			writeReactorJson({ toolbox: sub === "on" });
			ctx.ui.notify(`reactor: toolbox is now ${sub}`, "info");
			// Re-runs every extension's factory, so tool-registry and selector
			// pick up the new value immediately instead of waiting for the next
			// pi restart or a manual /reload. Last: pi invalidates this ctx (and
			// the closed-over `pi`) the moment this resolves, so nothing below
			// may touch either -- see the notify above, not below.
			await ctx.reload();
		},
	});

	if (!toolboxEnabled()) return;

	/**
	 * Last good payload. The registry is a statement about the machine, and a
	 * momentarily unavailable CLI is not evidence the tools vanished -- so a
	 * failed call keeps the previous block rather than silently emptying it.
	 */
	let last: RegistryPayload | undefined;
	let missingCliWarned = false;

	/**
	 * The anchor block: the key sorts before every other REactor status, so
	 * the tool count is the first block on the line, and the other blocks'
	 * leading separators (ADR-0029's vocabulary) land between blocks instead
	 * of dangling at the line's head. The shape -- glyph in the anchor
	 * colour, words in the text colour, the unit dim -- is the shared
	 * statusbar grammar; the count is this extension's fact.
	 */
	function countBlock(t: Theme, present: number, catalogued: number): string {
		return `${glyph(t, "🛠")} ${present}/${catalogued} ${t.fg("dim", "tools")}`;
	}

	async function fetchRegistry(
		ctx: ExtensionContext,
		args: string[] = [],
	): Promise<RegistryPayload | undefined> {
		const t = ctx.ui.theme;
		// pi's exec resolves rather than throwing, including on ENOENT: a
		// missing `reactor` arrives as code 1 with empty stdout, exactly like a
		// crashed one. Both mean the same thing here.
		const result = await pi.exec("reactor", ["registry", "--format", "json", ...args], {
			timeout: EXEC_TIMEOUT_MS,
			cwd: ctx.cwd,
		});

		if (result.killed) {
			ctx.ui.setStatus(STATUS_KEY, errorBlock(t, "reactor: probe timed out"));
			return undefined;
		}
		if (!result.stdout.trim()) {
			// Say this once, then stay quiet. REactor not being installed is a
			// valid state for a pi session, not an error to repeat every turn.
			if (!missingCliWarned) {
				missingCliWarned = true;
				ctx.ui.setStatus(STATUS_KEY, errorBlock(t, "reactor: CLI unavailable"));
			}
			return undefined;
		}

		let payload: RegistryPayload;
		try {
			payload = JSON.parse(result.stdout) as RegistryPayload;
		} catch {
			ctx.ui.setStatus(STATUS_KEY, errorBlock(t, "reactor: unreadable output"));
			return undefined;
		}
		if (payload.error) {
			ctx.ui.setStatus(STATUS_KEY, errorBlock(t, `reactor: ${payload.error}`));
			return undefined;
		}

		last = payload;
		const { present, catalogued } = payload.summary;
		ctx.ui.setStatus(STATUS_KEY, countBlock(t, present, catalogued));
		return payload;
	}

	// Warm the cache once per session so the first turn is not the one that
	// pays for detecting every tool.
	pi.on("session_start", async (_event: SessionStartEvent, ctx: ExtensionContext) => {
		await fetchRegistry(ctx, ["--refresh"]);
	});

	/**
	 * The injection point. Fires once per user turn -- not once per LLM call --
	 * which is what makes replacing the system prompt affordable here and not
	 * in the `context` event (ADR-0006).
	 */
	pi.on(
		"before_agent_start",
		async (
			event: BeforeAgentStartEvent,
			ctx: ExtensionContext,
		): Promise<BeforeAgentStartEventResult | void> => {
			const payload = (await fetchRegistry(ctx)) ?? last;
			if (!payload?.block) return;
			return { systemPrompt: `${event.systemPrompt}\n\n${payload.block}` };
		},
	);

	/**
	 * Skills for active, present tools only. Deactivating a toolset therefore
	 * drops both its registry lines and its skills' descriptions; ctx.reload()
	 * from the selector re-runs this with reason "reload".
	 */
	pi.on(
		"resources_discover",
		async (
			_event: ResourcesDiscoverEvent,
			ctx: ExtensionContext,
		): Promise<ResourcesDiscoverResult | void> => {
			const payload = (await fetchRegistry(ctx, ["--cached"])) ?? last;
			if (!payload?.skillPaths.length) return;
			return { skillPaths: payload.skillPaths };
		},
	);

	pi.registerCommand("reactor", {
		description: "REactor: refresh the tool registry, or show it",
		getArgumentCompletions: (prefix: string) =>
			["refresh", "show"]
				.filter((c) => c.startsWith(prefix))
				.map((c) => ({ value: c, label: c })),
		handler: async (args, ctx) => {
			const sub = args.trim() || "show";
			if (sub !== "refresh" && sub !== "show") {
				ctx.ui.notify(`reactor: unknown subcommand "${sub}" -- try refresh or show`, "error");
				return;
			}
			const payload = await fetchRegistry(ctx, sub === "refresh" ? ["--refresh"] : []);
			if (!payload) {
				ctx.ui.notify("reactor: could not reach the CLI -- try `reactor doctor`", "error");
				return;
			}
			pi.sendMessage({
				customType: "reactor-registry",
				content: payload.block,
				display: true,
			});
			if (sub === "refresh") {
				// Skills are gated on the same probe, so a refresh that changed
				// what is present must re-run resources_discover too. Last: pi
				// invalidates this ctx (and the closed-over `pi`) the moment this
				// resolves, so nothing may follow it.
				await ctx.reload();
			}
		},
	});
}
