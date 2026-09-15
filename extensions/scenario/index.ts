/**
 * scenario -- steer a multi-step analysis instead of leaving it to instinct.
 *
 * An eager model finishes triage and immediately starts patching. It is not
 * wrong to be *capable* of that, but it skips the evidence-gathering that
 * would have made the patch correct. A scenario is a chain of phases -- triage,
 * then static, then dynamic, then report -- and the agent advances by calling
 * `reactor_phase_complete(summary)`. The tool's own *return content* is the
 * next phase's briefing: no extra message, no extra turn boundary, and it
 * arrives exactly where the model is already looking
 * (ADR-0009).
 *
 * Phase definitions are Markdown files under `prompts/scenarios/<id>/`, read
 * directly rather than surfaced as pi prompt commands -- a bare phase is not a
 * useful thing to invoke on its own, since advancing is stateful and a slash
 * command has no memory of what came before (ADR-0017).
 *
 * `reactor_phase_complete` is registered once, always, and advertised exactly
 * while a scenario is running: the extension withdraws it from the active
 * tools list when the scenario ends and re-advertises when one starts
 * (ADR-0030). Calling it with no
 * scenario active is a normal, answered case, not an error path.
 */

import type { ExtensionAPI, ExtensionContext, SessionStartEvent } from "@earendil-works/pi-coding-agent";
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Type } from "typebox";

// This file lives at <package root>/extensions/scenario/index.ts, so two
// `dirname` calls above it is the package root -- the same self-location
// `bin/reactor` uses (`Path(__file__).resolve().parent.parent`), just in
// TypeScript. `ctx.cwd` is the *session's* working directory and has nothing
// to do with where this package's own `prompts/` tree lives.
const PACKAGE_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");

// REACTOR_SCENARIOS_DIR mirrors REACTOR_CONFIG_DIR: an escape hatch for
// tests (a throwaway scenario, isolated from this package's own shipped
// prose) and, incidentally, for a person who wants their own scenarios
// without editing the installed package. There is no seed/diff-config story
// for it the way tools.toml gets one -- out of scope until someone other
// than a test actually wants it.
const SCENARIOS_DIR = process.env.REACTOR_SCENARIOS_DIR
	? path.resolve(process.env.REACTOR_SCENARIOS_DIR)
	: path.join(PACKAGE_ROOT, "prompts", "scenarios");

/** Same backstop as the other extensions: a wedged CLI costs one slow step, not a hung one. */
const EXEC_TIMEOUT_MS = 20_000;

const ENTRY_TYPE = "reactor-scenario";
const TOOL_NAME = "reactor_phase_complete";

interface Step {
	title: string;
	toolset?: string;
	body: string;
}

interface ScenarioState {
	scenarioId: string;
	/** Index into that scenario's steps -- the step currently in progress. */
	stepIndex: number;
	/** One per completed step, parallel to steps[0..stepIndex-1]. */
	summaries: string[];
}

function listScenarios(): string[] {
	try {
		return readdirSync(SCENARIOS_DIR, { withFileTypes: true })
			.filter((d) => d.isDirectory())
			.map((d) => d.name)
			.sort();
	} catch {
		return [];
	}
}

/**
 * Two frontmatter fields, hand-parsed rather than pulling in a YAML library
 * for `title:` and `toolset:` -- everything else about a step (which tools
 * just became relevant, what not to start yet) is prose the author writes
 * directly into the body (ADR-0017).
 */
function parseStep(raw: string): Step {
	const m = raw.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?([\s\S]*)$/);
	if (!m) return { title: "", body: raw.trim() };
	const [, frontmatter, body] = m;
	const fields: Record<string, string> = {};
	for (const line of frontmatter.split("\n")) {
		const kv = line.match(/^(\w+):\s*(.*)$/);
		if (kv) fields[kv[1]] = kv[2].trim();
	}
	return { title: fields.title ?? "", toolset: fields.toolset || undefined, body: body.trim() };
}

