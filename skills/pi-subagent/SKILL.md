---
name: pi-subagent
description: Delegate a self-contained subtask to a fresh, non-interactive pi agent, scoped via pi's own CLI flags to exactly what it should be able to touch.
requires: [pi-subagent]
---

# pi-subagent

`pi` is on this machine's `PATH` because it is the harness this agent is
already running inside. Spinning up a second instance for a bounded
subtask -- summarize a 4000-line log, check whether a string is valid
base64-decoded shellcode, triage a second sample while the current one stays
in progress -- is often cheaper than doing it inline: it costs none of the
calling agent's own context, and it can be scoped tighter than the calling
agent itself.

This skill is about the two things `--help` alone does not connect: running
it non-interactively, and *actually limiting* what the delegated agent can
do -- not just telling it to behave.

## Non-interactive: `-p`, never bare

```
pi -p "summarize the crash pattern in this log: <paste, or @file>"
```

Bare `pi` (no `-p`) wants a real terminal. Invoked from a tool call that has
none, it hangs until the timeout kills it -- an entire turn burned on this
one mistake. `-p` is pi's own `--help` wording for it: "Non-interactive mode:
process prompt and exit."

Two variations worth knowing:
- `pi @findings.txt -p "what's the root cause?"` -- attach a file instead of
  pasting its contents into the prompt string.
- `pi -p --mode json "..."` -- structured JSON on stdout instead of plain
  text, for a caller that parses the result rather than reading it. For a
  persistent, embeddable protocol server instead of a one-shot subtask,
  reach for `--mode rpc`.

## Scoping: two mechanisms, only one of them enforced

It is tempting to scope a subagent by telling it what to do in the prompt
("only look at strings, don't run anything"). That is advice, not a
boundary -- indistinguishable, from the model's perspective, from every other
instruction it might or might not follow closely. Two real mechanisms exist,
and they are not interchangeable:

**1. pi's own tool flags -- the actual boundary.** These control which of
*pi's own callable tools* (`bash`, `edit`, `read`, `write`, `find`, `grep`,
`ls`, plus anything an extension registers) the subagent process can invoke
at all. A tool it does not have is not callable, full stop -- this is
enforcement, not steering.

```
--no-tools, -nt              no tools at all -- pure text in, text out
--no-builtin-tools, -nbt     no bash/edit/read/write/find/grep/ls, but
                              extension tools (if any load) still work
--tools, -t <list>           allowlist by name, comma-separated
--exclude-tools, -xt <list>  denylist by name, comma-separated
```

**2. REactor's own toolbox/toolset activation -- advisory, not enforced.**
`reactor tools enable|disable`, `/reactor-tools`, `/reactor-toolbox off` all
change what the *registry block* advertises in the system prompt; nothing
about it is a permission check. Narrowing this reduces noise and steers the
subagent toward the tools relevant to its task -- it does **not** stop it
from running `bn`/`frida`/anything else via `bash`, because those are
external binaries invoked *through* bash, not pi tools of their own. If
`bash` is in the subagent's allowlist, toolset narrowing is a hint, not a
wall.

**The rule that follows from this:** if the subagent genuinely must not run
arbitrary commands or touch the filesystem, that has to come from `-nt`/`-t`
without `bash`/`write`/`edit` -- never from prompt wording or toolset
narrowing alone. If it needs to run RE tools but should be pointed at the
right ones, combine both: keep `bash` in `--tools`, *and* narrow the
catalogue it sees, so what it reaches for is both possible and relevant.

## Recipes

**Pure summarization / analysis, no side effects at all:**
```
pi -p --no-tools "summarize what changed in this diff and why it might have broken the build: @diff.txt"
```

**Read-only triage** -- can inspect files and run read-only RE tools via
bash, cannot write or edit anything:
```
pi -p -t bash,read,find,grep "run `strings` and `file` on this sample, report anything suspicious: @sample.bin"
```

**Bounded triage delegation** -- keep `bash` so it can drive real RE tools,
narrow *which* it's steered toward so the extra registry noise from an
unrelated toolset (dynamic, mobile, ...) doesn't compete for its attention:
```
reactor toolsets enable triage
pi -p -t bash,read "triage this binary: is it packed, what's the entry point, any obvious strings worth following up": @target
reactor toolsets disable triage   # only if this changed the machine's default
```
(`reactor tools`/`toolsets` activation is per-machine state in
`~/.pi/reactor/state.json`, not per-invocation -- toggling it for a subagent
toggles it for whoever runs `pi` next in the same environment too, until
toggled back. Fine when the delegation's toolset already matches what the
machine normally runs; worth restoring afterward when it does not.)

**Extra constraints beyond the registry:** `--append-system-prompt "confine
all file access to /tmp/workdir"` layers additional text onto the default
system prompt for just this invocation -- for a rule specific to the
delegation itself, not something worth teaching the whole catalogue about.

**Leave no trace:** `--no-session` makes the subagent's run ephemeral --
nothing added to session history for someone to stumble on later. Reach for
this by default for a one-shot delegated task; drop it only when the
subagent's work is meant to be resumable.

## Composing a full call

```
pi -p --no-session --model sonnet -t bash,read,find,grep \
  --append-system-prompt "Report findings only; treat the sample read-only." \
  "Triage /samples/second.bin for obvious packing or anti-debug tricks while the primary analysis continues on the first sample."
```

Non-interactive, scoped to read-only tools (enforced, not advised),
ephemeral, with one extra constraint layered on top of the default prompt --
every piece here is doing something the others cannot substitute for.
