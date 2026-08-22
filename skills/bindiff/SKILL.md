---
name: bindiff
description: Diff two versions of the same binary with BinDiff -- export both from Binary Ninja, run bindiff headlessly, then query the resulting .BinDiff SQLite result for matched/changed/unique functions. Use when the user wants to patch-diff or otherwise compare two binaries to find which functions changed.
requires: [bindiff, bn]
---

# bindiff

Three steps: export both binaries from Binary Ninja, run `bindiff` to
produce a `.BinDiff` result, then query it. The first two are mechanical;
the third is where the actual analysis happens.

## 1. Export both binaries to `.BinExport`

This catalogue uses `bn` (Binary Ninja) as the disassembler, not IDA --
BinDiff's IDA integration and its `--export` batch mode are irrelevant here.
Binary Ninja ships BinExport as a built-in plugin; trigger it per binary
with:

```
bn load-binary /path/to/primary.bin
bn execute-script "from binaryninja import PluginCommandContext, PluginCommand; cxt = PluginCommandContext(bv); PluginCommand.get_valid_list(cxt)['BinExport (Quick)'].execute(cxt)"
```

then the same two commands for the secondary binary. `load-binary` also
selects the newly opened binary as current, so no separate select step is
needed between the two. (If you have live Binary Ninja MCP tools instead of
the `bn` CLI, run the same script via your `execute_script` tool directly.)

The `from binaryninja import ...` is not optional, even though Binary
Ninja's own BinExport docs omit it -- `PluginCommandContext`/`PluginCommand`
are not in scope by default in a scripted/headless call, only inside BN's
interactive console. Without it this fails with `NameError:
PluginCommandContext is not defined`. Verified against a real Binary Ninja
session; this exact snippet writes `<binary>.BinExport` next to the source
file, with no dialog and no return value to check -- confirm by checking
that the file now exists (`os.path.exists`) if you need to be sure before
the next step.

Done when: both `.BinExport` files exist next to their source binaries.

## 2. Run bindiff

```
bindiff --primary=primary.BinExport --secondary=secondary.BinExport --output_dir=.
```

Writes `primary_vs_secondary.BinDiff` (a SQLite database) into `output_dir`,
and prints a one-line summary (`matched: N of A/B`, overall similarity and
confidence) to stdout as it runs -- worth reading before step 3, since it is
the fastest sanity check that the diff found what you expected (near-0%
matched almost always means the two inputs are not actually related
versions of the same binary, not that BinDiff failed).

`bindiff DIRECTORY` (no `--primary`/`--secondary`) batch-diffs every
`.BinExport` file in a directory against every other one instead, if you
have more than two versions to compare at once.

Done when: the `.BinDiff` file exists and the printed summary line has a
plausible match count (not 0, not wildly lower than the smaller binary's
function count unless that is genuinely expected).

## 3. Query the result

`scripts/bindiff_report.py` answers the common questions directly, as JSON:

```
python3 scripts/bindiff_report.py summary  primary_vs_secondary.BinDiff
python3 scripts/bindiff_report.py matched  primary_vs_secondary.BinDiff --limit 50
python3 scripts/bindiff_report.py changed  primary_vs_secondary.BinDiff --limit 20 --max-similarity 0.9
python3 scripts/bindiff_report.py algorithms primary_vs_secondary.BinDiff
```

- `summary` -- overall similarity/confidence, per-file counts, and matched
  vs. unique-to-each-side counts.
- `matched` -- every matched function pair, most similar first.
- `changed` -- matched pairs sorted **least** similar first (`--max-similarity`
  narrows to only the matches that actually changed; `1.0` means identical).
  This is "which functions differ the most."
- `algorithms` -- which matching heuristic found which matches, for when a
  match looks suspicious and you want to know how confident to be in it.

**A `.BinDiff` file only records matches.** A function that exists in just
one binary has no row anywhere in it -- `summary`'s unique-to-each-side
numbers are a count derived by subtraction, not a list of names. If you need
the actual identities of added/removed functions, list functions from each
binary directly (Binary Ninja's own function list) and subtract the matched
address set -- see `references/schema.md`, "What this schema cannot tell
you."

For anything the script doesn't cover -- filtering on a specific change type,
walking down to matched instructions inside one function, anything schema-
shaped -- it is a plain SQLite file; `references/schema.md` has the full
table layout (verified against BinDiff's own source and a real generated
file), the `flags` bitfield, and example queries to build from.

Done when: you can name which functions matched, which of those changed the
most, and how many functions on each side had no match at all.
