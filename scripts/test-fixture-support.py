#!/usr/bin/env python3
"""Fixture portability and acknowledgement data checks without an X server."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from fixture_support import (
    decode_ron_string,
    fixture_directory,
    fixture_environment,
    write_python_launcher,
)


class FixtureSupportTests(unittest.TestCase):
    def test_ron_storage_decodes_quoted_unicode_paths_before_the_nested_json(self):
        payload = {"project ' with spaces λ": {"path": 'backslash\\ and "quote"'}}
        inner = json.dumps(payload, ensure_ascii=False)
        outer = '"' + inner.replace("\\", "\\\\").replace('"', '\\"').replace("'", "\\'") + '"'
        with self.assertRaises(json.JSONDecodeError):
            json.loads(outer)
        self.assertEqual(json.loads(decode_ron_string(outer)), payload)
        self.assertEqual(decode_ron_string(r'"\u{3bb}\n\t\0\'"'), "λ\n\t\0'")

    def test_malformed_ron_escapes_are_not_hidden_as_pending_preferences(self):
        for literal in ("unquoted", r'"\q"', r'"\u{d800}"', r'"\u{110000}"', r'"\u{ff"'):
            with self.subTest(literal=literal), self.assertRaises(ValueError):
                decode_ron_string(literal)

    def test_canonical_paths_match_host_rows_when_the_temporary_parent_is_a_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            actual = root / "real parent ' with spaces"
            actual.mkdir()
            alias = root / "alias"
            alias.symlink_to(actual, target_is_directory=True)
            with patch("fixture_support.tempfile.tempdir", str(alias)):
                with fixture_directory(prefix="project-") as base:
                    self.assertEqual(base.parent, actual.resolve())
                    self.assertEqual(base, base.resolve())
                    self.assertEqual(base.stat().st_mode & 0o777, 0o700)
            self.assertFalse(base.exists())

    def test_each_fixture_has_private_host_state_and_preserves_explicit_tools(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(
                os.environ, CODEX_HOME="ambient-agent-home", RODE_TEST_RUFF="chosen-tool"
            ):
                before = dict(os.environ)
                first, second = root / "first", root / "second"
                first.mkdir()
                second.mkdir()
                a, b = fixture_environment(first), fixture_environment(second)
                self.assertEqual(dict(os.environ), before)
                self.assertNotIn("CODEX_HOME", a)
                self.assertEqual(a["RODE_TEST_RUFF"], "chosen-tool")
                for name in ("HOME", "XDG_CACHE_HOME", "RODE_SESSION_DIR", "RODE_VAULT_DIR"):
                    self.assertTrue(Path(a[name]).is_relative_to(first))
                    self.assertNotEqual(a[name], b[name])

    def test_python_launchers_preserve_literal_paths_interpreters_and_arguments(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "fixture ' $(literal) with spaces"
            root.mkdir()
            interpreter = root / "python interpreter ' with spaces"
            interpreter.symlink_to(sys.executable)
            launcher = root / "fixture command"
            write_python_launcher(
                launcher,
                "import json,sys; print(json.dumps(sys.argv[1:]))\n",
                python=interpreter,
            )
            expected = ["literal ' \" $value", "two words", "λ"]
            output = subprocess.check_output([launcher, *expected], text=True, timeout=10)
            self.assertEqual(json.loads(output), expected)


if __name__ == "__main__":
    unittest.main()
