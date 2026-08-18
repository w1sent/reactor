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

import contextlib
import importlib.util
import io
import json
import os
import subprocess
import sys
import tempfile
import types
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
tags   = ["static", "python"]

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

# Both tags are carried by something, so a union would widen these and an
# intersection narrows them -- which is what makes them worth asserting.
[toolset.narrow]
desc = "two tags, one tool"
tags = ["static", "python"]

[toolset.miss]
desc = "nothing carries both"
tags = ["static", "dynamic"]

[toolset.plus]
desc  = "a tag, and one more by name"
tags  = ["dynamic", "python"]
tools = ["gamma"]
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
        # gamma's only tag is "odd", which no toolset names. A group built from
        # tags would drop it either way; `all = true` must not.
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

    def test_tags_intersect_rather_than_union(self):
        # ADR-0013. beta is the only "python" tool that is also "dynamic";
        # alpha is python but static. A union would return both, which is how
        # [toolset.native] once collected every static tool there is.
        cat = R.load_catalogue()
        sets = R.load_toolsets()
        self.assertEqual(R.toolset_members(sets["narrow"], cat), ["alpha"])
        self.assertEqual(R.toolset_members(sets["plus"], cat), ["beta", "gamma"])

    def test_tags_nothing_carries_together_select_nothing(self):
        # An over-specified list is now empty rather than over-wide. That is the
        # better failure -- visibly nothing beats quietly everything -- but it
        # is a failure, so `reactor doctor` reports it.
        cat = R.load_catalogue()
        sets = R.load_toolsets()
        self.assertEqual(R.toolset_members(sets["miss"], cat), [])

    def tools_list(self, *tags):
        args = types.SimpleNamespace(
            tag=list(tags) or None, active=False, present=False, missing=False,
            refresh=False, cached=True, format="json",
        )
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            R.cmd_tools_list(args)
        return [t["id"] for t in json.loads(out.getvalue())["tools"]]

    def test_repeating_the_tag_filter_narrows(self):
        # `--tag` is action="append", so it was the other place a list of tags
        # had a meaning -- and it had the other one (ADR-0013).
        self.assertEqual(self.tools_list("python"), ["alpha", "beta"])
        self.assertEqual(self.tools_list("python", "static"), ["alpha"])
        self.assertEqual(self.tools_list("python", "odd"), [])

    def test_doctor_reports_a_toolset_that_selects_nothing(self):
        args = types.SimpleNamespace(format="json", cached=True, check_skills=False)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            R.cmd_doctor(args)
        problems = json.loads(out.getvalue())["problems"]
        empty = [p["toolset"] for p in problems if p["kind"] == "toolset-empty"]
        self.assertEqual(empty, ["miss"])


SERVICE_TOOLS = """
version = 1

[probe]
timeout = 5.0

[tool.answering]
name    = "Answering"
desc    = "a service that answers"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "echo 'a device'; echo 'b device'"], label = "answering", count = { pattern = 'device$', noun = "device" } }

[tool.refusing]
name    = "Refusing"
desc    = "a service that is not running"
invoke  = "sh"
detect  = { binary = "sh" }
service = { probe = ["sh", "-c", "exit 3"], label = "refusing" }

[tool.uninstalled]
name    = "Uninstalled"
desc    = "declares a service but is not here"
invoke  = "reactor-absent-by-design"
detect  = { binary = "reactor-absent-by-design" }
service = { probe = ["sh", "-c", "exit 0"], label = "uninstalled" }

[tool.plain]
name   = "Plain"
desc   = "no service at all"
invoke = "sh"
detect = { binary = "sh" }
"""


class TestServices(CatalogueFixture):
    """`reactor services` -- what is up, for a status line to ask every turn."""

    def setUp(self):
        super().setUp()
        (self.dir / "tools.toml").write_text(SERVICE_TOOLS)

    def services(self, **flags):
        args = types.SimpleNamespace(**{"format": "json", "refresh": False, "cached": False, **flags})
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            R.cmd_services(args)
        return json.loads(out.getvalue())

    def test_only_tools_with_a_service_probe_are_reported(self):
        # `plain` is installed and irrelevant here. Listing it would make the
        # status line a second, worse copy of the registry.
        ids = [s["id"] for s in self.services()["services"]]
        self.assertEqual(ids, ["answering", "refusing", "uninstalled"])

    def test_a_running_service_carries_its_count(self):
        by_id = {s["id"]: s for s in self.services()["services"]}
        self.assertEqual(by_id["answering"]["state"], R.UP)
        self.assertEqual(by_id["answering"]["detail"], "2 devices")
        self.assertEqual(by_id["answering"]["label"], "answering")

    def test_a_refused_probe_is_down_with_no_detail(self):
        by_id = {s["id"]: s for s in self.services()["services"]}
        self.assertEqual(by_id["refusing"]["state"], R.DOWN)
        self.assertIsNone(by_id["refusing"]["detail"])

    def test_an_uninstalled_tool_is_unknown_not_down(self):
        # "down" is a claim that something exists and is not running. For a
        # tool that is not installed, that claim is false and misleading -- it
        # would send the agent looking for a service to start.
        by_id = {s["id"]: s for s in self.services()["services"]}
        self.assertEqual(by_id["uninstalled"]["state"], R.UNKNOWN)
        self.assertEqual(by_id["uninstalled"]["status"], R.ABSENT)

    def test_the_summary_counts_every_reported_service(self):
        payload = self.services()
        self.assertEqual(payload["summary"], {R.UP: 1, R.DOWN: 1, R.UNKNOWN: 1})
        self.assertEqual(sum(payload["summary"].values()), len(payload["services"]))

    def test_cached_never_probes_and_says_unknown_instead(self):
        # The cache is the whole sharing mechanism between extensions
        # (ADR-0014), so `--cached` has to be honest about a cold one rather
        # than reporting a state nobody measured.
        states = {s["id"]: s["state"] for s in self.services(cached=True)["services"]}
        self.assertEqual(set(states.values()), {R.UNKNOWN})

    def test_text_output_names_every_service(self):
        args = types.SimpleNamespace(format="text", refresh=False, cached=False)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            R.cmd_services(args)
        text = out.getvalue()
        for tid in ("answering", "refusing", "uninstalled"):
            self.assertIn(tid, text)
        self.assertNotIn("plain", text)


