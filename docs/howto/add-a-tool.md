# How to add a tool to the catalogue

A tool is one `[tool.<id>]` entry in `tools.toml` plus, when the tool talks to
something that runs, a service probe and an install table. The schema reference
lives in the file itself (`tools.toml`, "Schema" comment block) and the
decisions behind it in [ADR-0003](../adr/0003-tools-toml-single-source-of-truth.md)
and [ADR-0010](../adr/0010-install-recipes-keyed-by-package-manager.md). This
guide is the sequence.

## 0. Where to edit

`tools.toml` at the package root is the **shipped seed**. A user machine reads
the installed copy at `~/.pi/reactor/tools.toml` — never edit that one in the
package tree on a user machine (pi's package update runs `git clean -fdx`).
Work here in the repo; after an update, `reactor diff-config` shows the
difference against an installed copy and you merge by hand
([ADR-0004](../adr/0004-config-updates-via-plain-diff.md)).

## 1. Write the entry

Start from an existing entry that resembles your tool — `[tool.bn]` (service +
upstream skill), `[tool.bindiff]` (manual install + REactor-authored skill),
`[tool.lief]` (Python module) cover the shapes.

- **`desc` is the highest-leverage text in the project.** It lands in every
  system prompt for as long as the tool is installed and active. One line,
  capability not usage — `<tool> --help` is the usage documentation.
- **`invoke`** is what the agent types — shown in the registry block.
- **`detect`** is exactly one of `binary` (a `shutil.which` lookup) or
  `python_module` (importlib, in the interpreter `[probe].python` names). If
  neither detects your tool, that is a schema conversation, not a hack.
- **`version`** is optional argv whose stdout carries a version string.
- **`tags`** are free-form but shared vocabulary with `toolsets.toml` — reuse
  existing tags (`static`, `native`, `forensics`, …) before inventing one.
- **`service`** only when the tool drives something that may not be running.
  Exit 0 is up. The registry carries at most a *count* (`count = { pattern =
  'regex', noun = "device" }`) — never free text, because free text rewrites
  the system prompt every time a service jitter
  ([ADR-0006](../adr/0006-registry-injected-into-system-prompt.md)).
- **`skill`** only when the tool's author ships an Agent Skill:
  `{ source = "git+URL", path = "…", ref = "…" }`. No skill is normal —
  `--help` is the documentation. Writing a REactor-authored skill instead is a
  decision with a bar: cross-tool workflow knowledge that no `--help` contains
  ([ADR-0008](../adr/0008-aggregate-upstream-skills.md), and
  `docs/package-resources.md` § `skills/`).

## 2. Write the install table

`[tool.<id>.install]` keys are **package managers, not distributions** — a key
whose manager binary is on `PATH` is a candidate, ranked by
`[platform].prefer`. Anything else (`manual`, a bare URL) is a note: always
shown, never selected, never executed.

Then run:

```bash
python3 scripts/verify-recipes.py
```

It fails if a recipe names a package that does not exist in its manager's
index. Re-run it every time you touch an install table.

## 3. Verify

```bash
reactor doctor          # the tool appears: present, with your desc
reactor registry        # the block the agent sees — check the line, the
                        # invoke column, and that the block is unchanged
                        # for every other tool
python3 tests/test_reactor.py   # TestShippedConfig checks the shipped
                                # catalogue's shape, not its prose
```

The registry block must stay byte-identical for an unchanged machine
([ADR-0006](../adr/0006-registry-injected-into-system-prompt.md)); if your
`desc` is two lines, or your service probe emits free text, determinism is the
first thing to break.

## 4. If it opens a new area

Check `docs/concept.md`'s target surface. If the tool covers an area already
listed, nothing else to do. If it opens a *new* area, add it there — and if it
needs a schema change, that wants an ADR first.
