/**
 * goal-setting -- the session manifest, injected into the system prompt: the
 * user's goal, session guidelines, and steps the agent maintains itself as
 * short conceptual summaries with a 3-word status.
 *
 * Split out of `rolling-context/` (ADR-0024) so that durable session memory
 * works independently of the fade: the manifest survives pi's own compaction
 * just as well as rolling-context's fade, and the two are switched
 * independently -- this extension has its own `/manifest [on|off]` toggle,
 * rolling-context has `/rolling`. Nothing here calls `reactor`; nothing here
 * is RE-specific.
 *
 * The manifest goes into the **system prompt** via `before_agent_start`, not
 * into the message array via a `context` handler. Two reasons (ADR-0024):
 * the block changes only when the goal/guidelines/steps change, so the
 * prompt cache holds across turns where nothing was updated; and it makes
 * the split order-independent -- pi composes `before_agent_start` by chaining
 * `event.systemPrompt` (runner.js), so appending a section needs no
 * assumption about which extension loads first. pi's own
 * `ctx.getSystemPrompt()` reports the *chained* prompt, so the fade's budget
 * math accounts for this block exactly, with no coupling between the two
 * extensions (ADR-0014: no shared state; presentation code is shared
 * through `extensions/lib/`, ADR-0029).
 *
 * The `update_steps` tool is registered once, always, but answers with
 * instructions instead of acting until it is useful: it is active while a
 * goal is set **and** the switch is on -- so in a session with no goal it is
 * simply inactive (the default), and `/manifest off` forces it off even when
 * a goal exists. Like everything else here it is registered as a plain tool,
 * not hidden via `pi.setActiveTools()` -- that list is shared across all
 * extensions, and ADR-0017 records why touching it for one tool's visibility
 * is a bad trade.
 *
 * Per-session state (switch, goal, guidelines, steps) lives in the session
 * itself as a `custom` entry, restored on `session_start` by taking the
 * latest one on the branch -- the same pattern `scenario/` uses for its own
 * state (ADR-0009). Config: `~/.pi/agent/pi-goal-setting.json` (global), its
 * own file per the one-file-per-extension convention; unrelated to
 * `reactor.json` (ADR-0016) and to rolling-context's files. No migration from
 * the old single `pi-rolling-context.json` or its custom entries -- REactor
 * is in alpha, so the split is a clean break (ADR-0024).
 */

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { Type } from "typebox";
import type {
	ExtensionAPI,
	ExtensionContext,
	SessionManager,
	Theme,
} from "@earendil-works/pi-coding-agent";
import { convertToLlm, getAgentDir, serializeConversation } from "@earendil-works/pi-coding-agent";
import type { AutocompleteItem, Component, TUI } from "@earendil-works/pi-tui";
import { truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

// ============================================================================
// Types
// ============================================================================

const CUSTOM_TYPE = "pi-goal-setting";
const STATUS_KEY = "goal-setting";
const WIDGET_KEY = "goal-setting";

interface Step {
	summary: string;
	status: string;
}

interface SessionState {
	/** The `/manifest` switch. Absent reads as on -- the gate below is what keeps `update_steps` out of the way until a goal exists. */
	enabled?: boolean;
	goal?: string;
	guidelines?: string;
	steps: Step[];
}

interface GoalSettingConfig {
	softStepLimit: number;
	maxDescription: number;
	statusWords: number;
	/** Char budget for the session tail handed to the derive call. */
	deriveContextChars: number;
}

const DEFAULT_CONFIG: GoalSettingConfig = {
	softStepLimit: 20,
	maxDescription: 80,
	statusWords: 3,
	deriveContextChars: 24_000,
};

// getAgentDir(), not homedir() + ".pi/agent" by hand: it is also what respects
// PI_CODING_AGENT_DIR when set (ADR-0019).
const GLOBAL_CONFIG_PATH = join(getAgentDir(), "pi-goal-setting.json");

// ============================================================================
// Module-level session state
// ============================================================================

let config: GoalSettingConfig = { ...DEFAULT_CONFIG };
let state: SessionState = { steps: [] };

function isEnabled(): boolean {
	return state.enabled ?? true;
}

function hasGoal(): boolean {
	return Boolean(state.goal?.trim());
}

/** `update_steps` answers instead of acting unless both halves of its gate hold. */
function toolGateMessage(): string | undefined {
	if (!isEnabled()) return "goal-setting is off. Run /manifest on to enable it.";
	if (!hasGoal()) return "update_steps is inactive until a session goal is set. Set one with /goal <text>.";
	return undefined;
}

// ============================================================================
// The goal row: the manifest's own line, directly above the footer
// ============================================================================

/** How much goal text the row carries before it yields to an ellipsis. */
const MAX_GOAL_CHARS = 96;

function hasContent(): boolean {
	return Boolean(hasGoal() || state.guidelines?.trim() || state.steps.length > 0);
}

/** ` · 3 steps`, coloured: muted is metadata, past the soft limit is a warning. */
function stepsSuffix(t: Theme): string {
	const n = state.steps.length;
	if (!n) return "";
	const colour = n > config.softStepLimit ? "warning" : "muted";
	return ` ${t.fg("dim", "·")} ${t.fg(colour, `${n} step${n === 1 ? "" : "s"}`)}`;
}

/** The row: `◎ <goal>` with its steps count, or `◎ manifest` without a goal. */
function goalRow(t: Theme): string {
	const head = hasGoal() ? truncate(state.goal!.trim(), MAX_GOAL_CHARS) : "manifest";
	return `${t.fg("accent", "◎")} ${head}${stepsSuffix(t)}`;
}

/**
 * The row as a component, so a narrow window shortens the goal instead of
 * cutting into the steps count: prose can ellipsize, a number cannot.
 */
class GoalRow implements Component {
	constructor(private theme: Theme) {}

	invalidate(): void {}

	render(width: number): string[] {
		const t = this.theme;
		const suffix = stepsSuffix(t);
		const head = hasGoal() ? truncate(state.goal!.trim(), MAX_GOAL_CHARS) : "manifest";
		const full = `${t.fg("accent", "◎")} ${head}${suffix}`;
		if (visibleWidth(full) <= width) return [full];
		const available = Math.max(8, width - visibleWidth(suffix) - 3);
		return [truncateToWidth(`${t.fg("accent", "◎")} ${truncateToWidth(head, available)}${suffix}`, width)];
	}
}

/**
 * The manifest's own row, above the footer: the footer's status line is a
 * guest shelf shared with every other extension, and a goal is prose, not a
 * one-glance fact. Rendered fresh from module state on every paint, so a
 * `update_steps` mid-turn changes the count without a setWidget round trip;
 * the set itself only makes the row appear or vanish. Replaces the footer
 * entry this extension used to keep.
 */
function refreshStatus(ctx: ExtensionContext): void {
	if (ctx.mode !== "tui" && ctx.mode !== "rpc") return; // print and json carry no UI
	try {
		ctx.ui.setStatus(STATUS_KEY, undefined); // the row replaces the footer entry
		if (ctx.mode === "rpc") {
			// rpc takes string lines, not component factories; every state
			// mutation calls refreshStatus, so the snapshot stays current.
			ctx.ui.setWidget(
				WIDGET_KEY,
				isEnabled() && hasContent() ? [goalRow(ctx.ui.theme)] : undefined,
				{ placement: "belowEditor" },
			);
			return;
		}
		ctx.ui.setWidget(
			WIDGET_KEY,
			isEnabled() && hasContent() ? (_tui: TUI, theme: Theme) => new GoalRow(theme) : undefined,
			{ placement: "belowEditor" },
		);
	} catch {
		// no terminal -- print and json modes carry no UI
	}
}

// ============================================================================
// Config
// ============================================================================

function loadGlobalConfig(): void {
	const merged: GoalSettingConfig = { ...DEFAULT_CONFIG };
	try {
		if (existsSync(GLOBAL_CONFIG_PATH)) {
			const raw = JSON.parse(readFileSync(GLOBAL_CONFIG_PATH, "utf8"));
			for (const key of Object.keys(DEFAULT_CONFIG) as (keyof GoalSettingConfig)[]) {
				if (raw[key] !== undefined) (merged as any)[key] = raw[key];
			}
		}
	} catch {
		// ignore malformed config, fall back to defaults
	}
	config = merged;
}

// ============================================================================
// Session state persistence (in-session `custom` entry)
// ============================================================================

function loadSessionState(sm: SessionManager): void {
	state = { steps: [] };
	for (const entry of sm.getBranch()) {
		if (entry.type === "custom" && entry.customType === CUSTOM_TYPE) {
			// latest on the branch wins
			state = normalizeState(entry.data);
		}
	}
}

function normalizeState(raw: unknown): SessionState {
	const base: SessionState = { steps: [] };
	if (!raw || typeof raw !== "object") return base;
	const r = raw as Record<string, any>;
	if (typeof r.enabled === "boolean") base.enabled = r.enabled;
	if (typeof r.goal === "string") base.goal = r.goal;
	if (typeof r.guidelines === "string") base.guidelines = r.guidelines;
	if (Array.isArray(r.steps)) {
		base.steps = r.steps
			.filter((s: any) => s && typeof s.summary === "string")
			.map((s: any) => ({ summary: s.summary, status: typeof s.status === "string" ? s.status : "" }));
	}
	return base;
}

// ============================================================================
// System prompt (the manifest block, on content)
// ============================================================================

/**
 * The manifest block: goal and steps (the manifest proper), plus the session
 * guidelines, each on its own content -- injected only on content, so a
 * session with nothing set gets nothing, and an untouched session's system
 * prompt stays byte-identical, which is what the prompt cache needs.
 */
function manifestBlock(): string | undefined {
	// The switch pauses the whole extension: block and tool both go quiet,
	// while the goal/guidelines/steps state is preserved for /manifest on.
	if (!isEnabled()) return undefined;
	const parts: string[] = [];
	if (hasGoal()) {
		parts.push("## Session Manifest");
		parts.push("");
		parts.push(`Goal: ${state.goal!.trim()}`);
		parts.push("");
		parts.push(`Steps (${state.steps.length}/${config.softStepLimit}):`);
		if (state.steps.length === 0) {
			parts.push("(none yet)");
		} else {
			parts.push(state.steps.map((s, i) => `${i + 1}. [${s.status}] ${s.summary}`).join("\n"));
		}
		parts.push("");
		parts.push(
			"Steps are the session's durable memory: record progress as conceptual summaries (an investigative question or milestone, not a micro-action), each with a 3-word status; overwrite the full list with update_steps; consolidate past the soft limit. If a past decision or finding matters, it belongs in the steps -- not only in messages that may later be compacted or faded away.",
		);
	}
	if (state.guidelines?.trim()) {
		parts.push("## Session Guidelines");
		parts.push("");
		parts.push(state.guidelines.trim());
	}
	if (parts.length === 0) return undefined;
	return parts.join("\n\n");
}

// ============================================================================
// Derive: goal / guidelines / steps from the session, via a direct provider
// call -- no chat message, no agent loop, no tools. The result is applied
// straight to the manifest state, exactly like /goal writes it.
// ============================================================================

const DERIVE_SYSTEM =
	"You derive a session manifest from a transcript. Respond with ONLY the requested JSON object -- no markdown fences, no commentary.";

function deriveTask(scope: "all" | "goal" | "guidelines" | "steps"): string {
	const shape =
		scope === "goal"
			? '{"goal": "<one sentence: what this session is trying to achieve>"}'
		: scope === "guidelines"
			? '{"guidelines": "<standing constraints the work implies, or \"\" if none>"}'
		: scope === "steps"
			? '{"steps": [{"summary": "<conceptual step, not a micro-action>", "status": "<3 words>"}]}'
			: '{"goal": "<one sentence>", "guidelines": "<standing constraints, or \"\" if none>", "steps": [{"summary": "<conceptual step>", "status": "<3 words>"}]}';
	const what =
		scope === "goal"
			? "the session goal"
		: scope === "guidelines"
			? "standing guidelines"
		: scope === "steps"
			? "the steps (covering the REMAINING work)"
			: "the goal, guidelines and steps";
	return (
		`Based on the transcript below, derive ${what} for this session. ` +
		`Respond with ONLY a JSON object of exactly this shape: ${shape}. ` +
		`Steps cover what remains, not history; statuses are 3 words each.\n\n` +
		`--- session transcript (tail) ---\n`
	);
}

/** Own copy: it walks session entries, which are facts, not presentation (ADR-0029 scopes what may be shared). */
function serializeBranchTail(sm: SessionManager, budget: number): string {
	const blocks: string[] = [];
	for (const entry of sm.getBranch()) {
		let text = "";
		try {
			if (entry.type === "message") {
				text = serializeConversation(convertToLlm([entry.message]));
			} else if (entry.type === "custom_message") {
				const content = (entry as any).content;
				text = `[Custom]: ${typeof content === "string" ? content : (content ?? []).filter((c: any) => c.type === "text").map((c: any) => c.text).join(" ")}`;
			} else if (entry.type === "compaction" || entry.type === "branch_summary") {
				text = `[History summary]: ${(entry as any).summary ?? ""}`;
			}
		} catch {
			text = "";
		}
		if (text.trim()) blocks.push(text.trim());
	}
	// newest last: accumulate from the end while under budget
	let out = "";
	for (let i = blocks.length - 1; i >= 0; i--) {
		const candidate = out ? `${blocks[i]}\n\n${out}` : blocks[i];
		if (candidate.length > budget && out) break;
		out = candidate;
	}
	return out;
}

/** Pull the first JSON object out of a model response, fences and prose notwithstanding. */
function extractJson(text: string): any | undefined {
	const stripped = text.replace(/```(?:json)?/g, "").trim();
	const start = stripped.indexOf("{");
	const end = stripped.lastIndexOf("}");
	if (start === -1 || end === -1 || end <= start) return undefined;
	try {
		return JSON.parse(stripped.slice(start, end + 1));
	} catch {
		return undefined;
	}
}

interface DerivedParts {
	goal?: string;
	guidelines?: string;
	steps?: Step[];
}

function parseDerivation(text: string, scope: string): DerivedParts | undefined {
	const parsed = extractJson(text);
	if (!parsed || typeof parsed !== "object") return undefined;
	const parts: DerivedParts = {};
	if ((scope === "all" || scope === "goal") && typeof parsed.goal === "string" && parsed.goal.trim()) {
		parts.goal = parsed.goal.trim();
	}
	if ((scope === "all" || scope === "guidelines") && typeof parsed.guidelines === "string") {
		parts.guidelines = parsed.guidelines.trim();
	}
	if ((scope === "all" || scope === "steps") && Array.isArray(parsed.steps)) {
		const steps = parsed.steps
			.filter((st: any) => st && typeof st.summary === "string" && st.summary.trim())
			.map((st: any) => ({
				summary: truncate(st.summary, config.maxDescription),
				status: truncateWords(typeof st.status === "string" ? st.status : "", config.statusWords),
			}));
		parts.steps = steps;
	}
	if (parts.goal === undefined && parts.guidelines === undefined && parts.steps === undefined) return undefined;
	return parts;
}

async function callProvider(ctx: any, task: string): Promise<string> {
	const model = ctx.model;
	if (!model) throw new Error("no model selected -- /model first");
	// pi's own facade, exposed to extensions: it resolves auth for the
	// configured model itself -- including custom providers from models.json,
	// whose keys readStoredCredential (auth.json) never sees.
	const registry = ctx.modelRegistry;
	if (!registry?.complete) throw new Error("no model registry in this context");
	const response = await registry.complete(model, {
		systemPrompt: DERIVE_SYSTEM,
		messages: [{ role: "user", content: task + serializeBranchTail(ctx.sessionManager, config.deriveContextChars), timestamp: Date.now() }],
	});
	if (response.stopReason === "error") {
		throw new Error(response.errorMessage ?? "provider error");
	}
	const text = (response.content ?? [])
		.filter((b: any) => b.type === "text")
		.map((b: any) => b.text)
		.join("")
		.trim();
	if (!text) throw new Error("the model returned no text");
	return text;
}

function applyDerived(pi: ExtensionAPI, parts: DerivedParts): string[] {
	const applied: string[] = [];
	if (parts.goal !== undefined) {
		state = { ...state, goal: parts.goal };
		applied.push(`goal: ${parts.goal}`);
	}
	if (parts.guidelines !== undefined) {
		state = { ...state, guidelines: parts.guidelines };
		applied.push("guidelines");
	}
	if (parts.steps !== undefined) {
		state = { ...state, steps: parts.steps };
		applied.push(`steps: ${parts.steps.length}`);
	}
	pi.appendEntry(CUSTOM_TYPE, state);
	return applied;
}

let deriving = false;

async function runDerive(pi: ExtensionAPI, ctx: any, scope: "all" | "goal" | "guidelines" | "steps"): Promise<void> {
	if (deriving) {
		ctx.ui.notify("derive: already running -- wait for it to finish", "warning");
		return;
	}
	deriving = true;
	try {
		const text = await callProvider(ctx, deriveTask(scope));
		const parts = parseDerivation(text, scope);
		if (!parts) {
			ctx.ui.notify("derive: the response was not the requested JSON -- nothing applied", "warning");
			return;
		}
		const applied = applyDerived(pi, parts);
		refreshStatus(ctx);
		ctx.ui.notify(`derive: ${applied.join(", ")} -- /frame to review`, "info");
	} catch (err) {
		const message = err instanceof Error ? err.message : String(err);
		ctx.ui.notify(`derive failed: ${message}`, "error");
	} finally {
		deriving = false;
	}
}

const DERIVE_SCOPES = ["all", "goal", "guidelines", "steps"];

function deriveCompletions(argumentText: string): AutocompleteItem[] | null {
	const prefix = argumentText.trim().toLowerCase();
	const items: AutocompleteItem[] = DERIVE_SCOPES.map((scope) => ({
		value: scope,
		label: scope,
		description: scope === "all" ? "derive goal, guidelines and steps" : `derive the ${scope} only`,
	}));
	const candidates = prefix ? items.filter((item) => item.value.startsWith(prefix)) : items;
	return candidates.length > 0 ? candidates : null;
}

// ============================================================================
// Extension
// ============================================================================

export default function (pi: ExtensionAPI) {
	loadGlobalConfig();

	// ---- session lifecycle ------------------------------------------------
	pi.on("session_start", (_event, ctx) => {
		loadSessionState(ctx.sessionManager);
		refreshStatus(ctx);
	});

	pi.on("session_shutdown", (_event, ctx) => {
		state = { steps: [] };
		refreshStatus(ctx);
	});

	// ---- the manifest block into the system prompt ------------------------
	pi.on("before_agent_start", (event) => {
		const block = manifestBlock();
		if (!block) return undefined;
		return { systemPrompt: `${event.systemPrompt}\n\n${block}` };
	});

	// ---- commands -----------------------------------------------------------
	pi.registerCommand("goal", {
		description: "Set the session goal (survives /resume). Activates update_steps.",
		handler: (args, ctx) => {
			const text = (args || "").trim();
			if (text.toLowerCase() === "clear") {
				state = { ...state, goal: undefined };
				pi.appendEntry(CUSTOM_TYPE, state);
				refreshStatus(ctx);
				ctx.ui.notify("goal cleared", "info");
				return;
			}
			if (!text) {
				ctx.ui.notify("usage: /goal <text> | /goal clear", "warning");
				return;
			}
			state = { ...state, goal: text };
			pi.appendEntry(CUSTOM_TYPE, state);
			refreshStatus(ctx);
			ctx.ui.notify(`goal set: ${text}`, "info");
		},
	});

	pi.registerCommand("guidelines", {
		description: "Set session-specific guidelines (goes into the system prompt).",
		handler: (args, ctx) => {
			const text = (args || "").trim();
			if (text.toLowerCase() === "clear") {
				state = { ...state, guidelines: undefined };
				pi.appendEntry(CUSTOM_TYPE, state);
				refreshStatus(ctx);
				ctx.ui.notify("guidelines cleared", "info");
				return;
			}
			if (!text) {
				ctx.ui.notify("usage: /guidelines <text> | /guidelines clear", "warning");
				return;
			}
			state = { ...state, guidelines: text };
			pi.appendEntry(CUSTOM_TYPE, state);
			refreshStatus(ctx);
			ctx.ui.notify(`guidelines set: ${text}`, "info");
		},
	});

	const manifestCompletions = (argumentText: string): AutocompleteItem[] => {
		const prefix = argumentText.trim().toLowerCase();
		const items: AutocompleteItem[] = [
			{ value: "on", label: "on", description: "activate the update_steps switch" },
			{ value: "off", label: "off", description: "deactivate the update_steps switch" },
			{ value: "clear", label: "clear", description: "clear goal, guidelines and steps" },
		];
		const candidates = prefix ? items.filter((item) => item.value.startsWith(prefix)) : items;
		return candidates.length > 0 ? candidates : null;
	};

	pi.registerCommand("manifest", {
		description:
			"Toggle goal-setting's update_steps switch (/manifest on|off), clear the whole manifest (/manifest clear), or show status with no arg.",
		getArgumentCompletions: manifestCompletions,
		handler: (args, ctx) => {
			const arg = (args || "").trim().toLowerCase();
			if (arg === "clear") {
				state = { ...state, goal: undefined, guidelines: undefined, steps: [] };
				pi.appendEntry(CUSTOM_TYPE, state);
				refreshStatus(ctx);
				ctx.ui.notify("manifest cleared -- goal, guidelines and steps are gone", "info");
				return;
			}
			let next: boolean;
			if (arg === "on") next = true;
			else if (arg === "off") next = false;
			else if (arg === "") next = !isEnabled();
			else {
				ctx.ui.notify(`manifest: unknown argument "${arg}" -- try on, off or clear`, "warning");
				return;
			}
			state = { ...state, enabled: next };
			pi.appendEntry(CUSTOM_TYPE, state);
			refreshStatus(ctx);
			ctx.ui.notify(`goal-setting ${next ? "enabled" : "disabled"}`, next ? "info" : "warning");
		},
	});

	pi.registerCommand("frame", {
		description: "View the current manifest (goal, guidelines, steps, switch state).",
		handler: (_args, ctx) => {
			ctx.ui.notify(buildFrameView(), "info");
		},
	});

	pi.registerCommand("derive", {
		description:
			"Derive the manifest from this session with a direct model call -- no chat, no tools. /derive [all|goal|guidelines|steps]; writes the result straight into the manifest.",
		getArgumentCompletions: deriveCompletions,
		handler: async (args, ctx) => {
			const sub = (args || "").trim().toLowerCase() || "all";
			if (!DERIVE_SCOPES.includes(sub)) {
				ctx.ui.notify(`derive: unknown scope "${sub}" -- try all, goal, guidelines or steps`, "warning");
				return;
			}
			await runDerive(pi, ctx, sub as "all" | "goal" | "guidelines" | "steps");
		},
	});

	function buildFrameView(): string {
		const out = [`goal: ${state.goal ?? "(none)"}`, `guidelines: ${state.guidelines ?? "(none)"}`, `steps (${state.steps.length}/${config.softStepLimit}):`];
		if (state.steps.length === 0) out.push("  (none)");
		else out.push(state.steps.map((s, i) => `  ${i + 1}. [${s.status}] ${s.summary}`).join("\n"));
		out.push(`switch: ${isEnabled() ? "on" : "off"}`);
		return out.join("\n");
	}

	// ---- tools ---------------------------------------------------------------
	pi.registerTool({
		name: "update_steps",
		label: "Update Steps",
		description:
			"Overwrite the entire step list of the session manifest. Each step has a short conceptual summary (an investigative question or milestone, not a micro-action) and a 3-word status. Inactive until a session goal is set; returns instructions when it cannot act.",
		parameters: Type.Object({
			steps: Type.Array(
				Type.Object({
					summary: Type.String({ description: "Short conceptual summary (max ~80 chars)" }),
					status: Type.String({ description: "3-word status, e.g. 'in progress' / 'done' / 'blocked on'." }),
				}),
				{ description: "Full replacement step list." },
			),
		}),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const gate = toolGateMessage();
			if (gate) {
				return { content: [{ type: "text", text: gate }], details: {} };
			}
			const clamped: Step[] = params.steps.map((s: Step) => ({
				summary: truncate(s.summary, config.maxDescription),
				status: truncateWords(s.status, config.statusWords),
			}));
			state = { ...state, steps: clamped };
			pi.appendEntry(CUSTOM_TYPE, state);
			refreshStatus(ctx); // the row's steps count is live, but the row must appear
			const over = clamped.length > config.softStepLimit;
			let text = `Steps updated: ${clamped.length} step(s).`;
			if (over) {
				text += `\n\nWARNING: step count (${clamped.length}) exceeds the soft limit (${config.softStepLimit}). Consolidate or mark finished steps complete and rewrite the list via update_steps.`;
			}
			return { content: [{ type: "text", text }], details: {} };
		},
	});
}

// ============================================================================
// helpers
// ============================================================================

function truncate(s: string, n: number): string {
	return s.length <= n ? s : s.slice(0, Math.max(0, n - 1)) + "…";
}
function truncateWords(s: string, n: number): string {
	return s.trim().split(/\s+/).slice(0, n).join(" ");
}