#!/usr/bin/env node
/**
 * Capture what the pi extensions actually say and do, as the specification the
 * Rust `reactor-context` crate must reproduce byte for byte (MIGRATE.md phase 3).
 *
 *   node tests/extensions/golden/capture.mjs          # rewrite the goldens
 *   node tests/extensions/golden/capture.mjs --check  # fail if they are stale
 *
 * Each case is a scripted session: a sequence of operations against one
 * extension loaded through pi's own loader (the same harness the extension tests
 * use), with what it produced recorded beside each -- notifications, session
 * entries it appended, the system-prompt text it added, tool results, toolset
 * activations. `crates/reactor-context/tests/golden.rs` replays the same
 * operations against the Rust modules and compares.
 *
 * The extensions are frozen (ADR-0035), so these goldens should never change; the
 * `--check` mode is how a change to one of them gets noticed.
 *
 * Needs pi on PATH and a Node with TypeScript support, like the extension suite.
 */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { loadExtension, makeContext, needsPi, withFixture } from "../harness.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.resolve(HERE, "../../../crates/reactor-context/tests/golden");

if (needsPi.skip) {
	console.error(`capture: ${needsPi.skip}`);
	process.exit(2);
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

const BASE_PROMPT = "BASE PROMPT";

/** Records the new items of `arr` since the last call. */
function cursor(arr) {
	let seen = 0;
	return () => {
		const fresh = arr.slice(seen);
		seen = arr.length;
		return fresh;
	};
}

const notes = (calls) => cursor(calls.notify);

/** Everything a handler said to the person, as plain data. */
const plainNotify = (items) => items.map((n) => ({ message: n.message, level: n.level }));

/** What `data` was on a session entry; `undefined` (a cleared scenario) becomes null. */
const entryData = (e) => (e.data === undefined ? null : e.data);

async function fire(extension, event, payload, ctx) {
	let result;
	for (const handler of extension.handlers.get(event) ?? []) {
		result = (await handler(payload, ctx)) ?? result;
	}
	return result;
}

const advertised = (loaded, name) => {
	const last = loaded.activeToolWrites.at(-1);
	return last === undefined ? true : last.includes(name);
};

// ---------------------------------------------------------------------------
// manifest (goal-setting)
// ---------------------------------------------------------------------------

const LONG = "x".repeat(100);
const EMOJI_EDGE = `${"a".repeat(77)}\u{1F600}${"b".repeat(10)}`;
const stepList = (n) => Array.from({ length: n }, (_, i) => ({ summary: `step ${i + 1}`, status: "in progress" }));

const MANIFEST_CASES = [
	{
		name: "walkthrough",
		ops: [
			{ op: "prompt" },
			{ op: "tool", steps: [{ summary: "x", status: "done" }] },
			{ op: "command", name: "frame", args: "" },
			{ op: "command", name: "goal", args: "" },
			{ op: "command", name: "goal", args: "  find the crash  " },
			{ op: "prompt" },
			{ op: "tool", steps: [{ summary: LONG, status: "one two three four five" }, { summary: "ok", status: "  done  " }] },
			{ op: "prompt" },
			{ op: "command", name: "frame", args: "" },
			{ op: "command", name: "guidelines", args: "" },
			{ op: "command", name: "guidelines", args: "never touch prod" },
			{ op: "prompt" },
			{ op: "command", name: "manifest", args: "off" },
			{ op: "prompt" },
			{ op: "tool", steps: [{ summary: "x", status: "y" }] },
			{ op: "command", name: "frame", args: "" },
			{ op: "command", name: "manifest", args: "" },
			{ op: "command", name: "manifest", args: "bogus" },
			{ op: "command", name: "guidelines", args: "CLEAR" },
			{ op: "prompt" },
			{ op: "command", name: "goal", args: "clear" },
			{ op: "prompt" },
			{ op: "tool", steps: [{ summary: "x", status: "y" }] },
			{ op: "command", name: "goal", args: "again" },
			{ op: "command", name: "manifest", args: "clear" },
			{ op: "prompt" },
			{ op: "command", name: "frame", args: "" },
		],
	},
	{
		name: "guidelines-only",
		ops: [
			{ op: "command", name: "guidelines", args: "just rules" },
			{ op: "prompt" },
			{ op: "tool", steps: [{ summary: "x", status: "y" }] },
			{ op: "command", name: "frame", args: "" },
		],
	},
	{
		name: "limits-from-config",
		config: { softStepLimit: 2, maxDescription: 10, statusWords: 1, deriveContextChars: 100 },
		ops: [
			{ op: "command", name: "goal", args: "small limits" },
			{ op: "tool", steps: [{ summary: "a long summary here", status: "two words" }, ...stepList(2)] },
			{ op: "prompt" },
			{ op: "command", name: "frame", args: "" },
		],
	},
	{
		name: "over-the-default-soft-limit",
		ops: [
			{ op: "command", name: "goal", args: "many" },
			{ op: "tool", steps: stepList(22) },
			{ op: "prompt" },
		],
	},
	{
		name: "js-string-lengths",
		ops: [
			{ op: "command", name: "goal", args: "unicode" },
			{ op: "tool", steps: [{ summary: EMOJI_EDGE, status: "déjà vu ✓ ok now" }] },
			{ op: "prompt" },
		],
	},
	{
		name: "derive",
		ops: [
			{ op: "derive", args: "", response: "```json\n{\"goal\": \"  crack the license check  \", \"guidelines\": \"be quiet\", \"steps\": [{\"summary\": \"find the check\", \"status\": \"in progress now really\"}, {\"summary\": \"  \", \"status\": \"x\"}, {\"status\": \"no summary\"}, {\"summary\": \"patch it\"}]}\n```" },
			{ op: "prompt" },
			{ op: "derive", args: "goal", response: "Sure! Here you go: {\"goal\": \"second goal\", \"steps\": [{\"summary\": \"ignored\", \"status\": \"x\"}]} hope it helps" },
			{ op: "derive", args: "GUIDELINES", response: "{\"guidelines\": \"\"}" },
			{ op: "derive", args: "steps", response: "{\"steps\": []}" },
			{ op: "command", name: "frame", args: "" },
			{ op: "derive", args: "goal", response: "{\"goal\": \"   \"}" },
			{ op: "derive", args: "all", response: "no json at all" },
			{ op: "derive", args: "all", response: "} backwards {" },
			{ op: "derive", args: "bogus", response: "{}" },
			{ op: "derive", args: "steps", response: "{\"goal\": \"wrong scope\"}" },
		],
	},
];

async function runManifest(c) {
	return withFixture({}, async (fixture) => {
		if (c.config) fs.writeFileSync(path.join(fixture.agentDir, "pi-goal-setting.json"), JSON.stringify(c.config));
		const loaded = await loadExtension("extensions/goal-setting/index.ts", fixture);
		let response = "";
		const provided = [];
		const registry = {
			complete: async (_model, request) => {
				provided.push(request);
				return { stopReason: "stop", content: [{ type: "text", text: response }] };
			},
		};
		const made = makeContext(fixture, { entries: loaded.entries, model: { id: "m" }, modelRegistry: registry });
		await fire(loaded.extension, "session_start", {}, made.ctx);
		const newNotes = notes(made.calls);
		const newEntries = cursor(loaded.entries);

		const ops = [];
		for (const op of c.ops) {
			const out = {};
			if (op.op === "command") {
				await loaded.extension.commands.get(op.name).handler(op.args, made.ctx);
			} else if (op.op === "tool") {
				const tool = loaded.extension.tools.get("update_steps").definition;
				const result = await tool.execute("id", { steps: op.steps }, undefined, undefined, made.ctx);
				out.text = result.content[0].text;
			} else if (op.op === "prompt") {
				const result = await fire(loaded.extension, "before_agent_start", { systemPrompt: BASE_PROMPT, prompt: "p" }, made.ctx);
				out.appended = result ? result.systemPrompt.slice(BASE_PROMPT.length) : null;
			} else if (op.op === "derive") {
				response = op.response;
				const before = provided.length;
				await loaded.extension.commands.get("derive").handler(op.args, made.ctx);
				if (provided.length > before) {
					out.systemPrompt = provided[before].systemPrompt;
					out.userMessage = provided[before].messages[0].content;
				}
			}
			out.notify = plainNotify(newNotes());
			out.entries = newEntries().map(entryData);
			out.advertised = advertised(loaded, "update_steps");
			ops.push({ ...op, out });
		}
		return { name: c.name, config: c.config ?? null, ops };
	});
}

// ---------------------------------------------------------------------------
// identity
// ---------------------------------------------------------------------------

const IDENTITY_CASES = [
	{
		name: "default-and-saved",
		config: { default: "publisher", user: { zeta: "Zeta text.", alpha: "Alpha text." } },
		ops: [
			{ op: "prompt" },
			{ op: "command", args: "" },
			{ op: "command", args: "show" },
			{ op: "command", args: "off" },
			{ op: "prompt" },
			{ op: "command", args: "" },
			{ op: "command", args: "show" },
			{ op: "command", args: "reverse-engineer" },
			{ op: "prompt" },
			{ op: "command", args: "show forensics" },
			{ op: "command", args: "show nosuch" },
			{ op: "command", args: "show   zeta  " },
			{ op: "command", args: "zeta" },
			{ op: "prompt" },
			{ op: "command", args: "custom" },
			{ op: "command", args: "xyz" },
			{ op: "command", args: "write" },
			{ op: "command", args: "write   " },
			{ op: "command", args: "write   my own persona  " },
			{ op: "prompt" },
			{ op: "command", args: "" },
			{ op: "command", args: "show" },
			{ op: "command", args: "save" },
			{ op: "command", args: "save publisher" },
			{ op: "command", args: "save mine" },
			{ op: "command", args: "save alpha" },
			{ op: "command", args: "delete mine" },
			{ op: "command", args: "delete publisher" },
			{ op: "command", args: "delete ghost" },
			{ op: "command", args: "delete" },
			{ op: "command", args: "editor", mode: "rpc" },
			{ op: "command", args: "custom" },
			{ op: "command", args: "  forensics  " },
			{ op: "prompt" },
		],
	},
	{
		name: "no-config",
		ops: [
			{ op: "prompt" },
			{ op: "command", args: "" },
			{ op: "command", args: "custom" },
			{ op: "command", args: "save x" },
			{ op: "command", args: "write text" },
			{ op: "command", args: "save x" },
			{ op: "command", args: "" },
			{ op: "command", args: "off" },
			{ op: "command", args: "" },
			{ op: "prompt" },
		],
	},
	{
		name: "custom-on-default",
		config: { default: "custom", user: {} },
		ops: [
			{ op: "prompt" },
			{ op: "command", args: "" },
			{ op: "command", args: "write now it has text" },
			{ op: "prompt" },
		],
	},
];

async function runIdentity(c) {
	return withFixture({}, async (fixture) => {
		const file = path.join(fixture.agentDir, "pi-identity.json");
		if (c.config) fs.writeFileSync(file, JSON.stringify(c.config));
		const loaded = await loadExtension("extensions/identity/index.ts", fixture);
		const contexts = {};
		const ctxFor = (mode = "tui") =>
			(contexts[mode] ??= makeContext(fixture, { entries: loaded.entries, mode }));
		await fire(loaded.extension, "session_start", {}, ctxFor().ctx);
		const newEntries = cursor(loaded.entries);
		const cursors = {};

		const ops = [];
		for (const op of c.ops) {
			const out = {};
			const made = ctxFor(op.mode);
			cursors[op.mode ?? "tui"] ??= notes(made.calls);
			if (op.op === "command") {
				await loaded.extension.commands.get("identity").handler(op.args, made.ctx);
			} else if (op.op === "prompt") {
				const result = await fire(loaded.extension, "before_agent_start", { systemPrompt: BASE_PROMPT, prompt: "p" }, made.ctx);
				out.appended = result ? result.systemPrompt.slice(BASE_PROMPT.length) : null;
			}
			out.notify = plainNotify(cursors[op.mode ?? "tui"]());
			out.entries = newEntries().map(entryData);
			out.settings = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, "utf8")) : null;
			ops.push({ ...op, out });
		}
		return { name: c.name, config: c.config ?? null, ops };
	});
}

