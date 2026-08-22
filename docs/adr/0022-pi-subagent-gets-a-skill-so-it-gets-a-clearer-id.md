# pi-subagent gets a skill, so it gets a clearer id

`[tool.pi]` ([ADR-0018](0018-pi-itself-is-a-catalogue-entry-non-interactive-only.md))
is renamed `[tool.pi-subagent]`. `name` stays `"pi"` — that is still the
product's actual name, shown as the registry's left column exactly as
before. Only the catalogue *id* changes: what `tags`, `requires:`, `reactor
tools show <id>`, and now a skill's frontmatter refer to it by.

Landing alongside it: `skills/pi-subagent/`, the first entry in `skills/` —
a real answer to the question `TODO.md` and
[`docs/package-resources.md`](../package-resources.md) both left open
("decide before the first skill lands in it").

## Why

**A skill needs a name to bind to, and `pi` was already taken.** Skill
frontmatter's `requires:` (documentation today, intended to eventually gate
loading on activation state — `docs/package-resources.md`) lists catalogue
ids. Writing `requires: [pi]` would work today, syntactically, but reads as
"requires the pi harness itself" — true of every skill in this package,
every extension, the whole session. `requires: [pi-subagent]` says the
narrower, actually-true thing: this skill is about *delegating to a
subagent*, one specific way of reaching for `pi`, not about `pi` in general.

**The rename earns its keep independently of the skill, too.** A registry
line reading `pi-subagent  spin up a fresh, scoped subagent; ...` tells a
model what the entry is *for* from the id alone, before it reads `desc` at
all. `pi` bare does not — recursive and ambiguous is exactly the shape of
name ADR-0018 already had to spend a paragraph defusing ("Recursive, and not
treated specially for it"). The clearer id was available the whole time;
adding a skill is what made spending it worth doing now rather than leaving
it as a nice-to-have.

**Not a second catalogue entry alongside `pi`.** Both would `detect = {
binary = "pi" }` against the exact same binary and print two registry lines
for one tool — the thing `desc` fields exist to prevent duplicating in the
first place. One entry, renamed, is the only version of this that does not
put contradictory advice in front of the same fact twice.

**Whether `skills/` gating matters here: it does not, for this one.**
`docs/package-resources.md`'s open question — a skill in `skills/` loads
unconditionally, regardless of whether its `requires:` tool is present or
active, because package-root discovery runs before `resources_discover` —
is real and still unresolved. It costs nothing for `pi-subagent` specifically:
the tool in question is the harness this session is already running inside,
so "is `pi-subagent` present" is true by construction almost always (the one
exception, a differently-named `pi` binary or wrapper, is already documented
in ADR-0018's Consequences and unrelated to skill loading). The first skill
to land does not force the gating decision; a second one whose tool is
*not* always present would.

## Consequences

- **`reactor install pi-subagent`**, not `reactor install pi`, is the
  command now — same recipe, `npm install -g --ignore-scripts
  @earendil-works/pi-coding-agent`, unaffected by the id.
- **`[toolset.agent]` needed no change.** It selects by `tags = ["agent"]`,
  not by id, so the rename is invisible to toolset membership.
- **ADR-0018 is not rewritten.** It correctly describes the entry as it was
  when written; this ADR is the record of what changed and why, the same
  pattern `TODO.md`'s "Resolved" section already uses for revisiting earlier
  decisions without pretending they were wrong.
- **The skill teaches the one thing `pi --help` cannot connect on its
  own**: that pi's own tool-scoping flags (`--no-tools`, `--no-builtin-tools`,
  `--tools`, `--exclude-tools`) are an *enforced* boundary on what a
  delegated subagent can call, while REactor's own toolbox/toolset
  activation is *advisory* (ADR-0007) — narrowing the registry steers a
  subagent that still has `bash`, it does not stop one. Conflating the two
  is the one mistake this skill exists to prevent; it is cross-tool
  synthesis (pi's CLI + this package's own activation model), which is
  exactly the bar `docs/package-resources.md` sets for anything landing in
  `skills/` at all.

## Considered and rejected

- **Leave the id `pi`, bind the skill to it anyway.** Rejected per the Why
  above: `requires: [pi]` is true of everything in the session and
  communicates nothing about what the skill is actually for.
- **A second catalogue entry, `pi-subagent`, alongside the existing `pi`.**
  Rejected: same binary, same `detect`, two registry lines describing one
  fact differently — the exact duplication a single-source-of-truth
  catalogue ([ADR-0003](0003-tools-toml-single-source-of-truth.md)) exists
  to prevent.
- **Force the `skills/` gating decision now, since a skill is finally
  landing.** Rejected as unnecessary work ahead of need: this skill's tool
  is always present by construction, so gating changes nothing observable
  for it. Left for whichever skill actually needs it, per `TODO.md`.
