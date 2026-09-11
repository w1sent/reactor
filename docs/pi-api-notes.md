# pi API notes

Facts about pi that REactor's design depends on, established against **pi
0.83.0** installed at `~/.local/lib/node_modules/@earendil-works/pi-coding-agent`.
Items marked **[verified]** were read out of the shipped `dist/` (source or
`.d.ts`); items marked **[docs]** come from <https://pi.dev/docs/latest> and were
not independently confirmed against the code. A handful of items are marked
**upgraded from [docs] to [verified] against pi 0.84.2** where implementing
`extensions/scenario/` required actually confirming them — the rest of the
file has not been re-checked against that version.

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

### `await ctx.reload()` invalidates the ctx (and `pi`) that called it

**[verified]** `dist/core/extensions/loader.js` — `invalidate(message)` sets a
`staleMessage` on the extension's runtime state; `assertActive()`, called from
inside every `pi.*`/`ctx.*` action implementation, throws that message the
moment it is set:

> This extension ctx is stale after session replacement or reload. Do not use
> a captured pi or command ctx after `ctx.newSession()`, `ctx.fork()`,
> `ctx.switchSession()`, or `ctx.reload()`. For newSession, fork, and
> switchSession, move post-replacement work into `withSession` and use the
> ctx passed to `withSession`. For reload, do not use the old ctx after
> `await ctx.reload()`.

