#!/usr/bin/env python3
"""Run native Rust tests with a private home and runtime directories per binary.

No arguments runs the ordinary locked CI workspace suite. Arguments replace the
default Cargo test options, so focused checks use the same isolated fixtures.
Cargo invokes the internal --runner entry point for each built test executable.
Toolchain locations remain captured from the invoking environment; application
preferences, caches, session services and vault leases belong to the test home.
"""

import hashlib
import json
import os
import subprocess
import sys
from pathlib import Path

import ci_jobs
from fixture_support import fixture_directory, private_directory

ROOT = Path(__file__).resolve().parent.parent
DEFAULT = ["--locked", "--profile", "ci", "--workspace", "--all-targets", "--no-fail-fast"]


def test_commands(output):
    """Collect Cargo's exact invocations, including runtime env and crate cwd.

    The collector runs instead of each test during Cargo's planning pass. Its
    environment stays in memory: never publish the raw JSON stream, since
    inherited variables can contain credentials. Real test outcomes below
    determine success; a successful planning pass is never a test result.
    """
    executables = {}
    commands = []
    for line in output.splitlines():
        if not line.strip():
            continue
        record = json.loads(line)
        if not isinstance(record, dict):
            raise ValueError("Expected a Cargo JSON object")
        if record.get("rode_test_command") == 1:
            commands.append(record)
            continue
        if record.get("reason") == "compiler-message":
            rendered = record.get("message", {}).get("rendered")
            if rendered:
                print(rendered, end="", file=sys.stderr)
        if record.get("reason") != "compiler-artifact" or not record.get("profile", {}).get("test"):
            continue
        executable = record.get("executable")
        if not executable:
            continue
        target = record["target"]
        identity = hashlib.sha256(record["package_id"].encode()).hexdigest()[:8]
        name = f"{target['name']}-{'-'.join(target['kind'])}-{identity}"
        executables[executable] = (name, target["kind"])
    if not commands:
        raise ValueError("Cargo reported no test invocations; refusing empty coverage")
    for command in commands:
        argv = command["argv"]
        if not isinstance(argv, list) or not argv or not all(isinstance(a, str) for a in argv):
            raise ValueError("Invalid Cargo test arguments")
        if argv[0] not in executables:
            raise ValueError("Cargo invoked a test outside its current build")
        command["name"], command["kind"] = executables[argv[0]]
        if (
            not isinstance(command["env"], dict)
            or not all(isinstance(k, str) and isinstance(v, str) for k, v in command["env"].items())
            or not Path(command["cwd"]).is_absolute()
        ):
            raise ValueError("Incomplete Cargo runtime context")
    if {command["argv"][0] for command in commands} != set(executables):
        raise ValueError(
            "Cargo did not invoke every built test target; refusing incomplete coverage"
        )
    return sorted(commands, key=lambda item: (item["kind"] != ["test"], item["name"]))


