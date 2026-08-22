#!/usr/bin/env python3
"""Disassemble a .pyc with the interpreter that (probably) compiled it, then
dump every string constant found anywhere in it -- nested code objects
included -- since that is usually the fastest way to get oriented before
reading opcodes line by line.

Usage:
    pythonX.Y disassemble.py somefile.pyc [-o out.txt]

`pythonX.Y` must be the *matching* interpreter -- see find_interpreter.py.
marshal's code-object format is undocumented, unversioned wire format that
just happens to match between a compiler and its own interpreter; running
this under the wrong version will misparse the header at best and crash the
interpreter at worst.

This talks to the file directly with marshal rather than shelling out to
`python -m dis somefile.pyc`, on purpose: `dis`'s own CLI grew the ability to
accept a .pyc directly at some point in its history and this does not try to
track which release that was, and doing it by hand also gets us the actual
code object -- needed for the constant dump below, which plain `-m dis`
output does not give you.
"""
from __future__ import annotations

import argparse
import dis
import importlib.util
import marshal
import sys


def _header_size() -> int:
    """How many bytes precede the marshalled code object, for *this running
    interpreter's own* pyc format:
      - 3.7+  (PEP 552): magic(4) + flags(4) + (mtime,size) or hash (8) = 16
      - 3.3-3.6:          magic(4) + mtime(4) + source size(4)          = 12
      - earlier:          magic(4) + mtime(4)                          =  8
    """
    if sys.version_info >= (3, 7):
        return 16
    if sys.version_info >= (3, 3):
        return 12
    return 8


def load_code(path: str):
    with open(path, "rb") as f:
        header = f.read(_header_size())
        magic = header[:4]
        if magic != importlib.util.MAGIC_NUMBER:
            print(
                f"warning: this file's magic ({magic.hex()}) does not match this "
                f"interpreter's ({importlib.util.MAGIC_NUMBER.hex()}, "
                f"Python {sys.version.split()[0]}) -- wrong version; re-run "
                "find_interpreter.py and use what it prints",
                file=sys.stderr,
            )
        return marshal.load(f)


def iter_code_objects(code, seen=None):
    """Depth-first over every nested code object. Every function, lambda,
    comprehension and class body compiles to a code object living in some
    enclosing scope's co_consts -- this is how you reach the ones dis.dis's
    top-level call does not walk into on older interpreters."""
    seen = seen if seen is not None else set()
    if id(code) in seen:
        return
    seen.add(id(code))
    yield code
    for const in code.co_consts:
        if isinstance(const, type(code)):
            yield from iter_code_objects(const, seen)


def dump_string_constants(code) -> list[str]:
    strings, seen = [], set()
    for c in iter_code_objects(code):
        for const in c.co_consts:
            if isinstance(const, str) and const and const not in seen:
                seen.add(const)
                strings.append(const)
    return strings


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("pyc", help="the .pyc file to disassemble")
    p.add_argument("-o", "--output", help="write to this file instead of stdout")
    args = p.parse_args()

    code = load_code(args.pyc)
    out = open(args.output, "w") if args.output else sys.stdout
    try:
        print(f"# top-level co_names:    {code.co_names}", file=out)
        print(f"# top-level co_varnames: {code.co_varnames}", file=out)
        print("", file=out)
        dis.dis(code, file=out)
        print("\n# ---- string constants, every nested code object, deduplicated ----", file=out)
        for s in dump_string_constants(code):
            print(repr(s), file=out)
    finally:
        if args.output:
            out.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
