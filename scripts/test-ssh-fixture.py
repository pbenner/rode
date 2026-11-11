#!/usr/bin/env python3
"""Verify SSH helper isolation and portable probes in the existing shell process."""

import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from ssh_fixture import isolated_path, shell_probe


class SSHFixtureTests(unittest.TestCase):
    def test_access_probes_keep_arguments_and_the_existing_pid_in_sh_and_bash(self):
        for shell in ("/bin/sh", "/bin/bash"):
            with self.subTest(shell=shell), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / "check-access").write_text(
                    'printf "host:%s" "$$" > "$1.tmp" && mv "$1.tmp" "$1"\n'
                )
                names = ["first marker", "retained-$(false); marker"]
                result = subprocess.run(
                    [shell, "-c", "".join(shell_probe(name) for name in names)],
                    cwd=root,
                    text=True,
                    capture_output=True,
                    check=True,
                    timeout=5,
                )
                first, retained = [(root / name).read_text() for name in names]
                self.assertEqual(first, retained)
                mode, pid = first.split(":")
                self.assertEqual(mode, "host")
                self.assertGreater(int(pid), 0)
                self.assertEqual(result.stderr, "")

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
