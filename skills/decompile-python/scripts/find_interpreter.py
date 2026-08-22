#!/usr/bin/env python3
"""Find which locally installed Python interpreter matches a .pyc file's
magic number.

Deliberately does *not* hardcode a magic-number-to-version table: that table
changes on every feature release and sometimes between prereleases of the
same release, so it would start going stale the day it was written. Every
interpreter already knows its own magic number -- `importlib.util.MAGIC_NUMBER`
-- so this just asks each candidate on PATH and compares bytes.

Usage:
    python3 find_interpreter.py somefile.pyc

Prints one matching interpreter path per line (normally exactly one) and
exits 0. If nothing matches, it lists every interpreter it tried and the
magic number each one reported, then exits 1 -- at which point `reactor
install 'decompile-python[all]'` is the fastest way to get more candidates
onto PATH.
"""
from __future__ import annotations

import shutil
import subprocess
import sys

# `pythonX.Y` has been the convention for the versioned binary name for as
# long as this matters in practice; `python`/`python3` cover whatever a
# distro aliases its default to. Widen the ranges if a build ever needs it --
# this list costs nothing to check items outside your actual history that
# just are not present.
CANDIDATE_NAMES = (
    ["python3", "python"]
    + [f"python3.{minor}" for minor in range(3, 15)]
    + [f"python2.{minor}" for minor in range(6, 8)]
    + ["python2"]
)

_PROBE = "import importlib.util,sys; sys.stdout.buffer.write(importlib.util.MAGIC_NUMBER)"


def magic_number(interpreter: str) -> bytes | None:
    try:
        proc = subprocess.run(
            [interpreter, "-c", _PROBE],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    return proc.stdout if proc.returncode == 0 and len(proc.stdout) == 4 else None


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <file.pyc>", file=sys.stderr)
        return 2

    with open(sys.argv[1], "rb") as f:
        target = f.read(4)
    if len(target) < 4:
        print("not a valid .pyc: fewer than 4 bytes", file=sys.stderr)
        return 1

    tried: dict[str, bytes] = {}
    matches = []
    for name in CANDIDATE_NAMES:
        path = shutil.which(name)
        if not path or path in tried:
            continue
        magic = magic_number(path)
        if magic is None:
            continue
        tried[path] = magic
        if magic == target:
            matches.append(path)

    if matches:
        print("\n".join(matches))
        return 0

    print(f"no interpreter on PATH produced magic number {target.hex()}", file=sys.stderr)
    print("tried:", file=sys.stderr)
    for path, magic in tried.items():
        print(f"  {path}: {magic.hex()}", file=sys.stderr)
    print("install more versions: reactor install 'decompile-python[all]'", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
