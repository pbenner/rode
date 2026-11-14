#!/usr/bin/env python3
"""Validation aggregation, capability enforcement and repeat isolation regressions."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import ci
import test_capabilities
import validate
import validation_report
from fixture_support import temporary_root
from test_capabilities import skip


class CapabilityProbeTests(unittest.TestCase):
    def test_landlock_probe_requires_the_requested_active_backend_and_uses_private_paths(self):
        for active, backend, expected in [
            (True, "landlock", True),
            (False, "landlock", False),
            (True, "bwrap", False),
        ]:
            with tempfile.TemporaryDirectory(prefix="capability path ") as directory:
                environment = {
                    "RODE_TEST_SESSION_HELPER": "/fixture/helper with spaces",
                    "RODE_TEST_TMPDIR": directory,
                }
                result = subprocess.CompletedProcess(
                    [],
                    0,
                    stdout=json.dumps(
                        {
                            "project": {
                                "active": active,
                                "backend": backend,
                                "reason": "fixture reason",
                            }
                        }
                    ),
                    stderr="",
                )
                with patch.object(test_capabilities.subprocess, "run", return_value=result) as run:
                    available, _ = test_capabilities.landlock_probe(environment)
                self.assertEqual(available, expected)
                self.assertEqual(
                    run.call_args.args[0],
                    [environment["RODE_TEST_SESSION_HELPER"], "--file-helper"],
                )
                request = json.loads(run.call_args.kwargs["input"])
                self.assertEqual(request["sandbox"]["backend"], "landlock")
                path = Path(request["path"])
                self.assertEqual(path.parent, Path(directory).resolve())
                self.assertFalse(
                    path.exists(), "Probe fixtures must be removed after the helper exits"
                )

    def test_landlock_probe_rejects_missing_malformed_and_failed_helpers(self):
        self.assertFalse(test_capabilities.landlock_probe({})[0])
        with tempfile.TemporaryDirectory() as directory:
            environment = {
                "RODE_TEST_SESSION_HELPER": "/fixture/helper",
                "RODE_TEST_TMPDIR": directory,
            }
            for stdout in ("invalid JSON", "null", "[]", '{"project": []}'):
                result = subprocess.CompletedProcess([], 0, stdout=stdout, stderr="")
                with patch.object(test_capabilities.subprocess, "run", return_value=result):
                    self.assertFalse(test_capabilities.landlock_probe(environment)[0])
            for error in (
                PermissionError("helper denied"),
                subprocess.TimeoutExpired("helper", 15),
            ):
                with patch.object(test_capabilities.subprocess, "run", side_effect=error):
                    available, reason = test_capabilities.landlock_probe(environment)
                self.assertFalse(available)
                self.assertTrue(reason)


class ValidationTests(unittest.TestCase):
    def setUp(self):
        versions = patch.object(validation_report, "toolchain", return_value={})
        versions.start()
        self.addCleanup(versions.stop)

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

    def test_truncated_skip_record_fails_its_command_and_keeps_the_next_result(self):
        with tempfile.TemporaryDirectory() as directory:
            source = "import os,pathlib; pathlib.Path(os.environ['RODE_TEST_SKIP_DIR'],'partial.json').write_text('{')"
            with patch.dict(os.environ, RODE_CI_RESULTS_DIR=directory):
                with self.assertRaises(SystemExit) as failed:
                    ci.measure("partial-skip", [sys.executable, "-c", source])
                self.assertEqual(failed.exception.code, 1)
                following = ci.measure("following", [sys.executable, "-c", "pass"])
            record = json.loads((Path(directory) / "commands/partial-skip.json").read_text())
            self.assertEqual(record["status"], "failed")
            self.assertTrue(record["metadata_errors"])
            self.assertEqual(following["status"], "passed")

    def test_validation_retains_truncated_skip_diagnostics_without_aborting(self):
        with tempfile.TemporaryDirectory() as directory:

            def measure(label, command, environment, *_args, **_kwargs):
                if label == "rust-format":
                    skips = Path(environment["RODE_CI_RESULTS_DIR"]) / "commands/fixture-skips"
                    skips.mkdir(parents=True)
                    (skips / "partial.json").write_text("{")

            with (
                patch.object(validate, "ROOT", Path(directory)),
                patch.object(validate, "inventory", return_value={}),
                patch.object(ci, "measure", side_effect=measure),
                patch.dict(os.environ),
            ):
                self.assertEqual(validate.main(["--phases", "lint"]), 1)
            report = json.loads(next(Path(directory).rglob("summary.json")).read_text())
            self.assertTrue(report["finished"])
            self.assertFalse(report["records_complete"])
            self.assertTrue(report["metadata_errors"])
            self.assertGreater(len(report["results"]), 1)

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
            report = json.loads((Path(directory) / "runtimes/suite.json").read_text())
            self.assertEqual(report["status"], "blocked")
            self.assertIn("jupyter", report["reason"])
            self.assertEqual(report["completed"], [])
            self.assertTrue(report["planned"])

    def test_enforcement_lanes_require_only_their_own_backend(self):
        for suite, capability, other in (
            ("landlock", "landlock", "bwrap"),
            ("sandbox", "bwrap", "landlock"),
        ):
            for available in (True, False):
                with tempfile.TemporaryDirectory() as directory:
                    with (
                        patch.dict(
                            os.environ,
                            RODE_CI_RESULTS_DIR=directory,
                            RODE_TEST_REQUIRE_LANDLOCK="1",
                            RODE_TEST_REQUIRE_SANDBOX="1",
                            RODE_TEST_REQUIRED_CAPABILITIES="bwrap,landlock",
                        ),
                        patch.object(ci.os, "access", return_value=True),
                        patch.object(
                            ci,
                            "inventory",
                            return_value={
                                capability: {"available": available, "detail": "fixture"},
                                other: {"available": False, "detail": "Other backend denied"},
                            },
                        ),
                        patch.object(ci, "measure") as run,
                    ):
                        environment = ci.suite_environment(suite)
                        self.assertEqual(environment["RODE_TEST_REQUIRED_CAPABILITIES"], capability)
                        if capability == "landlock":
                            self.assertEqual(environment["RODE_TEST_REQUIRE_LANDLOCK"], "1")
                        else:
                            self.assertNotIn("RODE_TEST_REQUIRE_LANDLOCK", environment)
                        if available:
                            ci.run_suite(suite)
                            self.assertEqual(run.call_count, len(ci.SUITES[suite]))
                        else:
                            with self.assertRaisesRegex(
                                SystemExit, "Missing required suite capabilities: " + capability
                            ):
                                ci.run_suite(suite)
                            run.assert_not_called()
                    report = json.loads((Path(directory) / suite / "suite.json").read_text())
                    self.assertEqual(report["status"], "passed" if available else "blocked")

    def test_missing_bundle_records_blocked_suite_and_replaces_an_older_success(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "workspace"
            output.mkdir()
            (output / "suite.json").write_text('{"suite":"workspace","status":"passed"}')
            with (
                patch.dict(os.environ, RODE_CI_RESULTS_DIR=directory),
                patch.object(ci, "suite_environment", side_effect=SystemExit("Missing binary")),
                patch.object(ci, "measure") as run,
            ):
                with self.assertRaisesRegex(SystemExit, "Missing binary"):
                    ci.run_suite("workspace")
            report = json.loads((output / "suite.json").read_text())
            self.assertEqual(report["status"], "blocked")
            self.assertEqual(report["completed"], [])
            run.assert_not_called()

    def test_each_case_has_private_state_without_changing_the_parent_or_toolchain_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            code = (
                "import os,pathlib,stat; "
                "h=pathlib.Path.home(); assert not (h/'previous-case').exists(); "
                "(h/'previous-case').write_text('owned'); "
                "assert stat.S_IMODE(h.stat().st_mode)==0o700; "
                "assert os.environ['CARGO_HOME']=='/explicit/toolchain'; "
                "assert 'CODEX_HOME' not in os.environ; print(str(h))"
            )
            with (
                patch.dict(os.environ, RODE_CI_RESULTS_DIR=directory),
                patch.dict(ci.SUITES, fixture=[([sys.executable, "-c", code], {}, 5)] * 2),
                patch.object(
                    ci,
                    "suite_environment",
                    return_value=dict(
                        os.environ, CARGO_HOME="/explicit/toolchain", CODEX_HOME="/caller/codex"
                    ),
                ),
                patch.object(ci, "ROOT", root),
            ):
                before = dict(os.environ)
                ci.run_suite("fixture")
                self.assertEqual(before, dict(os.environ))
            homes = [Path(p.read_text().strip()) for p in sorted((root / "fixture").glob("*.log"))]
            self.assertEqual(len(set(homes)), 2)
            self.assertTrue(all(not home.exists() for home in homes))

    def test_nested_suites_keep_the_short_shared_fixture_parent(self):
        with tempfile.TemporaryDirectory() as directory:
            with (
                patch.dict(os.environ, RODE_CI_RESULTS_DIR=directory),
                patch.dict(ci.SUITES, fixture=[([sys.executable, "-c", "pass"], {}, 5)]),
                patch.object(ci, "suite_environment", return_value=dict(os.environ)),
                patch.object(ci, "measure") as run,
            ):
                # Exercise platform default selection instead of an explicit override.
                os.environ.pop("RODE_TEST_TMPDIR", None)
                parent = temporary_root()
                selected = dict(os.environ)
                with patch.object(ci, "suite_environment", return_value=selected):
                    ci.run_suite("fixture")
                environment = run.call_args.args[2]
            self.assertEqual(environment["RODE_TEST_TMPDIR"], str(parent))
            self.assertNotEqual(environment["TMPDIR"], str(parent))

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
                landlock = ci.suite_environment("landlock")
            self.assertNotIn("RODE_TEST_REQUIRE_LANDLOCK", normal)
            self.assertEqual(runtime["RODE_TEST_REQUIRED_CAPABILITIES"], "jupyter,debugpy,lldb")
            self.assertNotIn("RODE_TEST_REQUIRE_SANDBOX", runtime)
            self.assertNotIn("RODE_TEST_REQUIRE_LANDLOCK", sandbox)
            self.assertEqual(sandbox["RODE_TEST_REQUIRED_CAPABILITIES"], "bwrap")
            self.assertEqual(landlock["RODE_TEST_REQUIRE_LANDLOCK"], "1")
            self.assertEqual(landlock["RODE_TEST_REQUIRED_CAPABILITIES"], "landlock")

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


class ProvenanceTests(unittest.TestCase):
    def test_version_identity_ignores_stderr_warnings_but_keeps_stderr_only_versions(self):
        release = "mise 1.0"
        warning = "initial warning"

        def probe(command, **_kwargs):
            if command[0] == "fixture-mise":
                return subprocess.CompletedProcess(command, 0, release + "\n", warning + "\n")
            if command[0] == "ssh":
                return subprocess.CompletedProcess(command, 0, "", "OpenSSH_fixture\n")
            return subprocess.CompletedProcess(command, 0, "stable version\n", "")

        with (
            patch.object(validation_report.subprocess, "run", side_effect=probe),
            patch.object(validation_report.shutil, "which", return_value=None),
        ):
            environment = {"RODE_TEST_MISE": "fixture-mise"}
            before = validation_report.toolchain(environment)
            warning = "later automatic maintenance warning"
            after = validation_report.toolchain(environment)
            self.assertEqual(before, after)
            self.assertEqual(after["tools"]["mise"]["version"], "mise 1.0")
            self.assertEqual(after["tools"]["ssh"]["version"], "OpenSSH_fixture")
            release = "mise 1.1"
            self.assertNotEqual(before, validation_report.toolchain(environment))

    def test_identity_includes_dirty_untracked_and_executable_source_but_not_ignored_results(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            git_environment = dict(
                os.environ,
                GIT_CONFIG_GLOBAL=os.devnull,
                GIT_CONFIG_NOSYSTEM="1",
                GIT_CONFIG_COUNT="0",
            )
            subprocess.run(["git", "init", "-q", str(root)], check=True, env=git_environment)
            (root / ".gitignore").write_text("target/\n")
            source = root / "source.py"
            source.write_text("original source")
            subprocess.run(["git", "-C", str(root), "add", "."], check=True, env=git_environment)
            subprocess.run(
                [
                    "git",
                    "-C",
                    str(root),
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-qm",
                    "fixture",
                ],
                check=True,
                env=git_environment,
            )
            initial = validation_report.source_identity(root)
            self.assertTrue(initial["available"])
            self.assertFalse(initial["dirty"])
            (root / "target").mkdir()
            (root / "target/results.json").write_text("ignored")
            self.assertEqual(validation_report.source_identity(root), initial)
            source.write_text("changed source")
            changed = validation_report.source_identity(root)
            self.assertEqual(changed["head"], initial["head"])
            self.assertNotEqual(changed["sha256"], initial["sha256"])
            self.assertTrue(changed["dirty"])
            source.chmod(0o755)
            executable = validation_report.source_identity(root)
            self.assertNotEqual(executable["sha256"], changed["sha256"])
            (root / "untracked.py").write_text("new source")
            untracked = validation_report.source_identity(root)
            self.assertNotEqual(untracked["sha256"], executable["sha256"])
            self.assertNotIn("new source", json.dumps(untracked))

    def test_changing_checkout_or_artifacts_fails_validation_even_when_commands_pass(self):
        for source_changed in (False, True):
            with (
                self.subTest(source_changed=source_changed),
                tempfile.TemporaryDirectory() as directory,
            ):
                identity = dict(available=True, sha256="before")
                after = dict(available=True, sha256="after") if source_changed else identity
                with (
                    patch.object(validate, "ROOT", Path(directory)),
                    patch.object(validate, "inventory", return_value={}),
                    patch.object(validate, "publish_helper"),
                    patch.object(ci, "measure"),
                    patch.object(validation_report, "toolchain", return_value={}),
                    patch.object(
                        validation_report, "source_identity", side_effect=[identity, after]
                    ),
                    patch.object(
                        validation_report,
                        "artifact_identity",
                        side_effect=[
                            {"binary": "before"},
                            {"binary": "before" if source_changed else "after"},
                        ],
                    ),
                    patch.dict(os.environ),
                ):
                    self.assertEqual(validate.main(["--phases", "ui", "--suite", "reliability"]), 1)
                report = json.loads(next(Path(directory).rglob("summary.json")).read_text())
                self.assertFalse(report["commands_passed"])
                self.assertEqual(
                    report["results"][-1]["name"],
                    "source-changed" if source_changed else "artifacts-changed",
                )
                self.assertTrue(
                    all(row["command_failure_rate"] is None for row in report["repeat_results"])
                )


class RepeatReportTests(unittest.TestCase):
    def run_record(self, root, iteration, status, *, condition="baseline"):
        label = f"{iteration:02d}-fixture"
        directory = root / label / "fixture"
        directory.mkdir(parents=True)
        completed = [] if status == "blocked" else ["01-case"]
        suite_status = (
            "blocked"
            if status == "blocked"
            else "passed"
            if status.startswith("passed")
            else "failed"
        )
        (directory / "suite.json").write_text(
            json.dumps(
                dict(suite="fixture", status=suite_status, planned=["01-case"], completed=completed)
            )
        )
        # A stale successful case must not count when this invocation was blocked.
        (directory / "01-case.json").write_text(
            json.dumps(dict(status="passed" if status == "blocked" else status))
        )
        return dict(
            name=label,
            suite="fixture",
            iteration=iteration,
            condition=condition,
            planned=["01-case"],
        )

    def test_mixed_results_preserve_failure_and_exclude_blocked_coverage_from_rate(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runs = [
                self.run_record(root, i, status)
                for i, status in enumerate(
                    ["failed", "passed", "timeout", "blocked", "passed-with-skips"], 1
                )
            ]
            (result,) = validation_report.repeats(root, runs, comparable=True)
            self.assertEqual(result["executed"], 4)
            self.assertEqual(result["command_failure_rate"], 0.5)
            self.assertEqual(result["counts"]["blocked"], 1)
            self.assertTrue(result["intermittent_failure_candidate"])
            (unknown,) = validation_report.repeats(root, runs, comparable=False)
            self.assertIsNone(unknown["command_failure_rate"])
            self.assertFalse(unknown["intermittent_failure_candidate"])

    def test_stress_conditions_are_separate_and_missing_launcher_is_not_a_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runs = [
                self.run_record(root, 1, "failed", condition="stress-umask-002"),
                self.run_record(root, 2, "passed", condition="stress-umask-077"),
                dict(
                    name="03-fixture",
                    suite="fixture",
                    iteration=3,
                    condition="baseline",
                    planned=["01-case"],
                ),
            ]
            rows = validation_report.repeats(root, runs, comparable=True)
            self.assertEqual(len(rows), 3)
            self.assertTrue(all(not row["intermittent_failure_candidate"] for row in rows))
            missing = next(row for row in rows if row["condition"] == "baseline")
            self.assertEqual(missing["counts"], {"not-run": 1})
            self.assertIsNone(missing["command_failure_rate"])

    def test_truncated_records_do_not_erase_other_attempts_or_count_as_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            runs = [self.run_record(root, i, "passed") for i in range(1, 5)]
            (root / runs[1]["name"] / "fixture/01-case.json").write_text('{"status":')
            (root / runs[2]["name"] / "fixture/suite.json").write_text('{"suite":')
            (root / runs[3]["name"] / "fixture/suite.json").write_text("{}")
            (result,) = validation_report.repeats(root, runs, comparable=True)
            self.assertEqual(result["counts"], {"passed": 1, "not-run": 3})
            self.assertEqual(result["executed"], 1)
            self.assertTrue(all("error" in attempt for attempt in result["attempts"][1:]))

    def test_suite_plan_cannot_hide_expected_cases_or_replace_the_parent_report(self):
        for mutation in ("empty-plan", "missing-parent", "wrong-types"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                run = self.run_record(root, 1, "passed")
                path = root / run["name"] / "fixture/suite.json"
                suite = json.loads(path.read_text())
                if mutation == "missing-parent":
                    child = path.parent.parent / "child"
                    child.mkdir()
                    (child / "suite.json").write_text(json.dumps(suite | {"suite": "child"}))
                    path.unlink()
                else:
                    suite.update(planned=[], completed=[])
                    if mutation == "wrong-types":
                        suite.update(planned=[None], completed=[{}])
                    path.write_text(json.dumps(suite))
                rows = validation_report.repeats(root, [run], comparable=True)
                parent = next(row for row in rows if row["case"] == "fixture/01-case")
                self.assertEqual(parent["counts"], {"not-run": 1})
                self.assertIsNone(parent["command_failure_rate"])

    def test_successful_parent_without_case_records_cannot_pass_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            with (
                patch.object(validate, "ROOT", Path(directory)),
                patch.object(validate, "inventory", return_value={}),
                patch.object(validate, "publish_helper"),
                patch.object(ci, "measure"),
                patch.object(validation_report, "toolchain", return_value={}),
                patch.object(
                    validation_report,
                    "source_identity",
                    return_value=dict(available=True, sha256="same"),
                ),
                patch.object(validation_report, "artifact_identity", return_value={}),
                patch.dict(os.environ),
            ):
                self.assertEqual(validate.main(["--phases", "ui", "--suite", "reliability"]), 1)
            report = json.loads(next(Path(directory).rglob("summary.json")).read_text())
            self.assertFalse(report["commands_passed"])
            self.assertFalse(report["records_complete"])
            self.assertTrue(all(result["exit_code"] == 0 for result in report["results"]))


if __name__ == "__main__":
    unittest.main()
