#!/usr/bin/env python3
"""Check every install recipe in the catalogue against its manager's registry.

A wrong recipe is worse than an absent one, because `reactor install` will run
it. This asks each package index whether the name a recipe would install
actually exists, so a typo or a package that moved repositories is caught at
authoring time rather than on a user's machine.

    scripts/verify-recipes.py [tools.toml]

Exits 1 if any recipe is unresolved, so it can gate a release. Needs network for
every manager except pacman, which uses the local sync database. Read-only GETs
against public indexes; nothing is installed and nothing is written.

What this does NOT check: that the package actually provides the binary the
tool's `detect` looks for. Package and binary names diverge often enough to
matter -- Debian's `aapt` ships /usr/bin/aapt2, Arch's `android-tools` ships
adb, `wireshark-cli` ships tshark -- and a recipe can name a real package that
installs the wrong thing. Verifying that needs a per-manager file listing, which
only some indexes expose. So a clean run means "this package exists", not "this
recipe works"; the second still wants a human who knows the tool.

This is deliberately *not* part of the test suite: the tests are offline and
fast, and this is neither. Run it when the catalogue's install recipes change.
"""
from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

DEFAULT_CATALOGUE = Path(__file__).resolve().parent.parent / "tools.toml"

HEADERS = {"User-Agent": "reactor-verify-recipes/1 (+https://github.com/w1sent/reactor)"}
TIMEOUT = 20
WORKERS = 12

OK = "ok"


def get(url: str) -> tuple[int, bytes]:
    """GET, returning (status, body). Never raises; -1 is a transport failure."""
    req = urllib.request.Request(url, headers=HEADERS)
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, b""
    except Exception as e:  # noqa: BLE001 — transport failure is a result, not a crash
        return -1, str(e).encode()


# --- what a recipe would install -------------------------------------------

# Command words that are never a package name. Anything else that is not a flag
# is taken to be one, so a recipe that installs two packages checks both.
NOISE = {
    "sudo", "install", "add", "tool",
    "pacman", "paru", "yay", "apt", "apt-get", "dnf", "zypper", "apk",
    "brew", "port", "uv", "pip", "pip3", "pipx", "cargo", "go", "npm", "gem",
    "dotnet",
}


def packages(cmd: str) -> list[str]:
    return [t for t in cmd.split() if t not in NOISE and not t.startswith("-")]


# --- per-manager existence checks ------------------------------------------
#
# Each returns (status, detail). status == OK means the recipe is runnable as
# written; anything else is reported and fails the run.


def _pacman_repos(pkg: str) -> tuple[str, str] | None:
    r = subprocess.run(["pacman", "-Si", pkg], capture_output=True, text=True)
    if r.returncode != 0:
        return None
    repo = version = ""
    for line in r.stdout.splitlines():
        if line.startswith("Repository"):
            repo = line.split(":", 1)[1].strip()
        elif line.startswith("Version"):
            version = line.split(":", 1)[1].strip()
    return repo, version


def _aur(pkg: str) -> str | None:
    status, body = get(f"https://aur.archlinux.org/rpc/v5/info?arg[]={pkg}")
    if status != 200:
        return None
    results = json.loads(body).get("results") or []
    return results[0].get("Version", "") if results else None


def check_pacman(cmd: str, pkg: str) -> tuple[str, str]:
    hit = _pacman_repos(pkg)
    if hit:
        return OK, f"{hit[0]} {hit[1]}"
    version = _aur(pkg)
    if version:
        return "AUR-ONLY", f"AUR has {version}; pacman cannot install it"
    return "MISSING", "in neither the official repos nor the AUR"


def check_aur_helper(cmd: str, pkg: str) -> tuple[str, str]:
    version = _aur(pkg)
    if version:
        return OK, f"AUR {version}"
    hit = _pacman_repos(pkg)
    if hit:
        return "USE-PACMAN", f"in {hit[0]}; prefer a pacman recipe"
    return "MISSING", "in neither the AUR nor the official repos"


def check_apt(cmd: str, pkg: str) -> tuple[str, str]:
    found = [
        f"debian:{suite}"
        for suite in ("stable", "sid")
        if get(f"https://packages.debian.org/{suite}/{pkg}")[0] == 200
    ]
    status, body = get(
        "https://api.launchpad.net/1.0/ubuntu/+archive/primary"
        f"?ws.op=getPublishedBinaries&binary_name={pkg}"
        "&exact_match=true&status=Published"
    )
    if status == 200:
        try:
            if json.loads(body).get("total_size"):
                found.append("ubuntu")
        except ValueError:
            pass
    if found:
        return OK, ", ".join(found)
    return "MISSING", "not a binary package in Debian or Ubuntu"


def check_dnf(cmd: str, pkg: str) -> tuple[str, str]:
    for release in ("f42", "f41", "rawhide"):
        status, body = get(f"https://mdapi.fedoraproject.org/{release}/pkg/{pkg}")
        if status == 200:
            try:
                return OK, f"fedora:{release} {json.loads(body).get('version', '')}"
            except ValueError:
                return OK, f"fedora:{release}"
    return "MISSING", "not in Fedora"


