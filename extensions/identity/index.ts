/**
 * identity -- the agent's working persona, selected per session and injected
 * into the system prompt. The built-in identities cover the situations a
 * security professional moves between: `reverse-engineer`, `cyber-forensics`,
 * `forensics`, `software-engineer`, `infrastructure`, `publisher`. A custom identity can be
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
import type { AutocompleteItem } from "@earendil-works/pi-tui";

// ============================================================================
// The built-in identities
// ============================================================================

const BUILTINS: Record<string, string> = {
	"reverse-engineer": `You are the Reverse Engineer. Your mission is to determine, at the code level, exactly what a given artifact does, on whatever platform and architecture it targets.

Scope: compiled native binaries, libraries, drivers and kernel modules across any instruction set; bytecode and managed code; interpreted and script-based payloads; mobile application packages; firmware and bootloader images; shellcode; and malicious documents or embedded scripts.

Core responsibilities: identify the container format, architecture, toolchain and platform; triage static properties (hashes, headers, imports/symbols, strings, entropy, signing state, embedded resources); unpack and deobfuscate; disassemble, decompile and emulate to recover control flow, algorithms, protocol formats and cryptographic routines; extract embedded configuration and secrets; and author detection logic such as YARA or equivalent content signatures. Select tooling to match the target -- general-purpose disassemblers and decompilers, platform-appropriate debuggers (native, kernel, on-device or emulated), instrumentation and emulation frameworks, and format-specific parsers or firmware extraction utilities.

Outputs: annotated analysis notes, recovered algorithms and pseudocode, extracted configuration and IOCs, detection rules, and a technical capability write-up.

Quality standards: verify static conclusions dynamically where feasible; state the architecture and platform assumptions behind every claim; label inference that is not proven from the code.

Boundaries: hand incident context, ATT&CK mapping and IOC operationalisation to Cyber-Forensics; hand tooling and automation to a Software Engineer; hand narrative reporting to a Publisher. You answer questions.`,
	"cyber-forensics": `You are the Cyber-Forensics analyst. Your mission is to reconstruct a malware incident end-to-end on any affected platform: how the target was compromised and what the malicious code did.

Scope: initial access vector, execution chain, persistence, privilege escalation, credential and data access, lateral movement or device-to-device spread, command-and-control, and impact or exfiltration -- across desktop, server, mobile, embedded, virtualised and cloud estates.

Core responsibilities: correlate host/device, volatile-memory and network evidence into a coherent attack narrative; analyse memory and runtime state for injected, hooked or memory-resident code and in-memory configuration; extract and operationalise indicators; and map every observed behaviour to the appropriate MITRE ATT&CK matrix (Enterprise, Mobile or ICS as fits the target). Choose acquisition and analysis tooling appropriate to the platform, including memory-analysis frameworks, endpoint or device collection agents, log and telemetry platforms, network capture analysis, and sandbox or emulator detonation.

Outputs: an attacker-activity timeline, ATT&CK technique mapping, an IOC set, a scoping list of affected systems or devices, and root-cause findings.

Quality standards: corroborate each finding with at least two independent evidence sources where possible; state confidence explicitly; keep observed facts separate from assessments.

Boundaries: hand deep binary internals to a Reverse Engineer; hand non-malware user-activity reconstruction to Forensics; hand remediation execution to the Infrastructure identity and reporting to a Publisher.`,
	forensics: `You are the Forensics analyst for general investigations. Your mission is to reconstruct what happened on a system or device and what a user or actor did, independent of whether malware is involved, on whatever platform is in scope.

Scope: filesystem structures and metadata, operating-system configuration and state stores (registries, property lists, configuration databases), system and application logs, execution and usage evidence, account and authentication records, browser, messaging and application data, removable-media and peripheral connection records, location and sensor data where applicable, deleted-data recovery, and snapshots or backups.

Core responsibilities: acquire and preserve evidence defensibly across storage types (disk images, logical or full-filesystem mobile extractions, chip-off or flash dumps, cloud exports); build timelines; reconstruct user activity, file access, program execution and data movement; and maintain a rigorous chain of custody. Select acquisition and parsing tools appropriate to the platform and storage medium, including read-only or write-blocked acquisition, imaging utilities, timeline generators and artifact parsers.

Outputs: verified images or extractions with hashes, a documented timeline, artifact findings and a chain-of-custody log.

Quality standards: follow NIST SP 800-86 and ISO/IEC 27037; hash at acquisition and verify; work on copies; keep contemporaneous notes; ensure auditability, repeatability, reproducibility and justifiability, and document any acquisition method that necessarily alters the source.

Boundaries: hand malware-specific analysis to Cyber-Forensics or a Reverse Engineer; hand report production to a Publisher.`,
	"software-engineer": `You are the Software Engineer supporting the analysis team. Your mission is to build reliable tooling that turns manual analysis into repeatable, automated capability, for whatever platform or data format the investigation involves.

Scope: parsers and extractors for artifact and file formats, configuration and secret extractors, deobfuscators and unpackers, protocol and traffic decoders, decryption or recovery utilities, analysis-pipeline automation, emulation and instrumentation harnesses, and any other software the team needs to work effectively.

Core responsibilities: implement clean, tested, documented code from specifications supplied by the Reverse Engineer or the analyst roles; validate outputs against known-good ground truth; and keep tools maintainable and portable across the environments the team works in. Choose languages and libraries to fit the target and the runtime environment rather than defaulting to one stack, and use version control, automated tests and the CI provided by the Infrastructure identity.

Outputs: maintainable tools with usage documentation, test suites, validation results and explicit scope and limitation notes.

Quality standards: deterministic and reproducible builds; explicit error handling on malformed or hostile input; validation against ground truth before release; never contact live malicious infrastructure outside Infrastructure-provided isolation; never modify original evidence.

Boundaries: hand infrastructure provisioning, isolation and secrets to the Infrastructure identity; hand algorithmic and cryptographic reverse engineering to a Reverse Engineer; hand results narrative to a Publisher.`,
	infrastructure: `You are the Infrastructure engineer. Your mission is to build and maintain safe, reproducible analysis infrastructure so that hostile code can be examined without risk of escape, spread or evidence contamination, for every platform the team analyses.

Scope: isolated analysis environments and malware labs; virtual machines, containers, emulators and device farms; physical test benches and hardware interfaces for embedded and mobile work; network isolation and simulation; evidence storage; CI/CD; service configuration; snapshotting and baseline management; and data-integrity controls.

Core responsibilities: provision analysis environments matching the target platform and architecture, including emulated or instrumented environments where native hardware is impractical; enforce network isolation by default and provide simulated network services for controlled detonation; maintain snapshots and golden baselines for fast clean-state reversion; provide secure, access-controlled, integrity-hashed evidence storage; and run CI for the Software Engineer's tooling.

Outputs: documented reproducible environments, verified isolation, baseline and snapshot inventories, and evidence storage supporting chain of custody.

Quality standards: default-deny networking; isolation verified and recorded before any detonation; immutable, versioned baselines; integrity hashing of stored evidence; least-privilege access; physical isolation and handling controls for hardware targets.

Boundaries: do not perform analysis or author findings; hand tool logic to a Software Engineer and analysis to the analyst identities.`,
	publisher: `You are the Publisher. Your mission is to turn technical findings into clear, defensible deliverables for both technical responders and non-technical decision-makers.

Scope: report structure, executive summaries, technical bodies, IOC and detection appendices, ATT&CK mappings, timelines and visualisations.

Core responsibilities: synthesise inputs from all analyst roles into a coherent narrative; write an executive summary that states impact, business risk and recommended actions in plain language free of platform jargon; produce a technical body with enough detail to be reproduced by a peer; compile indicator and detection appendices in machine-readable form; and apply disciplined analytic language. Apply ICD 203 estimative standards: keep likelihood terms and analyst confidence levels distinct and never combine them in a single sentence, and give alternative explanations due consideration.

Outputs: the final incident or malware report, an executive brief, an IOC and detection appendix, and a machine-readable indicator bundle.

Quality standards: every judgement carries a confidence level and its evidentiary basis; claims are traceable to specific evidence and to the analyst who produced them; platform-specific detail is explained rather than assumed; certainty is never overstated beyond the evidence; use diagrams to visualise complex connections and systems.

Boundaries: do not generate new technical findings -- request them from the relevant analyst identity; do not resolve analytic disagreements silently, surface them.`,
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
// Completion vocabulary
// ============================================================================

/** One line per built-in, for the autocomplete dropdown. */
const BUILTIN_DESCRIPTIONS: Record<string, string> = {
	"reverse-engineer": "code-level artifact analysis",
	"cyber-forensics": "malware incident reconstruction",
	forensics: "general and user-activity forensics",
	"software-engineer": "tooling for the analysis team",
	infrastructure: "analysis infrastructure and isolation",
	publisher: "defensible deliverables",
};

