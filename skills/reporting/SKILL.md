---
name: reactor-reporting
description: Lab notebook discipline for reverse-engineering findings -- short, factual, sourced entries written as you go, split across topic pages instead of one growing file. Use whenever REactor's reporting mode is on, or whenever asked to write up or document findings from an analysis.
---

# reactor-reporting

Keep a lab notebook, not a memoir. A notebook entry gets written the moment
you find something, in the words you had at that moment -- a memoir gets
written from memory afterward and loses the one thing that made the notebook
useful: exactly where each claim came from.

## Rules

- **Short.** A finding is a sentence or two, not an essay.
- **Plain.** State the claim directly: skip hedging filler ("it appears
  that", "interestingly") in favor of the claim itself, with your confidence
  in it stated separately.
- **Factual.** Distinguish observed from inferred. "The function at
  `0x4012a0` XORs the buffer with `0x5a`" is observed; "this is probably a
  simple obfuscation layer" is inferred -- label it as such rather than
  blending the two into one sentence.
- **Leaves a paper trail.** Every finding cites where in the analysis target
  it came from -- file:line, function name/address, packet number, log
  timestamp, registry key, whatever the target's own addressing scheme is. A
  claim with no paper trail is not a finding yet, it's a note to go find one.
- **Says where the trail forks.** When static and dynamic analysis (or any
  two sources) point different ways, say so plainly in `summary.md`'s
  Disagreements section. Smoothing a fork into one confident paragraph is
  worse than leaving it visibly unresolved.

## Structure: pages, not a scroll

A lab notebook is pages, one topic at a time -- not a single scroll that
keeps growing until nobody rereads the top of it. Inside the reporting
folder (`report/` by default; whatever REactor's reporting config points
at):

- **`summary.md`** -- the front door. One paragraph on what the target is
  and does, an index of every findings page with a one-line takeaway each,
  the Disagreements section above, and Open Questions. This is the file
  that stays current; keep it short enough to reread in full each time you
  touch it.
- **`findings/<topic>.md`** -- one page per subsystem or question under
  investigation (`findings/auth-flow.md`, `findings/crypto.md`,
  `findings/network-protocol.md`), not one page per single claim and not
  all claims in one file. Add a line to `summary.md`'s index the first time
  a page exists.
- **`data/`** -- raw material a finding cites but doesn't need to embed:
  full packet captures, large extracted tables, raw tool output. Findings
  pages link into it; nothing here is read top to bottom on its own.

Start both `summary.md` and a page's skeleton from `references/template.md`
next to this file, unless REactor's `templatePath` config names a
different one. Drop sections that don't apply to this target rather than
padding an empty one to keep the skeleton's shape.

## Tables and diagrams: stay plain-text readable

A table earns an inline spot only if it is still plain-text readable --
aligned, a handful of columns, short enough to scan unrendered. A table
that would sprawl past that (many rows, wide columns, a raw dump) belongs
in `data/<name>.csv` instead, with a two- or three-row sample inline and a
link to the rest.

A mermaid diagram earns its spot the same way a table does: only where a
flow, state machine, or sequence is genuinely easier to see than to read --
a call graph, a protocol handshake, more states than fit in a sentence. One
diagram, one thing; don't cram a whole subsystem into a single graph.
Caption it with the one-line takeaway above the fence, so the point
survives even where mermaid doesn't render.