def check_brew(cmd: str, pkg: str) -> tuple[str, str]:
    wants_cask = "--cask" in cmd
    name = pkg.rsplit("/", 1)[-1]
    tapped = "/" in pkg

    status, body = get(f"https://formulae.brew.sh/api/formula/{name}.json")
    formula = json.loads(body)["versions"]["stable"] if status == 200 else None
    status, body = get(f"https://formulae.brew.sh/api/cask/{name}.json")
    cask = json.loads(body)["version"] if status == 200 else None

    if tapped:
        # A third-party tap: the formula file must exist in the tap repository.
        user, tap, formula_name = pkg.split("/", 2)
        repo = f"{user}/homebrew-{tap}"
        for path in (f"Formula/{formula_name}.rb", f"{formula_name}.rb"):
            if get(f"https://raw.githubusercontent.com/{repo}/HEAD/{path}")[0] == 200:
                if formula:
                    return "PREFER-CORE", f"{repo} has it, but core has {formula}"
                return OK, f"tap {repo}"
        return "MISSING", f"tap {repo} has no {formula_name}.rb"

    if wants_cask:
        if cask:
            return OK, f"cask {cask}"
        if formula:
            return "NOT-A-CASK", f"--cask given but {name} is a formula ({formula})"
        return "MISSING", "no such cask"
    if formula:
        return OK, f"formula {formula}"
    if cask:
        return "IS-A-CASK", f"cask {cask} — recipe needs --cask"
    return "MISSING", "no core formula or cask"


def check_pypi(cmd: str, pkg: str) -> tuple[str, str]:
    status, body = get(f"https://pypi.org/pypi/{pkg}/json")
    if status == 200:
        return OK, f"pypi {json.loads(body)['info']['version']}"
    return "MISSING", "not on PyPI"


def check_cargo(cmd: str, pkg: str) -> tuple[str, str]:
    status, body = get(f"https://crates.io/api/v1/crates/{pkg}")
    if status == 200:
        crate = json.loads(body).get("crate", {})
        return OK, f"crates.io {crate.get('max_stable_version') or crate.get('max_version', '')}"
    return "MISSING", "not on crates.io"


def check_dotnet(cmd: str, pkg: str) -> tuple[str, str]:
    # NuGet ids are case-insensitive; the flat container is lowercase-only.
    status, body = get(
        f"https://api.nuget.org/v3-flatcontainer/{pkg.lower()}/index.json"
    )
    if status != 200:
        return "MISSING", "not on nuget.org"
    versions = json.loads(body).get("versions") or []
    if "--global" not in cmd and "-g" not in cmd.split():
        return "NOT-GLOBAL", "`dotnet tool install` without --global is project-scoped"
    return OK, f"nuget {versions[-1] if versions else '?'}"


def check_npm(cmd: str, pkg: str) -> tuple[str, str]:
    status, body = get(f"https://registry.npmjs.org/{pkg}")
    if status == 200:
        return OK, f"npm {json.loads(body).get('dist-tags', {}).get('latest', '')}"
    return "MISSING", "not on the npm registry"


CHECKS = {
    "pacman": check_pacman,
    "paru": check_aur_helper,
    "yay": check_aur_helper,
    "apt": check_apt,
    "dnf": check_dnf,
    "brew": check_brew,
    "uv": check_pypi,
    "pip": check_pypi,
    "pipx": check_pypi,
    "cargo": check_cargo,
    "npm": check_npm,
    "dotnet": check_dotnet,
}


def main(argv: list[str]) -> int:
    path = Path(argv[1]) if len(argv) > 1 else DEFAULT_CATALOGUE
    catalogue = tomllib.loads(path.read_text())
    managers = set(catalogue["platform"]["manager"])

    jobs, unknown = [], []
    for tool_id, tool in catalogue["tool"].items():
        for key, cmd in (tool.get("install") or {}).items():
            if key not in managers:
                continue  # a note: shown, never executed, nothing to verify
            if key not in CHECKS:
                unknown.append((tool_id, key))
                continue
            jobs.extend((tool_id, key, cmd, pkg) for pkg in packages(cmd))

    def run(job):
        tool_id, key, cmd, pkg = job
        try:
            status, detail = CHECKS[key](cmd, pkg)
        except Exception as e:  # noqa: BLE001 — one bad lookup must not stop the run
            status, detail = "ERROR", repr(e)
        return tool_id, key, cmd, pkg, status, detail

    with ThreadPoolExecutor(max_workers=WORKERS) as pool:
        results = list(pool.map(run, jobs))

    results.sort(key=lambda r: (r[4] == OK, r[0], r[1]))
    width = max((len(f"{r[0]}/{r[1]}") for r in results), default=0)

    failed = 0
    for tool_id, key, cmd, pkg, status, detail in results:
        print(f"{status:11} {tool_id + '/' + key:{width}}  {pkg:28} {detail}")
        if status != OK:
            failed += 1
            print(f"{'':11} {'':{width}}  cmd: {cmd}")

    for tool_id, key in unknown:
        failed += 1
        print(f"{'NO-CHECKER':11} {tool_id + '/' + key:{width}}  "
              f"declared manager with no registry check in this script")

    print(f"\n{len(results)} package references, {failed} unresolved")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
