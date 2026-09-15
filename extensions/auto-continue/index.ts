/**
 * auto-continue -- when an automatic compaction ends the agent's turn, keep
 * it going: send the model a short user message ("continue") so it picks its
 * work back up, instead of leaving the session parked until someone types it.
 *
 * Off by default; `/auto-continue [on|off]` opts a session in (per-session,
 * riding its own `custom` entry). Config: `~/.pi/agent/pi-auto-continue.json`
 * (global `enabled` default, the continuation `message`, and
 * `maxConsecutive`). Nothing here is RE-specific and nothing calls `reactor`.
 *
 * The pi facts this rides on (verified against pi 0.85.1):
 *
 * - `_checkCompaction`'s three automatic cases: overflow *with* retry
 *   (`willRetry: true`) is continued by pi's own post-run loop
 *   (`agent.continue()` after `_handlePostAgentRun`) -- it needs no nudge,
 *   and sending one would inject a message into the retrying run. Only the
 *   two *settled* cases (`willRetry: false` -- threshold after a completed
 *   response, overflow with a preserved response) leave the turn ended, and
 *   those are exactly what this extension continues.
 * - `session_compact` fires per successful compaction with `reason` and
 *   `willRetry`; `session_compact_failed` fires when compaction failed or
 *   was aborted. A failed compaction never shrunk the context, so a
 *   "continue" would just overflow again -- skipped, deliberately.
 * - Manual `/compact` is skipped too: it is deliberate housekeeping, not an
 *   interruption.
 * - `agent_settled` is the first point pi guarantees nothing is streaming and
 *   no compaction is in progress (the same point reporting/ level 2 uses for
 *   its revert resend); `pi.sendUserMessage(msg, { deliverAs: "followUp" })`
 *   starts a run from there, or queues behind one if another extension's own
 *   agent_settled dispatch is already running. The pre-prompt path in
 *   `prompt()` (which compacts before sending, `agent-session.js:~880`) means
 *   a stale pending flag cleared in `before_agent_start` cannot turn a
 *   pre-prompt housekeeping compaction into a spurious continuation.
 *
 * Runaway guard: compaction -> continue -> compaction can in principle cycle
 * forever when every turn refills the window. `before_agent_start` counts
 * consecutive turns whose prompt is exactly the continuation message; past
 * `maxConsecutive` the extension pauses (one notification) and only your next
 * real prompt starts counting anew.
 */

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

import type {
	ExtensionAPI,
	BeforeAgentStartEvent,
	ExtensionContext,
	SessionCompactEvent,
	SessionManager,
	Theme,
} from "@earendil-works/pi-coding-agent";
import { getAgentDir } from "@earendil-works/pi-coding-agent";
import { glyph, lead } from "../lib/statusbar.ts";

// ============================================================================
// Types
// ============================================================================

const CUSTOM_TYPE = "pi-auto-continue";

interface AutoContinueConfig {
	enabled: boolean;
	message: string;
	maxConsecutive: number;
}

const DEFAULT_CONFIG: AutoContinueConfig = {
	enabled: false,
	message: "continue",
	maxConsecutive: 10,
};

const GLOBAL_CONFIG_PATH = join(getAgentDir(), "pi-auto-continue.json");

// ============================================================================
// Module-level session state
// ============================================================================

let config: AutoContinueConfig = { ...DEFAULT_CONFIG };
/** Per-session toggle: absent reads as the global default (off). */
let sessionEnabled: boolean | undefined = undefined;
/** Set by a successful automatic compaction with no pending pi retry; consumed at agent_settled. */
let pendingContinue = false;
/** How many turns in a row were started by this extension's own message. */
let consecutive = 0;
/** Whether the pause notice has been shown for the current run of continuations. */
let pausedNotified = false;

function isEnabled(): boolean {
	return sessionEnabled ?? config.enabled;
}

// ============================================================================
// Config
// ============================================================================

