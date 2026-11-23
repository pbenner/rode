#!/usr/bin/env python3
"""Verify private Rust test homes, cleanup, argument forwarding and exit codes."""

import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import ci_jobs
import rust_tests


class RustPlanTests(unittest.TestCase):
    def plan(self):
        records = []
        for kind in ("lib", "bin", "test", "bench"):
            executable = "/compiled/" + kind
            records.extend(
                [
                    dict(
                        reason="compiler-artifact",
                        package_id="fixture#1",
                        profile=dict(test=True),
                        target=dict(name=kind, kind=[kind]),
                        executable=executable,
                    ),
                    dict(
                        rode_test_command=1,
                        argv=[executable, "--test"] if kind == "bench" else [executable],
                        cwd="/fixture/crate",
                        env={
                            "CARGO_MANIFEST_DIR": "/fixture/crate",
                            "LD_LIBRARY_PATH": "/cargo/library/path",
                            "PRIVATE_VALUE": "not for logs",
                        },
                    ),
                ]
            )
        return records

    def test_cargo_plan_retains_all_target_kinds_arguments_and_runtime_context(self):
        records = self.plan()
        records.append(records[0])  # Cargo may repeat artifact metadata.
        commands = rust_tests.test_commands("\n".join(json.dumps(r) for r in records))
        self.assertEqual(len(commands), 4)
        self.assertCountEqual(
            [c["argv"][0] for c in commands],
            ["/compiled/" + k for k in ("lib", "bin", "test", "bench")],
        )
        for command in commands:
            self.assertEqual(command["cwd"], "/fixture/crate")
            self.assertEqual(command["env"]["LD_LIBRARY_PATH"], "/cargo/library/path")
        self.assertEqual(
            next(c for c in commands if c["kind"] == ["bench"])["argv"],
            ["/compiled/bench", "--test"],
        )

    def test_missing_invocations_stale_executables_and_invalid_streams_fail_closed(self):
        for records in (
            [],
            self.plan()[::2],
            self.plan()[:-1],
            [None],
            [dict(rode_test_command=1, argv=["/stale/test"], cwd="/", env={})],
        ):
            with self.assertRaises(ValueError):
                rust_tests.test_commands("\n".join(json.dumps(r) for r in records))
        with self.assertRaises(ValueError):
            rust_tests.test_commands("truncated JSON")

    def test_successful_planning_cannot_hide_a_test_failure_or_publish_private_env(self):
        plan = "\n".join(json.dumps(r) for r in self.plan())
        with (
            patch.object(
                rust_tests.subprocess, "check_output", return_value="host: fixture-target\n"
            ),
            patch.object(
                rust_tests.subprocess,
                "run",
                return_value=subprocess.CompletedProcess([], 0, stdout=plan, stderr=""),
            ) as build,
            patch.object(ci_jobs, "run", return_value=[ci_jobs.Result("failed", 7)]) as run,
            patch("sys.stdout", new_callable=io.StringIO) as output,
        ):
            self.assertEqual(rust_tests.workspace_tests(), 1)
        self.assertNotIn("not for logs", output.getvalue())
        self.assertEqual(len(run.call_args.args[0]), 4)
        job = run.call_args.args[0][0]
        self.assertEqual(job.cwd, "/fixture/crate")
        self.assertEqual(job.env["CARGO_MANIFEST_DIR"], "/fixture/crate")
        self.assertIn("--collect-command", build.call_args.args[0][3])

    def test_failed_build_never_executes_partially_collected_targets(self):
        with (
            patch.object(
                rust_tests.subprocess, "check_output", return_value="host: fixture-target\n"
            ),
            patch.object(
                rust_tests.subprocess,
                "run",
                return_value=subprocess.CompletedProcess(
                    [], 101, stdout="", stderr="compile failed"
                ),
            ),
            patch.object(ci_jobs, "run") as run,
        ):
            self.assertEqual(rust_tests.workspace_tests(), 101)
            run.assert_not_called()


