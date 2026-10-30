#!/usr/bin/env python3
"""Check component independence without compilers, network access or package installation."""

import argparse
import contextlib
import io
import json
import runpy
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

BUILDER = runpy.run_path(str(Path(__file__).with_name("build-deb.py")))


class Components(unittest.TestCase):
    def test_editor_does_not_consult_codex_or_download_a_runtime(self):
        """Even a tree with no Codex manifest must assemble the core component."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "README.md").write_text("editor")
            scope = BUILDER["main"].__globals__
            metadata = {
                "packages": [{"name": "rode", "version": "1.2.3"}],
                "target_directory": str(root / "target"),
            }

            def output(*command):
                if "metadata" in command:
                    return json.dumps(metadata)
                self.assertEqual(command, ("dpkg", "--print-architecture"))
                return "amd64"

            def write_package(
                _args,
                name,
                version,
                _architecture,
                _destination,
                files,
                _executables,
                _documents,
                **_fields,
            ):
                self.assertEqual((name, version), ("rode", "1.2.3-1"))
                self.assertFalse(any("codex" in name.lower() for name in files))
                return root / "rode.deb"

            with (
                patch.dict(
                    scope,
                    ROOT=root,
                    output=output,
                    check_elf=lambda *_a, **_k: None,
                    notices=lambda _binary: b"licenses",
                    write_package=write_package,
                ),
                patch.object(
                    sys, "argv", ["build-deb.py", "--skip-build", "--shlibs-depends", "libc6"]
                ),
                patch("shutil.which", return_value="/build-tool"),
                patch("runpy.run_path", side_effect=AssertionError("Core must not prepare Codex")),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                BUILDER["main"]()

    def test_missing_codex_target_fails_only_the_explicit_runtime_component(self):
        """An unsupported runtime target must not run Cargo or download anything."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "packaging").mkdir()
            (root / "packaging/codex.json").write_text('{"version":"9.0.0","targets":{}}')
            scope = BUILDER["package_codex"].__globals__
            with (
                patch.dict(scope, ROOT=root, output=lambda *_a: "amd64"),
                patch("runpy.run_path", side_effect=AssertionError("No download")),
            ):
                with self.assertRaisesRegex(
                    ValueError, "build the editor without --component codex"
                ):
                    BUILDER["package_codex"](argparse.Namespace())


if __name__ == "__main__":
    unittest.main()
