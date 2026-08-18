#!/usr/bin/env python3
"""Install REactor's non-pi half.

`pi install git:.../reactor` places the package and its extensions, skills,
prompts and themes. It does not build anything, does not initialise submodules,
and runs `git clean -fdx` inside the package on every update -- so the CLI
symlink, the seeded config and the fetched upstream skills all have to live
outside pi's package tree, put there by something pi does not manage. That is
this script. See docs/adr/0002 and docs/adr/0003.

Usage:
    python scripts/install.py                 # symlink the CLI, seed config, fetch skills
    python scripts/install.py --copy          # copy the CLI instead of symlinking
    python scripts/install.py --cli-dest PATH # somewhere other than ~/.local/bin/reactor
    python scripts/install.py --no-skills     # skip the network step
    python scripts/install.py --no-completions # skip the shell completion scripts
    python scripts/install.py --dry-run       # say what would happen and stop

Idempotent. Seeding never clobbers an existing config file: if yours differs
from the shipped copy it says so and points at `reactor diff-config`
(docs/adr/0004). Re-running is the supported way to update.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CONFIG_FILES = ("tools.toml", "toolsets.toml")

DEFAULT_CONFIG_DIR = Path.home() / ".pi" / "reactor"
DEFAULT_CLI_DEST = Path.home() / ".local" / "bin" / "reactor"


class Report:
    def __init__(self, dry_run: bool):
        self.dry_run = dry_run
        self.warnings: list[str] = []

    def act(self, message: str) -> None:
        print(("would " if self.dry_run else "") + message)

    def note(self, message: str) -> None:
        print(f"  {message}")

    def warn(self, message: str) -> None:
        self.warnings.append(message)
        print(f"  ! {message}")


def install_cli(dest: Path, *, copy: bool, report: Report) -> None:
    src = REPO_ROOT / "bin" / "reactor"
    if not src.is_file():
        report.warn(f"{src}: missing; nothing to install")
        return

    if dest.is_symlink() or dest.exists():
        current = dest.resolve() if dest.is_symlink() else None
        if current == src and not copy:
            report.note(f"{dest} → {src} (already linked)")
            return
        report.act(f"replace {dest}")
        if not report.dry_run:
            dest.unlink()
    else:
        report.act(f"install {dest}")

    if report.dry_run:
        return
    dest.parent.mkdir(parents=True, exist_ok=True)
    if copy:
        shutil.copy2(src, dest)
    else:
        dest.symlink_to(src)
    dest.chmod(dest.stat().st_mode | 0o111)

    if str(dest.parent) not in (os.environ.get("PATH") or "").split(os.pathsep):
        report.warn(f"{dest.parent} is not on your PATH")


def seed_config(config_dir: Path, *, report: Report) -> None:
    for name in CONFIG_FILES:
        shipped, live = REPO_ROOT / name, config_dir / name
        if not live.exists():
            report.act(f"seed {live}")
            if not report.dry_run:
                live.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(shipped, live)
        elif live.read_bytes() != shipped.read_bytes():
            # Never clobber: this is the user's file now (ADR-0003/0004).
            report.warn(f"{name} differs from the shipped copy -- `reactor diff-config`")
        else:
            report.note(f"{live} (up to date)")

    state = config_dir / "state.json"
    if not state.exists():
        report.act(f"create {state}")
        if not report.dry_run:
            state.parent.mkdir(parents=True, exist_ok=True)
            state.write_text(
                json.dumps(
                    {"version": 1, "toolsets": [], "tools": {"enabled": [], "disabled": []}},
                    indent=2,
                )
                + "\n"
            )
    else:
        report.note(f"{state} (kept)")


def fetch_skills(cli: Path, config_dir: Path, *, report: Report) -> None:
    """Delegate to the CLI so there is one implementation of skill fetching.

    Only a *configured* skill that could not be fetched is a warning. A tool
    with no configured skill is the normal case and says nothing (ADR-0008).
    """
    env = {**os.environ, "REACTOR_CONFIG_DIR": str(config_dir)}
    proc = subprocess.run(
        [sys.executable, str(cli), "skills", "fetch", "--format", "json"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
    )
    try:
        payload = json.loads(proc.stdout)
    except ValueError:
        report.warn(f"skill fetch produced no usable output: {proc.stderr.strip() or proc.stdout.strip()}")
        return
    if "error" in payload:
        report.warn(f"skill fetch: {payload['error']}")
        return
    for result in payload.get("fetched", []):
        if result["ok"]:
            report.note(f"{result['tool']} skill → {result['message']}")
        else:
            report.warn(f"{result['tool']} skill not fetched: {result['message']}")


# Where each shell's completion file goes, in the conventional per-user
# location for that shell (ADR-0015). bash and fish need no shell config
# change to pick these up (bash-completion v2's dir is auto-sourced if the
# bash-completion package is installed; fish autoloads its completions dir
# unconditionally). zsh needs `~/.zfunc` on `fpath` before `compinit`, which
# `main()` prints a one-time reminder about rather than editing `.zshrc`.
COMPLETION_TARGETS = {
    "bash": lambda: Path(os.environ.get("XDG_DATA_HOME") or (Path.home() / ".local" / "share"))
    / "bash-completion" / "completions" / "reactor",
    "zsh": lambda: Path.home() / ".zfunc" / "_reactor",
    "fish": lambda: Path(os.environ.get("XDG_CONFIG_HOME") or (Path.home() / ".config"))
    / "fish" / "completions" / "reactor.fish",
}


def install_completions(cli: Path, *, report: Report) -> None:
    """One script per shell, generated by `reactor completion <shell>` itself
    so there is one source of truth for the command surface (ADR-0015).
    Written unconditionally: these are reactor's files, not the user's, so
    there is nothing to preserve the way `seed_config` preserves tools.toml.
    """
    for shell, target in COMPLETION_TARGETS.items():
        dest = target()
        proc = subprocess.run(
            [sys.executable, str(cli), "completion", shell],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        if proc.returncode != 0:
            report.warn(f"{shell} completion: {proc.stderr.strip() or 'failed'}")
            continue
        script = proc.stdout
        if dest.is_file() and dest.read_text() == script:
            report.note(f"{dest} (up to date)")
            continue
        report.act(f"write {dest}")
        if not report.dry_run:
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(script)

    zfunc = COMPLETION_TARGETS["zsh"]().parent
    report.note(f"zsh: add `fpath+=({zfunc})` before `compinit` in .zshrc if not already there")


def main() -> int:
    p = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument("--cli-dest", type=Path, default=DEFAULT_CLI_DEST,
                   help=f"where the reactor command goes (default {DEFAULT_CLI_DEST})")
    p.add_argument("--config-dir", type=Path,
                   default=Path(os.environ.get("REACTOR_CONFIG_DIR") or DEFAULT_CONFIG_DIR),
                   help=f"live config directory (default {DEFAULT_CONFIG_DIR})")
    mode = p.add_mutually_exclusive_group()
    mode.add_argument("--link", action="store_true", default=True,
                      help="symlink the CLI (default; edits in this repo take effect at once)")
    mode.add_argument("--copy", action="store_true", help="copy the CLI instead of symlinking")
    p.add_argument("--no-skills", action="store_true", help="skip fetching upstream skills")
    p.add_argument("--no-completions", action="store_true",
                   help="skip installing bash/zsh/fish completion scripts")
    p.add_argument("--dry-run", action="store_true", help="report what would happen and stop")
    args = p.parse_args()

    if sys.version_info < (3, 11):
        print(f"reactor needs Python 3.11+ for tomllib; this is {sys.version.split()[0]}",
              file=sys.stderr)
        return 1

    report = Report(args.dry_run)

    print("CLI")
    install_cli(args.cli_dest, copy=args.copy, report=report)

    print("config")
    seed_config(args.config_dir, report=report)

    print("skills")
    if args.no_skills:
        report.note("skipped (--no-skills)")
    elif args.dry_run:
        report.note("skipped (--dry-run)")
    else:
        fetch_skills(REPO_ROOT / "bin" / "reactor", args.config_dir, report=report)

    print("completions")
    if args.no_completions:
        report.note("skipped (--no-completions)")
    else:
        install_completions(REPO_ROOT / "bin" / "reactor", report=report)

    print()
    if report.warnings:
        print(f"{len(report.warnings)} warning(s). REactor still works -- see above.")
    print("Next: `reactor doctor` for what is present and what is missing.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
