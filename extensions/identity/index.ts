/**
 * identity -- the agent's working persona, selected per session and injected
 * into the system prompt. The built-in identities cover the situations a
 * security professional moves between: `reverse-engineer`, `forensics`,
 * `software-engineer`, `devops`, `publisher`. A custom identity can be
 * written adhoc in the session (`/identity write <text>` or an external
 * editor) and saved as a named, reusable one (`/identity save <name>`) once
 * it has proven useful.
 *
 * Off by default -- with no selection, nothing is injected and the system
 * prompt stays byte-identical, the same cache property goal-setting's
 * manifest block keeps. The block is appended from `before_agent_start`
 * (chained, order-independent -- see ADR-0024 and docs/pi-api-notes.md), so
 * switching identity mid-session costs exactly one cache invalidation.
 *
 * Storage follows the one-file-per-extension rule: the user's saved
 * identities and the global default live in `~/.pi/agent/pi-identity.json`
 * (`{ "default": "<name>", "user": { "<name>": "<text>" } }`, hand-editable),
 * while the per-session selection and any adhoc custom text ride their own
 * `custom` entry -- the same pattern every state-owning extension here uses
 * (ADR-0009). Built-in texts are code constants: no build step, nothing for
 * the install step to seed, and they cannot drift from the extension that
 * renders them. Nothing here calls `reactor`.
 */

import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { ExtensionAPI, ExtensionCommandContext, SessionManager } from "@earendil-works/pi-coding-agent";
import { getAgentDir } from "@earendil-works/pi-coding-agent";

// ============================================================================
// The built-in identities
// ============================================================================

const BUILTINS: Record<string, string> = {
	"reverse-engineer":
		"You are acting as a reverse engineer. Work from evidence in the artifact -- strings, imports, control flow, runtime behavior -- and keep verified facts separate from inference; label which is which. Prefer the machine's own tooling: read `--help`/`man` pages before invoking a tool, and check what is actually installed before relying on it. Treat unknown code as hostile: no execution outside an isolated environment, and say when isolation is uncertain. Record concrete, reproducible findings -- addresses, offsets, hashes, the exact commands that produced them -- so another analyst can re-derive each one.",
	"cyber-forensics":
		"You are acting as a cyber-forensics analyst: inspecting systems where malware is suspected or known to have executed. Treat every source as evidence -- work on verified copies, never write to originals, record hashes before and after touching anything, and prefer read-only inspection and read-only mounts. Hunt what execution leaves behind: process and service artifacts, event logs, persistence mechanisms, scheduled tasks, staged tooling, and the timeline that ties them together -- noting for each finding the artifact, the exact command that produced it, and the timestamp basis (UTC where possible). Label observation, interpretation, and speculation distinctly -- an artifact being present is not proof malware ran, so say what would confirm it. When sources conflict, report the conflict instead of averaging; when evidence is thin, collect more rather than assuming. Write every step so another analyst can repeat it exactly.",
	forensics:
		"You are acting as a forensic analyst: reconstructing what a user did on a system. Work from the traces user actions leave -- logons and session boundaries, files opened and written, programs run, removable-media activity, browser and shell history, deleted-but-recoverable content -- and build a timeline; note for each step the artifact, the exact command that produced it, and the timestamp basis (UTC where possible). Treat every source as evidence: work on verified copies, never write to originals, record hashes before and after touching anything, and prefer read-only inspection. Distinguish the user's own actions from automated or attacker-driven ones, and label observation, interpretation, and speculation distinctly. When sources conflict, report the conflict instead of averaging; when evidence is thin, collect more rather than assuming. Write every step so another analyst can repeat it exactly.",
	"software-engineer":
		"You are acting as a software engineer. Read before writing: understand the surrounding code, its tests, and its conventions, and match them. Prefer the smallest change that solves the problem -- no drive-by refactors, no new dependencies without need, no rewrites when a fix will do. Run the relevant tests before claiming done, and add the test that would have caught the bug you just fixed. When behavior is ambiguous, prefer the reading that keeps the public contract stable and state what you assumed. Remove dead code you touch, but say what you removed and why.",
	devops:
		"You are acting as a DevOps and infrastructure engineer. Treat production as fragile: inspect current state before changing anything, prefer incremental reversible changes, and know the rollback before you apply anything. Express changes as code (config, manifests, pipelines) and keep them idempotent; never leave one-off shell state behind. Verify after acting -- a zero exit code is not a healthy service; check it the way a client would. Assume everything you touch is shared (DNS, load balancers, databases, CI) and say what you changed and when. If a change needs a maintenance window or a second pair of eyes, stop and say so instead of improvising on live systems.",
	publisher:
		"You are acting as a publisher: turning technical findings into documents people can act on. Structure first -- an executive summary a non-specialist can read, findings in decreasing order of importance, technical detail in an appendix. Every claim traces to an observation from this session; cite the command, log line, or artifact behind it, and mark anything unverified as such. Write for the reader: no unexplained jargon, no walls of text, tables over prose when comparing. Never invent severity, numbers, or quotes -- a shorter report that is entirely true beats a longer one that is partly guessed. Match the requested format, template, and tone exactly.",
};

