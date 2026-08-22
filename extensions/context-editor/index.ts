/**
 * context-editor -- let the person at the keyboard decide what the agent
 * sees, not just watch the manifest rolling-context builds automatically.
 *
 * Two views onto the same thing: the *landscape* (`/context-editor`) lists
 * every context-visible entry -- one row per user/assistant/tool-result turn
 * -- and space toggles whether it stays visible. The *manual* view
 * (`/context-editor manual`) dumps the same entries as one text file and
 * opens it in whatever `$VISUAL`/`$EDITOR` is configured (vi, nano, vim,
 * nvim, helix, ...); deleting a block hides that entry the same way a
 * landscape toggle does, and editing a plain-text block's body replaces its
 * content (fork mode only -- see below).
 *
 * Both views end the same way: a choice of how the edit takes effect.
 *   - **Fork** (default) -- `ctx.newSession()` builds a brand-new session
 *     containing only the entries kept, in order. Nothing about the current
 *     session is touched; the fork is a fresh branch with the new context.
 *   - **Current branch** -- pi's session store is append-only by design
 *     (`SessionManager`'s own doc comment: "Entries cannot be modified or
 *     deleted"), so there is no literal in-place rewrite to do. This applies
 *     the edit as a standing filter instead: hidden entry ids are persisted
 *     in the session (the same `custom`-entry-on-session_start pattern
 *     `rolling-context/` and `scenario/` use) and a `context` hook drops
 *     them from every future prompt. The session file itself is untouched --
 *     nothing is lost, only unsent, the same guarantee rolling-context makes
 *     for its own fade. Because nothing can be rewritten in place, text
 *     edits from the manual view only ever apply through fork; current-branch
 *     mode can hide entries but not reword them, and says so.
 *
 * General-purpose, not RE- or rolling-context-specific -- it happens to be
 * useful for exactly the same reason rolling-context is (ADR-0019): a
 * disassembly listing or packet capture the agent pasted in five turns ago
 * is exactly the kind of thing a person wants to point at and say "not that
 * one" without waiting for an automatic fade to get around to it.
 */

import type {
	ExtensionAPI,
	ExtensionCommandContext,
	SessionEntry,
	SessionManager,
	Theme,
} from "@earendil-works/pi-coding-agent";
import { sessionEntryToContextMessages } from "@earendil-works/pi-coding-agent";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { Component, TUI } from "@earendil-works/pi-tui";
import { getKeybindings, truncateToWidth } from "@earendil-works/pi-tui";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const CUSTOM_TYPE = "pi-context-editor";

interface EditorState {
	/** Entry ids hidden from context in "current branch" mode. */
	hiddenIds: string[];
}

let state: EditorState = { hiddenIds: [] };

function loadState(sm: SessionManager): void {
	state = { hiddenIds: [] };
	for (const entry of sm.getBranch()) {
		if (entry.type === "custom" && entry.customType === CUSTOM_TYPE) {
			const raw = entry.data as Partial<EditorState> | undefined;
			state = { hiddenIds: Array.isArray(raw?.hiddenIds) ? raw!.hiddenIds.filter((x) => typeof x === "string") : [] };
		}
	}
}

// ============================================================================
// Rows: one per context-visible message-bearing entry
// ============================================================================

interface Row {
	entryId: string;
	role: string;
	snippet: string;
	body: string;
	/** For a toolResult row, the toolCallId it answers. */
	toolCallId?: string;
	/** For an assistant row, the ids of the tool calls it makes. */
	toolCallIds: string[];
}

function textOf(content: unknown): string {
	if (typeof content === "string") return content;
	if (Array.isArray(content)) {
		return content
			.filter((b: any) => b?.type === "text")
			.map((b: any) => b.text)
			.join("\n");
	}
	return "";
}

function bodyOf(message: AgentMessage): string {
	switch (message.role) {
		case "user":
		case "toolResult":
		case "custom":
			return textOf((message as any).content);
		case "assistant": {
			const parts: string[] = [];
			for (const block of (message as any).content ?? []) {
				if (block.type === "text") parts.push(block.text);
				else if (block.type === "thinking") parts.push(`[thinking] ${block.thinking}`);
				else if (block.type === "toolCall") parts.push(`[tool call ${block.name}] ${JSON.stringify(block.arguments)}`);
			}
			return parts.join("\n");
		}
		case "bashExecution":
			return `$ ${(message as any).command}\n${(message as any).output}`;
		default:
			return "";
	}
}

