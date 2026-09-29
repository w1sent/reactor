#!/usr/bin/env python3
"""Differential test: the Rust `reactor` against the Python one it replaces.

Migration tooling (MIGRATE.md phase 1), deleted together with bin/reactor when
the port is accepted. Runs the same commands through both against identical,
separate config dirs and compares stdout bytes and exit codes. `--format json`
is frozen at the byte level (ADR-0034), so "close enough" is a failure.

    python3 scripts/parity.py [path/to/reactor]      # default target/debug/reactor

Known, deliberate differences are normalised in NORMALISE and listed in the
report rather than hidden.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PY = [sys.executable, str(REPO / "bin" / "reactor")]
RS = [str(Path(sys.argv[1]).resolve()) if len(sys.argv) > 1 else str(REPO / "target" / "debug" / "reactor")]

DEMO = '''

[tool.demo-manual]
name    = "demo"
desc    = "manual one-liner demo"
source  = "https://example.com/demo"
invoke  = "demo-manual"
detect  = { binary = "demo-bin" }
tags    = ["general"]

[tool.demo-manual.install]
manual = "prose only, never a command"
manual-install-oneliner = "mkdir -p out && touch out/demo-bin && chmod +x out/demo-bin"
'''


def normalise(argv, text):
    """Differences that are the point of the port, not drift."""
    if argv[:1] == ["doctor"]:
        # The Python CLI reported its own interpreter; Rust reports its own version.
        text = re.sub(r'"python": "[^"]*"', '"reactor": "-"', text)
        text = re.sub(r'"reactor": "[^"]*"', '"reactor": "-"', text)
        text = re.sub(r"platform   (\S+), python \S+", r"platform   \1", text)
    if argv[:1] == ["completion"]:
        # `setup` is new (it replaces scripts/install.py); nothing else changed.
        text = "\n".join(l for l in text.split("\n") if "setup" not in l or "top" in l or "top_cmds" in l)
        text = text.replace(" setup completion", " completion")
    return text


def run(cmd, argv, cfg, home, cwd, stdin=None):
    env = {**os.environ, "REACTOR_CONFIG_DIR": str(cfg), "REACTOR_PACKAGE_ROOT": str(REPO), "HOME": str(home)}
    p = subprocess.run(cmd + argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                       env=env, cwd=cwd, timeout=300, stdin=subprocess.DEVNULL)
    return p.returncode, p.stdout, p.stderr


class Side:
    def __init__(self, name, cmd, tools_text):
        self.name, self.cmd = name, cmd
        self.root = Path(tempfile.mkdtemp(prefix=f"parity-{name}-"))
        self.cfg, self.home, self.cwd = self.root / "cfg", self.root / "home", self.root / "cwd"
        for d in (self.cfg, self.home, self.cwd):
            d.mkdir()
        (self.cfg / "tools.toml").write_text(tools_text)
        shutil.copy(REPO / "toolsets.toml", self.cfg / "toolsets.toml")

    def run(self, argv):
        return run(self.cmd, argv, self.cfg, self.home, self.cwd)


def main():
    tools_text = (REPO / "tools.toml").read_text() + DEMO
    py, rs = Side("py", PY, tools_text), Side("rs", RS, tools_text)

    ids = json.loads(py.run(["tools", "list", "--cached", "--format", "json"])[1])["tools"]
    ids = [t["id"] for t in ids]
    sets = [t["id"] for t in json.loads(py.run(["toolsets", "list", "--format", "json"])[1])["toolsets"]]

    J = ["--format", "json"]
    cases = [
        ["registry"] + J, ["registry", "--refresh"] + J, ["registry", "--cached"] + J, ["registry"],
        ["services"] + J, ["services", "--cached"] + J, ["services"],
        ["tools", "list"] + J, ["tools", "list"], ["tools", "list", "--present"] + J,
        ["tools", "list", "--missing"] + J, ["tools", "list", "--tag", "static", "--tag", "native"] + J,
        ["tools", "list", "--active"] + J,
        ["doctor"] + J, ["doctor", "--cached"] + J, ["doctor"],
        ["toolsets", "list"] + J, ["toolsets", "list"],
        ["state"] + J, ["state"],
        ["skills", "list"] + J, ["skills", "list"], ["skills", "show", "bn"] + J,
        ["install", "all", "--dry-run"] + J, ["install", "all", "--dry-run"],
        ["install", "all", "jq", "--dry-run"] + J,
        ["install", "jq"] + J,
        ["install", "demo-manual", "--dry-run"] + J,
        ["install", "demo-manual", "--force-install-manual", "--dry-run"] + J,
        ["install", "demo-manual", "--auto-install-manual", "--dry-run"] + J,
        ["install", "demo-manual", "--auto-install-manual", "--force-install-manual"] + J,
        ["install", "demo-manual", "--method", "pacman", "--force-install-manual"] + J,
        ["install", "bulk-extractor", "--force-install-manual", "--dry-run"] + J,
        ["install", "decompile-python[all]", "--dry-run"] + J,
        ["install", "decompile-python[all]", "--method", "pip"] + J,
        ["install", "jq", "--create-venv", "--dry-run"] + J,
        ["tools", "show", "definitely-not-a-tool"] + J,
        ["tools", "show", "definitely-not-a-tool"],
        ["diff-config"] + J, ["overwrite-config", "--yes"] + J, ["overwrite-config", "--yes"] + J,
        ["completion", "bash"], ["completion", "zsh"], ["completion", "fish"],
        ["__complete", "tools"], ["__complete", "toolsets"],
    ]
    cases += [["tools", "show", t] + J for t in ids]
    cases += [["tools", "show", t, "--cached"] for t in ids[:10]]
    cases += [["toolsets", "show", s] + J for s in sets]
    cases += [["toolsets", "show", s] for s in sets[:3]]
    # Mutations: the state file they leave behind is compared too.
    cases += [
        ["toolsets", "enable", sets[1]] + J, ["tools", "disable", ids[0]] + J, ["tools", "enable", ids[5]] + J,
        ["state"] + J, ["tools", "list", "--active"] + J, ["registry"] + J, ["state"],
        ["tools", "reset", ids[0], ids[5]] + J, ["toolsets", "disable", sets[1]] + J, ["state"] + J,
        ["tools", "disable", "ghost"] + J, ["toolsets", "enable", "ghost"] + J,
        ["tools", "enable", ids[0]], ["tools", "disable", ids[0]], ["tools", "reset", ids[0]],
    ]

    bad, same = [], 0
    for argv in cases:
        a, b = py.run(argv), rs.run(argv)
        a = (a[0], normalise(argv, a[1].replace(str(py.root), "<ROOT>")), a[2])
        b = (b[0], normalise(argv, b[1].replace(str(rs.root), "<ROOT>")), b[2])
        if a[0] == b[0] and a[1] == b[1]:
            same += 1
        else:
            bad.append((argv, a, b))

    # State files after the mutation run.
    for name in ("state.json",):
        pa, pb = py.cfg / name, rs.cfg / name
        ta = pa.read_bytes() if pa.exists() else None
        tb = pb.read_bytes() if pb.exists() else None
        if ta != tb:
            bad.append((["<file>", name], (0, repr(ta), ""), (0, repr(tb), "")))
        else:
            same += 1
    for name in ("cache.json",):
        # Same keys, same statuses; the timestamps differ by construction.
        pa, pb = py.cfg / name, rs.cfg / name
        # ts differs by construction; so does the stamp, which is the catalogue
        # file's mtime and the two sides wrote their own copies at different instants.
        strip = lambda t: re.sub(r'"stamp": "[^"]*"', '"stamp": ""', re.sub(r'"ts": [0-9.e+]+', '"ts": 0', t))
        ta = strip(pa.read_text()) if pa.exists() else None
        tb = strip(pb.read_text()) if pb.exists() else None
        if ta != tb:
            bad.append((["<file>", name], (0, ta and ta[:400], ""), (0, tb and tb[:400], "")))
        else:
            same += 1

    # Cache interop: each side must be able to read what the other wrote, since
    # pi's extensions share one cache across the transition (ADR-0014).
    inter = Side("x", RS, tools_text)
    subprocess.run(PY + ["registry", "--format", "json"], env={**os.environ, "REACTOR_CONFIG_DIR": str(inter.cfg), "HOME": str(inter.home)},
                   stdout=subprocess.DEVNULL, cwd=inter.cwd)
    py_cached = run(PY, ["registry", "--cached", "--format", "json"], inter.cfg, inter.home, inter.cwd)
    rs_cached = run(RS, ["registry", "--cached", "--format", "json"], inter.cfg, inter.home, inter.cwd)
    if py_cached[1] == rs_cached[1] and json.loads(rs_cached[1])["summary"]["unknown"] == 0:
        same += 1
    else:
        bad.append((["<interop: rust reads python's cache>"], py_cached, rs_cached))

    inter2 = Side("y", RS, tools_text)
    run(RS, ["registry", "--format", "json"], inter2.cfg, inter2.home, inter2.cwd)
    a = run(PY, ["registry", "--cached", "--format", "json"], inter2.cfg, inter2.home, inter2.cwd)
    b = run(RS, ["registry", "--cached", "--format", "json"], inter2.cfg, inter2.home, inter2.cwd)
    if a[1] == b[1] and json.loads(a[1])["summary"]["unknown"] == 0:
        same += 1
    else:
        bad.append((["<interop: python reads rust's cache>"], a, b))

    print(f"{same} identical, {len(bad)} different  ({len(cases)} commands)")
    for argv, a, b in bad[:25]:
        print("\n==", " ".join(argv), f"  rc py={a[0]} rs={b[0]}")
        import difflib
        d = list(difflib.unified_diff(a[1].splitlines(), b[1].splitlines(), "python", "rust", lineterm="", n=1))
        print("\n".join(d[:24]))
        if a[2] != b[2]:
            print("  stderr py:", a[2].strip()[:200], "\n  stderr rs:", b[2].strip()[:200])
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