class TestAtomicWrites(CatalogueFixture):
    """Two processes can be writing the cache at once, so the write is atomic
    and the temp file is per-process (ADR-0014)."""

    def test_the_temp_file_is_not_shared_between_processes(self):
        path = self.dir / "thing.json"
        R._write_json(path, {"a": 1})
        self.assertEqual(json.loads(path.read_text()), {"a": 1})
        # A fixed `thing.json.tmp` is what lets two writers interleave into one
        # buffer and then rename the mixture into place.
        self.assertNotIn(f"{path.name}.tmp", [p.name for p in self.dir.iterdir()])

    def test_a_failed_write_leaves_no_temp_file_behind(self):
        locked = self.dir / "locked"
        locked.mkdir()
        locked.chmod(0o500)
        self.addCleanup(locked.chmod, 0o700)
        with self.assertRaises(R.ReactorError):
            R._write_json(locked / "thing.json", {"a": 1})
        self.assertEqual(list(locked.iterdir()), [])


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


class TestOverrideEditing(CatalogueFixture):
    """`enable`/`disable`/`reset` write the smallest override that works (ADR-0011).

    The property that matters is the round trip: toggling a tool off and back on
    must leave state.json exactly as it was, or a selector accrues a pin per
    idle keystroke and the toolsets stop meaning anything.
    """

    def toggle(self, verb, *ids):
        args = types.SimpleNamespace(id=list(ids), format="text")
        on = {"enable": True, "disable": False, "reset": None}[verb]
        with contextlib.redirect_stdout(io.StringIO()):
            R._toggle(args, tools=True, on=on)
        return R.load_state()

    def written(self):
        path = self.dir / "state.json"
        return json.loads(path.read_text()) if path.is_file() else None

    def test_enabling_what_the_base_already_gives_writes_nothing(self):
        state = self.toggle("enable", "beta")
        self.assertEqual(state.enabled, [])
        self.assertIn("beta", R.active_ids(self.cat(), self.sets(), state))

    def test_disabling_what_the_base_does_not_give_writes_nothing(self):
        # Base is {alpha} here, so "off" for beta is already true.
        R.save_state(self.state(toolsets=["static"]))
        state = self.toggle("disable", "beta")
        self.assertEqual(state.disabled, [])
        self.assertNotIn("beta", R.active_ids(self.cat(), self.sets(), state))

    def test_off_then_on_is_a_round_trip(self):
        before = self.written()
        self.toggle("disable", "alpha")
        self.assertEqual(self.written()["tools"]["disabled"], ["alpha"])
        self.toggle("enable", "alpha")
        after = self.written()
        self.assertEqual(after["tools"], {"enabled": [], "disabled": []})
        if before is not None:
            self.assertEqual(before, after)

    def test_an_override_is_only_stored_against_the_toolsets(self):
        # With `static` active the base is {alpha}. Turning beta on is a real
        # deviation and is stored; turning alpha on is not.
        R.save_state(self.state(toolsets=["static"]))
        state = self.toggle("enable", "beta")
        self.assertEqual(state.enabled, ["beta"])
        state = self.toggle("enable", "alpha")
        self.assertEqual(state.enabled, ["beta"])

    def test_reset_drops_an_override_without_asserting_anything(self):
        R.save_state(self.state(toolsets=["static"], enabled=["beta"], disabled=["gamma"]))
        state = self.toggle("reset", "beta", "gamma")
        self.assertEqual((state.enabled, state.disabled), ([], []))
        self.assertEqual(R.active_ids(self.cat(), self.sets(), state), {"alpha"})

    def test_unknown_tool_is_refused_before_anything_is_written(self):
        with self.assertRaises(R.ReactorError):
            self.toggle("disable", "alpha", "ghost")
        self.assertIsNone(self.written())

    def cat(self):
        return R.load_catalogue()

    def sets(self):
        return R.load_toolsets()


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

    def test_a_toolset_named_after_a_tag_selects_only_that_tag(self):
        # Where a toolset's name is also a tag, that name is a claim about what
        # is inside it, and this checks the claim -- catching the toolset that
        # reads as narrow but is selected on some other tag entirely.
        #
        # It used to guard against the union widening such a set, which was how
        # [toolset.native] declaring ["static", "native"] came to hold every
        # static tool there is. Tags intersect now (ADR-0013), so widening is no
        # longer the way to get this wrong; naming the wrong tag still is.
        cat = R.load_catalogue()
        tags = {tag for t in cat.tools.values() for tag in t.tags}
        for ts in R.load_toolsets().values():
            if ts.everything or ts.id not in tags:
                continue
            for tid in R.toolset_members(ts, cat):
                # A tool named in `tools` is a deliberate exception -- frida is
                # in [toolset.android] precisely because it is not tagged
                # `android`. Writing the name is what makes that a decision
                # rather than an accident, so it is allowed and the tag rule
                # still binds everything else.
                if tid in ts.tools:
                    continue
                with self.subTest(toolset=ts.id, tool=tid):
                    self.assertIn(
                        ts.id, cat.tools[tid].tags,
                        f"toolset {ts.id!r} includes {tid!r}, which is not tagged {ts.id!r} "
                        f"and is not named in its `tools` list",
                    )

    def test_every_tool_belongs_to_a_toolset_other_than_all(self):
        # `all` is a catch-all, not a home. A tool reachable only through it is
        # one nobody decided where to put, which is a real state to be in
        # mid-edit but not one to ship: narrowing to any working set would hide
        # a tool the user has installed.
        cat = R.load_catalogue()
        placed = set()
        for ts in R.load_toolsets().values():
            if not ts.everything:
                placed.update(R.toolset_members(ts, cat))
        for tid in cat.tools:
            with self.subTest(tool=tid):
                self.assertIn(tid, placed, f"{tid} is in no toolset but `all`")


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

    def test_install_all_targets_the_whole_catalogue(self):
        cat = R.load_catalogue()
        payload, proc = self.run_cli("install", "all", "--dry-run")
        self.assertEqual(proc.returncode, 0)
        covered = {p["tool"] for p in payload["plan"]} | {s["tool"] for s in payload["skipped"]}
        self.assertEqual(covered, set(cat.tools))

    def test_install_all_cannot_be_mixed_with_a_real_id(self):
        # `all` already means everything; a second id has nothing left to
        # narrow, so it is rejected as an unknown tool rather than ignored.
        payload, proc = self.run_cli("install", "all", "jq", "--dry-run")
        self.assertEqual(proc.returncode, 1)
        self.assertIn("all", payload.get("error", ""))


