/**
 * rolling-context -- the fade: an alternative to pi's own compaction, for
 * models with a small context window and a compaction mechanism that fixates
 * on old content.
 *
 * Off by default; `/rolling on` opts a session in, independently of the
 * toolbox toggle and everything else this package ships (ADR-0019). Ships
 * here for the same reason `bin/reactor` and the other extensions do -- no
 * build step -- and travels unusually well with REactor even though nothing
 * in it is RE-specific: disassembly listings, `strings` dumps and packet
 * captures are exactly the kind of content that fills a context window fast.
 *
 * Instead of summarizing old messages (which tends to lose the thread on
 * *why* something was done, not just what), the fade:
 *   - cuts `event.messages` to only the newest messages that fit inside a
 *     configurable percentage of the context window. Older ones are simply
 *     left out -- the session file itself is untouched, so nothing is lost,
 *     only unsent,
 *   - keeps pi's own last-resort overflow recovery and manual /compact, and
 *     cancels only pi's *proactive* threshold compaction, which is redundant
 *     with the fade's own budget (ADR-0020),
 *   - explains the fade, manifest discipline and history-tool frugality in a
 *     system-prompt block (see the note there about which extension owns
 *     what).
 *
 * The manifest (goal + guidelines + steps) and the history recovery tools
 * used to live in this file; both were split out (ADR-0024) into
 * `goal-setting/` and `history-tools/`, which are switched independently of
 * `/rolling`. The manifest goes into the system prompt via
 * `goal-setting/`'s own `before_agent_start` block, which is why this
 * extension's token accounting needs no knowledge of it: `ctx.getSystemPrompt()`
 * returns the chained prompt *including* extension additions (verified against
 * pi 0.85.1: agent-session.js:932 writes the chained prompt into
 * `this.systemPrompt`, which the context-handler ctx reads), so subtracting
 * the whole system prompt accounts for the manifest exactly, with no
 * coupling between the two extensions (ADR-0014: no shared modules). The
 * split also removed this extension's only dependency on load order: pi
 * composes `before_agent_start` by chaining `event.systemPrompt`, and
 * chains `context` handlers in extension order -- which is the unsorted
 * `readdirSync` order of the package's `extensions/` directory, and therefore
 * not something an extension may assume anything about (ADR-0024).
 *
 * Numeric knobs live in `~/.pi/agent/pi-rolling-context.json` (global):
 * `enabled`, `pct`, `reserve`. Per-session state (the toggle) lives in the
 * session itself as a `custom` entry, restored on `session_start` -- the
 * same pattern `scenario/` uses (ADR-0009). Old entries of this same
 * `customType` also carried goal/steps fields; those belong to
 * `goal-setting/` now and are ignored here. No migration from the old single
 * config file -- REactor is in alpha, so the split is a clean break
 * (ADR-0024): unknown keys are simply ignored.
 */

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { estimateTokens as estimateMessageTokens, getAgentDir } from "@earendil-works/pi-coding-agent";
import type { AgentMessage } from "@earendil-works/pi-agent-core";

// ============================================================================
// Types
// ============================================================================

const CUSTOM_TYPE = "pi-rolling-context";

interface SessionState {
	enabled?: boolean;
}

interface RollingConfig {
	enabled: boolean;
	pct: number;
	reserve?: number;
}

const DEFAULT_CONFIG: RollingConfig = {
	enabled: false,
	pct: 0.9,
	reserve: undefined,
};

// getAgentDir(), not homedir() + ".pi/agent" by hand: it is the same path in
// the common case, but it is also what respects PI_CODING_AGENT_DIR when set
// -- which the hand-rolled version silently ignored, the one behaviour
// change the port into this package made (ADR-0019).
const GLOBAL_CONFIG_PATH = join(getAgentDir(), "pi-rolling-context.json");

// ============================================================================
// Module-level session state
// ============================================================================

let config: RollingConfig = { ...DEFAULT_CONFIG };
let state: SessionState = {};
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

function normalizeState(raw: unknown): SessionState {
	const base: SessionState = {};
	if (!raw || typeof raw !== "object") return base;
	const r = raw as Record<string, any>;
	if (typeof r.enabled === "boolean") base.enabled = r.enabled;
	// goal/guidelines/steps lived here before the split (ADR-0024); they are
	// goal-setting's fields now and are deliberately not read.
	return base;
}

