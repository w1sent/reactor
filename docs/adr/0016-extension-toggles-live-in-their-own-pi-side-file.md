# Extension toggles live in their own file next to pi's settings, not inside settings.json

Two independent, off-by-default-on toggles:

- **`toolbox`** (`tool-registry/` + `selector/`) — `false` removes both
  extensions from the running session as if they were never loaded: no
  `/reactor` or `/reactor-tools` command, no status-line entry, no registry
  block appended to the system prompt, no `resources_discover` answer.
- **`hiddenServices`** (`status/`) — a list of catalogue ids (`["adb"]` today)
  omitted from the footer and the panel, without touching the rest of what
  `status/` shows.

Both live in one small JSON file, `reactor.json`, in pi's global agent
directory (`getAgentDir()`, normally `~/.pi/agent/`) — next to `settings.json`,
`models.json` and `auth.json`, but not inside `settings.json` itself:

```json
{
  "toolbox": false,
  "hiddenServices": ["adb"]
}
```

Absent file, unreadable JSON, or a field of the wrong shape all mean "use the
default" (`toolbox: true`, `hiddenServices: []`) — this is a preference a
person sets by hand once, not a contract to fail loudly over.

## Why

**pi's own `Settings` type has no extension point, and a small file per
extension is pi's documented pattern for exactly this.** `Settings` in
`settings-manager.d.ts` is a closed interface — no index signature, no
`extensions` bag — so a third-party field placed inside `settings.json` would
survive there only because `SettingsManager.persistScopedSettings` happens to
merge over whatever unknown keys are already on disk, not because pi commits to
carrying it forward. pi's extension docs show the actual sanctioned shape:
`join(ctx.cwd, CONFIG_DIR_NAME, "my-extension.json")` — an extension owns a
file named for itself, at project scope, using `CONFIG_DIR_NAME` rather than a
hardcoded `.pi`. This reuses that pattern at global scope with
`getAgentDir()`, because a machine-wide "hide this feature" toggle is closer in
kind to `defaultProjectTrust` or `theme` than to anything a single repository
should decide.

**Global scope is not a simplification here — `toolbox` cannot be read at
project scope at all.** `ExtensionFactory` is `(pi: ExtensionAPI) => void`; it
has no `ctx` and therefore no `cwd` and no `isProjectTrusted()`. The gate that
decides whether `/reactor` and `/reactor-tools` register has to run at that
call, before any event fires and before any project is known — global is the
only scope reachable from there, at any trust level. `hiddenServices` does not
have this constraint on its own (it is read inside `fetchServices`, which does
have a `ctx`), but putting it in a second, project-scoped file for one of two
fields would be a two-tier design bought for a footer preference nobody asked
to vary per repository.

**Reading it is not the shared-mutable-state problem `cache.json` solved for.**
`extensions/README.md`'s rule against a shared module is about *state*: two
jiti instances of an imported module diverge because each holds its own copy.
A `readFileSync` + `JSON.parse` with no state at all cannot diverge — every
extension that calls it sees the same file. So there is no correctness reason
this reader could not be one shared module; it stays duplicated (a dozen lines,
once each in `tool-registry/` and `selector/`, and a different dozen in
`status/`) purely because every extension here is already a single
self-contained file by house style, and a third small module for a three-line
function is not worth breaking that.

**`hiddenServices` is a list of catalogue ids, not a boolean per known tool.**
Only `bn` and `adb` declare a service probe today, and a `hideAdb` /
`hideBn`-shaped schema would need a code change here *and* in `tools.toml`
every time a new service-backed tool is catalogued — the same distro-specific
hardcoding [ADR-0010](0010-install-recipes-keyed-by-package-manager.md)
rejected for install keys, one level up. A list of ids a person already reads
off `reactor doctor` costs nothing extra today and needs no code change when a
third service-probed tool arrives.

## Consequences

- **`toolbox: false` takes effect on the next load, not mid-session.**
  Registration happens once, at the top of each extension's factory function,
  before any event handler exists to react to a file changing under it. `/reload`
  re-invokes the factory and picks up a new value; a running session does not.
- **No CLI surface for this file.** `reactor` stays ignorant of pi
  ([ADR-0005](0005-reactor-cli-stdlib-python.md)) — this file's location and
  format are pi's convention, not the catalogue's, so teaching the Python CLI
  to write it would be teaching the wrong tool about the wrong platform. A
  person edits two JSON fields by hand.
- **No `"version"` field.** Every other file `reactor` persists carries one
  because each has a real migration story (`state.json`'s toolset/tool shape
  has already changed once). `toolbox` and `hiddenServices` are two
  independently-defaulted, unrelated fields with nothing to migrate between —
  adding a third field later needs no version bump, and a version field here
  would be process for its own sake.
- **Disabling `toolbox` hides only `tool-registry/` and `selector/`.**
  `status/` is a different question ("what is running" vs. "what is on this
  machine and told to the agent") and keeps its own toggle.

## Considered and rejected

- **A `"reactor": {...}` key inside `settings.json` itself.** Works today only
  because `persistScopedSettings` happens to preserve unknown keys across a
  save — an implementation detail of `SettingsManager`, not a contract pi
  documents or tests for third parties. pi's own docs show a different,
  intentional pattern (a file per extension) for exactly this need; reaching
  for the accidental one instead would be building on a foundation pi's
  maintainers are free to change without calling it a breaking change.
- **Project-scoped override of `toolbox`, mirroring `.reactor/state.json`**
  ([ADR-0003](0003-tools-toml-single-source-of-truth.md)). Rejected because it
  is not reachable: the registration decision runs before any `ctx` exists, so
  there is no `cwd` to look a project override up against, trusted or not.
- **A `reactor settings` subcommand to write this file.** Rejected with the
  no-CLI-surface point above — and because two fields set once do not need a
  writer at all, only documentation of where the file is.
- **One boolean per catalogued service** (`hideAdb`, `hideBn`, …). Rejected as
  the same hardcoding [ADR-0010](0010-install-recipes-keyed-by-package-manager.md)
  already ruled out for install recipes: a fixed enum of today's two
  service-probed tools baked into the schema, needing a code change for every
  future one, instead of a list of ids that already have a name everywhere
  else in REactor.