// ---------------------------------------------------------------------------
// reporting
// ---------------------------------------------------------------------------

const REPORTING_CASES = [
	{
		name: "levels-and-enforcement",
		config: { level: 0, folder: "docs", stepThreshold: 2, maxReverts: 1, templatePath: "tpl.md" },
		ops: [
			{ op: "prompt", prompt: "first" },
			{ op: "report", args: "" },
			{ op: "report", args: "status" },
			{ op: "report", args: "on" },
			{ op: "prompt", prompt: "second" },
			{ op: "report", args: "level" },
			{ op: "report", args: "level 3" },
			{ op: "report", args: "level 1.5" },
			{ op: "report", args: "level 1" },
			{ op: "tool_end" },
			{ op: "tool_end" },
			{ op: "tool_end" },
			{ op: "report", args: "status" },
			{ op: "context" },
			{ op: "settled" },
			{ op: "report", args: "level 2" },
			{ op: "context" },
			{ op: "settled" },
			{ op: "enforce" },
			{ op: "prompt", prompt: "the resend", resend: true },
			{ op: "tool_end" },
			{ op: "tool_end" },
			{ op: "settled" },
			{ op: "settled" },
			{ op: "enforce" },
			{ op: "prompt", prompt: "a fresh instruction" },
			{ op: "tool_end" },
			{ op: "settled" },
			{ op: "write", path: "docs/findings.md", content: "hello", mtime: 1000 },
			{ op: "tool_end" },
			{ op: "report", args: "status" },
			{ op: "tool_end" },
			{ op: "write", path: "docs/findings.md", content: "hello again", mtime: 2000 },
			{ op: "tool_end" },
			{ op: "write", path: "docs/sub/more.md", content: "x", mtime: 3000 },
			{ op: "tool_end" },
			{ op: "write", path: "docs/findings.md", content: "HELLO AGAIN", mtime: 4000 },
			{ op: "tool_end" },
			{ op: "tool_end" },
			{ op: "tool_end" },
			{ op: "context" },
			{ op: "report", args: "reset" },
			{ op: "report", args: "status" },
			{ op: "tool_end" },
			{ op: "tool_end" },
			{ op: "report", args: "folder" },
			{ op: "report", args: "folder   out   dir " },
			{ op: "report", args: "status" },
			{ op: "prompt", prompt: "after folder" },
			{ op: "report", args: "bogus" },
			{ op: "report", args: "off" },
			{ op: "tool_end" },
			{ op: "context" },
			{ op: "prompt", prompt: "off now" },
		],
	},
	{
		name: "defaults",
		ops: [
			{ op: "report", args: "status" },
			{ op: "report", args: "on" },
			{ op: "prompt", prompt: "p" },
			{ op: "report", args: "status" },
			{ op: "report", args: "level 2" },
			{ op: "report", args: "status" },
			{ op: "report", args: "off" },
			{ op: "report", args: "on" },
			{ op: "report", args: "status" },
		],
	},
	{
		name: "bad-config-values",
		config: { level: 7, folder: "   ", stepThreshold: -1, maxReverts: -2, templatePath: " " },
		ops: [{ op: "report", args: "on" }, { op: "prompt", prompt: "p" }, { op: "report", args: "status" }],
	},
];

