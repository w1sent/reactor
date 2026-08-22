---
name: decompile-python
description: Decompile a compiled Python .pyc file back to readable source by matching its interpreter version, disassembling with dis+marshal, and lifting the bytecode to Python by hand.
requires: [decompile-python]
---

# decompile-python

There is no reliable, generic `.pyc -> .py` decompiler binary to reach for
here. The community tools that exist (`uncompyle6`, `decompyle3`, `pycdc`)
each cover a narrow band of interpreter versions and silently produce wrong
or partial output outside it, which is worse than an error for anything you
plan to trust. What actually works across versions is the technique CPython
itself uses to load a `.pyc`: unmarshal it back into a code object with the
*matching* interpreter, disassemble that, and read the opcodes. The last
step -- turning opcodes into readable source -- is not automated by anything
here; it is you, reading `dis` output and writing the equivalent Python.

Three steps. Do them in order -- the first two are mechanical and scripted,
the third is the actual work.

## 1. Find the matching interpreter

A `.pyc`'s first 4 bytes are a magic number tied to the *exact* bytecode
format of the interpreter that compiled it. Every CPython minor version
(3.9, 3.10, 3.11, ...) has its own, and it also changes between an old
Python 2 and Python 3. Guessing wrong does not degrade gracefully -- marshal
will misparse the header, throw, or in bad cases the interpreter will
segfault trying to execute garbage as a code object.

```
python3 scripts/find_interpreter.py somefile.pyc
```

This asks every Python it finds on `PATH` for its own
`importlib.util.MAGIC_NUMBER` and reports which one(s) match, without
hardcoding a magic-number table (that table changes every release and would
just go stale). If nothing matches, it lists what it tried -- at which point
install more interpreters:

```
reactor install 'decompile-python[all]'
```

(quoted, because `[all]` is a real shell glob character in most shells).
This installs every `python3.x` package the machine's own package manager
currently offers -- not from the AUR or any other community repo, since the
point is versions the distro itself curates and keeps working. It is
intentionally not part of `reactor install decompile-python` on its own
(that only gets you the one interpreter reactor itself needs) or of
`reactor install all` (several full interpreters is a lot of installed
weight for one narrow skill) -- ask for it by name when a file's version
genuinely isn't on this machine yet.

## 2. Disassemble with that interpreter

```
pythonX.Y scripts/disassemble.py somefile.pyc -o somefile.dis.txt
```

`pythonX.Y` here must be one of the paths `find_interpreter.py` printed --
not whatever `python3` happens to mean on `PATH` by default. The script:

- Skips the right number of header bytes for *that interpreter's own* pyc
  format (16 bytes since 3.7 / PEP 552, 12 for 3.3-3.6, 8 before that) and
  `marshal.load`s the code object.
- Runs `dis.dis()` over it, which walks into nested code objects (every
  function, lambda, comprehension and class body is one) rather than
  stopping at the module level.
- Separately dumps every string constant found anywhere in the file,
  deduplicated. Read this section first -- format strings, URLs, error
  messages and dict/attribute names as string constants are usually the
  fastest way to guess what a function is *for* before reading how it does
  it.

If you need the raw code object for something the script doesn't already do
(inspecting `co_consts` structurally, comparing two versions of the same
file, etc.), the same four lines work directly once you have the matching
interpreter:

```python
import dis, marshal
with open("somefile.pyc", "rb") as f:
    f.read(16)  # header length depends on version -- see above
    code = marshal.load(f)
dis.dis(code)
```

## 3. Lift the bytecode to source

This is the actual work, and no script here does it for you. Read the
disassembly one code object at a time, **innermost first** -- reconstruct
nested functions/lambdas/comprehensions before the scope that defines them,
since the outer scope's `MAKE_FUNCTION` calls are easier to read once you
already know what each one does. For each code object:

- `co_varnames`, `co_names`, `co_consts` at the top of the dump are your
  variable, name, and literal tables -- cross-reference the indices dis
  prints against them rather than guessing from opcode order alone.
- Reconstruct straight-line code first (assignments, calls, returns), then
  control flow, then anything with exception tables (`try`/`except`/`with`)
  last -- that machinery is the most bytecode, for the least resemblance to
  the source that produced it, so leave it for when everything simpler
  already gives you naming context.

Opcode-to-source cheat sheet for the constructs that come up constantly (not
exhaustive -- `python3 -m dis --help` and the `dis` module docs cover the
full opcode list for whichever version you're reading):

| Opcodes | Reads as |
|---|---|
| `LOAD_FAST`/`STORE_FAST`, `LOAD_FAST_BORROW` (3.13+) | read/write a local variable |
| `LOAD_GLOBAL`, `LOAD_NAME`, `LOAD_DEREF` | read a global, module-level, or closed-over name |
| `LOAD_CONST` | a literal (number, string, `None`, a nested code object, ...) |
| `CALL`, `CALL_FUNCTION`, `CALL_FUNCTION_KW`, `PRECALL`/`PUSH_NULL` (version-dependent) | a function/method call — args are whatever was pushed just before it |
| `BINARY_OP`, `COMPARE_OP`, `IS_OP`, `CONTAINS_OP` | `+ - * / == != in is` etc. — the specific operator is the opcode's argument |
| `POP_JUMP_IF_FALSE`/`_TRUE`, `JUMP_FORWARD`/`JUMP_BACKWARD` | `if`/`while`/`for` control flow — backward jumps are loops |
| `FOR_ITER`, `GET_ITER` | a `for x in ...:` loop |
| `BUILD_LIST`/`_TUPLE`/`_SET`/`_MAP`, `BUILD_STRING` | literal collection/f-string construction |
| `MAKE_FUNCTION` | defines the nested function/lambda whose code object was just `LOAD_CONST`ed |
| `LOAD_BUILD_CLASS` ... `STORE_NAME` | a `class` statement |
| `RAISE_VARARGS`, exception table entries | `raise` / `try`/`except`/`finally` |
| `RETURN_VALUE`/`RETURN_CONST` | `return` |

Write the reconstruction as normal, readable Python -- meaningful names where
`co_names`/`co_varnames` give you real ones, best-effort names where they
don't (`obj.co_consts` and string literals are often the best hint for what
a variable actually holds). Once you have a candidate, you can sanity-check
it with the same matching interpreter you disassembled with:

```
pythonX.Y -m py_compile candidate.py            # writes __pycache__/candidate.*.pyc
pythonX.Y scripts/disassemble.py __pycache__/candidate.*.pyc -o candidate.dis.txt
```

then diff `candidate.dis.txt` against the original disassembly. It will not be byte-identical
(compilers are not required to be canonical), but a reconstruction whose
opcodes, constants and control flow *shape* line up closely is strong
evidence you read it right; a structural mismatch (a comparison flipped, a
loop that should have been a comprehension, a swallowed exception) is
exactly the kind of thing this catches before you trust the output.
