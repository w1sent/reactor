# A tool's description says what it does, and the rules of use are one line each in the prompt

The tool definitions had grown by accretion: a description was where a rule was put when
the rule was found to be needed, so the same rule sat in the description, in the case-graph
block and in a parameter's text, and the longest descriptions (`graph_annotate`,
`graph_pull`) were short manuals. Pi, which this project started as a package of
([ADR-0001](0001-pi-is-the-only-target-harness.md)), keeps these apart, and its four tools
cost about 570 tokens of text in all. This records the convention taken from it.

## Decisions

**A description says what the tool does and where its limits are**, in a sentence or two:
what it returns, what it will not do, and how large its output may be. How to behave is
not in it.

**A rule of use is one line, owned by the tool, and joins the prompt only with the tool.**
A tool may give `guidelines`; the ones of the tools in the request are rendered once each,
in the request's order, as `## Tool rules` after the registry block. A tool that is not in
the request (deferred, gated off) adds nothing. This is pi's `promptGuidelines`, and it
is a part of the prompt of its own, so the inspector shows it with the base prompt, as
"Tool rules".

**What a block says, a description does not.** The case-graph block owns the rules of the
graph (a claim needs a source, a conclusion is an annotation, bulk goes through
`graph_pull`); the five descriptions do not repeat them. A fact about a parameter lives in
the parameter's own description, next to the enum or the type, with its default where it has
one.

**The words are the ones models already use.** `edit` is `edits: [{old_string, new_string,
replace_all}]`, and `tool_search` takes `select:a,b`: other clients' shapes, so that what a
model has seen many times works the first time. `read` keeps `offset` and `limit`.

**`edit` is all or nothing, against the original.** Every replacement is matched against
the file as it was before the call, so an earlier one cannot change what a later one finds;
each must match once unless it sets `replace_all`; two that overlap are refused with their
numbers; and if any cannot be applied none is written. The older call, one `old_string` and
`new_string` at the top level, is read as one more replacement, so a session or a model
that still makes it is not refused.

## Consequences

- A change of a rule is one place. The case-graph block, `Tool::guidelines` and the
  parameter text each own something that the others do not.
- The graph's definitions are only a little shorter (2,570 to 2,438 estimated tokens with
  the block, before deferral): most of their cost is schema, which this does not change.
- The truncation note, not the description, says how to read the rest of a cut output
  ([ADR-0053](0053-a-call-chooses-how-much-of-its-output-it-sees.md)); `bash` no longer
  repeats it.
- Tests that pin a phrase of a description pin it where it now lives (the Markdown
  guidance of `graph_annotate`'s `text`).

## Considered and rejected

- **A `<rules>` section built from every tool, deduplicated across tools only.** What
  pi does, and what this is, with one difference: the rules appear only for tools that are
  in the request, because a tool that is not advertised should not be taught.
- **Moving the rules into the base prompt.** A custom base prompt would lose them, and
  they would be there with the tool off.
- **Renaming `path` to `file_path`.** Some clients use it; the schema says which it is, and
  a model follows the schema it is given. Not worth renaming stored sessions' calls.
