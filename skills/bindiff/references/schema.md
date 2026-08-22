# The .BinDiff SQLite schema

A `.BinDiff` file is a plain SQLite database (`sqlite3`, `python3 -c 'import
sqlite3'`, DB Browser for SQLite -- anything that reads SQLite reads this).
`scripts/bindiff_report.py` covers the common questions; this is for
anything it doesn't, straight from BinDiff's own writer
(`database_writer.cc` in the BinDiff source, verified against a real
generated `.BinDiff` file).

**`file1` in the tables below always means the `--primary` input, `file2`
always means `--secondary`** -- the writer hardcodes id 1 = primary, id 2 =
secondary, regardless of filenames.

## Tables

```
file (id, filename, exefilename, hash, functions, libfunctions, calls,
      basicblocks, libbasicblocks, edges, libedges, instructions, libinstructions)
```
One row per input (id 1 = primary, id 2 = secondary). `functions`/
`libfunctions` are **totals for that binary** -- matched and unmatched alike.
This is the only place unmatched functions are counted at all; see "What
this schema cannot tell you" below.

```
metadata (version, file1, file2, description, created, modified,
          similarity, confidence)
```
One row. `file1`/`file2` are foreign keys into `file.id`. `similarity` and
`confidence` here are the **whole-diff** scores (0.0-1.0) -- see "Scores"
below for what they mean.

```
function (id, address1, name1, address2, name2, similarity, confidence,
          flags, algorithm, evaluate, commentsported,
          basicblocks, edges, instructions)
```
**One row per matched function pair.** `address1`/`name1` are the primary
side, `address2`/`name2` the secondary side. `similarity`/`confidence` are
per-match (independent of the whole-diff scores in `metadata`). `algorithm`
is a foreign key into `functionalgorithm` naming which heuristic produced
this match. `flags` is a bitfield -- see "Decoding `flags`" below.
`basicblocks`/`edges`/`instructions` here count matched sub-elements within
this one function pair, not totals for either function.

```
basicblock (id, functionid, address1, address2, algorithm, evaluate)
```
One row per matched basic-block pair within a matched function.
`functionid` -> `function.id`, `algorithm` -> `basicblockalgorithm`.

```
instruction (basicblockid, address1, address2)
```
One row per matched instruction pair within a matched basic block.
`basicblockid` -> `basicblock.id`.

```
functionalgorithm (id, name)
basicblockalgorithm (id, name)
```
Static lookup tables, fully populated on every run regardless of which
algorithms actually found a match that time -- always query them directly
(`SELECT * FROM functionalgorithm`) rather than assuming the exact set below,
which is current as of BinDiff 8 and could change in a future major version:

`functionalgorithm`: name hash matching, hash matching, edges flowgraph MD
index, edges callgraph MD index, MD index matching (flowgraph, top down /
bottom up), prime signature matching, MD index matching (callgraph, top down
/ bottom up), relaxed MD index matching, instruction count, address
sequence, string references, loop count matching, call sequence matching
(exact / topology / sequence), call reference matching, manual.

`basicblockalgorithm`: edges prime product, hash matching, prime matching,
call reference matching, string references matching, edges MD index (top
down / bottom up), MD index matching (top down / bottom up), relaxed MD
index matching, edges Lengauer-Tarjan dominated, loop entry matching, self
loop matching, entry/exit point matching, instruction count matching, jump
sequence matching, propagation, manual.

## Decoding `flags`

`function.flags` is a bitfield, one bit per kind of change BinDiff detected
between the two matched functions (`CHANGE_NONE` = 0 = no detected change):

| Bit | Letter | Meaning |
|---|---|---|
| 1 | G | structural (basic-block graph shape) |
| 2 | I | instructions |
| 4 | O | operands |
| 8 | J | branch inversion (a jump condition flipped) |
| 16 | E | entry point (address shifted) |
| 32 | L | loops |
| 64 | C | calls |

`bindiff_report.py`'s `change` field decodes this the same way BinDiff's own
`GetChangeDescription()` does: a 7-character string, one letter per bit in
the order above, `-` where the bit is unset (`"-------"` = identical match,
`"GI--E--"` = structural + instruction + entry-point changes).

## Scores

From BinDiff's manual: **similarity** is a weighted sum of matched flow-graph
edges (~25%), matched basic blocks (~15%), matched instructions (~10%), and
flow-graph MD index difference (~50%). **Confidence** is the average
per-match algorithm confidence, weighted by a sigmoid. Both are 0.0-1.0;
`metadata` holds the whole-diff numbers, `function.similarity`/`confidence`
the per-match numbers -- a low whole-diff similarity can still contain
individual matches at 1.0, and vice versa.

## What this schema cannot tell you

**Unmatched functions are never written as rows.** `function` holds only
matched pairs -- a function that exists in just one binary has no row here
at all, in either direction. The only trace of it is arithmetic:
`file.functions + file.libfunctions - (SELECT COUNT(*) FROM function)` for
that side (which is exactly what `bindiff_report.py summary`'s
`unique_to_primary`/`unique_to_secondary` do). That gives you a **count**,
not identities.

To get the actual unmatched function names/addresses, list every function in
each binary directly (e.g. Binary Ninja's own function list, or the `list`
tool if you have live BN access) and subtract the matched address set:
`primary_addresses - {row.address1 for row in function}` is what's unique to
primary, and the mirror for secondary.

## Useful queries beyond what the script covers

```sql
-- Every function with exactly one specific kind of change (branch inversion)
SELECT name1, name2, similarity FROM function WHERE flags & 8 != 0;

-- Distribution of similarity scores, bucketed
SELECT CAST(similarity * 10 AS INT) AS bucket, COUNT(*)
FROM function GROUP BY bucket ORDER BY bucket;

-- Every instruction-level match inside one specific function pair
SELECT i.address1, i.address2
FROM instruction i JOIN basicblock b ON b.id = i.basicblockid
JOIN function f ON f.id = b.functionid
WHERE f.name1 = 'the_function_name';
```
