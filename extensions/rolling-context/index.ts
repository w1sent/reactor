/**
 * rolling-context -- an alternative to pi's own compaction, for models with a
 * small context window and a compaction mechanism that fixates on old content.
 *
 * Off by default; `/rolling on` opts a session in, independently of the
 * toolbox toggle and everything else this package ships (ADR-0019). Ships
 * here for the same reason `bin/reactor` and the other extensions do -- no
 * build step -- and travels unusually well with REactor even though nothing
 * in it is RE-specific: disassembly listings, `strings` dumps and packet
 * captures are exactly the kind of content that fills a context window fast.
 *
 * Instead of summarizing old messages (which tends to lose the thread on
 * *why* something was done, not just what), this:
 *   - keeps a tiny, always-present manifest at the front of every prompt --
 *     the user's goal, plus steps the agent maintains itself as short
 *     conceptual summaries with a 3-word status,
 *   - fades the rest: only the newest messages that fit inside a configurable
 *     percentage of the context window go to the model on the next call.
 *     Older ones are simply left out -- the session file itself is untouched,
 *     so nothing is lost, only unsent,
 *   - tells the model it can recover anything faded via three line-addressed
 *     history tools, and that keeping the manifest current is what makes work
 *     survive the fade at all.
 *
 * Numeric knobs live in `~/.pi/agent/pi-rolling-context.json` (global) --
 * unrelated to `<agent dir>/reactor.json` (ADR-0016) on purpose: the two
 * extensions share no state, so one file per extension is what pi's own
 * config-file convention already says to do. Per-session state (goal,
 * guidelines, steps, enabled) lives in the session itself as a `custom`
 * entry, restored on `session_start` by taking the latest one on the branch
 * -- the same pattern `scenario/` uses for its own state (ADR-0009).
 */

import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

import { Type } from "typebox";
import type { ExtensionAPI, ExtensionContext, SessionManager } from "@earendil-works/pi-coding-agent";
import { convertToLlm, getAgentDir, serializeConversation } from "@earendil-works/pi-coding-agent";
import type { AgentMessage, SessionEntry } from "@earendil-works/pi-agent-core";

// ============================================================================
// Types
// ============================================================================

const CUSTOM_TYPE = "pi-rolling-context";

interface Step {
	summary: string;
	status: string;
}

interface SessionState {
	enabled?: boolean;
	goal?: string;
	guidelines?: string;
	steps: Step[];
}

interface RollingConfig {
	enabled: boolean;
	pct: number;
	softStepLimit: number;
	maxDescription: number;
	statusWords: number;
	readBudget: number;
	searchHitLimit: number;
	contextLines: number;
	reserve?: number;
}

const DEFAULT_CONFIG: RollingConfig = {
	enabled: false,
	pct: 0.9,
	softStepLimit: 20,
	maxDescription: 80,
	statusWords: 3,
	readBudget: 3000,
	searchHitLimit: 8,
	contextLines: 2,
	reserve: undefined,
};

// getAgentDir(), not homedir() + ".pi/agent" by hand: it is the same path in
// the common case, but it is also what respects PI_CODING_AGENT_DIR when set
// -- which the hand-rolled version silently ignored, the one behaviour
// change this port makes (ADR-0019).
const GLOBAL_CONFIG_PATH = join(getAgentDir(), "pi-rolling-context.json");

// ============================================================================
// Module-level session state
// ============================================================================

let config: RollingConfig = { ...DEFAULT_CONFIG };
let state: SessionState = { steps: [] };
/** Branch line number from which the last truncation notice was reported. */
let lastReportedStartLine = 0;
let activeCwd = process.cwd();

function isEnabled(): boolean {
	return state.enabled ?? config.enabled;
}

// ============================================================================
// Config
// ============================================================================

function loadGlobalConfig(): void {
	const merged: RollingConfig = { ...DEFAULT_CONFIG };
	try {
		if (existsSync(GLOBAL_CONFIG_PATH)) {
			const raw = JSON.parse(readFileSync(GLOBAL_CONFIG_PATH, "utf8"));
			for (const key of Object.keys(DEFAULT_CONFIG) as (keyof RollingConfig)[]) {
				if (raw[key] !== undefined) (merged as any)[key] = raw[key];
			}
		}
	} catch {
		// ignore malformed config, fall back to defaults
	}
	config = merged;
}

