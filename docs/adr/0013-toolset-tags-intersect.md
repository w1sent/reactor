# Toolset tags intersect

A toolset's `tags` list selects the tools carrying **every** tag in it, not the
tools carrying any of them.

```toml
[toolset.native]
tags = ["static", "native"]   # angr, bn, lief -- not every static tool
```

`tools` and `tags` still union with each other: the members are the explicit
`tools`, plus everything matching the whole tag list. That is what a toolset
built around a tag but needing one extra entry looks like — "everything tagged
`android`, and also `frida`".

An empty or absent `tags` selects nothing rather than everything. The vacuous
reading is technically the correct one for an intersection and is useless here.

## Why

**Union was the reading, and it was wrong twice.** `[toolset.all]` written as a
tag list silently dropped every tool nobody had tagged, which is why `all = true`
exists as a separate key. `[toolset.native]` written as `["static", "native"]`
collected all six static tools, including a Java decompiler and a source-level
dataflow engine. Both were written by someone who knew the schema. In both cases
the author listed two tags expecting the second to *narrow* the first — which is
the instinct the semantics should match, because it is the instinct people
actually have.

**The activation model already supplies union, and cannot supply intersection.**
`state.toolsets` is a list and `base_ids` unions the members of everything in
it, so "native or managed" is two active toolsets — one keystroke each in the
selector, and visibly two things rather than one thing with hidden reach. There
is no equivalent move for "static and native": if the tag list does not
intersect, nothing does, and the only way to express it is an explicit `tools`
list that goes stale the moment the catalogue grows.

So the two operations are not equally scarce. Union is cheap and already
available at the layer above; intersection is available nowhere. The tag list
should provide the one that is missing.

**Tags in this catalogue are facets, and facets are what intersection is for.**
The vocabulary crosses several independent axes — analysis mode (`static`,
`dynamic`), target (`native`, `managed`, `source`, `mobile`, `android`, `ios`,
`firmware`, `network`), role (`decompiler`, `debugger`, `parsing`, `solving`,
`instrumentation`), implementation (`python`, `dotnet`). A useful group is
almost always a point in that space, not a sweep across it: static Android
tooling is `static` ∩ `android` (`jadx`); dynamic mobile tooling is `dynamic` ∩
`mobile` (`frida`, `objection`). Under union each of those is eleven or ten
tools, which is not a toolset, it is the catalogue with extra steps.

**Union has a failure mode that intersection does not.** With union, adding a
tag to one tool grows every toolset naming that tag, whether or not the toolset
is about the thing the tool does. With intersection the new tag has to satisfy
the whole conjunction, so the blast radius of an editorial change to
`tools.toml` is small and local. Tagging is editorial work done tool-by-tool
([ADR-0003](0003-tools-toml-single-source-of-truth.md)); it should not be able
to quietly redefine a group.

## Consequences

- **A union inside one toolset is no longer expressible.** "Firmware or mobile"
  is two toolsets activated together, or an explicit `tools` list, or a tag that
  says what the group actually has in common. This is the cost, and it is
  bounded by the fact that the selector makes activating two toolsets trivial.
- **A misspelled or over-specified tag list now selects nothing** instead of
  quietly selecting too much. That is the better failure — silence is visible in
  the selector, where a toolset showing `0 tools` is obviously broken — but it
  is a new way for a hand-edited file to be wrong, so `reactor doctor` reports
  any toolset that selects nothing.
- **Existing files keep working.** The change is a no-op for any toolset naming
  one tag, and every shipped toolset names one tag or lists tools explicitly.
  `version` stays at 1 for that reason: there is no file this silently
  reinterprets, and bumping it would make everyone edit a file to say the same
  thing.
- **`all = true` stays.** It is not "no tags" and it is not a wildcard tag; it
  is the statement that a group is defined by the catalogue rather than by any
  property of its members, and it is the only thing that keeps working when
  someone adds an untagged tool.
- **`reactor tools list --tag` narrows on repeats too.** It is `action="append"`,
  so it was the other place a list of tags had a meaning, and it had the other
  meaning. One spelling, one rule — a filter flag that unions while the config
  key intersects is the confusion this ADR exists to remove.

## Considered and rejected

- **Keep `tags` as union, add `all_tags` for intersection.** Nothing breaks, and
  both operations are available. Rejected because it leaves the wrong default in
  the seductive position: `tags` is the key people will reach for, it is the key
  that has already produced two broken toolsets, and `all_tags` only helps the
  person who already knows the distinction exists. Two keys differing by a
  subtle set operation is a thing to be got wrong, not a feature. If a real
  union-within-one-toolset case turns up that two active toolsets genuinely
  cannot serve, `any_tags` can be added then, as the *marked* case.
- **Keep union and document it harder.** The cheapest option, and the comment
  warning against it is already in `toolsets.toml`. Rejected because the comment
  was written *after* the second time it happened, by the person who made the
  mistake both times. Documentation that exists to warn about the design is
  evidence about the design.
- **Make it configurable per toolset (`match = "any" | "all"`).** Rejected as
  the worst of both: every toolset now carries a mode, every reader has to check
  it, and the default still has to be chosen — so the question is not settled,
  only moved into every entry in the file.
- **Drop tags entirely; toolsets list tools explicitly.** Honest and completely
  predictable, and the shipped `triage`, `android` and `ios` sets are already
  written that way. Rejected because it makes the catalogue's growth someone's
  manual problem: a new Android tool should join the Android toolset by being
  tagged `android`, not by being remembered in two files.
- **Bump `toolsets.toml` to `version = 2`.** The rigorous move for a semantic
  change. Rejected because it would reject every existing file to fix a meaning
  that no existing file relies on — the shipped seed has no multi-tag toolset,
  by construction, and a single-tag toolset means exactly what it did before.
