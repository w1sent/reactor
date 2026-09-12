# How to add a toolset to toolsets.toml

A toolset is a named group of tools activated as a unit. Activation shapes
only what the agent is *told about* — it never blocks anything
([ADR-0007](../adr/0007-deactivation-is-soft.md)). The schema reference lives
at the top of `toolsets.toml`; this guide is the sequence.

## 0. Shipped copy vs. user copy

The package's `toolsets.toml` is the seed, copied to
`~/.pi/reactor/toolsets.toml` at install time. **User-defined toolsets go in
the installed copy** — the shipped one only gains a toolset when the change is
meant for every REactor user, in which case edit the repo file and let
`reactor diff-config` surface the difference on user machines
([ADR-0004](../adr/0004-config-updates-via-plain-diff.md)).

## 1. Write the entry

```toml
[toolset.<id>]
desc  = "one line, shown in the selector — not injected into the prompt"
tags  = ["static", "native"]        # intersect: static AND native
tools = ["frida"]                   # only for members that lack the tag
```

- **Prefer `tags` over `tools`.** A tag list keeps working when the catalogue
  grows; a name list must be remembered in two files. Name a tool explicitly
  only when it genuinely does not carry the tag (the shipped `frida` in
  `android`/`ios` is the model: not an Android tool, an everything tool that
  Android work reaches for).
- **Tags intersect** ([ADR-0013](../adr/0013-toolset-tags-intersect.md)):
  `["static", "native"]` is *static AND native*. For "either", activate two
  toolsets — activation unions them — which is why the sets are deliberately
  tight and meant to be combined, not self-sufficient.
- **`all = true`** is for the `all` set only: every catalogue entry, including
  untagged ones.
- **Keep it tight.** A set that tries to be self-sufficient competes with the
  combining rule and ages badly. If a draft toolset is a superset of another,
  it is probably the wrong shape.

## 2. Verify

```bash
reactor toolsets enable <id> --format json   # the recomputed active list
reactor registry                             # what the agent would be told
```

Open the selector (`/reactor-tools`) and check the set reads well next to the
others: two keystrokes to combine, no wall of duplicated tools.

## 3. Where a scenario names it

A scenario step can carry `toolset: <id>` in its frontmatter; activating it
when the agent reaches that step is additive — the previous step's toolset is
never deactivated ([ADR-0007](../adr/0007-deactivation-is-soft.md)). If your
toolset exists to back a stage of `prompts/scenarios/investigation/`, wire it
there and re-run the scenario shape test:

```bash
node --test tests/extensions/scenario.test.mjs
```
