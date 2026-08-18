# docs/

Everything that needs documenting: the idea, the decisions behind it, reference
notes, and how-to material.

| Path | What |
|---|---|
| [`concept.md`](concept.md) | The idea in full — the problem, the inversion it rests on, the architecture, and what REactor deliberately does not do. Start here. |
| [`adr/`](adr/) | Architecture decision records, numbered in the order taken. One per significant decision, each closing with the alternatives that were walked and why they lost. |
| [`pi-api-notes.md`](pi-api-notes.md) | Facts about pi that the design depends on, established against a specific pi version, marked `[verified]` (read from the shipped `dist/`) or `[docs]`. Re-check when bumping pi. |
| [`package-resources.md`](package-resources.md) | The package's `skills/`, `prompts/` and `themes/` directories — what goes in each and how pi discovers it. Documented here rather than in the directories themselves, because pi registers every top-level `.md` in `prompts/` as a slash command. |
| [`howto/`](howto/) | Task-oriented guides. Empty until there is something to operate. |

Project-level material lives at the repo root rather than here: `README.md`
(orientation), `CONTEXT.md` (glossary), `TODO.md` (milestones, open questions).

## Conventions

- Write the ADR **before** implementing, not after. The plugins repo's own
  `TODO.md` records that discipline slipping; this repo starts with it.
- An ADR records what was decided, why, what it costs, and what was rejected.
  The rejected list is the part that stays useful — it is what stops a decision
  being relitigated from scratch a year later.
- Claims about pi's behaviour go in `pi-api-notes.md` with their evidence, and
  are cited from wherever they are relied on. Do not restate a mechanism from
  memory in an ADR; link to the note.