class TestCompletion(unittest.TestCase):
    """Shell completion (ADR-0015): `reactor completion <shell>` and the
    internal `reactor __complete <kind>` the scripts shell back into.

    Neither carries a `--format`, so this runs the CLI directly rather than
    through `TestJsonContract.run_cli`.
    """

    @classmethod
    def setUpClass(cls):
        cls._tmp = tempfile.TemporaryDirectory(prefix="reactor-completion-")
        cls.cfg = Path(cls._tmp.name)
        for name in R.CONFIG_FILES:
            (cls.cfg / name).write_text((REPO_ROOT / name).read_text())

    @classmethod
    def tearDownClass(cls):
        cls._tmp.cleanup()

    def run_raw(self, *args):
        env = {**os.environ, "REACTOR_CONFIG_DIR": str(self.cfg)}
        return subprocess.run(
            [sys.executable, str(CLI_PATH), *args],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env, timeout=60,
        )

    def test_complete_ids_lists_every_catalogued_tool_in_declaration_order(self):
        cat = R.load_catalogue()
        proc = self.run_raw("__complete", "tools")
        self.assertEqual(proc.returncode, 0)
        self.assertEqual(proc.stdout.splitlines(), list(cat.tools))

    def test_complete_ids_lists_every_toolset(self):
        toolsets = R.load_toolsets()
        proc = self.run_raw("__complete", "toolsets")
        self.assertEqual(set(proc.stdout.splitlines()), set(toolsets))

    def test_complete_ids_does_not_probe(self):
        # The whole point (ADR-0015): a <TAB> press must cost a catalogue
        # load, not a detection sweep. No probe means no cache is written.
        self.run_raw("__complete", "tools")
        self.assertFalse((self.cfg / "cache.json").exists())

    def test_complete_is_hidden_from_help(self):
        proc = self.run_raw("--help")
        self.assertNotIn("__complete", proc.stdout)

    def test_completion_prints_a_script_per_shell_that_calls_back_in(self):
        for shell in ("bash", "zsh", "fish"):
            proc = self.run_raw("completion", shell)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertIn("__complete", proc.stdout)

    def test_completion_rejects_an_unknown_shell(self):
        proc = self.run_raw("completion", "powershell")
        self.assertNotEqual(proc.returncode, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