// ============================================================================
// System prompt (fading guidance, while enabled)
// ============================================================================

const FADING_GUIDANCE = `Older conversation messages are not sent to the model: only the newest ones that fit inside a context budget go out on each call. They still exist in the session file and remain recoverable.

When the session provides a manifest (a goal and a step list, maintained via update_steps), it is what survives the fade on purpose -- trust it first, and keep it current: if a past decision or finding matters, it belongs in the steps, not in a recovery call you have to repeat every time it fades out again.

history_index / history_search / history_read, when available, are a recovery path, not a browsing habit -- each call spends part of the very budget the fade exists to protect. Reach for them only when you need one specific, concrete detail that is not in the manifest and is not reconstructible by re-deriving it; never as a routine first step, never to double-check something the manifest already states, and never speculatively "in case it's useful." If you find yourself calling history_search often, that is a signal to write more into the manifest via update_steps, not to search more.

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
		state = {};
		for (const entry of ctx.sessionManager.getBranch()) {
			if (entry.type === "custom" && entry.customType === CUSTOM_TYPE) {
				// latest on the branch wins
				state = normalizeState(entry.data);
			}
		}
		ctx.ui.setStatus("rolling-context", isEnabled() ? "rolling: on" : undefined);
	});

	pi.on("session_shutdown", (_event, ctx) => {
		state = {};
		try {
			ctx.ui.setStatus("rolling-context", undefined);
		} catch {
			// ignore
		}
	});

	// ---- disable auto-compaction (keep manual /compact and overflow recovery) ---
	//
	// Only "threshold" compaction is ours to preempt -- the fade already keeps
	// the outgoing prompt under budget, so pi's own summarization compaction
	// firing early on the same signal is redundant, not a safety net.
	// "overflow" is pi's *last-resort* recovery after a request has already
	// been rejected by the backend for exceeding the context window (a real
	// error already happened). Cancelling that too was the direct cause of
	// bug #2: a session that ever got here had no way back, because the one
	// mechanism able to recover from it was being vetoed unconditionally.
	// Leaving "overflow" (and "manual") alone costs nothing when our own hard
	// budget below is doing its job -- it only ever fires when it isn't.
	pi.on("session_before_compact", (event) => {
		if (!isEnabled()) return;
		if (event.reason === "threshold") return { cancel: true };
	});

	// ---- fading guidance into the system prompt ---------------------------
	pi.on("before_agent_start", (event) => {
		if (!isEnabled()) return undefined;
		return { systemPrompt: `${event.systemPrompt}\n\n## Rolling Context\n${FADING_GUIDANCE}` };
	});

	// ---- the fade ----------------------------------------------------------
	pi.on("context", (event, ctx) => {
		if (!isEnabled()) return;

		const window = ctx.model?.contextWindow ?? ctx.getContextUsage?.()?.contextWindow ?? 128_000;
		const reserve = getEffectiveReserve();
		// The hard ceiling: what pi itself will not send more than (its own
		// getContextUsage()/shouldCompact() are measured against exactly this).
		// Soft is where the fade aims, to leave headroom for the *next* turn's
		// growth rather than arriving at the ceiling exactly on this one.
		const hardBudget = Math.max(0, window - reserve);
		const softBudget = Math.max(0, config.pct * hardBudget);

		// The system prompt the model will actually receive -- including every
		// extension's `before_agent_start` addition (verified: pi chains those
		// into agent.state.systemPrompt, and this ctx's getSystemPrompt()
		// returns exactly that). Subtracting its estimate accounts for
		// goal-setting's manifest block with no knowledge of goal-setting
		// (ADR-0024).
		let sysTokens = 0;
		try {
			sysTokens = estimateStringTokens(ctx.getSystemPrompt());
		} catch {
			sysTokens = 0;
		}

		// Cut `event.messages` itself -- the actual array about to go over the
		// wire -- rather than a separately-serialized session-branch array whose
		// entry count never lined up with it 1:1 (custom/compaction/branch_summary
		// entries, and pi's own leaf-path assembly, all break the assumption that
		// "the Nth branch entry is the Nth outgoing message"). That mismatch is
		// what let a plain positional slice land mid tool-call/tool-result pair,
		// which is bug #3, and it also fed a stale-unit token estimate into pi's
		// own real usage accounting, which is bugs #1 and #2 -- fixed together by
		// measuring and cutting the one array that actually matters, in its own
		// units (estimateMessageTokens, pi's own estimator).
		const messages = event.messages;
		const soft = findSafeCut(messages, Math.max(0, softBudget - sysTokens));
		let kept = messages.slice(soft.keepFrom);

		// Hard boundary: even the newest turn must never be allowed to push the
		// *actual* request over the window. findSafeCut always keeps at least the
		// newest message no matter how large (matching the previous contract),
		// so this is the only place oversized content can still slip through --
		// enforceHardBudget archives (truncates, never drops) down to fit rather
		// than let it overflow, however small that leaves what reaches the model.
		const hard = enforceHardBudget(kept, hardBudget);
		kept = hard.messages;

		const dropCount = Math.max(0, messages.length - kept.length);
		if (dropCount > 0) {
			ctx.ui.notify?.(
				`[rolling-context] dropped ${dropCount} message(s) from the next request`,
				"muted",
			);
		}
		if (hard.archived) {
			ctx.ui.notify?.(
				"[rolling-context] the newest turn alone exceeded the hard context limit; its content was archived (truncated) to stay under it -- recover the rest via history_search/history_read",
				"warning",
			);
		}

		return { messages: kept };
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
			state = { ...state, enabled: next };
			pi.appendEntry(CUSTOM_TYPE, state);
			ctx.ui.setStatus("rolling-context", next ? "rolling: on" : undefined);
			ctx.ui.notify(`rolling-context ${next ? "enabled" : "disabled"}`, next ? "info" : "warning");
		},
	});
}