def workspace_tests():
    """Build once and run independent test targets with separate private homes.

    Test functions still use libtest's own threading; only independent binaries
    overlap. Real lease timeouts and every default Cargo target remain covered.
    Focused Cargo commands below retain their exact argument/filter semantics.
    """
    version = subprocess.check_output(["rustc", "-vV"], text=True)
    host = next(
        line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: ")
    )
    collector = [sys.executable, str(Path(__file__).resolve()), "--collect-command"]
    build = subprocess.run(
        [
            "cargo",
            "test",
            "--config",
            f"target.{host}.runner={json.dumps(collector)}",
            *DEFAULT,
            "--message-format=json",
        ],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        targets = test_commands(build.stdout)
    except (ValueError, KeyError, TypeError) as error:
        print(f"Cannot discover the complete Rust test build: {error}", file=sys.stderr)
        return build.returncode or 1
    if build.returncode:
        return build.returncode
    jobs = ci_jobs.default_jobs()
    configured = os.environ.get("RODE_TEST_JOBS")
    if configured is not None:
        try:
            jobs = int(configured)
        except ValueError:
            raise SystemExit("RODE_TEST_JOBS must be an integer between 1 and 64") from None
        if not 1 <= jobs <= 64:
            raise SystemExit("RODE_TEST_JOBS must be between 1 and 64")
    commands = [
        ci_jobs.Job(
            f"{index:02d}-{target['name']}",
            [sys.executable, str(Path(__file__).resolve()), "--runner", *target["argv"]],
            target["env"],
            timeout=600,
            group="rust-tests",
            cwd=target["cwd"],
        )
        for index, target in enumerate(targets, 1)
    ]
    if jobs > 1:
        # Avoid multiplying libtest's machine-wide default across every binary.
        # Explicit caller settings remain authoritative.
        threads = max(1, min(8, ci_jobs.available_cpus() // jobs))
        for command in commands:
            command.env.setdefault("RUST_TEST_THREADS", str(threads))
    results = ci_jobs.run(commands, jobs)
    return int(any(result.exit_code for result in results))


def isolated(command):
    """Execute one test binary without changing the parent process environment."""
    # Checkout paths can exceed the Unix-domain socket limit before a test has
    # even created its nested SSH agent/control socket. Keep the fixture short
    # independently of the checkout; callers can select an executable tmpfs.
    with fixture_directory(prefix="rt-", awkward=False) as fixture_home:
        environment = dict(os.environ)
        original_home = Path.home()
        virtualenv = environment.pop("VIRTUAL_ENV", None)
        implicit_bins = {os.path.abspath(ROOT / ".venv/bin")}
        if virtualenv:
            implicit_bins.add(os.path.abspath(Path(virtualenv) / "bin"))
        environment["PATH"] = os.pathsep.join(
            entry
            for entry in environment.get("PATH", "").split(os.pathsep)
            if os.path.abspath(entry) not in implicit_bins
        )
        for name, fallback in (
            ("CARGO_HOME", original_home / ".cargo"),
            ("RUSTUP_HOME", original_home / ".rustup"),
        ):
            environment.setdefault(name, str(fallback))
        for name in (".cache", "config", "data", "run", "t"):
            private_directory(fixture_home / name)
        environment.update(
            HOME=str(fixture_home),
            XDG_CACHE_HOME=str(fixture_home / ".cache"),
            XDG_CONFIG_HOME=str(fixture_home / "config"),
            XDG_DATA_HOME=str(fixture_home / "data"),
            XDG_RUNTIME_DIR=str(fixture_home / "run"),
            TMPDIR=str(fixture_home / "t"),
            RODE_SESSION_DIR=str(fixture_home / "run/sessions"),
            RODE_VAULT_DIR=str(fixture_home / "run/vaults"),
        )
        environment.pop("CODEX_HOME", None)
        # Library tests install their own per-context probes. An inherited native
        # window destination would replace those hooks and mix parallel snapshots.
        environment.pop("RODE_UI_TEST_STATE", None)
        return subprocess.call(command, env=environment)


def main(arguments=None):
    """Install a structured native Cargo runner, or execute its isolated child."""
    arguments = sys.argv[1:] if arguments is None else arguments
    if arguments[:1] == ["--collect-command"]:
        if len(arguments) < 2:
            raise SystemExit("--collect-command requires Cargo's test executable")
        print(
            json.dumps(
                dict(rode_test_command=1, argv=arguments[1:], cwd=os.getcwd(), env=dict(os.environ))
            )
        )
        return 0
    if arguments[:1] == ["--runner"]:
        if len(arguments) == 1:
            raise SystemExit("--runner requires a test executable")
        return isolated(arguments[1:])
    if not arguments:
        return workspace_tests()
    version = subprocess.check_output(["rustc", "-vV"], text=True)
    host = next(
        line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: ")
    )
    runner = [sys.executable, str(Path(__file__).resolve()), "--runner"]
    return subprocess.call(
        [
            "cargo",
            "test",
            "--config",
            f"target.{host}.runner={json.dumps(runner)}",
            *arguments,
        ],
        cwd=ROOT,
    )


if __name__ == "__main__":
    raise SystemExit(main())
