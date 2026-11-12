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
    fixture_artifacts,
    fixture_directory,
    fixture_environment,
    private_directory,
    read_preferences,
    temporary_root,
    write_python_launcher,
)


class FixtureSupportTests(unittest.TestCase):
    def test_explicit_root_takes_precedence_and_resolves_aliases(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            chosen, ordinary = root / "chosen ' root", root / "ordinary"
            chosen.mkdir()
            ordinary.mkdir()
            alias = root / "alias"
            alias.symlink_to(chosen, target_is_directory=True)
            with patch.dict(os.environ, RODE_TEST_TMPDIR=str(alias), TMPDIR=str(ordinary)):
                with fixture_directory(prefix="fixture-") as base:
                    self.assertEqual(base.parent, chosen.resolve())
                    self.assertEqual(base.stat().st_mode & 0o777, 0o700)
            self.assertFalse(base.exists())

    def test_standard_temp_root_is_honored_and_invalid_overrides_do_not_fall_back(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(os.environ, RODE_TEST_TMPDIR="", TMPDIR=str(root)):
                with patch("fixture_support.tempfile.tempdir", None):
                    self.assertEqual(temporary_root(), root.resolve())
            with patch.dict(os.environ, RODE_TEST_TMPDIR=str(root / "missing")):
                with self.assertRaises(FileNotFoundError):
                    temporary_root()
            file = root / "file"
            file.touch()
            with patch.dict(os.environ, RODE_TEST_TMPDIR=str(file)):
                with self.assertRaises(NotADirectoryError):
                    temporary_root()

    def test_nested_and_external_artifact_paths_do_not_overwrite_other_failures(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            results = root / "results"
            sources = [
                root / "target/git-identity-ui/ssh",
                root / "target/execution-modes-ui/ssh",
                root / "external-one/ssh",
                root / "external-two/ssh",
            ]
            destinations = [fixture_artifacts(root, path, results) for path in sources]
            self.assertEqual(destinations[0], results / "ui/git-identity-ui/ssh")
            for index, destination in enumerate(destinations):
                destination.mkdir(parents=True, exist_ok=True)
                (destination / "failure.png").write_text(str(index))
            for index, destination in enumerate(destinations):
                self.assertEqual((destination / "failure.png").read_text(), str(index))
            self.assertEqual(fixture_artifacts(root, sources[0]), sources[0])

    def test_home_is_private_under_permissive_umasks_without_changing_the_parent(self):
        script = "from pathlib import Path; from fixture_support import fixture_environment; import sys; env=fixture_environment(sys.argv[1]); assert Path(env['HOME']).stat().st_mode & 0o777 == 0o700"
        for mask in (0o002, 0o022, 0o077):
            with tempfile.TemporaryDirectory() as directory:
                subprocess.run(
                    [sys.executable, "-c", script, directory],
                    cwd=Path(__file__).parent,
                    umask=mask,
                    check=True,
                )

    def test_existing_unsafe_or_symlinked_homes_are_rejected_not_repaired(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            home = base / "home"
            home.mkdir()
            home.chmod(0o775)
            with self.assertRaisesRegex(ValueError, "0700"):
                fixture_environment(base)
            self.assertEqual(home.stat().st_mode & 0o777, 0o775)
            home.rmdir()
            other = private_directory(base / "other")
            home.symlink_to(other, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "non-symlink"):
                fixture_environment(base)

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

    def test_preference_map_decodes_each_ron_layer_and_accepts_json_string_maps(self):
        nested = json.dumps(
            {"project ' λ": {"path": 'backslash\\ and "quote"'}}, ensure_ascii=False
        )
        values = {"font_size": "15", "project_preferences": nested}

        def ron_string(value):
            return json.dumps(value, ensure_ascii=False).replace("'", "\\'")

        ron = (
            "{\n"
            + "\n".join(f"{ron_string(key)}: {ron_string(value)}," for key, value in values.items())
            + "\n}"
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "app.ron"
            self.assertEqual(read_preferences(path), {})
            for contents in (ron, json.dumps(values, ensure_ascii=False), "{}"):
                with self.subTest(contents=contents):
                    path.write_text(contents)
                    self.assertEqual(read_preferences(path), {} if contents == "{}" else values)

    def test_preference_map_rejects_malformed_entries_instead_of_waiting_forever(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "app.ron"
            for contents in (
                '{"key": 1}',
                '{"key": "value" "other": "value"}',
                '{"key": "one", "key": "two"}',
                '{"key": "value",}junk',
                r'{"key": "\q"}',
                "",
            ):
                with self.subTest(contents=contents), self.assertRaises(ValueError):
                    path.write_text(contents)
                    read_preferences(path)

    def test_canonical_paths_match_host_rows_when_the_temporary_parent_is_a_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            actual = root / "real parent ' with spaces"
            actual.mkdir()
            alias = root / "alias"
            alias.symlink_to(actual, target_is_directory=True)
            with (
                patch.dict(os.environ, RODE_TEST_TMPDIR=""),
                patch("fixture_support.tempfile.tempdir", str(alias)),
            ):
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
