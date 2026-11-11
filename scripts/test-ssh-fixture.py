#!/usr/bin/env python3
"""Verify SSH helper isolation without changing unrelated tool selection."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from ssh_fixture import isolated_path


class SSHPathTests(unittest.TestCase):
    def test_host_helper_is_hidden_without_reordering_or_copying_other_tools(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first, second, untouched = [root / name for name in ("first bin", "second", "last")]
            for folder in (first, second, untouched):
                folder.mkdir()
            for folder, name, output in (
                (first, "rode-sessiond", "ambient helper"),
                (second, "rode-sessiond", "another helper"),
                (first, "fixture-tool", "first"),
                (second, "fixture-tool", "second"),
                (untouched, "other-tool", "other"),
            ):
                program = folder / name
                program.write_text(f"#!/bin/sh\nprintf '%s' '{output}'\n")
                program.chmod(0o755)
            (first / "not-executable").write_text("not a command")
            path = isolated_path(
                os.pathsep.join(map(str, (first, second, untouched))), root / "path"
            )
            self.assertIsNone(shutil.which("rode-sessiond", path=path))
            tool = Path(shutil.which("fixture-tool", path=path))
            self.assertTrue(tool.is_symlink())
            self.assertEqual(subprocess.check_output([str(tool)]), b"first")
            self.assertIsNone(shutil.which("not-executable", path=path))
            self.assertEqual(shutil.which("other-tool", path=path), str(untouched / "other-tool"))
            self.assertEqual(
                (first / "rode-sessiond").read_text().splitlines()[-1],
                "printf '%s' 'ambient helper'",
            )


if __name__ == "__main__":
    unittest.main()