/** Only these roles/shapes can have their body text safely rewritten and replayed. */
function isTextEditable(message: AgentMessage): boolean {
	if (message.role === "user" || message.role === "custom") {
		const c = (message as any).content;
		return typeof c === "string" || (Array.isArray(c) && c.every((b: any) => b.type === "text"));
	}
	if (message.role === "toolResult") {
		const c = (message as any).content;
		return Array.isArray(c) && c.every((b: any) => b.type === "text");
	}
	if (message.role === "assistant") {
		const c = (message as any).content;
		return Array.isArray(c) && c.length === 1 && c[0]?.type === "text";
	}
	return false;
}

function withBody(message: AgentMessage, body: string): AgentMessage {
	if (typeof (message as any).content === "string") return { ...message, content: body } as AgentMessage;
	return { ...message, content: [{ type: "text", text: body }] } as AgentMessage;
}

/** Build one row per context-visible message/custom_message entry. Tool calls and their results stay separate rows so they can be closed over (see closeHidden). */
function buildRows(entries: SessionEntry[]): Row[] {
	const rows: Row[] = [];
	for (const entry of entries) {
		if (entry.type !== "message" && entry.type !== "custom_message") continue;
		const messages = sessionEntryToContextMessages(entry);
		for (const message of messages) {
			const body = bodyOf(message);
			const toolCallIds = message.role === "assistant" ? ((message as any).content ?? []).filter((b: any) => b.type === "toolCall").map((b: any) => b.id) : [];
			rows.push({
				entryId: entry.id,
				role: message.role,
				snippet: (body.split("\n").find((l) => l.trim() !== "") ?? "").slice(0, 100),
				body,
				toolCallId: message.role === "toolResult" ? (message as any).toolCallId : undefined,
				toolCallIds,
			});
		}
	}
	return rows;
}

/**
 * Extend a hidden-id set so a tool call and its result are always hidden
 * together -- hiding a giant tool result without also hiding its (usually
 * tiny) call, or vice versa, would send an orphaned half of the pair and the
 * backend would reject the request. Same concern rolling-context's cut point
 * has to respect, just as a closure over an explicit id set here instead of
 * a budget walk.
 */
function closeHidden(rows: Row[], hidden: Set<string>): Set<string> {
	const closed = new Set(hidden);
	const callOwner = new Map<string, string>(); // toolCallId -> entryId that made it
	const resultOwner = new Map<string, string>(); // toolCallId -> entryId of its result
	for (const row of rows) {
		for (const id of row.toolCallIds) callOwner.set(id, row.entryId);
		if (row.toolCallId) resultOwner.set(row.toolCallId, row.entryId);
	}
	let changed = true;
	while (changed) {
		changed = false;
		for (const [callId, callEntry] of callOwner) {
			const resultEntry = resultOwner.get(callId);
			if (!resultEntry) continue;
			const anyHidden = closed.has(callEntry) || closed.has(resultEntry);
			if (anyHidden && !(closed.has(callEntry) && closed.has(resultEntry))) {
				closed.add(callEntry);
				closed.add(resultEntry);
				changed = true;
			}
		}
	}
	return closed;
}

// ============================================================================
// Apply: fork a new session, or persist a filter on this one
// ============================================================================

type ApplyTarget = "fork" | "branch";

const APPLY_OPTIONS: Record<ApplyTarget, string> = {
	fork: "Fork branch with new context (default)",
	branch: "Apply to current branch (in place)",
};

async function promptApplyTarget(ctx: ExtensionCommandContext): Promise<ApplyTarget | undefined> {
	const choice = await ctx.ui.select("How should this edit take effect?", [APPLY_OPTIONS.fork, APPLY_OPTIONS.branch]);
	if (choice === APPLY_OPTIONS.branch) return "branch";
	if (choice === APPLY_OPTIONS.fork) return "fork";
	return undefined;
}

