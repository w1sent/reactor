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
 * extensions (ADR-0014: no shared modules, and none needed).
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
import type { ExtensionAPI, SessionManager } from "@earendil-works/pi-coding-agent";
import { getAgentDir } from "@earendil-works/pi-coding-agent";

// ============================================================================
// Types
// ============================================================================

const CUSTOM_TYPE = "pi-goal-setting";

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
}

const DEFAULT_CONFIG: GoalSettingConfig = {
	softStepLimit: 20,
	maxDescription: 80,
	statusWords: 3,
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
// Extension
// ============================================================================

export default function (pi: ExtensionAPI) {
	loadGlobalConfig();

	// ---- session lifecycle ------------------------------------------------
	pi.on("session_start", (_event, ctx) => {
		loadSessionState(ctx.sessionManager);
	});

	pi.on("session_shutdown", () => {
		state = { steps: [] };
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
			if (!text) {
				ctx.ui.notify("usage: /goal <text>", "warning");
				return;
			}
			state = { ...state, goal: text };
			pi.appendEntry(CUSTOM_TYPE, state);
			ctx.ui.notify(`goal set: ${text}`, "info");
		},
	});

	pi.registerCommand("guidelines", {
		description: "Set session-specific guidelines (goes into the system prompt).",
		handler: (args, ctx) => {
			const text = (args || "").trim();
			if (!text) {
				ctx.ui.notify("usage: /guidelines <text>", "warning");
				return;
			}
			state = { ...state, guidelines: text };
			pi.appendEntry(CUSTOM_TYPE, state);
			ctx.ui.notify(`guidelines set: ${text}`, "info");
		},
	});

	pi.registerCommand("manifest", {
		description: "Toggle goal-setting's update_steps switch (or /manifest on|off). Shows status with no arg.",
		handler: (args, ctx) => {
			const arg = (args || "").trim().toLowerCase();
			let next: boolean;
			if (arg === "on") next = true;
			else if (arg === "off") next = false;
			else next = !isEnabled();
			state = { ...state, enabled: next };
			pi.appendEntry(CUSTOM_TYPE, state);
			ctx.ui.notify(`goal-setting ${next ? "enabled" : "disabled"}`, next ? "info" : "warning");
		},
	});

	pi.registerCommand("frame", {
		description: "View the current manifest (goal, guidelines, steps, switch state).",
		handler: (_args, ctx) => {
			ctx.ui.notify(buildFrameView(), "info");
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