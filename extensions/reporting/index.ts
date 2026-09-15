/**
 * reporting -- keep an agent honest about writing up findings as it goes,
 * instead of reconstructing a report from memory once the analysis is done.
 *
 * Three enforcement levels, all opt-in and off by default:
 *   0. system prompt only -- one short block telling the agent to document
 *      findings in the configured folder.
 *   1. the same block, plus: once the agent has gone `stepThreshold` tool
 *      calls without the folder changing, every subsequent LLM call gets an
 *      extra reminder message until it changes again.
 *   2. the same tracking, but once the threshold is crossed and the turn
 *      settles without a folder change, the turn is reverted -- pi's own
 *      session-tree navigation, not a soft nag -- and the same prompt is
 *      resent with a demand appended. Repeats up to `maxReverts` times before
 *      falling back to level-1-style nagging, so a session can never spin
 *      forever on a model that will not comply.
 *
 * "Did the folder change" is answered by asking the filesystem -- a snapshot
 * of every file's size and mtime under the folder, diffed on each tool call
 * -- rather than by inspecting which tool ran or what path it touched. That
 * means a `bash` redirect, `git checkout`, or a hand edit in another window
 * all count exactly the same as the built-in `edit`/`write` tools: nothing is
 * special-cased, the folder is just asked what is actually there. Same stance
 * `status/` already takes for services ("ask the machine"), not the
 * command-line-guessing heuristic ADR-0007 rejected for tool enforcement --
 * see docs/adr/0023.
 *
 * Level 2's revert leans on two facts about pi verified directly against its
 * shipped `dist/` (docs/pi-api-notes.md carries both):
 *
 *   - `pi.sendUserMessage(text, { expandPromptTemplates: true })` dispatches
 *     a registered command with a real, session-tree-capable
 *     `ExtensionCommandContext`, even when called from inside a plain event
 *     handler -- `AgentSession.prompt()` checks for a leading "/" and hands
 *     off to `_tryExecuteExtensionCommand` *before* it even looks at whether
 *     the agent is streaming. That is the only way this extension reaches
 *     `navigateTree`, which ordinary event handlers are not given.
 *   - `navigateTree` throws if the agent is still streaming, so the revert
 *     has to be triggered from `agent_settled` (fired once the whole turn has
 *     genuinely finished) and never from `turn_end` (fired mid-loop, while a
 *     multi-turn tool-calling run is often still active).
 */

