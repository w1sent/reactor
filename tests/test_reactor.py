#!/usr/bin/env python3
"""Tests for the reactor CLI.

stdlib unittest, no dependencies -- same reasoning as the CLI itself
(docs/adr/0005). Run with:

    python3 -m unittest discover -s tests -v
    python3 tests/test_reactor.py

The load-bearing test here is TestRegistryDeterminism: replacing the system
prompt invalidates the provider's cached prefix, so the rendered block must be
byte-identical across turns when nothing about the machine changed
(docs/adr/0006). That is a property, not an intention, so it gets a test.
"""

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from importlib.machinery import SourceFileLoader
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CLI_PATH = REPO_ROOT / "bin" / "reactor"


def load_cli():
    """Import bin/reactor, which has no .py extension, as a module."""
    loader = SourceFileLoader("reactor_cli", str(CLI_PATH))
    spec = importlib.util.spec_from_loader("reactor_cli", loader)
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


R = load_cli()


FIXTURE_TOOLS = """
version = 1

[platform]
prefer = ["pacman", "uv", "pip"]

[platform.manager]
pacman = { binary = "pacman", os = "linux", sudo = true }
brew   = { binary = "brew", os = "darwin" }
uv     = { binary = "uv" }
pip    = { binary = "pip3" }

[probe]
timeout = 2.0

[tool.alpha]
name   = "Alpha"
desc   = "does alpha things"
invoke = "alpha"
detect = { binary = "alpha" }
tags   = ["static"]

[tool.alpha.install]
pacman = "pacman -S alpha"
uv     = "uv tool install alpha"
manual = "https://example.invalid/alpha"

[tool.beta]
name   = "Beta"
desc   = "does beta things"
invoke = "python3 -c 'import beta'"
detect = { python_module = "beta" }
tags   = ["dynamic", "python"]

[tool.gamma]
name    = "Gamma"
desc    = "does gamma things"
invoke  = "gamma"
detect  = { binary = "gamma" }
tags    = ["odd"]
service = { probe = ["gamma", "status"], label = "gamma", count = { pattern = 'ready$', noun = "worker" } }
"""

FIXTURE_TOOLSETS = """
version = 1

[toolset.all]
desc = "everything"
all  = true

[toolset.static]
desc = "static only"
tags = ["static"]

[toolset.pair]
desc  = "explicit"
tools = ["alpha", "gamma"]
"""


