# pi itself is a catalogue entry, and its invoke example is load-bearing

`pi` — the harness this agent is already running inside
([ADR-0001](0001-pi-is-the-only-target-harness.md)) — is now `[tool.pi]` in
`tools.toml`: detect by binary, a new `agent` tag, a new `[toolset.agent]`
selecting on it.

```toml
[tool.pi]
desc   = "spin up a fresh agent for a subtask; non-interactive only (-p), never bare"
invoke = 'pi -p "<prompt>"'
detect = { binary = "pi" }
tags   = ["agent"]
```

The `desc` and `invoke` both exist to prevent one specific failure: a model
running bare `pi` from a Bash tool call. Interactive `pi` wants a real
terminal; a Bash tool call has none, so that call hangs until the tool
timeout kills it — an entire subagent turn burned on a footgun the catalogue
entry exists to describe away. `invoke` is not a decorative example here, the
way it can be for a familiar CLI — it is the one line standing between a model
reading the registry and getting `-p` right the first time.

## Why

**This is a real capability, not a novelty.** A self-contained subtask —
"summarize this 4000-line log", "check whether this string is valid
base64-decoded shellcode", "triage this second sample while I keep working on
the first" — is often cheaper delegated to a fresh agent than done inline: it
does not consume the calling agent's context, and it can run detached.
`reactor` already advertises everything else on the machine worth reaching
for; the harness itself reaching for another instance of itself is the same
kind of fact, and withholding it because it is recursive would be
inconsistent with cataloguing angr for symbolic execution or joern for
dataflow queries.

**`-p` is specifically what "non-interactive, run to completion" means.**
pi's own `--help` calls it out in exactly those words: "Non-interactive mode:
process prompt and exit." `--mode json` and `--mode rpc` change the *output
shape*, not this — RPC mode in particular is a persistent JSONL server over
stdin/stdout (`docs/rpc.md`), the opposite of "run once and exit" a delegated
subtask wants. `-p` is the one flag that gets both "no TTY needed" and "exits
when done" at once, so it is the one the `invoke` example shows.

**A new tag and toolset, not folded into `general`.** `[toolset.general]`'s
whole membership is "search and JSON — worth adding back to any narrow set"
([ADR-0013](0013-toolset-tags-intersect.md) already establishes that toolsets
are tight on purpose). Widening that toolset's stated meaning to also cover
"delegate to another agent" would make its own `desc` a lie about what is
inside it — the exact failure mode
[`test_a_toolset_named_after_a_tag_selects_only_that_tag`](../../tests/test_reactor.py)
exists to catch, one level up from where that test looks. Agent delegation is
its own kind of task, on the same footing as `network` or `firmware`; it gets
its own toolset for the same reason those do.

**No service probe.** `bn` and `adb` declare one because there is a
*persistent* thing to be up or down — a BN session, a connected device. A `pi
-p` invocation is a one-shot subprocess with no session to poll between calls;
"is a subagent currently running" is not a fact this catalogue's `service`
schema is shaped to answer, and inventing a probe for it would be the same
mistake `TODO.md` already declined for network reachability in `status/`
(nothing in the schema describes it, and inventing one to fill a gap is how a
schema gets a feature nobody asked for).

## Consequences

- **Recursion depth, cost, and whether a spawned subagent may itself spawn
  another are all out of scope here.** The catalogue describes what a tool
  is and how to reach for it; it does not enforce how it is used
  ([ADR-0007](0007-deactivation-is-soft.md)) — a runaway spawn chain is a
  prompting and budget concern for whoever writes the calling agent's
  instructions, not a schema REactor's `tools.toml` was ever going to police.
- **`pi` can report absent even while it is unmistakably the running
  harness**, if the binary that launched this session is not the one named
  `pi` on this `PATH` (a differently-named wrapper, a Bun binary placed
  somewhere else). Ordinary and already handled: `reactor doctor` shows how
  to get it, the same as any other tool that is technically present but not
  discoverable by the declared `detect`.
- **`reactor install pi`** runs `npm install -g --ignore-scripts
  @earendil-works/pi-coding-agent` — verified against the real npm registry
  (`scripts/verify-recipes.py`), matching pi's own documented install command
  exactly, `--ignore-scripts` included. Installing or upgrading the tool
  currently running the installer is unusual but not unsafe; nothing about it
  needs special-casing.

## Considered and rejected

- **`invoke = "pi"`** (bare, matching most other single-word entries).
  Rejected: the whole point of this entry is that bare invocation is the
  wrong thing to do, so the one line demonstrating usage cannot be the one
  line that gets it wrong.
- **Recommending `--mode rpc`** for structured, scriptable output. Rejected
  as the default: RPC mode is a long-lived protocol server meant for
  embedding pi in a custom UI, not a quick delegated subtask — a much heavier
  thing to reach for than `-p`, and the wrong answer to "run once and exit."
  `--mode json` combined with `-p` remains available for a caller that wants
  structured output without the RPC server.
- **A service probe checking whether a spawned `pi` subprocess is still
  running.** Rejected with the no-service-probe point above — there is no
  persistent session for this tool the way there is for `bn`.
- **Tagging `pi` `general`** rather than giving it its own tag. Rejected per
  the toolset point above: `general`'s stated meaning is specific, and
  widening it to fit a new tool the toolset was never about is the mistake
  the toolset-naming test exists to catch.
