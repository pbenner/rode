#!/usr/bin/env python3
"""Regressions for CI selection, transferred binaries and bounded fixture execution."""

import json
import os
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import ci


class SelectionTests(unittest.TestCase):
    def test_docs_keep_the_fast_lane_without_expensive_viewers(self):
        self.assertEqual(ci.extended_suites(["README.md", "docs/editing.md"]), [])

    def test_shared_changes_and_full_runs_keep_all_extended_coverage(self):
        for path in ["src/app.rs", "crates/rode-session/src/lib.rs", "Cargo.lock", "tests/pdf.rs"]:
            self.assertEqual(ci.extended_suites([path]), ci.EXTENDED)
        self.assertEqual(ci.extended_suites([], full=True), ci.EXTENDED)
        self.assertEqual(ci.extended_suites(["scripts/build-sessiond.sh"]), ci.EXTENDED)
        self.assertEqual(ci.extended_suites(["scripts/test-new-ui.py"]), ci.EXTENDED)

    def test_viewer_harness_changes_select_both_local_and_remote_cases(self):
        self.assertEqual(
            ci.extended_suites(["scripts/test-markdown-ui.py"]), ["documents", "remote"]
        )
        self.assertEqual(ci.extended_suites(["scripts/test-notebooks-ui.py"]), ["runtimes"])

    def test_all_previous_workflow_cases_remain_reachable(self):
        required = {
            "workspace",
            "environment",
            "debugger",
            "terminal-keys",
            "sessions",
            "slurm",
            "slurm-notebooks",
            "binary",
            "notebooks",
            "layout",
            "languages",
            "syntax-themes",
            "agent",
            "agent-queue",
            "agent-attachments",
            "agent-persistence",
            "agent-history",
            "codex-install",
            "multiprocess",
            "project",
            "workbench",
            "images",
            "markdown",
            "pdf",
            "marp",
            "latex",
            "vaults",
        }
        scripts = {
            argument
            for entries in ci.SUITES.values()
            for command, _, _ in entries
            for argument in command
            if argument.startswith("scripts/") and argument.endswith(".py")
        }
        for name in required:
            self.assertIn(f"scripts/test-{name}-ui.py", scripts)
        for script in scripts:
            self.assertTrue((ci.ROOT / script).is_file(), script)


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="rode-ci-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.summary = self.root / "summary.md"
        root = patch.object(ci, "ROOT", self.root)
        environment = patch.dict(os.environ, GITHUB_STEP_SUMMARY=str(self.summary))
        root.start()
        environment.start()
        self.addCleanup(root.stop)
        self.addCleanup(environment.stop)

    def record(self):
        return json.loads((self.root / "target/ci-results/commands/timings.jsonl").read_text())

    def test_failed_command_preserves_exit_status_log_and_summary(self):
        with self.assertRaises(SystemExit) as error:
            ci.measure("failure", [sys.executable, "-c", "print('diagnostic');exit(7)"])
        self.assertEqual(error.exception.code, 7)
        self.assertEqual(self.record()["status"], "failed")
        self.assertIn(
            "diagnostic", (self.root / "target/ci-results/commands/failure.log").read_text()
        )
        self.assertIn("failed", self.summary.read_text())

    def test_missing_executable_also_has_a_diagnostic_and_failed_timing(self):
        with self.assertRaises(SystemExit) as error:
            ci.measure("missing", [str(self.root / "missing-executable")])
        self.assertEqual(error.exception.code, 127)
        self.assertEqual(self.record()["status"], "failed")
        self.assertIn(
            "Could not start", (self.root / "target/ci-results/commands/missing.log").read_text()
        )

    def test_timeout_terminates_descendants_as_well_as_the_test_driver(self):
        child_pid = self.root / "child.pid"
        source = (
            "import subprocess,sys,time\nfrom pathlib import Path\n"
            "child=subprocess.Popen([sys.executable,'-c',"
            "'import signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);time.sleep(60)'])\n"
            f"Path({str(child_pid)!r}).write_text(str(child.pid))\ntime.sleep(60)\n"
        )
        with self.assertRaises(SystemExit) as error:
            ci.measure("timeout", [sys.executable, "-c", source], timeout=0.5)
        self.assertEqual(error.exception.code, 124)
        self.assertEqual(self.record()["status"], "timeout")
        pid = child_pid.read_text()
        status = Path(f"/proc/{pid}/stat")
        deadline = time.monotonic() + 1
        while status.exists() and status.read_text().split()[2] != "Z":
            self.assertLess(time.monotonic(), deadline, "Timed-out descendant survived")
            time.sleep(0.01)

    def test_bundle_uses_cargo_paths_and_retains_executable_modes(self):
        build = self.root / "target/ci"
        build.mkdir(parents=True)
        for name in ["rode", "rode-sessiond"]:
            (build / name).write_text(name)
            (build / name).chmod(0o755)
        helper = self.root / "target/release/rode-sessiond-static"
        helper.parent.mkdir()
        helper.write_text("static helper")
        helper.chmod(0o755)
        ruff = self.root / "target/test-tools/bin/ruff"
        ruff.parent.mkdir(parents=True)
        ruff.write_text("pinned ruff")
        ruff.chmod(0o755)
        records = []
        for name in ci.TEST_BINARIES:
            executable = self.root / f"compiled-{name}-hash"
            executable.write_text(name)
            executable.chmod(0o755)
            records.append(dict(target=dict(name=name, kind=["test"]), executable=str(executable)))
        (self.root / "target/ci-test-binaries.json").write_text(
            "\n".join(json.dumps(record) for record in records)
        )
        ci.bundle()
        with tarfile.open(self.root / "target/ci-bundle.tar.gz") as archive:
            self.assertEqual(
                archive.extractfile("target/ci/rode-sessiond-static").read(), b"static helper"
            )
            self.assertEqual(archive.getmember("target/ci/tests/ssh_roundtrip").mode, 0o755)
            self.assertEqual(archive.extractfile("target/ci/rode").read(), b"rode")
            self.assertEqual(archive.extractfile("target/ci/tools/ruff").read(), b"pinned ruff")
            self.assertEqual(archive.getmember("target/ci/tools/ruff").mode, 0o755)

    def test_remote_documents_receive_bundled_ruff_and_fail_early_if_it_is_missing(self):
        for name in ["rode", "rode-sessiond-static", "tools/ruff"]:
            path = self.root / "target/ci" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture")
            path.chmod(0o755)
        with patch.dict(os.environ, {}, clear=True), patch.object(ci, "measure") as run:
            ci.run_suite("remote-documents")
            for call in run.call_args_list:
                self.assertEqual(
                    call.args[2]["RODE_TEST_RUFF"], str(self.root / "target/ci/tools/ruff")
                )
            run.reset_mock()
            (self.root / "target/ci/tools/ruff").unlink()
            with self.assertRaisesRegex(SystemExit, "RODE_TEST_RUFF"):
                ci.run_suite("remote-documents")
            run.assert_not_called()


