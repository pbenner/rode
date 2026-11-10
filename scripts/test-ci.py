#!/usr/bin/env python3
"""Regressions for CI selection, transferred binaries and bounded fixture execution."""

import io
import json
import os
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import ci


def process_exited(status):
    """Treat a zombie or vanished proc entry as exited, including races during read."""
    try:
        state = status.read_text().rsplit(") ", 1)[1].split()[0]
    except (FileNotFoundError, ProcessLookupError):
        return True
    return state == "Z"


class ProcessStateTests(unittest.TestCase):
    def test_exit_during_proc_read_is_treated_as_terminated(self):
        for error in (
            FileNotFoundError(2, "No such file"),
            ProcessLookupError(3, "No such process"),
        ):
            with self.subTest(error=type(error).__name__):
                with patch.object(Path, "read_text", side_effect=error):
                    self.assertTrue(process_exited(Path("/proc/123/stat")))

    def test_only_zombies_count_as_exited_when_proc_state_is_available(self):
        for state in ("R", "S", "D", "T", "Z"):
            with self.subTest(state=state):
                # The parenthesized command name may itself contain spaces and parentheses.
                status = f"123 (python worker (fixture)) {state} 1 123 123"
                with patch.object(Path, "read_text", return_value=status):
                    self.assertEqual(process_exited(Path("/proc/123/stat")), state == "Z")

    def test_unexpected_proc_read_errors_are_not_treated_as_successful_cleanup(self):
        with patch.object(Path, "read_text", side_effect=PermissionError(13, "Permission denied")):
            with self.assertRaises(PermissionError):
                process_exited(Path("/proc/123/stat"))


