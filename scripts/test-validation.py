#!/usr/bin/env python3
"""Validation aggregation, capability enforcement and repeat isolation regressions."""

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import ci
import validate
from test_capabilities import skip


class ValidationTests(unittest.TestCase):
    def test_interruption_records_the_failed_command_without_launching_later_cases(self):
        with tempfile.TemporaryDirectory() as directory:
            with (
                patch.object(validate, "ROOT", Path(directory)),
                patch.object(validate, "inventory", return_value={}),
                patch.object(ci, "measure", side_effect=SystemExit(130)) as run,
                patch.dict(os.environ),
            ):
                with self.assertRaises(SystemExit) as error:
                    validate.main(["--phases", "lint"])
                self.assertEqual(error.exception.code, 130)
                run.assert_called_once()
            report = json.loads(next(Path(directory).rglob("summary.json")).read_text())
            self.assertFalse(report["finished"])
            self.assertFalse(report["commands_passed"])
            self.assertEqual(report["results"], [dict(name="rust-format", exit_code=130)])

    def test_build_refreshes_capabilities_after_publishing_the_helper(self):
        with tempfile.TemporaryDirectory() as directory:
            before = {"bwrap": {"available": False, "detail": "Helper not built"}}
            after = {"bwrap": {"available": True, "detail": "Bundled launcher works"}}
            with (
                patch.object(validate, "ROOT", Path(directory)),
                patch.object(validate, "inventory", side_effect=[before, after]),
                patch.object(validate, "publish_tests"),
                patch.object(validate, "publish_helper") as publish,
                patch.object(ci, "measure"),
                patch.dict(os.environ),
            ):
                for name in ("RODE_TEST_SESSION_HELPER", "RODE_SESSION_HELPER", "RODE_HOST_HELPER"):
                    os.environ.pop(name, None)
                self.assertEqual(validate.main(["--phases", "build", "rust"]), 0)
                publish.assert_called_once()
            report = json.loads(next(Path(directory).rglob("summary.json")).read_text())
            self.assertEqual(report["capabilities"], after)
            self.assertEqual(report["unavailable_capabilities"], [])

    def test_command_record_distinguishes_omitted_coverage_and_drops_old_skip_records(self):
        with tempfile.TemporaryDirectory() as directory:
            source = (
                "import sys;sys.path.insert(0,"
                + repr(str(Path(__file__).resolve().parent))
                + ");from test_capabilities import skip;skip('optional-tool','not installed',test='optional case')"
            )
            with patch.dict(os.environ, RODE_CI_RESULTS_DIR=directory):
                first = ci.measure("optional", [sys.executable, "-c", source])
                self.assertEqual(first["status"], "passed-with-skips")
                self.assertEqual(first["skipped"][0]["test"], "optional case")
                second = ci.measure("optional", [sys.executable, "-c", "pass"])
                self.assertEqual(second["status"], "passed")
                self.assertEqual(second["skipped"], [])

    def test_runtime_lane_rejects_missing_capabilities_before_launching_tests(self):
        with tempfile.TemporaryDirectory() as directory:
            with (
                patch.dict(os.environ, RODE_CI_RESULTS_DIR=directory),
                patch.object(
                    ci,
                    "suite_environment",
                    return_value={"RODE_TEST_REQUIRED_CAPABILITIES": "jupyter"},
                ),
                patch.object(
                    ci,
                    "inventory",
                    return_value={"jupyter": {"available": False, "detail": "not installed"}},
                ),
                patch.object(ci, "measure") as run,
            ):
                with self.assertRaisesRegex(
                    SystemExit, "Missing required suite capabilities: jupyter"
                ):
                    ci.run_suite("runtimes")
                run.assert_not_called()
            self.assertTrue((Path(directory) / "runtimes/capabilities.json").is_file())

    def test_optional_coverage_survives_captured_output_and_required_coverage_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            environment = {"RODE_TEST_SKIP_DIR": directory}
            skip("jupyter", "interpreter absent", test="real kernel", environment=environment)
            report = json.loads(next(Path(directory).glob("*.json")).read_text())
            self.assertEqual(report["test"], "real kernel")
            environment["RODE_TEST_REQUIRED_CAPABILITIES"] = "debugpy,jupyter"
            with self.assertRaisesRegex(AssertionError, "Required capability jupyter"):
                skip("jupyter", "interpreter absent", environment=environment)

    def test_suite_runs_later_cases_without_erasing_an_earlier_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            entries = [
                ([sys.executable, "-c", "raise SystemExit(7)"], {}, 5),
                ([sys.executable, "-c", "print('later case ran')"], {}, 5),
            ]
            with (
                patch.dict(ci.SUITES, fixture=entries),
                patch.object(ci, "ROOT", root),
                patch.object(ci, "suite_environment", return_value=dict(os.environ)),
                patch.dict(os.environ, RODE_CI_RESULTS_DIR=directory),
            ):
                with self.assertRaisesRegex(SystemExit, "failed cases"):
                    ci.run_suite("fixture")
            records = [json.loads(path.read_text()) for path in (root / "fixture").glob("*-*.json")]
            self.assertEqual(
                [item["exit_code"] for item in sorted(records, key=lambda r: r["name"])], [7, 0]
            )
            self.assertEqual(
                len(json.loads((root / "fixture/suite.json").read_text())["failed"]), 1
            )

    def test_repeats_have_separate_reports_and_do_not_turn_a_failure_into_success(self):
        with tempfile.TemporaryDirectory() as directory:
            seen = []

            def measure(label, command, env, timeout, **kwargs):
                seen.append((label, command, env, kwargs))
                if len(seen) == 1:
                    raise SystemExit(1)

            with (
                patch.object(validate, "ROOT", Path(directory)),
                patch.object(validate, "inventory", return_value={}),
                patch.object(validate, "publish_helper"),
                patch.object(ci, "measure", side_effect=measure),
                patch.dict(os.environ),
            ):
                self.assertEqual(
                    validate.main(
                        ["--phases", "ui", "--suite", "documents", "--repeat", "3", "--stress"]
                    ),
                    1,
                )
            self.assertEqual([item[3]["umask"] for item in seen], [0o002, 0o077, 0o022])
            self.assertEqual(len({item[2]["RODE_CI_RESULTS_DIR"] for item in seen}), 3)
            for _, command, env, _ in seen:
                self.assertIn("scripts/ui_display.py", command)
                self.assertEqual(env["RODE_TEST_SLOW_UI"], "1")
            report = json.loads(next(Path(directory).rglob("summary.json")).read_text())
            self.assertFalse(report["commands_passed"])
            self.assertTrue(report["finished"])
            self.assertEqual([row["exit_code"] for row in report["results"]], [1, 0, 0])

    def test_publication_replaces_stale_symlinks_without_overwriting_their_targets(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "compiled-test"
            binary.write_text("new")
            stale = root / "old-debug-test"
            stale.write_text("old")
            destination = root / "target/ci/tests/fixture"
            destination.parent.mkdir(parents=True)
            destination.symlink_to(stale)
            metadata = root / "cargo.log"
            metadata.write_text(
                json.dumps(dict(target=dict(name="fixture", kind=["test"]), executable=str(binary)))
            )
            with (
                patch.object(validate, "ROOT", root),
                patch.object(ci, "TEST_BINARIES", {"fixture"}),
            ):
                validate.publish_tests(metadata)
            self.assertEqual(stale.read_text(), "old")
            self.assertFalse(destination.is_symlink())
            self.assertEqual(destination.read_text(), "new")
            self.assertEqual(destination.stat().st_mode & 0o777, 0o755)

    def test_requirement_flags_are_scoped_to_their_lane(self):
        with patch.dict(
            os.environ,
            RODE_TEST_REQUIRE_SANDBOX="1",
            RODE_TEST_REQUIRE_LANDLOCK="1",
            RODE_TEST_REQUIRED_CAPABILITIES="jupyter",
        ):
            env = validate.environment()
            self.assertNotIn("RODE_TEST_REQUIRE_SANDBOX", env)
            self.assertNotIn("RODE_TEST_REQUIRE_LANDLOCK", env)
            with patch("ci.os.access", return_value=True):
                normal = ci.suite_environment("workspace")
                runtime = ci.suite_environment("runtimes")
                sandbox = ci.suite_environment("sandbox")
            self.assertNotIn("RODE_TEST_REQUIRE_LANDLOCK", normal)
            self.assertEqual(runtime["RODE_TEST_REQUIRED_CAPABILITIES"], "jupyter,debugpy,lldb")
            self.assertNotIn("RODE_TEST_REQUIRE_SANDBOX", runtime)
            self.assertEqual(sandbox["RODE_TEST_REQUIRE_LANDLOCK"], "1")

    def test_helper_snapshot_is_safe_even_when_cargo_used_a_permissive_umask(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "target/ci/rode-sessiond"
            source.parent.mkdir(parents=True)
            source.write_bytes(b"fixture helper")
            source.chmod(0o775)
            with patch.object(validate, "ROOT", root):
                validate.publish_helper()
            snapshot = source.with_name("rode-sessiond-test")
            self.assertEqual(snapshot.read_bytes(), source.read_bytes())
            self.assertEqual(snapshot.stat().st_mode & 0o777, 0o755)
            self.assertEqual(source.stat().st_mode & 0o777, 0o775)


if __name__ == "__main__":
    unittest.main()
