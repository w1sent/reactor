# pi API notes

Facts about pi that REactor's design depends on, established against **pi
0.83.0** installed at `~/.local/lib/node_modules/@earendil-works/pi-coding-agent`.
Items marked **[verified]** were read out of the shipped `dist/` (source or
`.d.ts`); items marked **[docs]** come from <https://pi.dev/docs/latest> and were
not independently confirmed against the code.

Re-check this file when bumping pi. The mechanisms in the "load-bearing" section
are the ones whose removal would break REactor outright.

## Load-bearing mechanisms

### `before_agent_start` can replace the system prompt

**[verified]** `dist/core/extensions/types.d.ts`:

```ts
/** Fired after user submits prompt but before agent loop. */
export interface BeforeAgentStartEvent {
    type: "before_agent_start";
    prompt: string;
    images?: ImageContent[];
    /** The fully assembled system prompt string. */
    systemPrompt: string;
    systemPromptOptions: BuildSystemPromptOptions;
}

export interface BeforeAgentStartEventResult {
    message?: Pick<CustomMessage, "customType" | "content" | "display" | "details">;
    /** Replace the system prompt for this turn. If multiple extensions
        return this, they are chained. */
    systemPrompt?: string;
}
```

This is where the registry block is appended. Note "for this turn" — it fires
once per user turn, not once per LLM call inside the agent loop.

### `resources_discover` can gate skills and prompts

**[verified]** Same file:

```ts
/** Fired after session_start to allow extensions to provide additional
    resource paths. */
export interface ResourcesDiscoverEvent {
    type: "resources_discover";
    cwd: string;
    reason: "startup" | "reload";
}
export interface ResourcesDiscoverResult {
    skillPaths?: string[];
    promptPaths?: string[];
    themePaths?: string[];
}
```

Only fires on `"startup"` and `"reload"`. `ctx.reload()` (command context) is
what makes a toolset toggle take effect without restarting pi.

**[verified]** `dist/core/skills.js` — what a returned `skillPaths` entry may
be. `loadSkills` stats each path:

- a **directory containing `SKILL.md`** is one skill root, and is not recursed
  into;
- a directory *without* one is scanned for direct `.md` children and recursed
  into looking for `SKILL.md`;
- a path to a single `.md` file is loaded as that skill.

So `~/.pi/reactor/skills/<tool>/` — one fetched skill per directory — is handled
exactly as intended, and the fetched tree's own subdirectories (`references/`,
`scripts/`) are not mistaken for further skills. A path that does not exist
produces a warning diagnostic, not a crash.

**[verified]** `dist/core/agent-session.js` `extendResourcesFromExtensions`
returns early when every returned array is empty, and
`resource-loader.js extendResources` *merges* rather than replaces
(`mergePaths(this.lastSkillPaths, …)`). Within one `extendResources` call the
paths accumulate. Removal on toggle-off works because the surrounding reload
rebuilds `lastSkillPaths` from settings first — it is the reload that drops the
path, not the extension returning a shorter list. Worth re-checking if
deactivation ever appears not to take effect.

### Tool results carry an out-of-context `details` field

**[docs]** A tool's return is `{ content, details }`; `details` persists as
session state without entering the LLM context. `pi.appendEntry(type, data)`
persists extension state the same way, restorable by walking
`ctx.sessionManager.getEntries()` on `session_start`.

This is what scenario step-state rides on.

### `context` — the alternative injection point, rejected

**[verified]** `ContextEvent { messages }` → `ContextEventResult { messages? }`,
"Fired before each LLM call. Can modify messages." Freshest possible injection,
but fires once per agent-loop iteration rather than once per user turn, and
rewriting the message array fights prompt caching. Not used; see
[ADR-0006](adr/0006-registry-injected-into-system-prompt.md).

## Packaging and install

**[verified]** `dist/core/package-manager.js`. `pi install git:<repo>` performs
exactly:

1. `git clone <repo> <targetDir>` — **no `--recurse-submodules`**. The string
   `submodule` does not appear anywhere in `package-manager.js`.
2. `git checkout <ref>` if a ref was given.
3. If `package.json` exists: `npm install --omit=dev` in the clone.

Update (`ensureGitRef`) performs:

1. `git fetch <ref>`
2. compare `rev-parse HEAD` against `rev-parse <ref>^{commit}`; return if equal
3. `git reset --hard <ref>^{commit}`
4. **`git clean -fdx`** — comment in source: "Clean untracked files (extensions
   should be pristine)"