function loadGlobalConfig(): void {
	const merged: AutoContinueConfig = { ...DEFAULT_CONFIG };
	try {
		if (existsSync(GLOBAL_CONFIG_PATH)) {
			const raw = JSON.parse(readFileSync(GLOBAL_CONFIG_PATH, "utf8"));
			for (const key of Object.keys(DEFAULT_CONFIG) as (keyof AutoContinueConfig)[]) {
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
	pendingContinue = false;
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
// Extension
// ============================================================================

export default function (pi: ExtensionAPI) {
	loadGlobalConfig();

	// ---- session lifecycle ------------------------------------------------
	pi.on("session_start", (_event, ctx) => {
		loadSessionState(ctx.sessionManager);
		try {
			const t: Theme = ctx.ui.theme;
			ctx.ui.setStatus("auto-continue", isEnabled() ? `${lead(t)}${glyph(t, "↻")} auto-continue` : undefined);
		} catch {
			// no terminal -- print and json modes carry no footer
		}
	});

	pi.on("session_shutdown", (_event, ctx) => {
		sessionEnabled = undefined;
		pendingContinue = false;
		consecutive = 0;
		pausedNotified = false;
		try {
			ctx.ui.setStatus("auto-continue", undefined);
		} catch {
			// ignore
		}
	});

	// ---- trigger -----------------------------------------------------------
	pi.on("session_compact", (event: SessionCompactEvent) => {
		if (!event.reason || event.reason === "manual") return;
		// Overflow recovery retries the turn by itself (willRetry: true) --
		// queueing "continue" there would inject into pi's own retry.
		if (event.willRetry) return;
		pendingContinue = true;
	});

	// ---- the continuation ---------------------------------------------------
	pi.on("agent_settled", (_event, ctx) => {
		const wasPending = pendingContinue;
		pendingContinue = false;
		if (!wasPending || !isEnabled()) return;
		if (consecutive >= config.maxConsecutive) {
			if (!pausedNotified) {
				pausedNotified = true;
				ctx.ui.notify?.(
					`[auto-continue] paused after ${consecutive} consecutive continuations; it resumes after your next message`,
					"warning",
				);
			}
			return;
		}
		consecutive += 1;
		// followUp, not a bare prompt: if another extension's own agent_settled
		// handler started a run first (reporting/ level 2 reverts from the same
		// event), the bare call would throw "already processing" -- a queued
		// follow-up runs right after it instead.
		pi.sendUserMessage(config.message, { deliverAs: "followUp" });
	});

	// ---- the runaway counter -----------------------------------------------
	pi.on("before_agent_start", (event: BeforeAgentStartEvent) => {
		// Whatever turn is starting now, the pending flag (if any survived this
		// far) is stale: either this turn is our own continuation, or a real
		// prompt got in between and the pending compaction is housekeeping.
		pendingContinue = false;
		if ((event.prompt ?? "").trim() === config.message.trim()) return;
		// Anything else is a real prompt: the pause lifts and the count resets.
		consecutive = 0;
		pausedNotified = false;
	});

	// ---- command -------------------------------------------------------------
	pi.registerCommand("auto-continue", {
		description: "Toggle auto-continue after automatic compaction (or /auto-continue on|off). Shows status with no arg.",
		handler: (args, ctx) => {
			const arg = (args || "").trim().toLowerCase();
			let next: boolean;
			if (arg === "on") next = true;
			else if (arg === "off") next = false;
			else next = !isEnabled();
			sessionEnabled = next;
			pi.appendEntry(CUSTOM_TYPE, { enabled: next });
			const t: Theme = ctx.ui.theme;
			ctx.ui.setStatus("auto-continue", next ? `${lead(t)}${glyph(t, "↻")} auto-continue` : undefined);
			ctx.ui.notify(`auto-continue ${next ? "enabled" : "disabled"}`, next ? "info" : "warning");
		},
	});
}