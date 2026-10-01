# docs/

Everything that needs documenting: the idea, the decisions behind it, reference
notes, and how-to material.

| Path | What |
|---|---|
| [`concept.md`](concept.md) | The idea in full — the problem, the inversion it rests on, the architecture, and what REactor deliberately does not do. Start here. |
| [`adr/`](adr/) | Architecture decision records, numbered in the order taken. One per significant decision, each closing with the alternatives that were walked and why they lost. |
| [`howto/`](howto/) | Task-oriented guides: adding a tool, adding a toolset. |

Project-level material lives at the repo root rather than here: `README.md`
(orientation), `CONTEXT.md` (glossary), `TODO.md` (milestones, open questions).

## Conventions

- Write the ADR **before** implementing, not after. The plugins repo's own
  `TODO.md` records that discipline slipping; this repo starts with it.
- An ADR records what was decided, why, what it costs, and what was rejected.
  The rejected list is the part that stays useful — it is what stops a decision
  being relitigated from scratch a year later.
- ADRs 0001–0032 describe REactor as a pi package. They are kept as history
  (†-marked in the index where superseded); the pi code itself is in git history.