Hit for real: `/reactor-toolbox off`'s handler wrote settings, `await
ctx.reload()`d, and then called `ctx.ui.notify(...)` — which now throws,
because `runner.js` calls `invalidate()` the moment `reload()` resolves. The
fix is ordering, not a workaround: do every `ctx`/`pi` action a handler needs
*before* `await ctx.reload()`, and make the reload the literal last statement.
Same constraint applies to `ctx.newSession()`, `ctx.fork()`,
`ctx.switchSession()` — this file only had reload to worry about so far.

`tests/extensions/harness.mjs`'s `makeContext`/`loadExtension` share an
optional `guard` object for exactly this: pass the same `guard` to both and
`ctx.reload()` poisons the whole fake `ctx` (via a `Proxy`) and the `pi.sendMessage`/
`pi.appendEntry` spies, throwing `STALE_CTX_MESSAGE` on anything used
afterward — reproduced against the real bug before the fix landed, confirming
the mock now catches this class of error rather than silently accepting it.

`scripts/check-in-pi.mjs` checks the same thing with no mock at all: a real
`pi --mode rpc` process, real extension files, real `extension_error` events
on the wire. Reproduced the exact crash byte-for-byte (`"extensionPath":
"command:reactor-toolbox"`, the same message above) before the fix, clean
after.

**A second, wider form of the same invalidation exists across commands, not
just within one.** `invalidate()` marks the whole extension instance stale,
not just the handler that called `reload()` — so a *different* command from
the same extension, still suspended on an `await` when someone else's reload
resolves, goes stale too, however carefully its own code is ordered. Found
by accident: `scripts/check-in-pi.mjs` originally paced commands with a fixed
delay rather than waiting for each one's own RPC `response`, and `/reactor`
(no reload of its own) followed quickly by `/reactor-toolbox off`/`on`
(which does) reproduced `"extensionPath": "command:reactor"` — a false
positive from the script sending faster than a real caller would, not a
defect in `/reactor`'s own ordering. Fixed in the script by waiting for each
command's `response` (by `id`) before sending the next, which is what real
usage — a person, or an agent working through one result before issuing the
next — already does by construction. Recorded here because the underlying
fact is real even though this particular repro wasn't: two extension
commands from the *same* extension file, genuinely in flight at once, with
one of them reloading, is a real way to hit `assertActive()` that no amount
of intra-handler reordering fixes.

### Tool results carry an out-of-context `details` field

**[verified]** (upgraded from [docs]; confirmed against pi 0.84.2 —
`pi-agent-core/dist/types.d.ts`) `AgentToolResult<T>` is `{ content, details,
usage?, addedToolNames?, terminate? }`; `content` is what the model sees,
`details: T` is "arbitrary structured details for logs or UI rendering" and
does not enter LLM context. `pi.appendEntry(customType, data?)` persists
extension state the same way as a `CustomEntry` on the session, restorable by
walking `ctx.sessionManager.getEntries()` — `Pick<SessionManager, … |
"getEntries" | …>` — typically on `session_start`, taking the *last* entry
matching your `customType` since state changes over the session's life.

This is what `extensions/scenario/`'s step-state rides on
([ADR-0009](adr/0009-scenarios-advance-by-tool-result.md),
[ADR-0017](adr/0017-scenario-steps-are-read-directly-not-pi-prompts.md)): both
mechanisms, together — the tool's own `details` for the result that just
happened, and an explicit `appendEntry` call for a state pointer that is easy
to find again without re-scanning the transcript for the last matching tool
result.

### `pi.sendUserMessage("/cmd", { expandPromptTemplates: true })` dispatches a real command context, even from inside an event handler

**[verified]** against pi 0.84.2's shipped `core/agent-session.js`, read
directly rather than from the `.d.ts` alone -- traced for
`extensions/reporting/`, which needs this to reach `navigateTree` (below)
from `agent_settled`, a plain event handler that is not handed an
`ExtensionCommandContext`.

`ExtensionAPI.sendUserMessage` (`runner.bindCore`'s `sendUserMessage`) calls
`AgentSession.sendUserMessage(content, options)`, which normalizes `content`
to text and calls `this.prompt(text, { expandPromptTemplates: options
?.expandPromptTemplates ?? false, streamingBehavior: options?.deliverAs,
source: "extension" })`. `prompt()`'s very first branch, *before* it checks
`this.isStreaming` or does anything else:

```js
if (expandPromptTemplates && text.startsWith("/")) {
    const handled = await this._tryExecuteExtensionCommand(text);
    if (handled) { preflightResult?.(true); return; }
}
```

`_tryExecuteExtensionCommand` looks the command up and calls
`this._extensionRunner.createCommandContext()` for it -- the same
`ExtensionCommandContext` a real `/foo` typed by a person gets, complete with
`navigateTree`/`fork`/`newSession`/`reload`. So any event handler can reach
those by calling `pi.sendUserMessage("/own-command", { expandPromptTemplates:
true })` and having that command registered via `pi.registerCommand`.

Two things worth knowing about this path specifically:

- **It bypasses the streaming/queueing logic entirely** -- the command
  dispatch returns before `prompt()` ever reaches its `isStreaming` branch,
  so this works even while the agent is mid-loop (which is exactly when
  `agent_settled`/`turn_end`/etc. fire).
- **`ExtensionAPI.sendUserMessage` itself is fire-and-forget.** `runner.js`'s
  `bindCore` wires it as `(content, options) => { this.sendUserMessage(...)
  .catch(err => runner.emitError(...)); }` -- it does not return the promise,
  so an extension cannot `await` the dispatched command finishing. A handler
  that needs to know the outcome has to observe it some other way (a later
  event, a status/notify call from the command itself), not by awaiting the
  call that triggered it.

No other extension in this repo self-dispatches a command like this --
`scenario/`'s `/reactor-scenario next` and `status/`'s `mute`/`unmute` are
always a person (or a test) calling `registerCommand`'s handler directly.

### `navigateTree` throws while streaming, and does not invalidate `ctx`

**[verified]**, same source pass. `AgentSession.navigateTree(targetId,
options)`:

```js
async navigateTree(targetId, options = {}) {
    if (this.isStreaming) {
        throw new Error("Wait for the current response to finish before navigating the session tree.");
    }
    ...
```

So it cannot be called while the agent loop is still active. `isStreaming`
returns `this._isAgentRunActive`, which `_emitAgentSettled()` sets to `false`
*before* emitting `agent_settled` -- making `agent_settled` the first point
pi itself guarantees a safe call, and `turn_end` (fired mid-loop, per LLM
round trip) an unsafe one for this specifically, even though it looks like
the more natural "a turn just happened" hook.

Unlike `reload`/`newSession`/`fork`/`switchSession` -- which call
`this._extensionRunner.invalidate(...)`, poisoning the calling `ctx` and
`pi` the instant they resolve (see the `ctx.reload()` note above) --
`navigateTree` never calls `invalidate()`. It moves the active leaf pointer
within the same `SessionManager` rather than replacing the session, so the
same `ctx` stays valid afterward: a command handler can `await
ctx.navigateTree(...)` and then keep using `ctx`/`pi` (`extensions/reporting/`
does exactly this, following the revert with `pi.sendUserMessage(...)` to
resend the prompt on the now-rewound branch).

### `context` — the alternative injection point, rejected for the registry, load-bearing for rolling-context and context-editor

**[verified]** `ContextEvent { messages }` → `ContextEventResult { messages? }`,
"Fired before each LLM call. Can modify messages." Freshest possible injection,
but fires once per agent-loop iteration rather than once per user turn, and
rewriting the message array fights prompt caching. Not used for the registry
block; see [ADR-0006](adr/0006-registry-injected-into-system-prompt.md). It is
exactly the right hook for something that *removes* rather than injects,
though — `rolling-context/`'s fade and `context-editor/`'s current-branch
filter both trim `event.messages` here, and neither cares about the caching
cost since what they return is smaller, not different, on the common turn.

### `before_agent_start` chains systemPrompt, and `ctx.getSystemPrompt()` sees the chain

**[verified]** (pi 0.85.1, `runner.js` `emitBeforeAgentStart`) each handler's
`{ systemPrompt }` result becomes the next handler's `event.systemPrompt` —
appending a section needs no assumption about extension order. The chained
prompt is written into `agent.state.systemPrompt` (`agent-session.js:932`),
and the ctx's `ctx.getSystemPrompt()` returns exactly that
(`agent-session.js:2088`), including during later `context` events — which is
how the fade's budget math accounts for goal-setting's manifest block with no
knowledge of goal-setting ([ADR-0024](adr/0024-rolling-context-splits-into-goal-setting-history-tools-and-the-fade.md)).

**[verified]** the *order* of that chain is the order extensions were
loaded, and for a package's `extensions/` directory that is the unsorted
`readdirSync` order of `collectAutoExtensionEntries` (`package-manager.js`) —
filesystem enumeration order, not alphabetical by guarantee, not
controllable from the package. Extensions must therefore compose
order-independently: `before_agent_start` appends are fine, two extensions
both rewriting `event.messages` in `context` are not — whoever loads later
acts on the other's output.

### Compaction primitives are exported, not internal

**[verified]** (pi 0.83.0's `index.d.ts`) `calculateContextTokens`,
`DEFAULT_COMPACTION_SETTINGS`, `estimateTokens`, `findCutPoint`,
`findTurnStartIndex`, `getLastAssistantUsage`, `serializeConversation`,
`shouldCompact`, and `SessionEntry`/`SessionManager`/`buildContextEntries`/
`sessionEntryToContextMessages`/`getLatestCompactionEntry` are all re-exported
from the package root, not internal to `dist/core/compaction/`. An extension
that needs to trim or measure context does not have to reinvent any of this —
and should not: `rolling-context/`'s original bug (ADR-0020) was exactly a
hand-rolled chars/4 estimate and a positional slice standing in for these.

- `estimateTokens(message: AgentMessage): number` — pi's own per-message
  chars/4 estimator. The same function `getContextUsage()` and
  `shouldCompact()` measure against; using anything else for a budget decision
  guarantees disagreement with what pi itself reports.
- `findCutPoint(entries, startIndex, endIndex, keepRecentTokens):
  CutPointResult` walks `SessionEntry[]` newest → oldest and never returns a
  cut point at a `toolResult` (`isCutPointMessage`/`findValidCutPoints`,
  internal but trivial to mirror against `AgentMessage.role` directly when
  what needs cutting is a live message array rather than session entries —
  see `findSafeCut` in `extensions/rolling-context/index.ts`).
- `ctx.sessionManager.buildContextEntries()` **is** on `ReadonlySessionManager`
  (`Pick<SessionManager, … | "buildContextEntries" | …>`), despite being the
  function pi uses internally to build the compaction-aware, leaf-path entry
  list. An extension gets the same view pi's own turn loop does, not an
  approximation of it.
- `getContextUsage()` itself (`dist/core/agent-session.js`) does **not**
  re-measure every message on every call: it takes the last valid assistant
  `usage` on the branch (real, provider-reported tokens for whatever was
  actually sent) and adds `estimateTokens` only for messages *after* that —
  `estimateContextTokens` in `core/compaction/compaction.js`. This is why a
  fade that changes what gets sent shows up correctly in the footer/threshold
  check on the very next response: the real number comes from the provider,
  not from re-deriving it.

### `session_before_compact`'s three reasons are not interchangeable

**[verified]** `SessionBeforeCompactEvent.reason: "manual" | "threshold" |
"overflow"`. `"threshold"` is pi's own proactive compaction, checked against
`getContextUsage()`. `"overflow"` is different in kind, not just in trigger:
it only fires from `_checkCompaction`'s "Case 1" — *after* a request has
already been rejected or truncated for exceeding the context window — as a
last-resort compact-and-retry. Cancelling `session_before_compact`
unconditionally for every non-manual reason (as `rolling-context/` originally
did) blocks that recovery along with the proactive compaction it was meant to
preempt, and a session that ever hits real overflow with no recovery path has
no way back (ADR-0020). An extension that wants to preempt only the proactive
path should check `event.reason === "threshold"` specifically.

### `ctx.newSession()`'s `setup` gets a real, writable `SessionManager`

**[verified]** `ExtensionCommandContext.newSession(options?: { parentSession?,
setup?: (sessionManager: SessionManager) => Promise<void>, withSession? })`.
Unlike `ctx.sessionManager` (`ReadonlySessionManager` everywhere else), the
`SessionManager` `setup` receives is the full class —
`appendMessage`/`appendCustomEntry`/`appendCompaction`/`branchWithSummary` and
the rest — because it is building the *new* session before it becomes active,
not reading the current one. `appendMessage(message: Message | CustomMessage |
BashExecutionMessage)` pointedly excludes `BranchSummaryMessage` and
`CompactionSummaryMessage`, which get their own dedicated append methods —
there is no generic "append any AgentMessage" call. `context-editor/`'s fork
path (ADR-0021) is built entirely on this: replay the kept entries' messages
into a fresh session via `appendMessage`, skip the two message roles it
cannot take.

**Confirms and extends the `reload()` invalidation note above**: `newSession`
(like `fork`/`switchSession`) invalidates the calling `ctx` the moment it
resolves, same as `reload()` — this file previously only had `reload()`
actually reproduced against the real bug. `context-editor/`'s fork branch hit
it directly in testing (a `ctx.ui.notify()` call placed *after* `await
ctx.newSession(...)`, caught by `tests/extensions/harness.mjs`'s `guard`
wired the same way for `newSession` as it already was for `reload`): the fix
is the same ordering discipline, do every `ctx`/`pi` action first, make the
session-replacing call the literal last statement.

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
RPC. An overlay must therefore be gated on `ctx.mode === "tui"` — `hasUI` is the
wrong test, because RPC has dialogs but no terminal to mount a component into.

**[verified]** `ctx.ui.custom<T>(factory, options)` returns a `Promise<T>` that
settles when the factory's `done(result)` callback is called. The factory is
`(tui, theme, keybindings, done) => Component`, and `options.overlay` floats it
above the transcript. The component's own `handleInput(data)` receives raw key
data while it holds focus.

**[verified]** The two list primitives are narrower than they look
(`node_modules/@earendil-works/pi-tui/dist/components/`):

- `SelectList` has **no filter input of its own** — `setFilter` must be driven
  from outside, and it is a `startsWith` test on `item.value`, not a fuzzy one.
- `SettingsList` does have `enableSearch`, but its `fuzzyFilter` runs over
  `item.label` only, and an item may carry `values` (cycled on Enter/Space) *or*
  a `submenu`, never both.

`fuzzyFilter`, `truncateToWidth`, `visibleWidth` and `getKeybindings` are all
exported from the package root, so rendering a list by hand is a small job when
neither primitive fits.

**[verified]** `ctx.ui.setWidget(key, content, options)` puts persistent lines
next to the editor — `WidgetPlacement` is `"aboveEditor" | "belowEditor"`,
defaulting to the former. `content` is either a `string[]` or a
`(tui, theme) => Component` factory; take the factory form to get the theme.
Calling it again with the same key replaces the widget, and `undefined` clears
it, so "repaint" is just another call with fresh data — a widget needs no
setter and no mutable state of its own. Like `custom`, it needs a real terminal:
gate it on `ctx.mode === "tui"`.

**[verified]** `pi.sendMessage` is **not** free of context. `convertToLlm`
(`dist/core/messages.js`) maps a `role: "custom"` message to a **user** message,
content unchanged. To show something to the person without showing it to the
model, use `pi.appendEntry(customType, data)` — documented as "not sent to
LLM" — with a `pi.registerEntryRenderer(customType, renderer)` to draw it. The
renderer receives `{ expanded }`, so a long body should render short by default.

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

**[verified]** Those are not ordinary resolutions. `dist/core/extensions/loader.js`
builds the jiti instance with an `alias` map (or `virtualModules` in the Bun
binary) pointing each of those specifiers at pi's own copy, so a runtime import
of `@earendil-works/pi-tui` works from a package that has no `node_modules` of
its own and gets **the same module instance pi is using** — which is why
`getKeybindings()` inside an extension sees the user's keybindings and not a
fresh default set. Anything not in that map resolves by normal Node rules from
the extension's own directory.

Use `StringEnum` from `@earendil-works/pi-ai` for enum parameters — plain
`Type.Union` does not work with Google's APIs.

Extensions ship as `.ts` and are loaded directly; pi depends on `jiti`, so there
is no build step for extension code.

**[verified]** Each extension gets **its own jiti instance**: `loadExtensionModule`
calls `createJiti(import.meta.url, { moduleCache: false })` per extension path.
So a module imported by two extensions is *instantiated twice*, and two
extensions cannot share state through a common import — they get shared code and
separate state, which looks like it works and then drifts. Cross-extension state
has to go through something outside the process
([ADR-0014](adr/0014-extensions-share-the-cache-not-each-other.md)) or through
`pi.events`.

## Testing

**[docs]** `pi -e ./my-extension.ts` loads an extension from an arbitrary path.
Extensions in auto-discovered locations hot-reload via `/reload`.