/** Read compaction.reserveTokens from pi settings (global + project merged). */
function readPiReserveTokens(): number {
	try {
		const merge = (): any => {
			const settings: any = {};
			const files = [join(getAgentDir(), "settings.json"), join(activeCwd, ".pi", "settings.json")];
			for (const f of files) {
				if (!existsSync(f)) continue;
				const parsed = JSON.parse(readFileSync(f, "utf8"));
				Object.assign(settings, parsed);
			}
			return settings;
		};
		const settings = merge();
		return typeof settings.compaction?.reserveTokens === "number"
			? settings.compaction.reserveTokens
			: 16384;
	} catch {
		return 16384;
	}
}

function getEffectiveReserve(): number {
	return config.reserve ?? effectivePiReserve;
}

let effectivePiReserve = 16384;

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
	lastReportedStartLine = 0;
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
// History serialization (line-addressed, consistent everywhere)
// ============================================================================

function textOf(content: string | { type: "text"; text: string }[] | undefined): string {
	if (!content) return "";
	if (typeof content === "string") return content;
	return content.filter((c) => c.type === "text").map((c) => c.text).join(" ");
}

function serializeMessage(m: AgentMessage): string {
	try {
		return serializeConversation(convertToLlm([m]));
	} catch {
		return "";
	}
}

/** Serialize one session entry into a searchable text block. */
function serializeEntry(e: SessionEntry): string {
	switch (e.type) {
		case "message":
			return serializeMessage(e.message);
		case "custom_message":
			return `[Custom]: ${textOf(e.content)}`;
		case "compaction":
			return `[History summary]: ${e.summary}`;
		case "branch_summary":
			return `[Branch summary]: ${e.summary}`;
		default:
			return "";
	}
}

/**
 * Serialize session entries into a line-indexed text.
 * Returns an array of lines plus, for each entry, the line index it starts at.
 * Only entries that produce content contribute lines.
 */
function buildSerialized(entries: SessionEntry[]): { lines: string[]; lineStarts: number[] } {
	const lines: string[] = [];
	const lineStarts: number[] = [];
	for (const e of entries) {
		const s = serializeEntry(e);
		if (!s) {
			lineStarts.push(lines.length);
			continue;
		}
		// blank line between blocks for readability
		if (lines.length > 0 && lines[lines.length - 1] !== "") lines.push("");
		lineStarts.push(lines.length);
		for (const l of s.split("\n")) lines.push(l);
	}
	return { lines, lineStarts };
}

function estimateTokens(s: string): number {
	return Math.max(1, Math.ceil((s || "").length / 4));
}

// ============================================================================
// Manifest (the leading, always-present message)
// ============================================================================

function buildManifestText(visibleStartLine: number, totalLines: number, showTruncation: boolean): string {
	const parts: string[] = [];
	parts.push("## Session Goal");
	parts.push(state.goal?.trim() || "(none set)");
	parts.push("");
	parts.push(`## Steps (${state.steps.length}/${config.softStepLimit})`);
	if (state.steps.length === 0) {
		parts.push("(none yet)");
	} else {
		parts.push(state.steps.map((s, i) => `${i + 1}. [${s.status}] ${s.summary}`).join("\n"));
	}
	parts.push("");
	parts.push(`[context: visible from line ${visibleStartLine} of ${totalLines}]`);
	if (showTruncation) {
		parts.push("(note: earlier context was truncated; recover it via history_search / history_read and keep the manifest current)");
	}
	return parts.join("\n");
}

// ============================================================================
// System prompt (guidelines + standing operating guidance)
// ============================================================================

const STANDING_GUIDANCE = `The conversation history is partially faded: this session keeps a fixed manifest (goal + steps) at the front of every prompt and only the most recent messages fit inside a context budget. Older messages are not sent to the model (they still exist in the session file and are recoverable).

You are responsible for keeping the manifest current so work survives fading:
- Record progress as CONCEPTUAL steps (e.g. "verified input schema", "determined root cause of X"), not trivial micro-actions (e.g. "found function x").
- Each step has a short summary and a 3-word status. Overwrite the full step list with update_steps.
- If the step count exceeds the soft limit, consolidate/merge finished steps to bring it back under.
- If you need information that was removed from context, use history_index / history_search / history_read to recover it, then write the important details into the manifest via update_steps.

The session goal and guidelines are set by the user; you must not change them. Only the steps list is yours to maintain.`;