// ============================================================================
// Types and config
// ============================================================================

const CUSTOM_TYPE = "pi-identity";

interface IdentityConfig {
	/** Global default: a name applied when the session has not chosen one. Empty = off. */
	default: string;
	/** The user's own saved identities. */
	user: Record<string, string>;
}

const DEFAULT_CONFIG: IdentityConfig = { default: "", user: {} };

const GLOBAL_CONFIG_PATH = join(getAgentDir(), "pi-identity.json");

interface SessionState {
	/** The active identity's name: a built-in, a saved user identity, or "custom". Absent = off (config default applies). Empty string = explicitly off. */
	active?: string;
	/** The adhoc custom identity text, for active === "custom". */
	custom?: string;
}

let config: IdentityConfig = { ...DEFAULT_CONFIG };
let state: SessionState = {};

function activeName(): string | undefined {
	if (state.active === undefined) return config.default || undefined;
	return state.active || undefined; // "" is an explicit off
}

function resolveText(name: string | undefined): string | undefined {
	if (!name) return undefined;
	if (name === "custom") {
		const text = state.custom?.trim();
		return text ? text : undefined;
	}
	return BUILTINS[name] ?? config.user[name];
}

// ============================================================================
// Config file
// ============================================================================

function loadGlobalConfig(): void {
	const merged: IdentityConfig = { ...DEFAULT_CONFIG, user: {} };
	try {
		if (existsSync(GLOBAL_CONFIG_PATH)) {
			const raw = JSON.parse(readFileSync(GLOBAL_CONFIG_PATH, "utf8"));
			if (typeof raw.default === "string") merged.default = raw.default;
			if (raw.user && typeof raw.user === "object" && !Array.isArray(raw.user)) {
				for (const [name, text] of Object.entries(raw.user)) {
					if (typeof text === "string") merged.user[name] = text;
				}
			}
		}
	} catch {
		// ignore malformed config, fall back to defaults
	}
	config = merged;
}

function saveGlobalConfig(): void {
	try {
		writeFileSync(GLOBAL_CONFIG_PATH, `${JSON.stringify(config, null, 2)}\n`);
	} catch {
		// the file is a convenience, not a session requirement
	}
}

// ============================================================================
// Session state persistence (in-session `custom` entry)
// ============================================================================

function loadSessionState(sm: SessionManager): void {
	state = {};
	for (const entry of sm.getBranch()) {
		if (entry.type === "custom" && entry.customType === CUSTOM_TYPE) {
			// latest on the branch wins
			const r = entry.data as Record<string, any> | undefined;
			const next: SessionState = {};
			if (r && typeof r === "object") {
				if (typeof r.active === "string") next.active = r.active;
				if (typeof r.custom === "string") next.custom = r.custom;
			}
			state = next;
		}
	}
}