async function runReporting(c) {
	return withFixture({}, async (fixture) => {
		if (c.config) fs.writeFileSync(path.join(fixture.agentDir, "pi-reactor-reporting.json"), JSON.stringify(c.config));
		const loaded = await loadExtension("extensions/reporting/index.ts", fixture);
		const branch = [{ type: "message", id: "u1", message: { role: "user", content: "hi" } }];
		const made = makeContext(fixture, { entries: loaded.entries, branch });
		await fire(loaded.extension, "session_start", {}, made.ctx);
		const newNotes = notes(made.calls);
		const newEntries = cursor(loaded.entries);
		const newUser = cursor(loaded.sentUserMessages);
		const newNav = cursor(made.calls.navigateTree);
		const settingsFile = path.join(fixture.agentDir, "pi-reactor-reporting.json");

		const ops = [];
		for (const op of c.ops) {
			const out = {};
			if (op.op === "report") {
				await loaded.extension.commands.get("report").handler(op.args, made.ctx);
			} else if (op.op === "prompt") {
				const result = await fire(
					loaded.extension,
					"before_agent_start",
					{ systemPrompt: BASE_PROMPT, prompt: op.prompt },
					made.ctx,
				);
				out.appended = result ? result.systemPrompt.slice(BASE_PROMPT.length) : null;
			} else if (op.op === "write") {
				const file = path.join(fixture.dir, op.path);
				fs.mkdirSync(path.dirname(file), { recursive: true });
				fs.writeFileSync(file, op.content);
				fs.utimesSync(file, op.mtime, op.mtime);
			} else if (op.op === "tool_end") {
				await fire(loaded.extension, "tool_execution_end", {}, made.ctx);
			} else if (op.op === "context") {
				const result = await fire(loaded.extension, "context", { messages: [] }, made.ctx);
				const nag = result?.messages?.at(-1);
				out.nag = nag ? nag.content : null;
			} else if (op.op === "settled") {
				await fire(loaded.extension, "agent_settled", {}, made.ctx);
			} else if (op.op === "enforce") {
				await loaded.extension.commands.get("reactor-report-enforce").handler("", made.ctx);
			}
			out.notify = plainNotify(newNotes());
			out.entries = newEntries().map(entryData);
			out.userMessages = newUser().map((m) => m.content);
			out.navigated = newNav().map((n) => n.targetId);
			out.settings = fs.existsSync(settingsFile) ? JSON.parse(fs.readFileSync(settingsFile, "utf8")) : null;
			ops.push({ ...op, out });
		}
		return { name: c.name, config: c.config ?? null, ops };
	});
}