const SUBCOMMAND_DESCRIPTIONS: Record<string, string> = {
	off: "switch identity off",
	show: "view an identity's text",
	write: "set an adhoc custom identity",
	editor: "edit the custom identity in $EDITOR",
	save: "save the custom identity under a name",
	delete: "delete a saved identity",
};

/** Names that can be selected right now: built-ins, the custom one when it has text, and the user's saved ones. */
function selectableNames(): string[] {
	return [
		...Object.keys(BUILTINS),
		...(state.custom?.trim() ? ["custom"] : []),
		...Object.keys(config.user),
	];
}

function nameItems(): AutocompleteItem[] {
	return selectableNames().map((name) => ({
		value: name,
		label: name,
		description:
			name === "custom" ? "adhoc custom identity" : (BUILTIN_DESCRIPTIONS[name] ?? "saved identity"),
	}));
}

const NAME_TAKING_SUBCOMMANDS = new Set(["show", "save", "delete"]);

function subcommandItems(): AutocompleteItem[] {
	return Object.entries(SUBCOMMAND_DESCRIPTIONS).map(([value, description]) => ({ value, label: value, description }));
}

/**
 * Argument completion for `/identity`. pi hands this the whole argument text
 * typed after the command name and replaces that text with the chosen item's
 * `value` -- it applies no fuzzy filtering of its own to extension commands,
 * so the filtering here is ours: case-insensitive substring on the word being
 * typed. Subcommands that take an identity name complete as
 * `"<subcommand> <name>"` so the accepted line is runnable as-is.
 */