5. `npm install --omit=dev` again

Consequences REactor is built around:

- Submodules would arrive empty and stay empty. → tools are sibling repos
  ([ADR-0002](adr/0002-package-ships-assets-tools-are-sibling-repos.md)).
- Untracked build output inside the package is destroyed on every update. →
  nothing is built in-tree.
- **Anything the user edits inside the installed package is destroyed on every
  update.** → the live catalogue lives in `~/.pi/reactor/`
  ([ADR-0003](adr/0003-tools-toml-single-source-of-truth.md)).
- `npm install` is **not** passed `--ignore-scripts` here, so a `prepare` /
  `postinstall` in `package.json` does run. `--ignore-scripts` appears in
  `dist/config.js` only, on pi's *self-update* commands. REactor does not use a
  postinstall hook, but it is available.

**[docs]** Package manifest — a `pi` field in `package.json` naming
`extensions` / `skills` / `prompts` / `themes`, or the conventional directories
of those names at the package root with no manifest at all. REactor uses the
conventional layout.

**[verified]** ⚠ **Do not write `"pi": {}`.** `readPiManifest` returns
`pkg.pi ?? null`, and `{}` is truthy, so `collectPackageResources` takes the
manifest branch, finds `manifest[resourceType]` undefined for every type,
contributes nothing, and `return true`s — never reaching the
conventional-directory fallback. An empty `pi` object ships an empty package.
Omit the key entirely to get auto-discovery.

## Resource discovery paths

**[docs]** unless noted.

| Resource | Global | Project |
|---|---|---|
| Extensions | `~/.pi/agent/extensions/*.ts`, `*/index.ts` | `.pi/extensions/*.ts`, `*/index.ts` |
| Skills | `~/.pi/agent/skills/`, `~/.agents/skills/` | `.pi/skills/`, `.agents/skills/` (cwd or ancestor) |
| Prompts | `~/.pi/agent/prompts/*.md` | `.pi/prompts/*.md` |
| Themes | `~/.pi/agent/themes/` | — |

Project-scoped resources require project trust. Prompt discovery is
non-recursive.

**[verified]** ⚠ **Every `.md` file in a prompts directory becomes a command.**
`loadTemplatesFromDir` (`dist/core/prompt-templates.js`) takes each entry whose
name ends in `.md` and names the template `basename(filePath).replace(/\.md$/,
"")`. There is no frontmatter requirement, no filename convention, and — unlike
the skill loader, which builds an `ignore` matcher from `.gitignore`/`.ignore`/
`.fdignore` — no ignore-file support at all. A `README.md` in `prompts/` is a
`/README` command. This is why the package's resource directories are documented
in `package-resources.md` instead of by a `README.md` in each.

The other two loaders reject a stray `README.md`, but incidentally rather than
by design: themes are filtered to `.json`
(`dist/modes/interactive/theme/theme.js`), and `loadSkillFromFile` returns
`{ skill: null }` when frontmatter has no non-empty `description`
(`dist/core/skills.js`) — after emitting warning diagnostics for it.

**[verified]** Skills are keyed by their declared `name:`, and on a duplicate
pi keeps the **first** loaded and records a collision diagnostic
(`loadSkills`, `dist/core/skills.js`). Defaults — including `~/.agents/skills/`
— are added before extension-contributed paths, so an independently installed
copy of a skill beats the one REactor fetches and pins. Consequences in
`TODO.md`.

**[verified]** `dist/config.js` — path helpers and their defaults:

```
getAgentDir()    ~/.pi/agent          (override: $PI_CODING_AGENT_DIR)
getBinDir()      ~/.pi/agent/bin      (pi's own managed fd/rg live here)
getToolsDir()    ~/.pi/agent/tools
getPromptsDir()  ~/.pi/agent/prompts
getSessionsDir() ~/.pi/agent/sessions
getSettingsPath() ~/.pi/agent/settings.json
```

`CONFIG_DIR_NAME` is `.pi` and `APP_NAME` is `pi`, both overridable via a
`piConfig` block in pi's own `package.json` — relevant only if REactor is ever
rebranded onto a pi fork.

## Skill frontmatter

**[verified]** `dist/core/skills.js` reads exactly three keys:

- `name` — falls back to the parent directory name if absent
- `description` — **required**; a skill with no description does not load
- `disable-model-invocation` — boolean

