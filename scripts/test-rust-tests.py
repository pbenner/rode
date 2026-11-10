#!/usr/bin/env python3
"""Verify private Rust test homes, cleanup, argument forwarding and exit codes."""

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import rust_tests


class RustFixtureTests(unittest.TestCase):
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
