#!/usr/bin/env python3
"""SSH quoting, helper isolation and shell probes without a daemon, account or display."""

import os
import shlex
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from ssh_fixture import config_token, configure, isolated_path, shell_probe


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

    def configure(self, root, **options):
        (root / "home").mkdir()
        (root / "host-known.pub").write_text("ecdsa-sha2-nistp256 fixture-public-key\n")
        values = dict(
            search_path="/bin:/usr/bin",
            project_shell="",
            xauth="",
            port=12345,
            user="fixture-user",
            x11="yes",
        )
        values.update(options)
        configure(root, **values)

    def test_quoted_paths_are_literal_in_forced_commands_and_host_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "ssh fixture ' $(literal) with spaces"
            root.mkdir()
            path = str(root / "bin ' with spaces") + ":/bin:/usr/bin"
            shell = str(root / "shell ' with spaces")
            self.configure(root, search_path=path, project_shell=shell)
            environment = dict(
                os.environ,
                SSH_ORIGINAL_COMMAND='printf "%s\\000" "$HOME" "$TMPDIR" "$PATH" "$SHELL"',
            )
            result = subprocess.check_output([root / "host-shell"], env=environment, timeout=5)
            self.assertEqual(
                result.split(b"\0")[:-1],
                [
                    str(value).encode()
                    for value in (
                        root / "home",
                        root,
                        path,
                        shell,
                    )
                ],
            )
            directives = dict(
                line.split(" ", 1) for line in (root / "sshd_config").read_text().splitlines()
            )
            self.assertEqual(shlex.split(directives["ForceCommand"]), [str(root / "host-shell")])
            self.assertEqual(shlex.split(directives["HostKey"]), [str(root / "host-known")])
            for name in ("PermitUserRC", "PermitUserEnvironment", "UseDNS"):
                self.assertEqual(directives[name], "no")
            self.assertIn(
                "SetEnv " + config_token("HOME=" + str(root / "home")),
                (root / "sshd_config").read_text(),
            )

    def test_xauth_and_config_tokens_preserve_paths_with_spaces_and_quotes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "private ' ssh fixture"
            root.mkdir()
            binary = root / "xauth ' program"
            binary.write_text('#!/bin/sh\nprintf "%s\\000" "$@"\n')
            binary.chmod(0o700)
            self.configure(root, xauth=str(binary))
            result = subprocess.check_output([root / "xauth", "list", "literal ' value"], timeout=5)
            self.assertEqual(
                result.split(b"\0")[:-1],
                [
                    b"-f",
                    str(root / "home/.Xauthority").encode(),
                    b"list",
                    b"literal ' value",
                ],
            )
            config = (root / "ssh_config").read_text()
            self.assertIn("IdentityFile " + config_token(root / "client"), config)
            self.assertIn("XAuthLocation " + config_token(binary), config)
            self.assertIn(
                "GlobalKnownHostsFile " + config_token(root / "global_known_hosts"), config
            )
            self.assertEqual((root / "global_known_hosts").read_text(), "")
            self.assertEqual((root / "host-shell").stat().st_mode & 0o777, 0o700)

    def test_config_values_cannot_add_directives(self):
        for value in ("path\nInjected yes", "path\rInjected yes", "path\0tail"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                config_token(value)
        with tempfile.TemporaryDirectory() as directory:
            for options in (
                {"x11": "yes\nPasswordAuthentication yes"},
                {"port": "22\nListenAddress 0.0.0.0"},
                {"port": 0},
                {"port": 65536},
            ):
                values = dict(
                    search_path="/bin", project_shell="", xauth="", port=12345, user="fixture"
                )
                values.update(options)
                with self.subTest(options=options), self.assertRaises(ValueError):
                    configure(directory, **values)
            self.assertEqual(list(Path(directory).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
