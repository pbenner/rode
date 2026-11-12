#!/usr/bin/env python3
"""Run native Rust tests with a private home and runtime directories per binary.

No arguments runs the ordinary locked CI workspace suite. Arguments replace the
default Cargo test options, so focused checks use the same isolated fixtures.
Cargo invokes the internal --runner entry point for each built test executable.
Toolchain locations remain captured from the invoking environment; application
preferences, caches, session services and vault leases belong to the test home.
"""

import json
import os
import subprocess
import sys
from pathlib import Path

from fixture_support import fixture_directory, private_directory

ROOT = Path(__file__).resolve().parent.parent
DEFAULT = ["--locked", "--profile", "ci", "--workspace", "--all-targets", "--no-fail-fast"]


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
    if arguments[:1] == ["--runner"]:
        if len(arguments) == 1:
            raise SystemExit("--runner requires a test executable")
        return isolated(arguments[1:])
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
            *(arguments or DEFAULT),
        ],
        cwd=ROOT,
    )


if __name__ == "__main__":
    raise SystemExit(main())