// ============================================================================
// helpers
// ============================================================================

/**
 * chars/4 estimate for plain strings -- used only for the system prompt
 * estimate above (small, bounded, and a display-level subtraction from a
 * budget that pi's own estimator fills in exactly per message). The fade's
 * own budget math runs on pi's `estimateTokens(message)`, not on this.
 */
function estimateStringTokens(s: string): number {
	return Math.max(1, Math.ceil((s || "").length / 4));
}

// ============================================================================
// The fade's cut point -- mirrors pi's own compaction boundary rules
// ============================================================================
//
// pi's own `findCutPoint` (core/compaction) picks a cut point in *session
// entries*, never at a `toolResult` (which must follow its `toolCall`), and
// reports separately whether that point lands mid-turn. The same rules apply
// here, just against `AgentMessage[]` directly -- the array actually being
// sent -- rather than against entries, which is what let the old positional
// slice land inside a tool-call/tool-result pair (bug #3) and back a token
// budget with a different array than the one it was cutting (bugs #1, #2).

/** A `toolResult` can never start a kept window -- it must follow its call. */
function isCutPointRole(role: AgentMessage["role"]): boolean {
	return role !== "toolResult";
}

/** What pi considers the start of a fresh turn (not a tool-call continuation). */
function isTurnStartRole(role: AgentMessage["role"]): boolean {
	return role === "user" || role === "bashExecution" || role === "custom" || role === "branchSummary" || role === "compactionSummary";
}

interface SafeCut {
	keepFrom: number;
	isSplitTurn: boolean;
}

/**
 * Find the first index to keep from, walking newest -> oldest and stopping
 * once `keepTokens` is accounted for -- same algorithm as pi's `findCutPoint`,
 * retargeted from session entries to the live message array. Always keeps at
 * least the newest message, however large (the hard-budget pass afterward is
 * what keeps that from overflowing the real ceiling).
 */
function findSafeCut(messages: AgentMessage[], keepTokens: number): SafeCut {
	if (messages.length === 0) return { keepFrom: 0, isSplitTurn: false };

	const cutPoints: number[] = [];
	for (let i = 0; i < messages.length; i++) {
		if (isCutPointRole(messages[i].role)) cutPoints.push(i);
	}
	if (cutPoints.length === 0) return { keepFrom: 0, isSplitTurn: false };

	let accumulated = 0;
	let cutIndex = cutPoints[0];
	for (let i = messages.length - 1; i >= 0; i--) {
		const tokens = estimateMessageTokens(messages[i]);
		if (tokens === 0) continue;
		accumulated += tokens;
		if (accumulated >= keepTokens) {
			for (const c of cutPoints) {
				if (c >= i) {
					cutIndex = c;
					break;
				}
			}
			break;
		}
	}
	return { keepFrom: cutIndex, isSplitTurn: !isTurnStartRole(messages[cutIndex].role) };
}