import { getAgentDir } from "@earendil-works/pi-coding-agent";
import { glyph, lead } from "../lib/statusbar.ts";
import type {
	AgentSettledEvent,
	BeforeAgentStartEvent,
	BeforeAgentStartEventResult,
	ContextEvent,
	ContextEventResult,
	ExtensionAPI,
	ExtensionCommandContext,
	ExtensionContext,
	SessionStartEvent,
	Theme,
	ToolExecutionEndEvent,
} from "@earendil-works/pi-coding-agent";
import { mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

// ============================================================================
// Config: global knobs (own file, unrelated to reactor.json's toolbox/
// hiddenServices pair -- ADR-0016) plus per-session enabled/level, riding a
// custom entry the same way rolling-context's SessionState does.
// ============================================================================

type Level = 0 | 1 | 2;

interface ReportingConfig {
	level: Level;
	folder: string;
	stepThreshold: number;
	maxReverts: number;
	templatePath: string | null;
}

const DEFAULT_CONFIG: ReportingConfig = {
	level: 0,
	folder: "report",
	stepThreshold: 8,
	maxReverts: 3,
	templatePath: null,
};

const GLOBAL_CONFIG_PATH = () => join(getAgentDir(), "pi-reactor-reporting.json");

function loadGlobalConfig(): ReportingConfig {
	const merged: ReportingConfig = { ...DEFAULT_CONFIG };
	try {
		const raw = JSON.parse(readFileSync(GLOBAL_CONFIG_PATH(), "utf8"));
		if (raw && typeof raw === "object") {
			if ([0, 1, 2].includes(raw.level)) merged.level = raw.level;
			if (typeof raw.folder === "string" && raw.folder.trim()) merged.folder = raw.folder;
			if (typeof raw.stepThreshold === "number" && raw.stepThreshold > 0) merged.stepThreshold = raw.stepThreshold;
			if (typeof raw.maxReverts === "number" && raw.maxReverts >= 0) merged.maxReverts = raw.maxReverts;
			if (typeof raw.templatePath === "string" && raw.templatePath.trim()) merged.templatePath = raw.templatePath;
		}
	} catch {
		// absent or unreadable both mean "use the defaults" -- a preference file, not a contract.
	}
	return merged;
}

function writeGlobalConfig(patch: Partial<ReportingConfig>): void {
	const dir = getAgentDir();
	mkdirSync(dir, { recursive: true });
	const merged = { ...loadGlobalConfig(), ...patch };
	writeFileSync(GLOBAL_CONFIG_PATH(), `${JSON.stringify(merged, null, 2)}\n`);
}

const ENTRY_TYPE = "reactor-reporting";

interface SessionState {
	enabled?: boolean;
	level?: Level;
}

function normalizeSession(raw: unknown): SessionState {
	if (!raw || typeof raw !== "object") return {};
	const r = raw as Record<string, unknown>;
	const out: SessionState = {};
	if (typeof r.enabled === "boolean") out.enabled = r.enabled;
	if (r.level === 0 || r.level === 1 || r.level === 2) out.level = r.level;
	return out;
}

const STATUS_KEY = "reactor-reporting";

// ============================================================================
// Folder snapshot -- the only "was it documented" signal this extension has.
// ============================================================================

type Snapshot = Map<string, { size: number; mtimeMs: number }>;

/** Recursive file listing under `dir`, size + mtime per file. Missing or unreadable reads as empty -- a folder that does not exist yet has nothing in it, not an error. */
function takeSnapshot(dir: string): Snapshot {
	const out: Snapshot = new Map();
	function walk(d: string, prefix: string): void {
		let entries: import("node:fs").Dirent[];
		try {
			entries = readdirSync(d, { withFileTypes: true });
		} catch {
			return;
		}
		for (const e of entries) {
			const full = join(d, e.name);
			const rel = prefix ? `${prefix}/${e.name}` : e.name;
			if (e.isDirectory()) {
				walk(full, rel);
			} else if (e.isFile()) {
				try {
					const st = statSync(full);
					out.set(rel, { size: st.size, mtimeMs: st.mtimeMs });
				} catch {
					// A file that vanished between readdir and stat (race with the
					// agent's own write) is not worth failing the check over.
				}
			}
		}
	}
	walk(dir, "");
	return out;
}

function snapshotsEqual(a: Snapshot, b: Snapshot): boolean {
	if (a.size !== b.size) return false;
	for (const [rel, meta] of a) {
		const other = b.get(rel);
		if (!other || other.size !== meta.size || other.mtimeMs !== meta.mtimeMs) return false;
	}
	return true;
}

// ============================================================================
// Runtime state -- module-level, rebuilt on session_start the same way
// rolling-context's does, since none of it belongs in LLM context.
// ============================================================================

let config: ReportingConfig = { ...DEFAULT_CONFIG };
let session: SessionState = {};
/** undefined until the first check -- that first call establishes a baseline rather than crediting whatever was already there. */
let snapshot: Snapshot | undefined;
let stepsSinceChange = 0;
let revertsThisTurn = 0;
let maxRevertsWarned = false;
/** The last genuinely user-submitted prompt text -- what level 2 re-sends. Not updated while a revert's own resend is in flight (see `pendingRevert`). */
let basePrompt = "";
/** True from the moment a revert's resend is dispatched until before_agent_start observes it, so that handler does not mistake the resend for a fresh prompt and overwrite basePrompt or reset the counters. */
let pendingRevert = false;

function isEnabled(): boolean {
	return session.enabled ?? false;
}

function level(): Level {
	return session.level ?? config.level;
}

function folderPath(ctx: ExtensionContext): string {
	return resolve(ctx.cwd, config.folder);
}

/**
 * One line for the footer -- undefined clears it, same contract `ctx.ui.setStatus`
 * documents everywhere else. Same vocabulary as the rest of the statusbar
 * (each extension carries its own copy -- ADR-0014): a glyph in the anchor
 * colour marks the block, the level is the one word that changes behaviour,
 * so `strict` takes the warning colour.
 */
function statusText(t: Theme): string | undefined {
	if (!isEnabled()) return undefined;
	const lvl = level();
	if (lvl === 0) return `${lead(t)}${glyph(t, "¶")} reporting`;
	const word = lvl === 1 ? "low" : "strict";
	return `${lead(t)}${glyph(t, "¶")} reporting ${t.fg("dim", "·")} ${t.fg(lvl === 2 ? "warning" : "muted", word)}`;
}

function reportingBlock(): string {
	const lines = [
		"## Reporting mode",
		`Document findings as you go in \`${config.folder}/\` -- short, factual, plainly written; cite where in the target each finding came from (file:line, function/address, packet #, timestamp, ...). See the \`reactor-reporting\` skill for structure and style.`,
	];
	if (config.templatePath) lines.push(`Use the report structure in \`${config.templatePath}\`.`);
	return lines.join("\n");
}

export default function reporting(pi: ExtensionAPI) {
	// ---- session lifecycle --------------------------------------------------
	pi.on("session_start", (_event: SessionStartEvent, ctx: ExtensionContext) => {
		config = loadGlobalConfig();
		session = {};
		for (const entry of ctx.sessionManager.getBranch()) {
			if (entry.type === "custom" && entry.customType === ENTRY_TYPE) session = normalizeSession(entry.data);
		}
		snapshot = undefined;
		stepsSinceChange = 0;
		revertsThisTurn = 0;
		maxRevertsWarned = false;
		pendingRevert = false;
		basePrompt = "";
		ctx.ui.setStatus(STATUS_KEY, statusText(ctx.ui.theme));
	});

	function setSession(next: SessionState): void {
		session = next;
		pi.appendEntry(ENTRY_TYPE, session);
	}

	// ---- the system-prompt block, and capturing the real prompt to re-send ---
	pi.on(
		"before_agent_start",
		(event: BeforeAgentStartEvent, _ctx: ExtensionContext): BeforeAgentStartEventResult | void => {
			if (pendingRevert) {
				// This turn is our own resend, not a fresh instruction from the
				// user -- keep basePrompt as the *original* prompt so a second
				// revert (if needed) re-sends that, not an already-demand-appended
				// copy, and leave revertsThisTurn alone.
				pendingRevert = false;
			} else {
				basePrompt = event.prompt;
				revertsThisTurn = 0;
				maxRevertsWarned = false;
			}
			if (!isEnabled()) return;
			return { systemPrompt: `${event.systemPrompt}\n\n${reportingBlock()}` };
		},
	);

	// ---- the folder probe -- one step, one check -----------------------------
	pi.on("tool_execution_end", (_event: ToolExecutionEndEvent, ctx: ExtensionContext) => {
		if (!isEnabled()) return;
		const next = takeSnapshot(folderPath(ctx));
		if (snapshot === undefined) {
			snapshot = next;
			return;
		}
		if (!snapshotsEqual(snapshot, next)) {
			snapshot = next;
			stepsSinceChange = 0;
			revertsThisTurn = 0;
			maxRevertsWarned = false;
		} else {
			stepsSinceChange++;
		}
	});

	// ---- level 1: nag mid-loop, every LLM call, until it changes -------------
	pi.on("context", (event: ContextEvent, _ctx: ExtensionContext): ContextEventResult | void => {
		if (!isEnabled() || level() < 1) return;
		if (stepsSinceChange < config.stepThreshold) return;
		const nag = {
			role: "custom" as const,
			customType: "reactor-reporting-nag",
			content:
				`[reactor-reporting] ${stepsSinceChange} step(s) since \`${config.folder}/\` last changed ` +
				`(threshold ${config.stepThreshold}). Stop and document your findings there now, per the ` +
				"reactor-reporting skill, before doing anything else.",
			display: true,
			timestamp: new Date().toISOString(),
		};
		return { messages: [...event.messages, nag] };
	});

	// ---- level 2: revert once the turn has genuinely settled -----------------
	pi.on("agent_settled", (_event: AgentSettledEvent, ctx: ExtensionContext) => {
		if (!isEnabled() || level() < 2) return;
		if (stepsSinceChange < config.stepThreshold) return;
		if (revertsThisTurn >= config.maxReverts) {
			if (!maxRevertsWarned) {
				maxRevertsWarned = true;
				ctx.ui.notify(
					`reactor-reporting: gave up reverting after ${config.maxReverts} attempt(s) -- nagging instead. ` +
						`Document in ${config.folder}/ to clear it.`,
					"warning",
				);
			}
			return;
		}
		revertsThisTurn++;
		// Fire-and-forget by design (ExtensionAPI.sendUserMessage returns void):
		// this dispatches /reactor-report-enforce with a real
		// ExtensionCommandContext (see the module doc comment). The command
		// handler does the actual navigateTree + resend.
		pi.sendUserMessage("/reactor-report-enforce", { expandPromptTemplates: true });
	});

	// ---- the internal command level 2 self-dispatches ------------------------
	pi.registerCommand("reactor-report-enforce", {
		description:
			"(internal) reactor-reporting: revert the last turn and re-demand documentation. Not meant to be run by hand.",
		handler: async (_args: string, ctx: ExtensionCommandContext) => {
			const branch = ctx.sessionManager.getBranch();
			let targetId: string | undefined;
			for (let i = branch.length - 1; i >= 0; i--) {
				const entry = branch[i];
				if (entry.type === "message" && entry.message.role === "user") {
					targetId = entry.id;
					break;
				}
			}
			if (!targetId) return; // Nothing to revert to -- the level-1 nag still applies next context call.

			await ctx.navigateTree(targetId, { label: "reactor-reporting: reverted -- undocumented" });

			const demand =
				`${basePrompt}\n\n[reactor-reporting] You did not document your findings in ${config.folder}/ ` +
				"before finishing that turn. Do it now, then continue.";
			pendingRevert = true;
			pi.sendUserMessage(demand, { expandPromptTemplates: true });
		},
	});

	// ---- /report --------------------------------------------------------------
	pi.registerCommand("report", {
		description: "REactor: turn reporting mode on/off, set its enforcement level, or check status",
		getArgumentCompletions: (prefix: string) => {
			const parts = prefix.split(/\s+/);
			if (parts[0] === "level") {
				return ["0", "1", "2"]
					.filter((l) => l.startsWith(parts[1] ?? ""))
					.map((l) => ({ value: `level ${l}`, label: l }));
			}
			return ["on", "off", "level", "status", "folder", "reset"]
				.filter((c) => c.startsWith(prefix))
				.map((c) => ({ value: c, label: c }));
		},
		handler: async (args: string, ctx: ExtensionCommandContext) => {
			const [sub, ...rest] = args.trim().split(/\s+/).filter(Boolean);

			switch (sub ?? "status") {
				case "on":
					setSession({ ...session, enabled: true });
					ctx.ui.setStatus(STATUS_KEY, statusText(ctx.ui.theme));
					ctx.ui.notify(`reactor-reporting: on, level ${level()}`, "info");
					return;

				case "off":
					setSession({ ...session, enabled: false });
					ctx.ui.setStatus(STATUS_KEY, undefined);
					ctx.ui.notify("reactor-reporting: off", "info");
					return;

				case "level": {
					const n = Number(rest[0]);
					if (n !== 0 && n !== 1 && n !== 2) {
						ctx.ui.notify("reactor-reporting: level needs 0, 1, or 2", "error");
						return;
					}
					setSession({ enabled: true, level: n });
					ctx.ui.setStatus(STATUS_KEY, statusText(ctx.ui.theme));
					ctx.ui.notify(`reactor-reporting: on, level ${n}`, "info");
					return;
				}

				case "status":
					ctx.ui.notify(
						isEnabled()
							? `reactor-reporting: on, level ${level()}, folder "${config.folder}", ` +
									`${stepsSinceChange}/${config.stepThreshold} step(s) since it last changed`
							: "reactor-reporting: off",
						"info",
					);
					return;

				case "folder": {
					const f = rest.join(" ");
					if (!f) {
						ctx.ui.notify("reactor-reporting: folder needs a path, e.g. `folder report`", "error");
						return;
					}
					writeGlobalConfig({ folder: f });
					config = loadGlobalConfig();
					// A different folder means whatever was counted against the old
					// one is meaningless -- re-baseline and start the count over,
					// same as /report reset.
					snapshot = undefined;
					stepsSinceChange = 0;
					revertsThisTurn = 0;
					maxRevertsWarned = false;
					ctx.ui.notify(`reactor-reporting: folder set to "${f}"`, "info");
					return;
				}

				case "reset":
					stepsSinceChange = 0;
					revertsThisTurn = 0;
					maxRevertsWarned = false;
					snapshot = undefined;
					ctx.ui.notify("reactor-reporting: counters reset", "info");
					return;

				default:
					ctx.ui.notify(
						`reactor-reporting: unknown subcommand "${sub}" -- try on, off, level <0|1|2>, status, folder <path>, or reset`,
						"error",
					);
			}
		},
	});
}
