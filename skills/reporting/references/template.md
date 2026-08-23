<!--
  Default skeletons for the reactor-reporting skill: one for summary.md, one
  for each findings/<topic>.md page. Edit this file directly to change the
  shape every report follows -- there is no code behind it, the agent just
  reads whatever is here. Point the reactor-reporting extension's
  `templatePath` config at a different file instead of editing this one if
  you want to keep this default around, and give that file both skeletons
  below, since the skill expects both.
-->

# summary.md

# <target name>

## Overview

One paragraph: what this is, what it does, the bottom line.

## Findings

One line per findings page, newest first.

- [<topic>](findings/<topic>.md) -- <one-line takeaway>

## Disagreements

Where two findings pages (or two sources within one) conflict. State the
conflict plainly -- don't smooth it into one confident paragraph.

## Open questions

What's still unknown, and what would resolve it.

---

# findings/<topic>.md

# <topic>

One entry per finding. Each: the claim, your confidence, and the source.

- **<short claim>** -- <observed / likely / speculative>. Source:
  `<file:line | function@address | packet #N | timestamp | ...>`.

<!--
  A small table stays here, plain-text readable. Anything that would sprawl
  goes in data/<name>.csv with a short sample here and a link to the rest.
  A mermaid diagram goes here too, one per flow/state machine/sequence,
  captioned with its one-line takeaway.
-->