class SelectionTests(unittest.TestCase):
    def test_docs_keep_the_fast_lane_without_expensive_viewers(self):
        self.assertEqual(ci.extended_suites(["README.md", "docs/editing.md"]), [])

    def test_shared_changes_and_full_runs_keep_all_extended_coverage(self):
        for path in [
            "src/app.rs",
            "crates/rode-session/src/lib.rs",
            "Cargo.lock",
            "tests/pdf.rs",
            "mise.toml",
            "mise.lock",
            "rust-toolchain.toml",
            "xtask/src/helper.rs",
            ".cargo/config.toml",
        ]:
            self.assertEqual(ci.extended_suites([path]), ci.EXTENDED)
        self.assertEqual(ci.extended_suites([], full=True), ci.EXTENDED)
        self.assertEqual(ci.extended_suites(["scripts/test-new-ui.py"]), ci.EXTENDED)

    def test_viewer_harness_changes_select_both_local_and_remote_cases(self):
        self.assertEqual(
            ci.extended_suites(["scripts/test-markdown-ui.py"]), ["documents", "remote"]
        )
        self.assertEqual(ci.extended_suites(["scripts/test-notebooks-ui.py"]), ["runtimes"])

    def test_all_previous_workflow_cases_remain_reachable(self):
        required = {
            "workspace",
            "navigation",
            "explorer",
            "environment",
            "mise",
            "debugger",
            "terminal-keys",
            "sessions",
            "slurm",
            "slurm-notebooks",
            "binary",
            "large-files",
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

    def test_execution_modes_cover_required_enforcement_and_forced_ssh_fallback(self):
        script = "scripts/test-execution-modes-ui.py"
        self.assertTrue(
            any(
                script in command and overrides.get("RODE_TEST_REQUIRE_SANDBOX") == "1"
                for command, overrides, _ in ci.SUITES["sandbox"]
            )
        )
        self.assertTrue(
            any(
                script in command
                and "scripts/test-ssh.sh" in command
                and overrides.get("RODE_SSH_TEST_NO_BWRAP") == "1"
                and overrides.get("RODE_EXECUTION_UI_REMOTE") == "1"
                and overrides.get("RODE_SSH_RUN_TRANSPORT") == "0"
                for command, overrides, _ in ci.SUITES["ssh"]
            )
        )


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
        self.assertEqual(self.record()["exit_code"], 7)
        self.assertEqual(self.record()["timeout_seconds"], 300)
        self.assertIn(
            "diagnostic", (self.root / "target/ci-results/commands/failure.log").read_text()
        )
        self.assertIn("failed", self.summary.read_text())
        self.assertIn("failure.json", self.summary.read_text())
        self.assertIn("Run from the repository root", self.summary.read_text())

    def test_context_preserves_literal_arguments_and_excludes_unlisted_environment(self):
        literal = "literal argument '$HOME' $(false)\nsecond line"
        command = [
            sys.executable,
            "-c",
            f"import sys;assert sys.argv[1] == {literal!r}",
            literal,
        ]
        environment = dict(os.environ, RODE_TEST_REQUIRE_SANDBOX="1", GITHUB_TOKEN="fixture-secret")
        ci.measure("context", command, env=environment, timeout=4)
        context = json.loads((self.root / "target/ci-results/commands/context.json").read_text())
        self.assertEqual(context, self.record())
        self.assertEqual(shlex.split(context["invocation"]), command)
        self.assertEqual(context["exit_code"], 0)
        self.assertEqual(context["timeout_seconds"], 4)
        self.assertEqual(context["cwd"], str(self.root))
        self.assertEqual(context["environment"]["RODE_TEST_REQUIRE_SANDBOX"], "1")
        self.assertNotIn("GITHUB_TOKEN", context["environment"])
        self.assertNotIn("fixture-secret", json.dumps(context))
        self.assertNotIn("fixture-secret", self.summary.read_text())

    def test_missing_executable_also_has_a_diagnostic_and_failed_timing(self):
        with self.assertRaises(SystemExit) as error:
            ci.measure("missing", [str(self.root / "missing-executable")])
        self.assertEqual(error.exception.code, 127)
        self.assertEqual(self.record()["status"], "failed")
        self.assertIn(
            "Could not start", (self.root / "target/ci-results/commands/missing.log").read_text()
        )

    def test_verbose_failure_keeps_the_assertion_and_tail_in_console_output(self):
        command = [
            sys.executable,
            "-c",
            "print('original assertion');print('daemon chatter\\n'*6000);print('final diagnostic');exit(1)",
        ]
        console = io.StringIO()
        with patch("sys.stdout", console), self.assertRaises(SystemExit):
            ci.measure("verbose", command)
        self.assertIn("original assertion", console.getvalue())
        self.assertIn("final diagnostic", console.getvalue())
        self.assertIn("middle omitted", console.getvalue())
        self.assertLess(len(console.getvalue()), 25000)
        log = (self.root / "target/ci-results/commands/verbose.log").read_text()
        self.assertEqual(log.count("daemon chatter"), 6000)

    def test_nested_suite_failure_keeps_one_console_group_and_the_full_raw_log(self):
        child = "::group::child UI test\nTraceback: KeyError: 'git_sandbox'\n::endgroup::\n"
        console = io.StringIO()
        with patch("sys.stdout", console), self.assertRaises(SystemExit) as error:
            ci.measure("nested", [sys.executable, "-c", f"print({child!r}, end='');exit(1)"])
        self.assertEqual(error.exception.code, 1)
        # The command's title quotes its literal source, so inspect group markers
        # at line starts and the replayed diagnostic beneath that title.
        lines = console.getvalue().splitlines()
        self.assertEqual(sum(line.startswith("::group::") for line in lines), 1)
        self.assertEqual(sum(line == "::endgroup::" for line in lines), 1)
        self.assertIn("child UI test", lines)
        self.assertIn("Traceback: KeyError: 'git_sandbox'", lines)
        self.assertEqual((self.root / "target/ci-results/commands/nested.log").read_text(), child)

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
        self.assertEqual(self.record()["exit_code"], 124)
        self.assertEqual(self.record()["timeout_seconds"], 0.5)
        pid = child_pid.read_text()
        status = Path(f"/proc/{pid}/stat")
        deadline = time.monotonic() + 1
        while not process_exited(status):
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

    def test_transferred_tests_use_the_selected_helper_for_service_discovery(self):
        for name in ["rode", "rode-sessiond-static", "tests/ssh_roundtrip"]:
            path = self.root / "target/ci" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture")
            path.chmod(0o755)
        override = self.root / "custom-helper"
        override.write_text("fixture")
        override.chmod(0o755)
        for settings in [
            {},
            {"RODE_TEST_SESSION_HELPER": str(override)},
            {"RODE_SESSION_HELPER": str(override)},
        ]:
            with patch.dict(os.environ, settings, clear=True), patch.object(ci, "measure") as run:
                ci.run_suite("ssh")
                env = run.call_args.args[2]
                self.assertEqual(
                    env["RODE_SESSION_HELPER"],
                    settings.get("RODE_SESSION_HELPER", env["RODE_TEST_SESSION_HELPER"]),
                )


class DependencySetupTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="rode-ci-deps-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.tools = self.root / "bin"
        self.tools.mkdir()
        scripts = self.root / "scripts"
        scripts.mkdir()
        for name in ("ci-deps.sh", "uvx.sh"):
            shutil.copy2(ci.ROOT / "scripts" / name, scripts / name)
        # A private PATH prevents the fixture from finding real sudo/APT or
        # developer tools. Only harmless command plumbing reaches the host.
        for name in ("dirname", "env"):
            (self.tools / name).symlink_to(shutil.which(name))
        self.bash = shutil.which("bash")
        for name in ("sudo", "apt-get", "npm", "uv", "uvx"):
            self.tool(name)

    def tool(self, name):
        path = self.tools / name
        path.write_text(
            f"#!{sys.executable}\n"
            "import json,os,sys\nfrom pathlib import Path\n"
            "name=Path(sys.argv[0]).name\n"
            "if name=='sudo': os.execvp(sys.argv[1],sys.argv[1:])\n"
            "with Path('calls.jsonl').open('a') as log:\n"
            " log.write(json.dumps(dict(name=name,args=sys.argv[1:],frontend=os.environ.get('DEBIAN_FRONTEND')))+'\\n')\n"
            "if name=='uvx': sys.stdin.read()\n"
            "if name=='apt-get' and os.environ.get('FAIL_APT_PHASE') in sys.argv[1:]: sys.exit(100)\n"
        )
        path.chmod(0o755)

    def run_setup(self, suite="runtimes", fail=""):
        calls_path = self.root / "calls.jsonl"
        calls_path.write_text("")
        result = subprocess.run(
            [self.bash, str(self.root / "scripts/ci-deps.sh"), suite],
            env=dict(
                os.environ,
                PATH=str(self.tools),
                GITHUB_ENV=str(self.root / "exports"),
                FAIL_APT_PHASE=fail,
            ),
            capture_output=True,
            text=True,
            timeout=10,
        )
        calls = [json.loads(line) for line in calls_path.read_text().splitlines()]
        return result, calls

    def test_downloads_are_bounded_and_runtime_tools_still_get_installed(self):
        self.tool("go")
        self.tool("gofmt")
        result, calls = self.run_setup()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        apt = [call for call in calls if call["name"] == "apt-get"]
        self.assertEqual(len(apt), 2)
        for call in apt:
            for option in (
                "Acquire::Retries=3",
                "Acquire::http::Timeout=30",
                "Acquire::https::Timeout=30",
                "DPkg::Lock::Timeout=60",
                "APT::Update::Error-Mode=any",
            ):
                self.assertIn(option, call["args"])
            self.assertEqual(call["frontend"], "noninteractive")
        self.assertIn("update", apt[0]["args"])
        self.assertIn("install", apt[1]["args"])
        self.assertIn("--no-install-recommends", apt[1]["args"])
        self.assertFalse(any(arg.startswith("golang-") for call in apt for arg in call["args"]))
        for package in ("lldb-19", "openssh-server", "gocryptfs", "clang-format", "shfmt"):
            self.assertIn(package, apt[1]["args"])
        self.assertEqual((calls[0]["name"], calls[0]["args"]), ("go", ["version"]))
        self.assertTrue({"npm", "uvx", "uv"}.issubset(call["name"] for call in calls))
        exports = (self.root / "exports").read_text()
        for name in ("RODE_TEST_PRETTIER", "RODE_TEST_RUFF", "RODE_TEST_LLDB", "RODE_TEST_JUPYTER"):
            self.assertIn(name, exports)

    def test_missing_go_or_gofmt_fails_before_any_package_download(self):
        for suite in ("tools", "runtimes"):
            for present in (None, "go", "gofmt"):
                with self.subTest(suite=suite, present=present):
                    for name in ("go", "gofmt"):
                        (self.tools / name).unlink(missing_ok=True)
                    if present:
                        self.tool(present)
                    result, calls = self.run_setup(suite)
                    self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                    self.assertIn("mise install --locked go", result.stderr)
                    self.assertEqual(calls, [])

    def test_suites_without_language_tests_do_not_need_go(self):
        for suite in (
            "build",
            "lint",
            "workspace",
            "agents",
            "terminals",
            "ssh",
            "documents",
            "remote",
            "sandbox",
        ):
            with self.subTest(suite=suite):
                result, calls = self.run_setup(suite)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual([call["name"] for call in calls], ["apt-get", "apt-get"])
                self.assertFalse(
                    any(arg.startswith("golang-") for call in calls for arg in call["args"])
                )

    def test_apt_failure_stops_setup_without_retrying_install_or_starting_other_downloads(self):
        self.tool("go")
        self.tool("gofmt")
        for phase in ("update", "install"):
            with self.subTest(phase=phase):
                result, calls = self.run_setup(fail=phase)
                self.assertEqual(result.returncode, 100, result.stdout + result.stderr)
                self.assertEqual(calls[0]["name"], "go")
                self.assertTrue(all(c["name"] == "apt-get" for c in calls[1:]))
                self.assertEqual(len(calls[1:]), 1 if phase == "update" else 2)


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
            # This server only opens a TCP port; it does not speak SSH. Keep
            # the fixture's PTY capability probe out of the real ssh client.
            ssh = base / "ssh"
            ssh.write_text("#!/bin/sh\nexit 1\n")
            ssh.chmod(0o755)
            for tool in ("uv", "uvx"):
                path = base / tool
                path.write_text("#!/bin/sh\nprintf 'fixture tool\\n'\n")
                path.chmod(0o755)
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
            normal_shell_probe = [
                sys.executable,
                "-c",
                "import os,subprocess\nfrom pathlib import Path\n"
                "host=Path(os.environ['RODE_TEST_SSH_CONFIG']).with_name('host-shell')\n"
                'expected="/fixture/shell with \' quote"\n'
                'env=dict(os.environ,SHELL=expected,SSH_ORIGINAL_COMMAND="printf \'%s\' \\"$SHELL\\"")\n'
                "result=subprocess.run([host],env=env,text=True,capture_output=True)\n"
                "assert result.returncode == 0, result.stderr\n"
                "assert result.stdout == expected, result.stdout\n"
                "env['SSH_ORIGINAL_COMMAND']='env'\n"
                "result=subprocess.run([host],env=env,text=True,capture_output=True,check=True)\n"
                "state=dict(line.split('=',1) for line in result.stdout.splitlines() if '=' in line)\n"
                "assert state['RODE_SESSION_DIR'] == str(host.parent/'home/sessions'), state['RODE_SESSION_DIR']\n"
                "assert state['RODE_VAULT_DIR'] == str(host.parent/'home/vaults'), state['RODE_VAULT_DIR']\n"
                "env['SSH_ORIGINAL_COMMAND']='command -v uv; command -v uvx'\n"
                "result=subprocess.run([host],env=env,text=True,capture_output=True,check=True)\n"
                "for name, value in zip(('uv','uvx'),result.stdout.splitlines(),strict=True):\n"
                " tool=Path(value)\n"
                " assert tool == host.parent/'home/.local/bin'/name, value\n"
                " assert not tool.is_symlink(), value\n"
                " assert tool.stat().st_mode & 0o777 == 0o700, value\n"
                " assert subprocess.check_output([tool],text=True) == 'fixture tool\\n'\n",
            ]
            result = subprocess.run(
                command + normal_shell_probe,
                env=environment | {"RODE_SSH_RUN_TRANSPORT": "0"},
                capture_output=True,
                text=True,
                timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            profile = base / "must-not-source"
            profile.write_text("exit 88\n")
            shell_probe = [
                sys.executable,
                "-c",
                "import os,subprocess\n"
                "shell=os.environ['RODE_TEST_PROJECT_SHELL']\n"
                "result=subprocess.run([shell,'-l'],input='echo FIXTURE_SHELL_OK; exit'+chr(10),text=True,capture_output=True)\n"
                "assert result.returncode == 0, result.stderr\n"
                "assert 'FIXTURE_SHELL_OK' in result.stdout, result.stdout\n",
            ]
            result = subprocess.run(
                command + shell_probe,
                env=environment
                | {
                    "RODE_SSH_RUN_TRANSPORT": "0",
                    "RODE_SSH_TEST_NO_PYTHON": "1",
                    "ENV": str(profile),
                },
                capture_output=True,
                text=True,
                timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
