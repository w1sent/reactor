#!/usr/bin/env bash
# Compare two benchmark runs.
#
#   REACTOR_BENCH_OUT=before.tsv cargo bench -p reactor-gui --bench graph_model
#   ... change something ...
#   REACTOR_BENCH_OUT=after.tsv  cargo bench -p reactor-gui --bench graph_model
#   scripts/bench-compare.sh before.tsv after.tsv
#
# Each file holds the rows the benches append (suite, name, size, median ms, min ms). Prints
# the median of each measurement in both runs and how many times faster (>1) or slower (<1)
# the second is.
set -euo pipefail
[ $# -eq 2 ] || { echo "usage: $0 before.tsv after.tsv" >&2; exit 2; }
LC_ALL=C awk -F'\t' '
  NR == FNR { before[$1 FS $2 FS $3] = $4; order[++n] = $1 FS $2 FS $3; next }
  { after[$1 FS $2 FS $3] = $4 }
  END {
    printf "%-10s %-40s %8s %12s %12s %9s\n", "suite", "measurement", "size", "before ms", "after ms", "speed-up"
    for (i = 1; i <= n; i++) {
      k = order[i]
      if (!(k in after)) continue
      split(k, f, FS)
      b = before[k]; a = after[k]
      ratio = (a > 0) ? b / a : 0
      printf "%-10s %-40s %8s %12.3f %12.3f %8.1fx\n", f[1], f[2], f[3], b, a, ratio
    }
  }' "$1" "$2"
