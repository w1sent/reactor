# identity is a persona selected by the human, appended to the system prompt, with built-ins as code and user identities as config

`extensions/identity/` puts a working persona into the system prompt. The
built-in identities cover the situations a security professional moves
between — `reverse-engineer` (code-level artifact analysis), `cyber-forensics`
(malware incident reconstruction), `forensics` (general and user-activity
forensics), `software-engineer` (tooling for the analysis team),
`infrastructure` (safe reproducible analysis infrastructure), `publisher`
(defensible deliverables) — and an adhoc `custom` identity can be
written in the session (`/identity write <text>`, or an external editor via
`/identity editor`) and saved as a named, reusable one (`/identity save
<name>`) once it has proven useful. Off by default; with no selection,
`before_agent_start` returns `undefined` and the system prompt stays
byte-identical — the same cache property goal-setting's manifest block keeps
(ADR-0024). The block is appended, not rewritten, so it composes
order-independently with every other prompt contributor in this package.

## Why a system-prompt block, appended

The identity shapes *how the agent works across the whole session* — what it
prioritizes, what it refuses to do, what evidence standards it holds. That is
system-prompt territory: it must hold for every turn, and it composes with
the other contributors (tool registry, manifest, guidelines) rather than
fighting them. The chained-append composition (verified, docs/pi-api-notes.md)
makes the choice of injecting extension irrelevant, so identity needs no
assumption about load order and no communication with any sibling.

## Built-ins as code, user identities as config

The five built-in texts are constants in the extension file. They are
prompt-shaped strings a maintainer edits like code — shipping them as files
would either register them as pi prompt resources (the `prompts/` directory
turns files into `/commands`, which ADR-0017 rejected for exactly this kind
of content: a persona is not a thing to invoke on its own) or as seeded
config (which would couple them to the install step — but identity is
general-purpose and calls no `reactor`, so there is nothing for
`scripts/install.py` to seed). Code constants cannot drift from the code
that renders them.

Saved user identities live in `~/.pi/agent/pi-identity.json` next to the
global default (`{ "default": "<name>", "user": { "<name>": "<text>" } }`),
hand-editable the way `toolsets.toml` is — it is the user's content, written
by `/identity save` and editable by hand. The extension's own config file,
not a second file: one file per extension is the standing rule.

## Selection: session state, explicit off, disjoint name spaces

The per-session selection rides its own `custom` entry (ADR-0009's pattern).
An unset session inherits the global `default`; `/identity off` writes an
*explicit* empty selection so a session can override a configured default
without editing the file. Two name-space rules keep lookups unambiguous:

- **Built-ins win** in resolution, and `/identity save` therefore refuses a
  built-in name outright — a saved identity silently unreachable because a
  built-in shadows it is a config trap, and the refusal makes the name
  spaces provably disjoint.
- **`custom` is a name, not a slot**: the adhoc text rides the session, and
  `/identity save` is what lifts it out. A save is only offered while the
  custom identity is active — the thing being saved is the thing you are
  looking at.

## Costs

- The editor flow duplicates `context-editor/`'s external-editor handling
  (suspend TUI, spawn with `stdio: inherit`, restore) — the price ADR-0014
  names for every split; it is ~30 lines and the raw-mode stdin race it
  avoids is regression-tested here the same way.
- Personas are prose, and prose can contradict the rest of the system
  prompt. The built-ins are written to steer emphasis, not to override pi's
  own behavior or safety guidance, and each states its focus in one
  paragraph — the failure mode "the persona tells the agent to ignore
  evidence" is a text problem, fixed by editing the text.
- A saved identity is global across sessions but not across machines; it
  lives in the agent dir, which is where every other per-user config in this
  package lives.

## Considered and rejected

- **Shipping identity texts in `prompts/`** as pi prompt templates: pi turns
  every file there into a `/command`, making the persona invocable as a
  one-shot prompt expansion — the wrong mechanism for a persistent selection,
  per ADR-0017's reasoning.
- **A separate `identities.toml` seeded by `install.py`**: couples a
  general-purpose extension to the CLI's seeded config and adds an install
  step for content that needs none.
- **A `select-identity` *tool* the LLM calls**: identity is the user's call,
  not the model's; a tool invites the model to switch personas mid-task.
- **A shared editor module with `context-editor/`**: ADR-0014 — two jiti
  instances, two copies, guaranteed drift.
- **Persona selection via `pi.setActiveTools()` or tool-registry coupling**:
  a persona is prompt text, not tool visibility; mixing the two would make
  identity depend on the toolbox toggle for no benefit (ADR-0016).