function loadSteps(scenarioId: string): Step[] {
	const dir = path.join(SCENARIOS_DIR, scenarioId);
	let files: string[];
	try {
		files = readdirSync(dir).filter((f) => f.endsWith(".md")).sort();
	} catch {
		return [];
	}
	return files.map((f) => parseStep(readFileSync(path.join(dir, f), "utf8")));
}

/** `## Step 2/4: Static analysis`, then the step's own body verbatim. */
function briefing(steps: Step[], index: number): string {
	const step = steps[index];
	const title = step.title || `step ${index + 1}`;
	return `## Phase ${index + 1}/${steps.length}: ${title}\n\n${step.body}`;
}

export default function scenario(pi: ExtensionAPI) {
	/** In-memory mirror of the last `reactor-scenario` entry, warmed on session_start. */
	let state: ScenarioState | undefined;

	function persist(next: ScenarioState | undefined): void {
		state = next;
		pi.appendEntry(ENTRY_TYPE, next);
	}

	/**
	 * Best-effort and additive only: nothing here ever disables a toolset a
	 * previous step activated. Steers, does not restrict (ADR-0007) -- a
	 * failed or missing `reactor` means the step still advances, just without
	 * the registry highlighting anything new.
	 */
	async function activateToolset(ctx: ExtensionContext, toolset: string | undefined): Promise<void> {
		if (!toolset) return;
		await pi.exec("reactor", ["toolsets", "enable", toolset, "--format", "json"], {
			timeout: EXEC_TIMEOUT_MS,
			cwd: ctx.cwd,
		});
	}

	/**
	 * Shared by the tool and `/reactor-scenario next`: record the summary,
	 * move to the next step (or end the scenario), and return what the model
	 * -- or the message the command sends -- should see.
	 */
	/**
	 * Advertise `reactor_phase_complete` exactly while a scenario is running.
	 * The tool is registered once, always, so a stale list degrades to its own
	 * "no scenario is active" explanation (ADR-0007); the advertisement is
	 * what follows the state (ADR-0030). A no-op transition is skipped --
	 * same visibility, no prompt rebuild.
	 */
	function syncToolVisibility(): void {
		const active = pi.getActiveTools();
		const has = active.includes(TOOL_NAME);
		const wanted = state !== undefined;
		if (wanted === has) return;
		pi.setActiveTools(wanted ? [...active, TOOL_NAME] : active.filter((n) => n !== TOOL_NAME));
	}

	async function advance(ctx: ExtensionContext, summary: string): Promise<string> {
		if (!state) return 'reactor: no scenario is active -- start one with `/reactor-scenario start <id>`.';

		const steps = loadSteps(state.scenarioId);
		const finishedId = state.scenarioId;
		const nextIndex = state.stepIndex + 1;
		const summaries = [...state.summaries, summary];

		if (nextIndex >= steps.length) {
			persist(undefined);
			syncToolVisibility();
			return `reactor: scenario "${finishedId}" complete -- ${steps.length} phase(s) done.`;
		}

		persist({ scenarioId: finishedId, stepIndex: nextIndex, summaries });
		await activateToolset(ctx, steps[nextIndex].toolset);
		return briefing(steps, nextIndex);
	}

	// Restores across a session reload/resume the same way skill-fetch state
	// and everything else here does: the last matching entry wins, so a
	// `start` followed by two `next`s followed by a `stop` replays to "no
	// scenario active" -- `stop` persists `undefined` on purpose.
	pi.on("session_start", async (_event: SessionStartEvent, ctx: ExtensionContext) => {
		let found: ScenarioState | undefined;
		for (const entry of ctx.sessionManager.getEntries()) {
			if (entry.type === "custom" && entry.customType === ENTRY_TYPE) {
				found = entry.data as ScenarioState | undefined;
			}
		}
		state = found;
		syncToolVisibility();
	});

	pi.registerTool({
		name: TOOL_NAME,
		label: "Scenario: phase complete",
		description:
			"Mark the current REactor scenario's phase complete and receive the next phase's briefing. " +
			"Phases are the scenario's work stages -- unrelated to the session manifest's steps (update_steps). " +
			"No-op with an explanatory result if no scenario is active.",
		promptSnippet: "Advance the active REactor scenario once its current phase is genuinely done",
		promptGuidelines: [
			"Call reactor_phase_complete once a REactor scenario's current phase is genuinely finished, " +
				"with a summary of what you concluded -- not before, and not to narrate progress mid-phase.",
		],
		parameters: Type.Object({
			summary: Type.String({
				description: "What you concluded in this step. Required before the scenario advances.",
			}),
		}),
		async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
			const text = await advance(ctx, params.summary);
			return { content: [{ type: "text", text }], details: state };
		},
	});

	pi.registerCommand("reactor-scenario", {
		description: "REactor: run a multi-step analysis scenario",
		getArgumentCompletions: (prefix: string) => {
			const parts = prefix.split(/\s+/);
			if (parts[0] === "start" && parts.length > 1) {
				const idPrefix = parts.slice(1).join(" ");
				return listScenarios()
					.filter((id) => id.startsWith(idPrefix))
					.map((id) => ({ value: `start ${id}`, label: id }));
			}
			return ["list", "start", "status", "next", "stop"]
				.filter((c) => c.startsWith(prefix))
				.map((c) => ({ value: c, label: c }));
		},
		handler: async (args, ctx) => {
			const [sub, ...rest] = args.trim().split(/\s+/).filter(Boolean);

			switch (sub ?? "status") {
				case "list": {
					const ids = listScenarios();
					ctx.ui.notify(
						ids.length ? `reactor: scenarios -- ${ids.join(", ")}` : "reactor: no scenarios found in prompts/scenarios/",
						"info",
					);
					return;
				}

				case "start": {
					const id = rest.join(" ");
					if (!id) {
						ctx.ui.notify("reactor-scenario: start needs a scenario id -- try `list`", "error");
						return;
					}
					if (state) {
						ctx.ui.notify(`reactor: "${state.scenarioId}" is already running -- \`stop\` it first`, "error");
						return;
					}
					const steps = loadSteps(id);
					if (!steps.length) {
						ctx.ui.notify(`reactor-scenario: unknown scenario "${id}" -- try \`list\``, "error");
						return;
					}
					persist({ scenarioId: id, stepIndex: 0, summaries: [] });
				syncToolVisibility();
					await activateToolset(ctx, steps[0].toolset);
					pi.sendMessage(
						{ customType: ENTRY_TYPE, content: briefing(steps, 0), display: true },
						{ triggerTurn: true },
					);
					ctx.ui.notify(`reactor: started "${id}" -- step 1/${steps.length}`, "info");
					return;
				}

				case "status": {
					if (!state) {
						ctx.ui.notify("reactor: no scenario active", "info");
						return;
					}
					const steps = loadSteps(state.scenarioId);
					const title = steps[state.stepIndex]?.title || `step ${state.stepIndex + 1}`;
					ctx.ui.notify(
						`reactor: "${state.scenarioId}" -- step ${state.stepIndex + 1}/${steps.length}: ${title}`,
						"info",
					);
					return;
				}

				case "next": {
					if (!state) {
						ctx.ui.notify("reactor: no scenario active -- `start` one first", "error");
						return;
					}
					// The human is the better judge of whether a step is genuinely
					// finished (ADR-0009); this bypasses the model entirely.
					const text = await advance(ctx, rest.join(" ") || "(advanced manually)");
					syncToolVisibility();
					pi.sendMessage({ customType: ENTRY_TYPE, content: text, display: true }, { triggerTurn: true });
					return;
				}

				case "stop": {
					if (!state) {
						ctx.ui.notify("reactor: no scenario active", "info");
						return;
					}
					const id = state.scenarioId;
					persist(undefined);
					syncToolVisibility();
					ctx.ui.notify(`reactor: stopped "${id}"`, "info");
					return;
				}

				default:
					ctx.ui.notify(
						`reactor-scenario: unknown subcommand "${sub}" -- try list, start <id>, status, next, or stop`,
						"error",
					);
			}
		},
	});
}