// ---------------------------------------------------------------------------
// scenario
// ---------------------------------------------------------------------------

const SCENARIOS = {
	alpha: [
		"---\ntitle: Scoping\ntoolset: triage\n---\nDecide what is in scope.\n\nSecond paragraph.\n",
		"No frontmatter here, so no title.\n",
		"---\r\ntitle: Static analysis\r\ntoolset: native\r\n---\r\nCRLF body.\r\n",
	],
	beta: ["---\ntitle: Only\n---\nsolo body", "---\ntoolset:   \ntitle:\n---\nempty fields\n"],
	gamma: ["---\ntitle: Extra: colon\nunknown: ignored\ntoolset: dynamic\n---\n\n  padded body  \n\n"],
};

/** The scenario REactor actually ships, file by file, so its real prose is what gets compared. */
function shippedScenario(id) {
	const dir = path.resolve(HERE, "../../../prompts/scenarios", id);
	return fs
		.readdirSync(dir)
		.filter((f) => f.endsWith(".md"))
		.sort()
		.map((f) => fs.readFileSync(path.join(dir, f), "utf8"));
}

const SHIPPED = shippedScenario("investigation");

const SCENARIO_CASES = [
	{
		// Every phase of the real thing: start it, complete each in turn, and let the
		// last one end it. Parsing, titles, toolsets and briefings all as shipped.
		name: "shipped-investigation",
		scenarios: { investigation: SHIPPED },
		ops: [
			{ op: "command", args: "list" },
			{ op: "command", args: "start investigation" },
			...SHIPPED.slice(1).map((_, i) => ({ op: "tool", summary: `phase ${i + 1} done` })),
			{ op: "command", args: "status" },
			{ op: "tool", summary: "the last one" },
			{ op: "command", args: "status" },
		],
	},
	{
		name: "walkthrough",
		scenarios: SCENARIOS,
		ops: [
			{ op: "command", args: "" },
			{ op: "command", args: "list" },
			{ op: "command", args: "status" },
			{ op: "command", args: "next" },
			{ op: "command", args: "stop" },
			{ op: "tool", summary: "nothing running" },
			{ op: "command", args: "start" },
			{ op: "command", args: "start ghost" },
			{ op: "command", args: "start alpha" },
			{ op: "command", args: "start beta" },
			{ op: "command", args: "" },
			{ op: "tool", summary: "scope agreed" },
			{ op: "command", args: "status" },
			{ op: "command", args: "next   handled   by   hand " },
			{ op: "command", args: "status" },
			{ op: "tool", summary: "static done" },
			{ op: "command", args: "status" },
			{ op: "tool", summary: "after complete" },
			{ op: "command", args: "start beta" },
			{ op: "command", args: "next" },
			{ op: "command", args: "stop" },
			{ op: "command", args: "stop" },
			{ op: "command", args: "start gamma" },
			{ op: "command", args: "status" },
			{ op: "command", args: "next" },
			{ op: "command", args: "bogus" },
		],
	},
	{ name: "no-scenarios", scenarios: {}, ops: [{ op: "command", args: "list" }, { op: "command", args: "start alpha" }] },
];