class CatalogueFixture(unittest.TestCase):
    """Points the module's globals at a throwaway config directory."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="reactor-test-")
        self.dir = Path(self._tmp.name)
        (self.dir / "tools.toml").write_text(FIXTURE_TOOLS)
        (self.dir / "toolsets.toml").write_text(FIXTURE_TOOLSETS)
        self._saved = (R.CONFIG_DIR, R.PACKAGE_ROOT)
        R.CONFIG_DIR = self.dir
        R.PACKAGE_ROOT = self.dir
        self.addCleanup(self._restore)

    def _restore(self):
        R.CONFIG_DIR, R.PACKAGE_ROOT = self._saved
        self._tmp.cleanup()

    def state(self, **kwargs):
        return R.State(path=self.dir / "state.json", scope="machine", **kwargs)


class TestCatalogue(CatalogueFixture):
    def test_loads_every_detect_kind(self):
        cat = R.load_catalogue()
        self.assertEqual(list(cat.tools), ["alpha", "beta", "gamma"])
        self.assertEqual(cat.tools["alpha"].detect_kind, "binary")
        self.assertEqual(cat.tools["beta"].detect_kind, "python_module")
        self.assertEqual(cat.tools["gamma"].service_probe, ["gamma", "status"])

    def test_declaration_order_is_preserved(self):
        # The registry's ordering is the catalogue's ordering, and a stable
        # order is what keeps the rendered block cache-friendly.
        cat = R.load_catalogue()
        self.assertEqual(R.catalogue_order(cat), ["alpha", "beta", "gamma"])

    def test_rejects_unknown_detect_kind(self):
        (self.dir / "tools.toml").write_text(
            'version = 1\n[tool.x]\nname="X"\ndesc="d"\ninvoke="x"\ndetect={ magic = "x" }\n'
        )
        with self.assertRaises(R.ReactorError) as cm:
            R.load_catalogue()
        self.assertIn("unknown kind", str(cm.exception))

    def test_rejects_missing_required_field(self):
        (self.dir / "tools.toml").write_text(
            'version = 1\n[tool.x]\nname="X"\ninvoke="x"\ndetect={ binary = "x" }\n'
        )
        with self.assertRaises(R.ReactorError) as cm:
            R.load_catalogue()
        self.assertIn("desc", str(cm.exception))

    def test_rejects_wrong_version(self):
        (self.dir / "tools.toml").write_text("version = 99\n")
        with self.assertRaises(R.ReactorError):
            R.load_catalogue()


class TestToolsets(CatalogueFixture):
    def test_all_covers_every_tool_whatever_its_tags(self):
        # gamma's only tag is "odd", which no toolset names. A tag-union "all"
        # would silently drop it; `all = true` must not.
        cat = R.load_catalogue()
        sets = R.load_toolsets()
        self.assertEqual(R.toolset_members(sets["all"], cat), ["alpha", "beta", "gamma"])

    def test_tag_selection(self):
        cat = R.load_catalogue()
        sets = R.load_toolsets()
        self.assertEqual(R.toolset_members(sets["static"], cat), ["alpha"])

    def test_explicit_selection(self):
        cat = R.load_catalogue()
        sets = R.load_toolsets()
        self.assertEqual(R.toolset_members(sets["pair"], cat), ["alpha", "gamma"])


class TestActivation(CatalogueFixture):
    def setUp(self):
        super().setUp()
        self.cat = R.load_catalogue()
        self.sets = R.load_toolsets()

    def test_no_toolsets_means_everything(self):
        active = R.active_ids(self.cat, self.sets, self.state())
        self.assertEqual(active, {"alpha", "beta", "gamma"})

    def test_toolset_narrows(self):
        active = R.active_ids(self.cat, self.sets, self.state(toolsets=["static"]))
        self.assertEqual(active, {"alpha"})

    def test_enable_adds_outside_the_toolset(self):
        active = R.active_ids(self.cat, self.sets, self.state(toolsets=["static"], enabled=["beta"]))
        self.assertEqual(active, {"alpha", "beta"})

    def test_disable_wins_over_enable(self):
        active = R.active_ids(
            self.cat, self.sets, self.state(toolsets=["static"], enabled=["beta"], disabled=["beta"])
        )
        self.assertEqual(active, {"alpha"})

    def test_unknown_toolset_is_ignored_not_fatal(self):
        # A stale state.json naming a toolset the user has since deleted must
        # not make every command fail.
        active = R.active_ids(self.cat, self.sets, self.state(toolsets=["static", "ghost"]))
        self.assertEqual(active, {"alpha"})


class TestRecipeRanking(CatalogueFixture):
    def test_prefer_order_wins(self):
        cat = R.load_catalogue()
        candidates, notes = R.rank_recipes(
            cat.tools["alpha"], cat, {"uv": "/bin/uv", "pacman": "/bin/pacman"}
        )
        self.assertEqual([c["method"] for c in candidates], ["pacman", "uv"])
        self.assertTrue(candidates[0]["sudo"])
        self.assertEqual(notes, {"manual": "https://example.invalid/alpha"})

    def test_absent_manager_is_not_a_candidate(self):
        cat = R.load_catalogue()
        candidates, notes = R.rank_recipes(cat.tools["alpha"], cat, {"uv": "/bin/uv"})
        self.assertEqual([c["method"] for c in candidates], ["uv"])
        self.assertIn("pacman", notes)

    def test_unknown_key_is_a_note_never_a_candidate(self):
        # This is what stops `reactor install` ever running free text.
        cat = R.load_catalogue()
        candidates, notes = R.rank_recipes(cat.tools["alpha"], cat, {})
        self.assertEqual(candidates, [])
        self.assertIn("manual", notes)

    def test_os_constrained_manager_is_filtered(self):
        cat = R.load_catalogue()
        available = R.available_managers(cat)
        expect = "brew" in available
        self.assertEqual(expect, sys.platform.startswith("darwin") and bool(available.get("brew")))


class TestHelpers(unittest.TestCase):
    def test_version_extraction_drops_the_banner(self):
        # Full --version banners carry build dates and hostnames; letting one
        # into the registry would rewrite the system prompt for no reason.
        self.assertEqual(R._version_of("ripgrep 14.1.1 (rev abc123)"), "14.1.1")
        self.assertEqual(R._version_of("GNU gdb (GDB) 16.2"), "16.2")
        self.assertEqual(R._version_of("frida 17.2"), "17.2")

    def test_version_extraction_survives_no_number(self):
        self.assertEqual(R._version_of(""), None)
        self.assertEqual(R._version_of("unknown build"), "unknown build")

    def test_service_detail_counts_matching_lines(self):
        tool = R.Tool(
            id="x", name="X", desc="d", invoke="x", detect_kind="binary", detect_value="x",
            service_count={"pattern": r"\sdevice$", "noun": "device"},
        )
        out = "List of devices attached\nemulator-5554\tdevice\nRZ8N\tdevice\n"
        self.assertEqual(R._service_detail(tool, out), "2 devices")

    def test_service_detail_singular(self):
        tool = R.Tool(
            id="x", name="X", desc="d", invoke="x", detect_kind="binary", detect_value="x",
            service_count={"pattern": r"\sdevice$", "noun": "device"},
        )
        self.assertEqual(R._service_detail(tool, "a\tdevice\n"), "1 device")

    def test_service_detail_is_none_without_a_count_spec(self):
        # No spec means state only. Free-text service output must never reach
        # the registry (ADR-0006).
        tool = R.Tool(id="x", name="X", desc="d", invoke="x", detect_kind="binary", detect_value="x")
        self.assertIsNone(R._service_detail(tool, "anything at all"))

    def test_service_detail_survives_a_bad_pattern(self):
        tool = R.Tool(
            id="x", name="X", desc="d", invoke="x", detect_kind="binary", detect_value="x",
            service_count={"pattern": "(unclosed", "noun": "thing"},
        )
        self.assertIsNone(R._service_detail(tool, "x"))


def entry(tid, **kw):
    base = {
        "id": tid, "name": tid, "desc": f"does {tid}", "invoke": tid, "tags": [],
        "detect": {"kind": "binary", "value": tid}, "status": R.PRESENT, "path": f"/bin/{tid}",
        "version": None, "active": True, "service": None, "skill": None,
    }
    base.update(kw)
    return base


class TestRegistryRendering(unittest.TestCase):
    def test_only_present_and_active_tools_are_listed(self):
        block = R.render_registry([
            entry("alpha"),
            entry("beta", status=R.ABSENT),
            entry("gamma", active=False),
        ])
        self.assertIn("alpha", block)
        self.assertNotIn("beta", block)
        self.assertNotIn("gamma", block)

    def test_empty_registry_says_so_and_points_somewhere(self):
        block = R.render_registry([entry("alpha", status=R.ABSENT)])
        self.assertIn("reactor doctor", block)

    def test_python_modules_show_their_module_not_the_usage_example(self):
        block = R.render_registry([
            entry("beta", invoke="python3 -c 'import beta'",
                  detect={"kind": "python_module", "value": "beta"})
        ])
        self.assertRegex(block, r"(?m)^beta\s")
        self.assertIn("(python module)", block)

    def test_service_state_is_annotated(self):
        block = R.render_registry([
            entry("adb", service={"label": "adb", "state": R.UP, "detail": "2 devices"})
        ])
        self.assertIn("[adb: 2 devices]", block)

    def test_service_down_is_shown_not_hidden(self):
        # A tool whose service is down is still installed and still worth
        # knowing about -- and `bn` reporting "down" is how the agent learns to
        # start Binary Ninja rather than concluding it does not exist.
        block = R.render_registry([entry("bn", service={"label": "BN", "state": R.DOWN, "detail": None})])
        self.assertIn("[BN: down]", block)

    def test_no_line_has_trailing_whitespace(self):
        block = R.render_registry([
            entry("a", desc="short", version="1.0"),
            entry("bbbbbb", desc="a much longer description here"),
        ])
        for line in block.splitlines():
            self.assertEqual(line, line.rstrip(), f"trailing whitespace: {line!r}")


class TestRegistryDeterminism(unittest.TestCase):
    """The prompt-cache constraint, as a test rather than an intention."""

    ENTRIES = [
        entry("bn", desc="reverse engineering framework",
              service={"label": "BN session", "state": R.UP, "detail": None}),
        entry("frida", desc="dynamic instrumentation", version="17.2"),
        entry("jadx", desc="decompile Android DEX/APK to Java"),
        entry("lief", desc="parse ELF/PE/Mach-O", invoke="python3 -c 'import lief'",
              detect={"kind": "python_module", "value": "lief"}),
    ]

    def test_identical_input_renders_identical_bytes(self):
        first = R.render_registry(self.ENTRIES)
        second = R.render_registry([dict(e) for e in self.ENTRIES])
        self.assertEqual(first.encode(), second.encode())

    def test_rendering_carries_nothing_time_derived(self):
        import re

        block = R.render_registry(self.ENTRIES)
        # Any four-digit year, clock time, or epoch-scale integer would mean a
        # new system prompt every turn.
        self.assertNotRegex(block, r"\b(19|20)\d{2}\b")
        self.assertNotRegex(block, r"\b\d{2}:\d{2}\b")
        self.assertNotRegex(block, r"\b1[6-9]\d{8}\b")

    def test_a_real_change_does_change_the_bytes(self):
        # The flip side: caching must not be bought by ignoring reality.
        before = R.render_registry(self.ENTRIES)
        changed = [dict(e) for e in self.ENTRIES]
        changed[0]["service"] = {"label": "BN session", "state": R.DOWN, "detail": None}
        self.assertNotEqual(before, R.render_registry(changed))

    def test_deactivating_a_tool_changes_the_bytes(self):
        before = R.render_registry(self.ENTRIES)
        changed = [dict(e) for e in self.ENTRIES]
        changed[1]["active"] = False
        self.assertNotEqual(before, R.render_registry(changed))


class TestShippedConfig(unittest.TestCase):
    """The real files, loaded through the real loader."""

    def setUp(self):
        self._saved = (R.CONFIG_DIR, R.PACKAGE_ROOT)
        R.CONFIG_DIR = REPO_ROOT / "does-not-exist"
        R.PACKAGE_ROOT = REPO_ROOT
        self.addCleanup(lambda: setattr_pair(R, self._saved))

    def test_shipped_catalogue_is_valid(self):
        cat = R.load_catalogue()
        self.assertTrue(cat.tools)
        self.assertTrue(cat.managers)

    def test_every_desc_fits_the_registry_budget(self):
        # desc lands in every system prompt for as long as the tool is
        # installed, so it gets a length budget (ADR-0003/0006).
        cat = R.load_catalogue()
        for t in cat.tools.values():
            with self.subTest(tool=t.id):
                self.assertLessEqual(len(t.desc), 80, f"{t.id}: desc is {len(t.desc)} chars")
                self.assertNotIn("\n", t.desc)

    def test_every_install_key_is_a_manager_or_deliberately_free_text(self):
        cat = R.load_catalogue()
        allowed_notes = {"manual"}
        for t in cat.tools.values():
            for key in t.install:
                with self.subTest(tool=t.id, key=key):
                    self.assertTrue(
                        key in cat.managers or key in allowed_notes,
                        f"{t.id}.install.{key}: neither a declared manager nor `manual` -- "
                        "a distro name here would never be selected (ADR-0010)",
                    )

    def test_every_prefer_entry_names_a_declared_manager(self):
        cat = R.load_catalogue()
        for mid in cat.prefer:
            self.assertIn(mid, cat.managers, f"[platform].prefer names undeclared manager {mid!r}")

    def test_shipped_toolsets_reference_real_tools_and_tags(self):
        cat = R.load_catalogue()
        tags = {tag for t in cat.tools.values() for tag in t.tags}
        for ts in R.load_toolsets().values():
            for tid in ts.tools:
                self.assertIn(tid, cat.tools, f"toolset {ts.id} names unknown tool {tid!r}")
            for tag in ts.tags:
                self.assertIn(tag, tags, f"toolset {ts.id} names unused tag {tag!r}")

    def test_no_shipped_toolset_is_empty(self):
        cat = R.load_catalogue()
        for ts in R.load_toolsets().values():
            self.assertTrue(R.toolset_members(ts, cat), f"toolset {ts.id} selects nothing")


def setattr_pair(module, saved):
    module.CONFIG_DIR, module.PACKAGE_ROOT = saved


class TestJsonContract(unittest.TestCase):
    """`--format json` is the extensions' only interface; its shape is pinned."""

    @classmethod
    def setUpClass(cls):
        cls._tmp = tempfile.TemporaryDirectory(prefix="reactor-json-")
        cls.cfg = Path(cls._tmp.name)
        for name in R.CONFIG_FILES:
            (cls.cfg / name).write_text((REPO_ROOT / name).read_text())

    @classmethod
    def tearDownClass(cls):
        cls._tmp.cleanup()

    def run_cli(self, *args):
        env = {**os.environ, "REACTOR_CONFIG_DIR": str(self.cfg)}
        proc = subprocess.run(
            [sys.executable, str(CLI_PATH), *args, "--format", "json"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env, timeout=120,
        )
        return json.loads(proc.stdout), proc

    def test_registry_shape(self):
        payload, _ = self.run_cli("registry")
        self.assertEqual(payload["schema"], R.SCHEMA)
        for key in ("block", "tools", "summary", "skillPaths"):
            self.assertIn(key, payload)
        for key in ("present", "absent", "unknown", "catalogued"):
            self.assertIn(key, payload["summary"])
        for tool in payload["tools"]:
            for key in ("id", "desc", "invoke", "status", "active", "detect"):
                self.assertIn(key, tool)
            self.assertIn(tool["status"], (R.PRESENT, R.ABSENT, R.UNKNOWN))

    def test_registry_block_is_stable_across_processes(self):
        # The end-to-end version of the determinism test: two separate runs,
        # separate probe passes, one cache -- same bytes.
        first, _ = self.run_cli("registry")
        second, _ = self.run_cli("registry")
        self.assertEqual(first["block"], second["block"])

    def test_doctor_shape(self):
        payload, _ = self.run_cli("doctor")
        for key in ("platform", "config", "tools", "problems"):
            self.assertIn(key, payload)
        for tool in payload["tools"]:
            self.assertIn("install", tool)
            self.assertIn("recommended", tool["install"])

    def test_unknown_tool_reports_an_error_not_a_traceback(self):
        payload, proc = self.run_cli("tools", "show", "definitely-not-a-tool")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("error", payload)
        self.assertNotIn("Traceback", proc.stderr)

    def test_install_never_runs_without_confirmation(self):
        # json mode has no tty to confirm on, so it must refuse rather than
        # assume yes. Every catalogued tool is a plausible `sudo pacman -S`.
        payload, proc = self.run_cli("install", "jq")
        self.assertEqual(payload.get("ran", []), [])


if __name__ == "__main__":
    unittest.main(verbosity=2)