interface Edit {
	hiddenIds: Set<string>;
	textEdits: Map<string, string>;
}

async function applyEdit(pi: ExtensionAPI, ctx: ExtensionCommandContext, entries: SessionEntry[], rows: Row[], edit: Edit): Promise<void> {
	const hidden = closeHidden(rows, edit.hiddenIds);
	if (hidden.size === 0 && edit.textEdits.size === 0) {
		ctx.ui.notify("context-editor: nothing changed", "info");
		return;
	}

	const target = await promptApplyTarget(ctx);
	if (!target) return; // cancelled

	if (edit.textEdits.size > 0 && target === "branch") {
		ctx.ui.notify(
			"context-editor: text edits only take effect via fork -- pi's session store is append-only, so an entry's wording cannot change in place. Hiding still applies to the current branch.",
			"warning",
		);
	}

	if (target === "fork") {
		// ctx.newSession() invalidates this ctx (and the closed-over pi) the
		// moment it resolves (docs/pi-api-notes.md), so it has to be the last
		// thing this branch does -- no ctx.ui.notify() after it. The new
		// session becomes the active one, which is confirmation enough.
		await ctx.newSession({
			setup: async (sm: SessionManager) => {
				for (const entry of entries) {
					if (hidden.has(entry.id)) continue;
					if (entry.type !== "message" && entry.type !== "custom_message") continue;
					for (const message of sessionEntryToContextMessages(entry)) {
						const edited = edit.textEdits.get(entry.id);
						const toAppend = edited !== undefined && isTextEditable(message) ? withBody(message, edited) : message;
						if (toAppend.role === "branchSummary" || toAppend.role === "compactionSummary") continue;
						sm.appendMessage(toAppend as any);
					}
				}
			},
		});
		return;
	}

	// target === "branch": persist the hidden set; the context hook below
	// enforces it on every future prompt. Nothing is written to entries --
	// only a pointer to which ones to leave out next time.
	state = { hiddenIds: [...hidden] };
	pi.appendEntry(CUSTOM_TYPE, state);
	ctx.ui.notify(`context-editor: ${hidden.size} entr${hidden.size === 1 ? "y" : "ies"} hidden from this branch going forward`, "info");
}

// ============================================================================
// Manual view: serialize to a file, open $VISUAL/$EDITOR, parse the result
// ============================================================================

const BLOCK_HEADER = /^=== \[(.+?)\] (\S+) ===$/;
const HIDDEN_MARKER = "[HIDDEN -- delete this block to keep it hidden, or replace this line with real content to reveal it]";

/**
 * Rows already hidden (from a previous /context-editor apply to this branch)
 * are written as a marker, not their real body -- otherwise leaving that
 * block untouched (the natural thing to do with content you already decided
 * to hide) would silently reveal it again the moment the file round-trips,
 * since "block present with its original body" is indistinguishable from
 * "never touched". The marker makes "still hidden" and "now visible with
 * this content" two different, visible states of the same block.
 */
function serializeForEditing(rows: Row[], hidden: Set<string>): string {
	const parts = [
		"# context-editor -- manual edit",
		"#",
		"# Delete an entire block (its \"=== [id] role ===\" header through the blank",
		"# line before the next header) to hide that entry.",
		"#",
		"# Editing a block's body text replaces its content, but only takes effect",
		"# when applied via fork -- pi's session store cannot be rewritten in place,",
		"# so a current-branch apply can hide entries but not reword them.",
		"#",
		"# Save and exit to apply what you changed here; quit without saving (or",
		"# leave the file untouched) to cancel.",
		"",
	];
	for (const row of rows) {
		parts.push(`=== [${row.entryId}] ${row.role} ===`);
		parts.push(hidden.has(row.entryId) ? HIDDEN_MARKER : row.body);
		parts.push("");
	}
	return parts.join("\n");
}