// ============================================================================
// Extension
// ============================================================================

export default function (pi: ExtensionAPI) {
	loadGlobalConfig();

	// ---- session lifecycle ------------------------------------------------
	pi.on("session_start", (event, ctx) => {
		activeCwd = ctx.cwd;
		effectivePiReserve = readPiReserveTokens();
		loadSessionState(ctx.sessionManager);
		ctx.ui.setStatus("rolling-context", isEnabled() ? "rolling: on" : undefined);
	});

	pi.on("session_shutdown", (_event, ctx) => {
		state = { steps: [] };
		try {
			ctx.ui.setStatus("rolling-context", undefined);
		} catch {
			// ignore
		}
	});

	// ---- disable auto-compaction (keep manual /compact) -------------------
	pi.on("session_before_compact", (event) => {
		if (!isEnabled()) return;
		if (event.reason !== "manual") return { cancel: true };
	});

	// ---- guidelines + standing guidance into the system prompt ------------
	pi.on("before_agent_start", (event) => {
		if (!isEnabled()) return;
		const extra: string[] = [];
		if (state.guidelines?.trim()) extra.push(state.guidelines.trim());
		extra.push(STANDING_GUIDANCE);
		return { systemPrompt: `${event.systemPrompt}\n\n## Rolling Context\n${extra.join("\n\n")}` };
	});

	// ---- the fade ----------------------------------------------------------
	pi.on("context", (event, ctx) => {
		if (!isEnabled()) return;

		const sm = ctx.sessionManager;
		const branch = sm.getBranch();
		const content = branch.filter((e) => serializeEntry(e) !== "");
		const serialized = buildSerialized(content);
		const totalLines = serialized.lines.length;

		const window = ctx.model?.contextWindow ?? ctx.getContextUsage?.()?.contextWindow ?? 128_000;
		const reserve = getEffectiveReserve();
		const budget = Math.max(0, config.pct * (window - reserve));

		let sysTokens = 0;
		try {
			sysTokens = estimateTokens(ctx.getSystemPrompt());
		} catch {
			sysTokens = 0;
		}
		const manifestText = buildManifestText(0, totalLines, false);
		const manifestTokens = estimateTokens(manifestText);
		const historyBudget = Math.max(0, budget - sysTokens - manifestTokens);

		// walk newest -> oldest to find the first kept message
		let keepFirst = 0;
		let used = 0;
		for (let i = content.length - 1; i >= 0; i--) {
			const tokens = estimateTokens(serializeEntry(content[i]));
			if (used + tokens > historyBudget) {
				// always keep at least the newest message (the current turn)
				if (used === 0) keepFirst = i;
				else keepFirst = i + 1;
				break;
			}
			used += tokens;
			if (i === 0) keepFirst = 0;
		}

		const keptCount = content.length - keepFirst;
		const visibleStartLine = serialized.lineStarts[keepFirst] ?? 0;

		// apply the same trim to the outgoing messages
		let messages = event.messages;
		const dropCount = Math.max(0, messages.length - keptCount);
		if (dropCount > 0) messages = messages.slice(dropCount);

		const showTruncation = visibleStartLine > lastReportedStartLine;
		if (showTruncation) lastReportedStartLine = visibleStartLine;

		if (dropCount > 0) {
			ctx.ui.notify?.(
				`[rolling-context] dropped ${dropCount} message(s); visible context starts at line ${visibleStartLine}`,
				"muted",
			);
		}

		const manifest = {
			role: "custom" as const,
			customType: CUSTOM_TYPE,
			content: buildManifestText(visibleStartLine, totalLines, showTruncation),
			display: true,
			timestamp: new Date().toISOString(),
		};
		messages = [manifest, ...messages];

		return { messages };
	});

	// ---- commands -----------------------------------------------------------
	pi.registerCommand("rolling", {
		description: "Toggle rolling-context (or /rolling on|off). Shows status with no arg.",
		handler: (args, ctx) => {
			const arg = (args || "").trim().toLowerCase();
			let next: boolean;
			if (arg === "on") next = true;
			else if (arg === "off") next = false;
			else next = !isEnabled();
			setEnabled(next);
			ctx.ui.setStatus("rolling-context", next ? "rolling: on" : undefined);
			ctx.ui.notify(`rolling-context ${next ? "enabled" : "disabled"}`, next ? "info" : "warning");
		},
	});

	pi.registerCommand("goal", {
		description: "Set the session goal (survives /resume).",
		handler: (args, ctx) => {
			const text = (args || "").trim();
			if (!text) {
				ctx.ui.notify("usage: /goal <text>", "warning");
				return;
			}
			setGoal(text);
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
			setGuidelines(text);
			ctx.ui.notify(`guidelines set: ${text}`, "info");
		},
	});

	pi.registerCommand("frame", {
		description: "View the current manifest (goal, guidelines, steps, count/limit).",
		handler: (_args, ctx) => {
			ctx.ui.notify(buildFrameView(), "info");
		},
	});

	// ---- helper wrappers over pi.appendEntry --------------------------------
	function setGoal(text: string) {
		state = { ...state, goal: text };
		pi.appendEntry(CUSTOM_TYPE, state);
	}
	function setGuidelines(text: string) {
		state = { ...state, guidelines: text };
		pi.appendEntry(CUSTOM_TYPE, state);
	}
	function setEnabled(v: boolean) {
		state = { ...state, enabled: v };
		pi.appendEntry(CUSTOM_TYPE, state);
	}
	function buildFrameView(): string {
		const out = [`goal: ${state.goal ?? "(none)"}`, `guidelines: ${state.guidelines ?? "(none)"}`, `steps (${state.steps.length}/${config.softStepLimit}):`];
		if (state.steps.length === 0) out.push("  (none)");
		else out.push(state.steps.map((s, i) => `  ${i + 1}. [${s.status}] ${s.summary}`).join("\n"));
		out.push(`enabled: ${isEnabled()}`);
		return out.join("\n");
	}

	// ---- tools ---------------------------------------------------------------
	pi.registerTool({
		name: "update_steps",
		label: "Update Steps",
		description:
			"Overwrite the entire step list of the session manifest. Each step has a short conceptual summary (an investigative question or milestone, not a micro-action) and a 3-word status. Returns a warning when the count exceeds the soft limit.",
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
			if (!isEnabled()) return toolDisabled();
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

	pi.registerTool({
		name: "history_index",
		label: "History Index",
		description:
			"Returns a line-addressed map of the session history (one entry per message with its line range and a short snippet). Use it to orient yourself before reading a specific line range. Optionally restrict to a line range.",
		parameters: Type.Object({
			startLine: Type.Optional(Type.Integer({ minimum: 0, description: "Start line (0-based)." })),
			endLine: Type.Optional(Type.Integer({ description: "End line (inclusive)." })),
			maxEntries: Type.Optional(Type.Integer({ description: "Max entries to return (default 20)." })),
		}),
		async execute(_tool, params, _signal, _onUpdate, ctx) {
			if (!isEnabled()) return toolDisabled();
			const { lines, lineStarts } = buildSerialized(ctx.sessionManager.getBranch());
			const entries = buildIndex(lines, lineStarts);
			const filtered = entries.filter(
				(e) => (params.startLine ?? -1) <= e.start && (params.endLine ?? Infinity) >= e.end,
			);
			const shown = filtered.slice(-(params.maxEntries ?? 20));
			const text =
				shown.length === 0
					? "No messages in that range."
					: shown.map((e) => `line ${e.start}-${e.end} [${e.role}] ${e.snippet}`).join("\n");
			return { content: [{ type: "text", text }], details: { totalEntries: entries.length } };
		},
	});

	pi.registerTool({
		name: "history_search",
		label: "History Search",
		description:
			"Search the serialized session history (line-addressed). Returns matching lines with the line number to feed into history_read, plus context lines around each match.",
		parameters: Type.Object({
			query: Type.String({ description: "Text or regex to search for." }),
			regex: Type.Optional(Type.Boolean({ description: "Treat query as a regex (default false)." })),
			limit: Type.Optional(Type.Integer({ description: "Max hits (default 8)." })),
			contextLines: Type.Optional(Type.Integer({ description: "Lines shown around each hit (default 2)." })),
		}),
		async execute(_toolId, params, _state, _onUpdate, ctx) {
			if (!isEnabled()) return toolDisabled();
			const { lines } = buildSerialized(ctx.sessionManager.getBranch());
			const regex = params.regex ? safeRegex(params.query) : null;
			const needle = regex ? null : params.query.toLowerCase();
			const limit = params.limit ?? config.searchHitLimit;
			const context = params.contextLines ?? config.contextLines;
			const hits: string[] = [];
			for (let i = 0; i < lines.length && hits.length < limit; i++) {
				const line = lines[i];
				const hit = regex ? regex.test(line) : line.toLowerCase().includes(needle!);
				if (hit) {
					const from = Math.max(0, i - context);
					const to = Math.min(lines.length - 1, i + context);
					const block = lines.slice(from, to + 1);
					hits.push(`line ${i}:\n${block.map((l, k) => `${from + k} | ${l}`).join("\n")}`);
				}
			}
			if (hits.length === 0) return { content: [{ type: "text", text: "No matches." }], details: {} };
			return { content: [{ type: "text", text: hits.join("\n\n") }], details: { hitCount: hits.length } };
		},
	});

	pi.registerTool({
		name: "history_read",
		label: "History Read",
		description: "Read a range of history by line number (0-based, inclusive). Token-capped; paging via the returned note.",
		parameters: Type.Object({
			startLine: Type.Integer({ description: "First line to read (0-based)." }),
			endLine: Type.Integer({ description: "Last line to read (inclusive)." }),
		}),
		async execute(_toolId, params, _signal, _onUpdate, ctx) {
			if (!isEnabled()) return toolDisabled();
			const { lines } = buildSerialized(ctx.sessionManager.getBranch());
			const from = Math.max(0, params.startLine);
			const to = Math.min(lines.length - 1, params.endLine);
			if (from > to || from >= lines.length) return { content: [{ type: "text", text: "Out of range." }], details: {} };
			// token budget
			const budget = config.readBudget;
			let used = 0;
			const selected: string[] = [];
			let lastLine = from;
			for (let i = from; i <= to; i++) {
				const l = lines[i];
				used += estimateTokens(l);
				if (used > budget && selected.length > 0) break;
				selected.push(`${i} | ${l}`);
				lastLine = i;
			}
			let text = selected.join("\n");
			if (lastLine < to) {
				text += `\n…truncated (budget); more available from line ${lastLine + 1} to ${to}.`;
			}
			return { content: [{ type: "text", text }], details: {} };
		},
	});
}

// ============================================================================
// helpers (module-scope, shared with tools/commands)
// ============================================================================

function truncate(s: string, n: number): string {
	return s.length <= n ? s : s.slice(0, Math.max(0, n - 1)) + "…";
}
function truncateWords(s: string, n: number): string {
	return s.trim().split(/\s+/).slice(0, n).join(" ");
}
function safeRegex(pattern: string): RegExp | null {
	try {
		return new RegExp(pattern, "i");
	} catch {
		return null;
	}
}
function toolDisabled() {
	return {
		content: [{ type: "text" as const, text: "rolling-context is disabled. Run /rolling on to enable it." }],
		details: {},
	};
}

interface IndexEntry {
	start: number;
	end: number;
	role: string;
	snippet: string;
}
function buildIndex(lines: string[], lineStarts: number[]): IndexEntry[] {
	const out: IndexEntry[] = [];
	for (let m = 0; m < lineStarts.length; m++) {
		const start = lineStarts[m];
		const end = m + 1 < lineStarts.length ? lineStarts[m + 1] - 1 : lines.length - 1;
		const block = lines.slice(start, end + 1);
		const first = block.find((l) => l.trim() !== "") ?? "";
		const role = first.includes("]:") ? first.split("]:")[0].slice(1) : "msg";
		const snippet = first.length > 60 ? first.slice(0, 60) + "…" : first;
		out.push({ start, end, role, snippet });
	}
	return out;
}