class RustFixtureTests(unittest.TestCase):
    def test_tmpdir_and_explicit_override_apply_to_real_child_homes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            normal, explicit = root / "normal", root / "chosen ' root"
            normal.mkdir()
            explicit.mkdir()
            probe = "import os; from pathlib import Path; Path(os.environ['RODE_FIXTURE_RESULT']).write_text(os.environ['HOME'])"
            for override, expected in (("", normal), (str(explicit), explicit)):
                output = root / "home.txt"
                with (
                    patch.dict(
                        os.environ,
                        TMPDIR=str(normal),
                        RODE_TEST_TMPDIR=override,
                        RODE_FIXTURE_RESULT=str(output),
                        RODE_TEST_AWKWARD_PATHS="1",
                    ),
                    patch("fixture_support.tempfile.tempdir", None),
                ):
                    self.assertEqual(rust_tests.isolated([sys.executable, "-c", probe]), 0)
                home = Path(output.read_text())
                self.assertEqual(home.parent, expected.resolve())
                self.assertTrue(home.name.startswith("rt-"))
                self.assertNotIn("space", home.name)
                self.assertFalse(home.exists())

    def test_long_checkouts_do_not_overflow_nested_unix_socket_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / ("long-checkout-" * 10)
            root.mkdir()
            # Detect checkout-path leakage without consuming an arbitrary extra
            # socket-name budget from the caller's chosen temporary parent.
            code = """import os, socket
path = os.path.join(os.environ['TMPDIR'], 'nested-ssh-control')
with socket.socket(socket.AF_UNIX) as control:
    control.bind(path)
"""
            with patch.object(rust_tests, "ROOT", root):
                self.assertEqual(rust_tests.isolated([sys.executable, "-c", code]), 0)

    def test_mise_task_preserves_configured_ruff_and_discovers_it_only_when_unset(self):
        section = (rust_tests.ROOT / "mise.toml").read_text().split("[tasks.test]", 1)[1]
        section = section.split("\n[", 1)[0]
        # This task uses a quoted run string; decoding it needs no TOML package
        # or newer Python than the other standard-library fixture checks.
        task = json.loads(
            next(line[6:] for line in section.splitlines() if line.startswith("run = "))
        )
        task = task.replace("{{mise_bin}}", "fixture-mise")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "environment.json"
            probe = root / "probe.py"
            probe.write_text(
                "import json, os\n"
                "from pathlib import Path\n"
                "Path(os.environ['RODE_TEST_PROBE_OUTPUT']).write_text("
                "json.dumps({name:os.environ[name] for name in ['RODE_TEST_RUFF','RODE_TEST_MISE']}))\n"
            )
            binary = root / "python3"
            binary.write_text(
                '#!/bin/sh\nexec "$RODE_TEST_PROBE_PYTHON" "$RODE_TEST_PROBE_SCRIPT"\n'
            )
            binary.chmod(0o755)
            ruff = root / "ruff"
            ruff.write_text("#!/bin/sh\nexit 0\n")
            ruff.chmod(0o755)
            for configured in [str(root / "configured ruff"), None]:
                env = dict(os.environ)
                env.update(
                    PATH=str(root) + os.pathsep + env["PATH"],
                    RODE_TEST_PROBE_PYTHON=sys.executable,
                    RODE_TEST_PROBE_SCRIPT=str(probe),
                    RODE_TEST_PROBE_OUTPUT=str(output),
                )
                env.pop("RODE_TEST_RUFF", None)
                if configured is not None:
                    env["RODE_TEST_RUFF"] = configured
                subprocess.run(["/bin/sh", "-c", task], env=env, check=True)
                result = json.loads(output.read_text())
                self.assertEqual(result["RODE_TEST_RUFF"], configured or str(ruff))
                self.assertEqual(result["RODE_TEST_MISE"], "fixture-mise")

    def test_fixture_commands_do_not_inherit_the_development_virtualenv(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "environment.json"
            virtualenv = root / "development-venv"
            code = "import json,os,sys; open(sys.argv[1],'w').write(json.dumps({'venv':os.environ.get('VIRTUAL_ENV'),'path':os.environ['PATH']}))"
            with (
                patch.object(rust_tests, "ROOT", root),
                patch.dict(
                    os.environ,
                    VIRTUAL_ENV=str(virtualenv),
                    PATH=str(virtualenv / "bin") + os.pathsep + os.environ["PATH"],
                ),
            ):
                self.assertEqual(rust_tests.isolated([sys.executable, "-c", code, str(output)]), 0)
            result = json.loads(output.read_text())
            self.assertIsNone(result["venv"])
            self.assertNotIn(str(virtualenv / "bin"), result["path"].split(os.pathsep))

    def test_child_state_is_private_and_removed_without_mutating_the_parent(self):
        before = dict(os.environ)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "result.json"
            code = """import json, os, pathlib, stat, sys
home = pathlib.Path.home()
(home / 'test-only-data').write_text('fixture')
names = ['HOME', 'XDG_CACHE_HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME',
         'XDG_RUNTIME_DIR', 'TMPDIR', 'RODE_SESSION_DIR', 'RODE_VAULT_DIR']
pathlib.Path(sys.argv[1]).write_text(json.dumps({
    'paths': {name: os.environ[name] for name in names},
    'private': stat.S_IMODE(home.stat().st_mode) == 0o700,
    'argument': sys.argv[2], 'codex_home': os.environ.get('CODEX_HOME')}))
sys.exit(17)
"""
            with patch.object(rust_tests, "ROOT", root):
                status = rust_tests.isolated(
                    [sys.executable, "-c", code, str(output), "literal ' space"]
                )
            self.assertEqual(status, 17)
            result = json.loads(output.read_text())
            self.assertTrue(result["private"])
            self.assertEqual(result["argument"], "literal ' space")
            self.assertIsNone(result["codex_home"])
            fixture_home = Path(result["paths"]["HOME"])
            for path in result["paths"].values():
                self.assertTrue(Path(path).is_relative_to(fixture_home))
            self.assertFalse(fixture_home.exists())
        self.assertEqual(dict(os.environ), before)

    def test_inherited_native_probe_cannot_replace_independent_test_snapshots(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "caller-ui.json"
            path.write_text("caller snapshot")
            code = "import os; assert 'RODE_UI_TEST_STATE' not in os.environ"
            with patch.dict(os.environ, RODE_UI_TEST_STATE=str(path)):
                self.assertEqual(rust_tests.isolated([sys.executable, "-c", code]), 0)
                self.assertEqual(os.environ["RODE_UI_TEST_STATE"], str(path))
            self.assertEqual(path.read_text(), "caller snapshot")

    def test_cargo_runner_is_structured_and_custom_filters_are_preserved(self):
        with (
            patch(
                "rust_tests.subprocess.check_output",
                return_value="rustc fixture\nhost: x86_64-unknown-linux-gnu\n",
            ),
            patch("rust_tests.subprocess.call", return_value=7) as run,
        ):
            self.assertEqual(rust_tests.main(["--lib", "literal_test_filter"]), 7)
        command = run.call_args.args[0]
        self.assertEqual(command[:3], ["cargo", "test", "--config"])
        self.assertEqual(command[-2:], ["--lib", "literal_test_filter"])
        key, encoded = command[3].split("=", 1)
        self.assertEqual(key, "target.x86_64-unknown-linux-gnu.runner")
        self.assertEqual(
            json.loads(encoded),
            [sys.executable, str(Path(rust_tests.__file__).resolve()), "--runner"],
        )


if __name__ == "__main__":
    unittest.main()