class SshFixtureTests(unittest.TestCase):
    def test_ui_only_and_prebuilt_transport_modes_never_invoke_cargo(self):
        with tempfile.TemporaryDirectory(prefix="rode-ci-ssh-test-") as temporary:
            base = Path(temporary)
            sshd = base / "sshd"
            sshd.write_text(
                f"#!{sys.executable}\nimport socket,sys,time\nfrom pathlib import Path\n"
                "config=Path(sys.argv[sys.argv.index('-f')+1]).read_text()\n"
                "port=int(next(line.split()[1] for line in config.splitlines() if line.startswith('Port ')))\n"
                "s=socket.socket();s.bind(('127.0.0.1',port));s.listen();time.sleep(60)\n"
            )
            sshd.chmod(0o755)
            keygen = base / "ssh-keygen"
            keygen.write_text(
                f"#!{sys.executable}\nimport sys\nfrom pathlib import Path\n"
                "path=sys.argv[sys.argv.index('-f')+1]\n"
                "Path(path).write_text('private fixture')\n"
                "Path(path+'.pub').write_text('ssh-ed25519 Zml4dHVyZQ== fixture\\n')\n"
            )
            keygen.chmod(0o755)
            environment = dict(
                os.environ, PATH=str(base) + os.pathsep + os.environ["PATH"], CARGO="/no-cargo"
            )
            environment.pop("RODE_SSH_TEST_BINARY", None)
            environment.pop("RODE_CI_SSH_LOG_DIR", None)
            command = ["bash", str(ci.ROOT / "scripts/test-ssh.sh")]
            probe = [
                sys.executable,
                "-c",
                "import os;from pathlib import Path;assert Path(os.environ['RODE_TEST_SSH_CONFIG']).is_file();print('UI ran')",
            ]
            result = subprocess.run(
                command + probe,
                env=environment | {"RODE_SSH_RUN_TRANSPORT": "0"},
                capture_output=True,
                text=True,
                timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("UI ran", result.stdout)
            transport = base / "transport"
            transport.write_text("#!/bin/sh\nprintf 'transport ran\\n'\n")
            transport.chmod(0o755)
            result = subprocess.run(
                command + probe,
                env=environment
                | {"RODE_SSH_TEST_BINARY": str(transport), "RODE_SSH_RUN_TRANSPORT": "1"},
                capture_output=True,
                text=True,
                timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("transport ran\nUI ran", result.stdout)


if __name__ == "__main__":
    unittest.main()