function parseEdited(text: string, rows: Row[]): Edit {
	const present = new Map<string, string[]>();
	let currentId: string | undefined;
	for (const line of text.split("\n")) {
		const match = BLOCK_HEADER.exec(line);
		if (match) {
			currentId = match[1];
			present.set(currentId, []);
			continue;
		}
		if (currentId !== undefined) present.get(currentId)!.push(line);
	}

	const hiddenIds = new Set<string>();
	const textEdits = new Map<string, string>();
	for (const row of rows) {
		const lines = present.get(row.entryId);
		if (lines === undefined) {
			hiddenIds.add(row.entryId);
			continue;
		}
		// Body is everything up to the trailing blank separator line this file
		// always writes between blocks.
		while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
		const body = lines.join("\n");
		if (body === HIDDEN_MARKER) {
			hiddenIds.add(row.entryId);
			continue;
		}
		if (body !== row.body) textEdits.set(row.entryId, body);
	}
	return { hiddenIds, textEdits };
}

/** Same resolution order pi's own external-editor prompt uses. */
function externalEditorCommand(): string {
	return process.env.VISUAL || process.env.EDITOR || (process.platform === "win32" ? "notepad" : "nano");
}

type EditorResult = { status: "complete"; content: string } | { status: "cancelled" };

async function editInExternalEditor(content: string): Promise<EditorResult> {
	const dir = mkdtempSync(join(tmpdir(), "pi-context-editor-"));
	const filePath = join(dir, "context.md");
	try {
		writeFileSync(filePath, content, "utf-8");
		const command = externalEditorCommand();
		const [editor, ...args] = command.split(" ");
		process.stdout.write(`Launching external editor: ${command}\nPi will resume when it exits.\n`);
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
// Landscape view: a Component overlay, one row per entry, space to toggle
// ============================================================================

class LandscapeOverlay implements Component {
	private index = 0;
	private hidden: Set<string>;
	private detailFor: string | undefined;

	constructor(
		private tui: TUI,
		private theme: Theme,
		private done: (result: Edit | undefined) => void,
		private rows: Row[],
		initiallyHidden: Set<string>,
	) {
		this.hidden = new Set(initiallyHidden);
	}

	invalidate(): void {}

	handleInput(data: string): void {
		const kb = getKeybindings();

		if (this.detailFor !== undefined) {
			if (kb.matches(data, "tui.select.cancel") || kb.matches(data, "tui.select.confirm")) this.detailFor = undefined;
			this.tui.requestRender();
			return;
		}

		if (kb.matches(data, "tui.select.cancel")) {
			this.done(undefined);
			return;
		}
		if (kb.matches(data, "tui.select.up")) {
			this.index = this.index === 0 ? Math.max(0, this.rows.length - 1) : this.index - 1;
		} else if (kb.matches(data, "tui.select.down")) {
			this.index = this.index >= this.rows.length - 1 ? 0 : this.index + 1;
		} else if (data === " ") {
			const row = this.rows[this.index];
			if (row) {
				if (this.hidden.has(row.entryId)) this.hidden.delete(row.entryId);
				else this.hidden.add(row.entryId);
			}
		} else if (kb.matches(data, "tui.select.confirm")) {
			if (this.rows[this.index]) this.detailFor = this.rows[this.index].entryId;
		} else if (data === "a") {
			this.done({ hiddenIds: this.hidden, textEdits: new Map() });
			return;
		}
		this.tui.requestRender();
	}

	render(width: number): string[] {
		const t = this.theme;
		if (this.detailFor !== undefined) {
			const row = this.rows.find((r) => r.entryId === this.detailFor);
			const lines = [
				t.bold(t.fg("accent", `  [${row?.role}]`)),
				"",
				...(row?.body.split("\n").map((l) => `  ${l}`) ?? []),
				"",
				t.fg("dim", "  esc/enter back"),
			];
			return lines.map((l) => truncateToWidth(l, width));
		}

		const lines = [t.bold(`  context editor -- ${this.rows.length} entr${this.rows.length === 1 ? "y" : "ies"}, ${this.hidden.size} hidden`), ""];
		this.rows.forEach((row, i) => {
			const selected = i === this.index;
			const hidden = this.hidden.has(row.entryId);
			const box = hidden ? t.fg("dim", "[ ]") : t.fg("success", "[x]");
			const prefix = selected ? t.fg("accent", "→ ") : "  ";
			const role = row.role.padEnd(11);
			const snippet = t.fg(hidden ? "dim" : "muted", row.snippet || "(empty)");
			lines.push(truncateToWidth(`${prefix}${box} ${selected ? t.fg("accent", role) : role} ${snippet}`, width));
		});
		lines.push("");
		lines.push(truncateToWidth(t.fg("dim", "  space hide/show · enter view · a apply · esc cancel"), width));
		return lines;
	}
}

// ============================================================================
// Extension
// ============================================================================

export default function (pi: ExtensionAPI) {
	pi.on("session_start", (_event, ctx) => {
		loadState(ctx.sessionManager);
	});

	// ---- enforce the persisted hidden set on the current branch -----------
	pi.on("context", (event, ctx) => {
		if (state.hiddenIds.length === 0) return;
		const entries = ctx.sessionManager.buildContextEntries();
		const rows = buildRows(entries);
		const hidden = closeHidden(rows, new Set(state.hiddenIds));

		const keptEntries = entries.filter((e) => !hidden.has(e.id));
		const messages: AgentMessage[] = [];
		for (const entry of keptEntries) {
			if (entry.type !== "message" && entry.type !== "custom_message" && entry.type !== "branch_summary" && entry.type !== "compaction") continue;
			messages.push(...sessionEntryToContextMessages(entry));
		}
		return { messages };
	});

	async function openEditor(ctx: ExtensionCommandContext, mode: "landscape" | "manual"): Promise<void> {
		const entries = ctx.sessionManager.buildContextEntries();
		const rows = buildRows(entries);
		if (rows.length === 0) {
			ctx.ui.notify("context-editor: nothing in context yet", "info");
			return;
		}

		let edit: Edit | undefined;
		if (mode === "landscape") {
			if (ctx.mode !== "tui") {
				ctx.ui.notify("context-editor: the landscape view needs a terminal -- try /context-editor manual", "warning");
				return;
			}
			edit = await ctx.ui.custom<Edit | undefined>(
				(tui, theme, _keybindings, done) => new LandscapeOverlay(tui, theme, done, rows, new Set(state.hiddenIds)),
				{ overlay: true, overlayOptions: { width: "100%", maxHeight: "100%" } },
			);
		} else {
			if (ctx.mode !== "tui") {
				ctx.ui.notify("context-editor: manual editing needs a terminal to launch an external editor", "warning");
				return;
			}
			const content = serializeForEditing(rows, new Set(state.hiddenIds));
			// pi's TUI holds the terminal in raw mode for its own keystroke
			// handling, so spawning the external editor with stdio: "inherit"
			// while that is still active means both are reading the same stdin
			// at once -- keystrokes go to pi, not the editor. `ctx.ui.custom`
			// is the only way an extension gets the `tui` handle needed to
			// suspend that (`tui.stop()`/`tui.start()`), the same shape pi's
			// own `examples/extensions/interactive-shell.ts` uses to hand a
			// shelled-out interactive command the terminal.
			const result = await ctx.ui.custom<EditorResult>((tui, _theme, _keybindings, done) => {
				void (async () => {
					tui.stop();
					let outcome: EditorResult;
					try {
						outcome = await editInExternalEditor(content);
					} catch {
						outcome = { status: "cancelled" };
					} finally {
						tui.start();
						tui.requestRender(true);
					}
					done(outcome);
				})();
				// Nothing is ever actually shown -- done() fires as soon as the
				// editor exits, disposing this immediately, same as the shell
				// example's own comment says of its empty component.
				return { render: () => [], invalidate: () => {} };
			});
			if (result.status === "cancelled") {
				ctx.ui.notify("context-editor: editor exited without saving -- cancelled", "info");
				return;
			}
			edit = parseEdited(result.content, rows);
		}

		if (!edit) return; // cancelled from the overlay
		await applyEdit(pi, ctx, entries, rows, edit);
	}

	pi.registerCommand("context-editor", {
		description: "Edit what the agent sees: landscape view (toggle entries) or 'manual' (edit in $EDITOR)",
		handler: async (args, ctx) => {
			const mode = (args || "").trim().toLowerCase() === "manual" ? "manual" : "landscape";
			await openEditor(ctx, mode);
		},
	});
}
