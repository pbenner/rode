#!/usr/bin/env python3
"""Verify private Rust test homes, cleanup, argument forwarding and exit codes."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import rust_tests


class RustFixtureTests(unittest.TestCase):
    def test_long_checkouts_do_not_overflow_nested_unix_socket_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / ("long-checkout-" * 10)
            root.mkdir()
            code = """import os, socket
path = os.path.join(os.environ['TMPDIR'], 'nested-ssh-control-' + 'x' * 48)
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
