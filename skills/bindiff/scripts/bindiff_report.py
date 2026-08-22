#!/usr/bin/env python3
"""Summarize a .BinDiff result database without touching BinDiff's own UI.

Answers the common questions over the SQLite file BinDiff writes: what
matched, what changed the most, and how much of each binary wasn't matched
at all. stdlib-only (`sqlite3`, `json`) -- a .BinDiff file is a plain SQLite
database, nothing BinDiff-specific is needed to read it.

Usage:
    python3 bindiff_report.py summary   somefile.BinDiff
    python3 bindiff_report.py matched   somefile.BinDiff [--limit N]
    python3 bindiff_report.py changed   somefile.BinDiff [--limit N] [--max-similarity F]
    python3 bindiff_report.py algorithms somefile.BinDiff

Every subcommand prints one JSON document to stdout. See
references/schema.md for the table layout this queries, and to write
anything this script doesn't already cover.
"""
import argparse
import json
import sqlite3
import sys

# ChangeType bits (BinDiff's change_classifier.h), decoded the same way
# BinDiff's own GetChangeDescription() does: a 7-character "ls -l"-style
# string, one letter per bit, '-' where the bit is unset.
_CHANGE_BITS = [
    (1 << 0, "G"),  # CHANGE_STRUCTURAL -- basic-block graph shape
    (1 << 1, "I"),  # CHANGE_INSTRUCTIONS
    (1 << 2, "O"),  # CHANGE_OPERANDS
    (1 << 3, "J"),  # CHANGE_BRANCHINVERSION -- jump condition flipped
    (1 << 4, "E"),  # CHANGE_ENTRYPOINT
    (1 << 5, "L"),  # CHANGE_LOOPS
    (1 << 6, "C"),  # CHANGE_CALLS
]


def decode_flags(flags: int) -> str:
    return "".join(letter if flags & bit else "-" for bit, letter in _CHANGE_BITS)


def _function_row(row: sqlite3.Row) -> dict:
    d = dict(row)
    d["address1"] = hex(d["address1"])
    d["address2"] = hex(d["address2"])
    d["change"] = decode_flags(d["flags"])
    return d


_FUNCTION_COLUMNS = (
    "name1, address1, name2, address2, similarity, confidence, "
    "flags, basicblocks, edges, instructions"
)


def cmd_summary(conn: sqlite3.Connection) -> dict:
    meta = conn.execute(
        "SELECT version, similarity, confidence, created FROM metadata"
    ).fetchone()
    files = {row["id"]: dict(row) for row in conn.execute("SELECT * FROM file")}
    matched = conn.execute("SELECT COUNT(*) AS n FROM function").fetchone()["n"]

    def unique_count(file_id: int) -> int:
        # `file.functions`/`libfunctions` count every function on that side,
        # matched or not. Subtracting the match count is the only way to get
        # "how many are unique to this side" out of this database at all --
        # unmatched functions are never written as rows in `function` (see
        # references/schema.md). This gives a COUNT, not identities; for the
        # actual unmatched function names/addresses, list functions from
        # each binary directly (e.g. in Binary Ninja) and subtract the
        # matched address set reported by `matched` below.
        f = files.get(file_id, {})
        return f.get("functions", 0) + f.get("libfunctions", 0) - matched

    return {
        "bindiff_version": meta["version"],
        "similarity": meta["similarity"],
        "confidence": meta["confidence"],
        "created": meta["created"],
        "primary": files.get(1),
        "secondary": files.get(2),
        "matched_functions": matched,
        "unique_to_primary": unique_count(1),
        "unique_to_secondary": unique_count(2),
    }


def cmd_matched(conn: sqlite3.Connection, limit: int) -> list:
    rows = conn.execute(
        f"SELECT {_FUNCTION_COLUMNS} FROM function ORDER BY similarity DESC LIMIT ?",
        (limit,),
    ).fetchall()
    return [_function_row(r) for r in rows]


def cmd_changed(conn: sqlite3.Connection, limit: int, max_similarity: float) -> list:
    rows = conn.execute(
        f"SELECT {_FUNCTION_COLUMNS} FROM function "
        "WHERE similarity <= ? ORDER BY similarity ASC LIMIT ?",
        (max_similarity, limit),
    ).fetchall()
    return [_function_row(r) for r in rows]


def cmd_algorithms(conn: sqlite3.Connection) -> dict:
    return {
        "function": [
            dict(r)
            for r in conn.execute(
                "SELECT fa.name, COUNT(*) AS matches FROM function f "
                "JOIN functionalgorithm fa ON fa.id = f.algorithm "
                "GROUP BY fa.name ORDER BY matches DESC"
            )
        ],
        "basic_block": [
            dict(r)
            for r in conn.execute(
                "SELECT ba.name, COUNT(*) AS matches FROM basicblock b "
                "JOIN basicblockalgorithm ba ON ba.id = b.algorithm "
                "GROUP BY ba.name ORDER BY matches DESC"
            )
        ],
    }


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="command", required=True)

    def add(name: str, help: str):
        sp = sub.add_parser(name, help=help)
        sp.add_argument("bindiff_file", help="the .BinDiff SQLite database")
        return sp

    add("summary", "file-level counts, overall similarity/confidence")
    add("matched", "every matched function pair, most similar first").add_argument(
        "--limit", type=int, default=50
    )
    changed = add("changed", "matched pairs sorted by similarity, most different first")
    changed.add_argument("--limit", type=int, default=20)
    changed.add_argument(
        "--max-similarity", type=float, default=1.0,
        help="only include matches at or below this similarity (0.0-1.0)",
    )
    add("algorithms", "which matching heuristic found which matches")

    args = p.parse_args()
    conn = sqlite3.connect(args.bindiff_file)
    conn.row_factory = sqlite3.Row
    try:
        if args.command == "summary":
            result = cmd_summary(conn)
        elif args.command == "matched":
            result = cmd_matched(conn, args.limit)
        elif args.command == "changed":
            result = cmd_changed(conn, args.limit, args.max_similarity)
        else:
            result = cmd_algorithms(conn)
    finally:
        conn.close()

    json.dump(result, sys.stdout, indent=2)
    print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