`compatibility`, `license`, `metadata` and `allowed-tools` are documented as
optional fields but are **not** consumed by the loader. A `requires:` key is
therefore inert to pi and free for REactor to define
([ADR-0003](adr/0003-tools-toml-single-source-of-truth.md)).

Name validation: lowercase alphanumerics and hyphens, 1–64 chars, no leading,
trailing or consecutive hyphens. Name collisions keep the first discovered skill
and warn.

**[docs]** `/skill:<name> [args]` manual invocation requires
`"enableSkillCommands": true` in settings.

## Extension API surface

**[verified]** `ExtensionAPI` in `dist/core/extensions/types.d.ts`. Events
available to `pi.on(...)`:

```
project_trust  resources_discover  session_start  session_info_changed
session_before_switch  session_before_fork  session_before_compact
session_compact  session_shutdown  session_before_tree  session_tree
context  before_provider_request  before_provider_headers
after_provider_response  before_agent_start  agent_start  agent_end
agent_settled  turn_start  turn_end  message_start  message_update
message_end  tool_execution_start  tool_execution_update
tool_execution_end  model_select  thinking_level_select  tool_call
tool_result  user_bash  input
```

Registration and messaging:

```
registerTool  registerCommand  registerShortcut  registerFlag  getFlag
registerMessageRenderer  registerEntryRenderer  sendMessage  sendUserMessage
```

Result types worth knowing:

- `ToolCallEventResult { block?, reason? }` — blocks a tool call. REactor does
  **not** use this ([ADR-0007](adr/0007-deactivation-is-soft.md)).
- `InputEventResult` — `{action:"continue"}` | `{action:"transform", text}` |
  `{action:"handled"}`.
- `MessageEndEventResult { message? }` — replacement must keep the original role.
- `ToolResultEventResult { content?, details?, isError?, usage? }`.

## UI and TUI

**[verified]** `ExtensionUIContext` members include `select`, `confirm`,
`input`, `notify`, `onTerminalInput`, `setStatus(key, text)`,
`setWorkingMessage`, `setWorkingVisible`, `setWorkingIndicator`,
`setHiddenThinkingLabel`, `setWidget(key, string[] | factory, options)` and a
custom-footer factory receiving a `FooterDataProvider`.

**[docs]** `ctx.ui.custom(component, opts)` mounts a custom component (TUI mode
only). Component interface is `render(width)` → `string[]`, `handleInput(data)`,
`invalidate()`. Built-in primitives: `Text`, `Box`, `Container`, `SelectList`,
`SettingsList`, `Markdown`, `BorderedLoader`. Rules: take `theme` from the
callback rather than importing it, call `tui.requestRender()` after state
changes, never exceed the width passed to `render`.

`ctx.mode` is `"tui" | "rpc" | "json" | "print"`; `ctx.hasUI` is true for TUI and
RPC. Everything the selector and status extensions do must degrade cleanly when
`hasUI` is false.

## `pi.exec` never rejects

**[verified]** `dist/core/exec.js`. `execCommand` wraps `spawn` in a promise
that only ever **resolves**:

```js
waitForChildProcess(proc)
  .then((code) => resolve({ stdout, stderr, code: code ?? 0, killed }))
  .catch((_err) => resolve({ stdout, stderr, code: 1, killed }));
```

A binary that is not on `PATH` therefore arrives as `{ code: 1, stdout: "",
killed: false }` — indistinguishable from a command that ran and failed
silently. `try`/`catch` around `pi.exec` is dead code; branch on `killed` (the
timeout or abort case, which also `SIGTERM`s then `SIGKILL`s after 5s) and on
empty stdout instead.

`ExecOptions` is `{ signal?, timeout?, cwd? }`; `shell: false`, so no shell
quoting is involved and no shell is available.

## Imports available to an extension

**[docs]** `@earendil-works/pi-coding-agent`, `typebox`, `@earendil-works/pi-ai`,
`@earendil-works/pi-tui`, Node built-ins, and any dependency declared in a
`package.json` adjacent to the extension (run `npm install` in that directory).

Use `StringEnum` from `@earendil-works/pi-ai` for enum parameters — plain
`Type.Union` does not work with Google's APIs.

Extensions ship as `.ts` and are loaded directly; pi depends on `jiti`, so there
is no build step for extension code.

## Testing

**[docs]** `pi -e ./my-extension.ts` loads an extension from an arbitrary path.
Extensions in auto-discovered locations hot-reload via `/reload`.