// ============================================================================
// System prompt
// ============================================================================

function identityBlock(): string | undefined {
	const text = resolveText(activeName());
	if (!text) return undefined;
	return `## Identity\n\n${text}`;
}

function availableNames(): string {
	const user = Object.keys(config.user);
	return [...Object.keys(BUILTINS), "custom", ...user].join(", ");
}

// ============================================================================
// External editor (same shape as context-editor's manual mode, no shared
// module -- ADR-0014)
// ============================================================================

function externalEditorCommand(): string {
	return process.env.VISUAL || process.env.EDITOR || (process.platform === "win32" ? "notepad" : "nano");
}

type EditorResult = { status: "complete"; content: string } | { status: "cancelled" };

async function editInExternalEditor(content: string): Promise<EditorResult> {
	const dir = mkdtempSync(join(tmpdir(), "pi-identity-"));
	const filePath = join(dir, "identity.md");
	try {
		writeFileSync(filePath, content, "utf-8");
		const command = externalEditorCommand();
		const [editor, ...args] = command.split(" ");
		const exitCode = await new Promise<number | null>((resolve) => {
			const child = spawn(editor, [...args, filePath], { stdio: "inherit", shell: process.platform === "win32" });
			child.on("error", () => resolve(null));
			child.on("close", (code) => resolve(code));
		});
		if (exitCode !== 0) return { status: "cancelled" };
		return { status: "complete", content: readFileSync(filePath, "utf-8") };
	} finally {
		try {
			rmSync(dir, { recursive: true, force: true });
		} catch {
			// best effort
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
		const name = activeName();
		try {
			ctx.ui.setStatus("identity", name ? `identity: ${name}` : undefined);
		} catch {
			// no terminal -- print and json modes carry no footer
		}
	});

	pi.on("session_shutdown", (_event, ctx) => {
		state = {};
		try {
			ctx.ui.setStatus("identity", undefined);
		} catch {
			// ignore
		}
	});

	// ---- the block into the system prompt ---------------------------------
	pi.on("before_agent_start", (event) => {
		const block = identityBlock();
		if (!block) return undefined;
		return { systemPrompt: `${event.systemPrompt}\n\n${block}` };
	});

	// ---- commands -----------------------------------------------------------
	function select(name: string, ctx: ExtensionCommandContext): void {
		const known = name === "custom" || Boolean(BUILTINS[name] || config.user[name]);
		if (!known) {
			ctx.ui.notify(`identity: unknown identity "${name}" -- available: ${availableNames()}`, "warning");
			return;
		}
		if (name === "custom" && !state.custom?.trim()) {
			ctx.ui.notify('identity: the custom identity is empty -- write one with /identity write <text> or /identity editor', "warning");
			return;
		}
		state = { ...state, active: name };
		pi.appendEntry(CUSTOM_TYPE, state);
		ctx.ui.setStatus("identity", `identity: ${name}`);
		ctx.ui.notify(`identity: ${name}`, "info");
	}

	pi.registerCommand("identity", {
		description:
			"Show the active identity and the available ones (no arg), select one (/identity <name>), switch off (/identity off), view text (/identity show [name]), write an adhoc custom identity (/identity write <text> or /identity editor), save it (/identity save <name>) or delete a saved one (/identity delete <name>).",
		handler: async (args, ctx) => {
			const arg = (args || "").trim();
			if (!arg) {
				const name = activeName();
				const text = resolveText(name);
				const lines = [`identity: ${name ?? "(none)"}`];
				if (name === "custom" && !text) lines.push("  (the custom identity is empty -- write one with /identity write <text>)");
				lines.push(`available: ${availableNames()}`);
				ctx.ui.notify(lines.join("\n"), "info");
				return;
			}
			if (arg === "off") {
				state = { ...state, active: "" };
				pi.appendEntry(CUSTOM_TYPE, state);
				ctx.ui.setStatus("identity", undefined);
				ctx.ui.notify("identity: off", "info");
				return;
			}
			if (arg === "show") {
				const name = activeName();
				const text = resolveText(name);
				ctx.ui.notify(text ? `[${name}]\n${text}` : "identity: none active", "info");
				return;
			}
			if (arg.startsWith("show ")) {
				const name = arg.slice("show ".length).trim();
				const text = resolveText(name);
				ctx.ui.notify(text ? `[${name}]\n${text}` : `identity: no such identity "${name}"`, "info");
				return;
			}
			if (arg.startsWith("write ")) {
				const text = arg.slice("write ".length).trim();
				if (!text) {
					ctx.ui.notify("usage: /identity write <text>", "warning");
					return;
				}
				state = { ...state, active: "custom", custom: text };
				pi.appendEntry(CUSTOM_TYPE, state);
				ctx.ui.setStatus("identity", "identity: custom");
				ctx.ui.notify("identity: custom set -- save it with /identity save <name> if it proves useful", "info");
				return;
			}
			if (arg === "editor") {
				if (ctx.mode !== "tui") {
					ctx.ui.notify("identity: the editor needs a terminal; use /identity write <text> here", "warning");
					return;
				}
				const result = await ctx.ui.custom<EditorResult>((tui, _theme, _keybindings, done) => {
					void (async () => {
						tui.stop();
						let outcome: EditorResult;
						try {
							outcome = await editInExternalEditor(state.custom ?? "");
						} catch {
							outcome = { status: "cancelled" };
						} finally {
							tui.start();
							tui.requestRender(true);
						}
						done(outcome);
					})();
					// Nothing is drawn -- the editor has the terminal; done()
					// fires when it exits, same shape as the shell example.
					return { render: () => [], invalidate: () => {} };
				});
				if (result.status === "cancelled") {
					ctx.ui.notify("identity: editor exited without saving -- cancelled", "info");
					return;
				}
				const text = result.content.trim();
				if (!text) {
					ctx.ui.notify("identity: editor content was empty -- nothing set", "warning");
					return;
				}
				state = { ...state, active: "custom", custom: text };
				pi.appendEntry(CUSTOM_TYPE, state);
				ctx.ui.setStatus("identity", "identity: custom");
				ctx.ui.notify("identity: custom set -- save it with /identity save <name> if it proves useful", "info");
				return;
			}
			if (arg === "save" || arg.startsWith("save ")) {
				const name = arg === "save" ? "" : arg.slice("save ".length).trim();
				if (!name) {
					ctx.ui.notify("usage: /identity save <name>", "warning");
					return;
				}
				if (BUILTINS[name]) {
					ctx.ui.notify(`identity: "${name}" is built in -- pick another name`, "warning");
					return;
				}
				const text = state.active === "custom" ? state.custom?.trim() : undefined;
				if (!text) {
					ctx.ui.notify("identity: only an adhoc custom identity can be saved -- write one with /identity write <text>", "warning");
					return;
				}
				const existed = Boolean(config.user[name]);
				config.user[name] = text;
				saveGlobalConfig();
				ctx.ui.notify(`identity: saved "${name}"${existed ? " (overwrote an existing one)" : ""}`, "info");
				return;
			}
			if (arg.startsWith("delete ")) {
				const name = arg.slice("delete ".length).trim();
				if (BUILTINS[name]) {
					ctx.ui.notify(`identity: "${name}" is built in -- it cannot be deleted`, "warning");
					return;
				}
				if (!config.user[name]) {
					ctx.ui.notify(`identity: no saved identity "${name}"`, "warning");
					return;
				}
				delete config.user[name];
				saveGlobalConfig();
				ctx.ui.notify(`identity: deleted "${name}"`, "info");
				return;
			}
			select(arg, ctx);
		},
	});
}