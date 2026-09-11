/**
 * history-tools -- line-addressed recovery over the session history:
 * `history_index`, `history_search`, `history_read`. The session file is
 * serialized once per call into a stable line-indexed text; every tool
 * answers with line numbers so the next call can be addressed against them.
 *
 * Split out of `rolling-context/` (ADR-0024): the tools are useful in *any*
 * long session, not only one whose context is being faded -- with pi's own
 * compaction, or with no compaction at all, being able to look back at an
 * exact past message the same way is worth exactly as much. They are
 * registered unconditionally and work by default; `/history-tools [on|off]`
 * (per-session, riding its own `custom` entry) is the user's lever for a
 * model that calls them too often. Nothing here touches the system prompt,
 * the message array, or `reactor`.
 *
 * The fade's guidance tells the model these are "a recovery path, not a
 * browsing habit" -- that economics lives in rolling-context's block, not
 * here, so the tools carry no opinion of their own about when to be called
 * beyond what their descriptions say.
 *
 * Config: `~/.pi/agent/pi-history-tools.json` (global `enabled` default plus
 * the paging/search knobs), its own file per the one-file-per-extension
 * convention; unrelated to `reactor.json` (ADR-0016). No migration from the
 * old single `pi-rolling-context.json` -- REactor is in alpha, so the split
 * is a clean break (ADR-0024).
 */

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { Type } from "typebox";
import type { ExtensionAPI, SessionManager } from "@earendil-works/pi-coding-agent";
import { convertToLlm, getAgentDir, serializeConversation } from "@earendil-works/pi-coding-agent";
import type { AgentMessage, SessionEntry } from "@earendil-works/pi-agent-core";

// ============================================================================
// Types
// ============================================================================

const CUSTOM_TYPE = "pi-history-tools";

interface HistoryToolsConfig {
	enabled: boolean;
	readBudget: number;
	searchHitLimit: number;
	contextLines: number;
}

const DEFAULT_CONFIG: HistoryToolsConfig = {
	enabled: true,
	readBudget: 3000,
	searchHitLimit: 8,
	contextLines: 2,
};

const GLOBAL_CONFIG_PATH = join(getAgentDir(), "pi-history-tools.json");

// ============================================================================
// Module-level session state
// ============================================================================

let config: HistoryToolsConfig = { ...DEFAULT_CONFIG };
/** Per-session toggle: absent reads as the global default (on). */
let sessionEnabled: boolean | undefined = undefined;

function isEnabled(): boolean {
	return sessionEnabled ?? config.enabled;
}

// ============================================================================
// Config
// ============================================================================

function loadGlobalConfig(): void {
	const merged: HistoryToolsConfig = { ...DEFAULT_CONFIG };
	try {
		if (existsSync(GLOBAL_CONFIG_PATH)) {
			const raw = JSON.parse(readFileSync(GLOBAL_CONFIG_PATH, "utf8"));
			for (const key of Object.keys(DEFAULT_CONFIG) as (keyof HistoryToolsConfig)[]) {
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
	sessionEnabled = undefined;
	for (const entry of sm.getBranch()) {
		if (entry.type === "custom" && entry.customType === CUSTOM_TYPE) {
			// latest on the branch wins
			const r = entry.data as Record<string, any> | undefined;
			if (r && typeof r === "object" && typeof r.enabled === "boolean") {
				sessionEnabled = r.enabled;
			}
		}
	}
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

/**
 * chars/4 estimate for plain strings -- fine for history_read's paging budget,
 * which is a display concern, not a wire-format or budget decision.
 */
function estimateStringTokens(s: string): number {
	return Math.max(1, Math.ceil((s || "").length / 4));
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
		sessionEnabled = undefined;
	});

	// ---- commands -----------------------------------------------------------
	pi.registerCommand("history-tools", {
		description: "Toggle the history tools for this session (or /history-tools on|off). Shows status with no arg.",
		handler: (args, ctx) => {
			const arg = (args || "").trim().toLowerCase();
			let next: boolean;
			if (arg === "on") next = true;
			else if (arg === "off") next = false;
			else next = !isEnabled();
			sessionEnabled = next;
			pi.appendEntry(CUSTOM_TYPE, { enabled: next });
			ctx.ui.notify(`history tools ${next ? "enabled" : "disabled"}`, next ? "info" : "warning");
		},
	});

	// ---- tools ---------------------------------------------------------------
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
		async execute(_id, params, _signal, _onUpdate, ctx) {
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
				used += estimateStringTokens(l);
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
// helpers
// ============================================================================

function safeRegex(pattern: string): RegExp | null {
	try {
		return new RegExp(pattern, "i");
	} catch {
		return null;
	}
}

function toolDisabled() {
	return {
		content: [{ type: "text" as const, text: "history tools are disabled. Run /history-tools on to enable them." }],
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