async function runScenario(c) {
	return withFixture({ scenarios: c.scenarios }, async (fixture) => {
		// Log every `reactor` the extension runs, then run the real one.
		const shimDir = fs.mkdtempSync(path.join(os.tmpdir(), "capture-shim-"));
		const log = path.join(shimDir, "calls.log");
		fs.writeFileSync(
			path.join(shimDir, "reactor"),
			`#!/bin/sh\nprintf '%s\\n' "$*" >> ${JSON.stringify(log)}\nPATH=\${PATH#${shimDir}:} exec reactor "$@"\n`,
		);
		fs.chmodSync(path.join(shimDir, "reactor"), 0o755);
		const savedPath = process.env.PATH;
		process.env.PATH = `${shimDir}${path.delimiter}${process.env.PATH}`;
		try {
			const loaded = await loadExtension("extensions/scenario/index.ts", fixture);
			const made = makeContext(fixture, { entries: loaded.entries });
			await fire(loaded.extension, "session_start", {}, made.ctx);
			const newNotes = notes(made.calls);
			const newEntries = cursor(loaded.entries);
			const newSent = cursor(loaded.sent);
			let logged = 0;
			const readCalls = () => {
				const lines = fs.existsSync(log) ? fs.readFileSync(log, "utf8").split("\n").filter(Boolean) : [];
				const fresh = lines.slice(logged);
				logged = lines.length;
				return fresh;
			};

			const ops = [];
			for (const op of c.ops) {
				const out = {};
				if (op.op === "command") {
					await loaded.extension.commands.get("reactor-scenario").handler(op.args, made.ctx);
				} else if (op.op === "tool") {
					const tool = loaded.extension.tools.get("reactor_phase_complete").definition;
					const result = await tool.execute("id", { summary: op.summary }, undefined, undefined, made.ctx);
					out.text = result.content[0].text;
				}
				out.notify = plainNotify(newNotes());
				out.entries = newEntries().map(entryData);
				out.messages = newSent().map((m) => ({ content: m.content, display: m.display }));
				out.reactorCalls = readCalls();
				out.advertised = advertised(loaded, "reactor_phase_complete");
				ops.push({ ...op, out });
			}
			return { name: c.name, scenarios: c.scenarios, ops };
		} finally {
			process.env.PATH = savedPath;
			fs.rmSync(shimDir, { recursive: true, force: true });
		}
	});
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

// One case at a time: the harness points PI_CODING_AGENT_DIR, REACTOR_CONFIG_DIR and
// PATH at each fixture through process-global environment variables, so concurrent
// cases read one another's config files.
const results = { manifest: [], identity: [], reporting: [], scenario: [] };
for (const c of MANIFEST_CASES) results.manifest.push(await runManifest(c));
for (const c of IDENTITY_CASES) results.identity.push(await runIdentity(c));
for (const c of REPORTING_CASES) results.reporting.push(await runReporting(c));
for (const c of SCENARIO_CASES) results.scenario.push(await runScenario(c));

const check = process.argv.includes("--check");
let stale = 0;
for (const [name, cases] of Object.entries(results)) {
	const file = path.join(OUT, `${name}.json`);
	const text = `${JSON.stringify(cases, null, 2)}\n`;
	if (check) {
		const have = fs.existsSync(file) ? fs.readFileSync(file, "utf8") : "";
		if (have !== text) {
			console.error(`capture: ${file} is stale`);
			stale++;
		}
	} else {
		fs.mkdirSync(OUT, { recursive: true });
		fs.writeFileSync(file, text);
		console.log(`wrote ${file} (${cases.length} cases, ${cases.reduce((n, c) => n + c.ops.length, 0)} ops)`);
	}
}
process.exit(stale ? 1 : 0);