const ARCHIVE_NOTE =
	"\n\n…(content archived to stay under the hard context limit; recover the rest via history_search/history_read)";

/** Shrink the text-bearing content of one message to roughly `keepTokens`, never touching role, tool-call ids, or arguments. */
function archiveMessageContent(message: AgentMessage, keepTokens: number): AgentMessage {
	const keepChars = Math.max(0, Math.floor(keepTokens * 4));
	if (message.role === "assistant") {
		let budget = keepChars;
		const content = (message as any).content.map((block: any) => {
			if (block.type === "toolCall") return block;
			if (block.type !== "text" && block.type !== "thinking") return block;
			const field = block.type === "text" ? "text" : "thinking";
			const value: string = block[field] ?? "";
			if (budget <= 0) return { ...block, [field]: "" };
			const kept = value.length > budget ? value.slice(0, budget) + ARCHIVE_NOTE : value;
			budget -= value.length;
			return { ...block, [field]: kept };
		});
		return { ...message, content } as AgentMessage;
	}
	// user / toolResult / custom: content is a string or a text/image block array.
	const raw = (message as any).content;
	if (typeof raw === "string") {
		return { ...message, content: raw.length > keepChars ? raw.slice(0, keepChars) + ARCHIVE_NOTE : raw } as AgentMessage;
	}
	if (Array.isArray(raw)) {
		let budget = keepChars;
		const content = raw
			.map((block: any) => {
				if (block.type !== "text") return budget > 0 ? block : undefined;
				if (budget <= 0) return undefined;
				const text = block.text.length > budget ? block.text.slice(0, budget) + ARCHIVE_NOTE : block.text;
				budget -= block.text.length;
				return { ...block, text };
			})
			.filter((b: any) => b !== undefined);
		return { ...message, content: content.length > 0 ? content : [{ type: "text", text: ARCHIVE_NOTE.trim() }] } as AgentMessage;
	}
	return message;
}

interface HardBudgetResult {
	messages: AgentMessage[];
	archived: boolean;
}

/**
 * The hard boundary: `messages` is already a structurally-safe kept window
 * (see findSafeCut), but even that can exceed the real context window when a
 * single turn is enormous -- a giant disassembly listing or packet capture is
 * exactly the content this extension exists for. Rather than send it and let
 * the backend reject the request (bug #2) or let pi's own usage accounting
 * read over 100% (bug #1), drop the oldest of the kept messages first and, if
 * even the newest one alone is still too big, archive (truncate) its content
 * -- never drop the newest message outright, and never break a
 * toolCall/toolResult pairing by leaving an orphaned result at the front.
 */
function enforceHardBudget(messages: AgentMessage[], hardTokens: number): HardBudgetResult {
	if (messages.length === 0) return { messages, archived: false };

	let total = messages.reduce((sum, m) => sum + estimateMessageTokens(m), 0);
	if (total <= hardTokens) return { messages, archived: false };

	const out = [...messages];
	while (out.length > 1 && total > hardTokens) {
		total -= estimateMessageTokens(out[0]);
		out.shift();
	}
	// A dropped toolCall leaves its toolResult orphaned at the front; a dropped
	// toolResult never orphans anything on its own. Either way, clear any
	// leading toolResult the budget walk above could have exposed.
	while (out.length > 1 && out[0].role === "toolResult") {
		total -= estimateMessageTokens(out[0]);
		out.shift();
	}

	if (total > hardTokens) {
		// The one message left (necessarily the newest -- it is never dropped)
		// is alone bigger than the hard ceiling. Archive its content instead of
		// omitting it: a stub of the current turn beats silently overflowing.
		const floor = Math.max(64, hardTokens);
		out[out.length - 1] = archiveMessageContent(out[out.length - 1], floor);
	}

	return { messages: out, archived: true };
}