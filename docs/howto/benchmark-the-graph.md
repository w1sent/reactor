# Benchmarking the graph

The graph view has to stay usable as a case grows. Three things measure it, each on a
synthetic case that looks like an incident (clusters of related entities, a few claims
between clusters, three-way claims, notes about claims, tags, a few orphans) and is the same
every time for the same size. The generator and the timing harness are in
`reactor_db::bench`, compiled only with the `bench` feature, which the benches and the GUI's
dev-dependencies turn on; nothing here is in a shipped binary.

| What | Command | Measures |
|---|---|---|
| Database | `cargo bench -p reactor-db --bench graph` | writing a case (`apply`), the snapshot the view reads on every refresh, stored layout, single-element reads |
| View model | `cargo bench -p reactor-gui --bench graph_model` | building what is drawn, cards, every layout, `separate`, level of detail, filter layers, find, tag boxes |
| Whole window | `cargo test --release -p reactor-gui --test graph_frame -- --ignored --nocapture` | in a headless window: opening a case, a refresh with nothing new, a refresh after an agent wrote five related entities, and a frame at three zoom levels |

Knobs (environment):

- `REACTOR_BENCH_SIZES=200,1000,5000` — entities per case (each suite has defaults).
- `REACTOR_BENCH_BUDGET_MS` — how long one measurement keeps sampling (default 1500); a
  measurement slower than that is taken once after a warm-up.
- `REACTOR_BENCH_FORCE_MAX` — (`graph_model`) the node count above which a whole-graph
  force layout is skipped; 6000 by default, because the original was quadratic.
- `REACTOR_BENCH_OUT=file.tsv` — append every row as `suite name size median_ms min_ms`.
- `REACTOR_BENCH_KEEP=dir` — (`reactor-db`) build the case there and leave it, to open with
  `sqlite3` and `EXPLAIN QUERY PLAN`.
- A bare argument filters by name: `cargo bench -p reactor-gui --bench graph_model -- layout`.

To compare a change with what was there, run a suite with `REACTOR_BENCH_OUT=before.tsv`,
change, run with `after.tsv`, and `scripts/bench-compare.sh before.tsv after.tsv`. Take both
runs on an idle machine: a build running beside a bench shows up as a speed-up or a
slow-down that is not there.

The force layout has an oracle in its unit tests (`the_force_layout_gives_the_picture_it_always_did_to_the_last_bit`):
the original single-threaded, every-pair loop, compared bit for bit with the one in use. Any
further speed-up of `spring` has to keep that test passing, or say why the picture may change.

## What was found, and what was done

Numbers are medians from one machine (12 threads), release profile. "Nodes" are what is drawn,
about 1.2 per entity.

### Done, with the picture unchanged

| Where | Problem | Result |
|---|---|---|
| `Writer::link` (`graph/mod.rs`) | The edge between two elements was found through `edge_to` alone, so each `source` link read every edge into that external. Writes slowed as the case grew. New index `edge_from_to` (migration 10; it replaces `edge_from`, which it covers). | Writing a case of 52 500 elements: 107 s → 11.5 s; the cost per claim no longer grows with the graph. |
| `model::layout` → `spring` | On every refresh with something new, the whole connected group was run through 220 rounds of every-pair repulsion, though almost all of it was already placed and could not move. A pair of placed nodes now costs nothing, and the sums of the others are made row by row, over threads when there is enough to share. | An agent write among 3 641 placed nodes: 10.6 s → 33 ms; among 12 144: 120 s → 112 ms. A first layout of 3 641 nodes: 9.9 s → 3.0 s. Bit-identical. |
| `model::build` | Each node counted its supporters and refuters by scanning every edge. | 12 144 nodes: 1 182 ms → 40 ms. |
| `model::layout`, `arrange::layout_as` | The lines of each connected group were found by filtering all lines, once per group. | Flat in the number of groups. |
| `model::disclose` | For each claim not drawn, a scan of all lines. | 12 144 nodes: 15 ms → 0.3 ms per frame. |
| `GraphPanel::render` | Every line was built as elements, off screen or not (nodes already were culled); node positions were looked up by id for each end of each line. | With the graph zoomed in on a 7 875-element case: frame 82 ms → 2.7 ms. |
| `bash` output spill file | A predictable name in the shared temp directory, opened with `File::create`, readable by others. Now created exclusively and `0600`. | — |
| `ingest::office::read_entry` | A part of a document was read without a limit; a small archive can claim gigabytes. Capped at 256 MiB. | — |

### Not done, and why

- **Refresh runs on the UI thread, after every tool call of the agent** (`app.rs`, on
  `Event::ToolEnd`): a full snapshot, build and layout, whether or not the tool touched the
  graph. With the fixes above it is ~70 ms at 7 900 elements, ~500 ms at 50 000 (snapshot
  alone). Doing the read and the build on a background thread and applying the result is the
  right fix, and the tests that call `refresh` synchronously would need a hook. Skipping it
  when the change log (`event.seq`) and the vocabulary have not moved is the cheaper
  alternative; the vocabulary and field tables write no event, so the key has to include them.
- **A whole-graph frame is still ~200 ms at 7 900 elements** (zoomed all the way out, every
  node and line drawn). The `render` function is 0.4 ms of that; the rest is gpui laying out
  and painting one element per node and one per line label, and one `paint_path` per line
  (about a third). Fixes that change the picture or need eyes on it: draw lines of the same
  style as one path; below a zoom, stop building a label element per line and hit-test lines
  on the canvas; cache the card list between frames.
- **`snapshot` is SQLite's own time** (two joins with a correlated `max(ver)`; ~5 µs per
  element). Storing a `current` flag on versions would make it an index scan but changes the
  schema's invariants.
- **`read` tool loads the whole file** before applying its line limit; a multi-gigabyte
  sample is read into memory. Streaming it changes what a binary file late in a big file
  reports.
- **Case import has no limit on what an archive expands to** (only paths are checked). A limit
  that is not wrong for a legitimate case of large evidence is a policy decision.
- `query::relations` (the agent's relations tool) costs 160 ms at 13 000 elements: its view
  joins `edge_current` to itself.