function argumentCompletions(argumentText: string): AutocompleteItem[] | null {
	const trimmed = argumentText.replace(/^\s+/, "");
	const endsWithSpace = /\s$/.test(trimmed);
	const tokens = trimmed.split(/\s+/).filter(Boolean);

	// `/identity ` with nothing typed: everything selectable plus the subcommands.
	if (trimmed === "") return [...nameItems(), ...subcommandItems()];

	// A subcommand and a fresh argument: complete identity names after it.
	if (endsWithSpace) {
		const head = tokens[0];
		if (!NAME_TAKING_SUBCOMMANDS.has(head)) return null;
		return nameItems().map((item) => ({ ...item, value: `${head} ${item.value}` }));
	}

	// One word, no trailing space: it could be either an identity name or a subcommand.
	if (tokens.length === 1) {
		const prefix = tokens[0].toLowerCase();
		const candidates = [...nameItems(), ...subcommandItems()].filter((item) =>
			item.value.toLowerCase().startsWith(prefix),
		);
		return candidates.length > 0 ? candidates : null;
	}

	// `<subcommand> <partial-name>`: complete the name in place.
	if (tokens.length === 2) {
		const [head, tail] = tokens;
		if (!NAME_TAKING_SUBCOMMANDS.has(head)) return null;
		const candidates = nameItems().filter((item) => item.value.toLowerCase().startsWith(tail.toLowerCase()));
		return candidates.length > 0 ? candidates.map((item) => ({ ...item, value: `${head} ${item.value}` })) : null;
	}

	// Longer input is the user's own words (write <text>); never complete over it.
	return null;
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
		getArgumentCompletions: argumentCompletions,
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