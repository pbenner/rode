#!/usr/bin/env python3
"""Build and validate the checkout with isolated displays and a retained coverage report.

No dependencies are installed. --repeat repeats every selected UI suite; an
earlier failure is never erased by a later pass. --stress varies umasks, fixture
paths and native frame scheduling. Release packaging and upstream-download
tests remain separate, explicit workflows.
"""

import argparse
import json
import os
import shutil
import sys
import time
from pathlib import Path

import ci
from test_capabilities import inventory

ROOT = Path(__file__).resolve().parent.parent


def publish_tests(logfile):
    ci.publish_tests(logfile, ROOT / "target/ci/tests")


def environment():
    env = dict(os.environ)
    for key in (
        "RODE_TEST_REQUIRE_SANDBOX",
        "RODE_TEST_REQUIRE_LANDLOCK",
        "RODE_TEST_REQUIRED_CAPABILITIES",
    ):
        env.pop(key, None)
    env.setdefault("RODE_TEST_BINARY", str(ROOT / "target/ci/rode"))
    env.setdefault("RODE_TEST_SESSION_HELPER", str(ROOT / "target/ci/rode-sessiond-test"))
    env.setdefault("RODE_SESSION_HELPER", env["RODE_TEST_SESSION_HELPER"])
    env.setdefault("RODE_HOST_HELPER", env["RODE_TEST_SESSION_HELPER"])
    env.setdefault("RODE_TEST_RUFF", shutil.which("ruff") or str(ROOT / "target/ci/tools/ruff"))
    return env


def publish_helper():
    """Snapshot the build with explicit permissions, without modifying Cargo's executable."""
    source = ROOT / "target/ci/rode-sessiond"
    destination = source.with_name("rode-sessiond-test")
    ci.publish_executable(source, destination)


def main(arguments=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", action="append", choices=ci.FAST + ci.EXTENDED + ["reliability"])
    parser.add_argument(
        "--phases",
        nargs="+",
        choices=("lint", "build", "rust", "ui"),
        default=["lint", "build", "rust", "ui"],
    )
    parser.add_argument("--repeat", type=int, default=1)
    parser.add_argument("--stress", action="store_true")
    parser.add_argument(
        "--preflight", action="store_true", help="Print availability without starting tests"
    )
    args = parser.parse_args(arguments)
    if not 1 <= args.repeat <= 100:
        parser.error("--repeat must be between 1 and 100")
    env = environment()
    capabilities = inventory(env)
    if args.preflight:
        print(json.dumps(capabilities, indent=2))
        return 0
    output = (
        ROOT / "target/ci-results" / (time.strftime("validation-%Y%m%d-%H%M%S-") + str(os.getpid()))
    )
    output.mkdir(parents=True)
    os.environ["RODE_CI_RESULTS_DIR"] = str(output)
    env["RODE_CI_RESULTS_DIR"] = str(output)
    (output / "capabilities.json").write_text(json.dumps(capabilities, indent=2) + "\n")
    results = []

    def run(label, command, *, overrides=None, timeout=900, mask=None):
        try:
            ci.measure(label, command, env | (overrides or {}), timeout, umask=mask)
            code = 0
        except SystemExit as error:
            code = error.code
        results.append(dict(name=label, exit_code=code))
        write_report()
        if code in (130, 143):
            raise SystemExit(code)
        return code == 0

    def write_report(*, finished=False):
        skipped = []
        for path in output.rglob("*-skips/*.json"):
            skipped.append(json.loads(path.read_text()) | {"report": str(path.relative_to(output))})
        report = dict(
            schema_version=1,
            finished=finished,
            results=results,
            capabilities=capabilities,
            skipped=skipped,
            commands_passed=all(item["exit_code"] == 0 for item in results),
            unavailable_capabilities=[
                name for name, item in capabilities.items() if not item["available"]
            ],
            phases=args.phases,
            suites=args.suite or ci.FAST + ci.EXTENDED,
            repeat=args.repeat,
            stress=args.stress,
            exclusions=["Release packaging", "Opt-in upstream downloads", "Manual benchmarks"],
        )
        (output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")

    if "lint" in args.phases:
        commands = [
            ("rust-format", ["cargo", "fmt", "--all", "--check"]),
            ("python-lint", [env["RODE_TEST_RUFF"], "check", "."]),
            ("python-format", [env["RODE_TEST_RUFF"], "format", "--check", "."]),
            *[
                (name, [sys.executable, f"scripts/test-{name}.py"])
                for name in (
                    "ci",
                    "validation",
                    "rust-tests",
                    "fixture-support",
                    "ssh-fixture",
                    "packaging",
                )
            ],
            (
                "clippy",
                [
                    "cargo",
                    "clippy",
                    "--locked",
                    "--profile",
                    "ci",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
            (
                "rust-doc",
                [
                    "cargo",
                    "doc",
                    "--locked",
                    "--profile",
                    "ci",
                    "--workspace",
                    "--no-deps",
                    "--document-private-items",
                ],
            ),
        ]
        for label, command in commands:
            run(label, command, overrides={"RUSTDOCFLAGS": "-D warnings"})
    built = True
    if "build" in args.phases:
        built = run(
            "test-build",
            [
                "cargo",
                "test",
                "--locked",
                "--profile",
                "ci",
                "--workspace",
                "--all-targets",
                "--no-run",
                "--message-format=json",
            ],
            timeout=1200,
        )
        if built:
            try:
                publish_tests(output / "commands/test-build.log")
            except (OSError, ValueError) as error:
                print(f"Cannot publish test executables: {error}", file=sys.stderr)
                results.append(dict(name="publish-tests", exit_code=1, error=str(error)))
                write_report()
                built = False
        built = (
            run(
                "editor-build",
                ["cargo", "build", "--locked", "--profile", "ci", "--bins"],
                timeout=1200,
            )
            and built
        )
    if (
        built
        and any(
            env.get(name) == str(ROOT / "target/ci/rode-sessiond-test")
            for name in ("RODE_TEST_SESSION_HELPER", "RODE_SESSION_HELPER", "RODE_HOST_HELPER")
        )
        and any(phase in args.phases for phase in ("rust", "ui"))
    ):
        try:
            publish_helper()
        except OSError as error:
            results.append(dict(name="publish-helper", exit_code=1, error=str(error)))
            print(f"Cannot publish helper: {error}", file=sys.stderr)
            built = False
    if built and "build" in args.phases:
        # A fresh build can supply the bundled sandbox launcher. Do not retain
        # a missing-helper preflight result after publishing that executable.
        capabilities = inventory(env)
        (output / "capabilities.json").write_text(json.dumps(capabilities, indent=2) + "\n")
    if "rust" in args.phases and built:
        run("rust", [sys.executable, "scripts/rust_tests.py"])
        run(
            "doctests",
            [
                sys.executable,
                "scripts/rust_tests.py",
                "--locked",
                "--profile",
                "ci",
                "--workspace",
                "--doc",
            ],
        )
    if "ui" in args.phases and built:
        for iteration in range(1, args.repeat + 1):
            for suite in args.suite or ci.FAST + ci.EXTENDED:
                label = f"{iteration:02d}-{suite}"
                overrides = {"RODE_CI_RESULTS_DIR": str(output / label)}
                if args.stress:
                    overrides.update(RODE_TEST_AWKWARD_PATHS="1", RODE_TEST_SLOW_UI="1")
                run(
                    label,
                    [
                        sys.executable,
                        "scripts/ui_display.py",
                        "--",
                        sys.executable,
                        "scripts/ci.py",
                        "run",
                        suite,
                    ],
                    overrides=overrides,
                    timeout=3600,
                    mask=(0o002, 0o077, 0o022)[(iteration - 1) % 3] if args.stress else None,
                )
    write_report(finished=True)
    print(f"Validation report: {output / 'summary.json'}", flush=True)
    return int(any(result["exit_code"] != 0 for result in results))


if __name__ == "__main__":
    raise SystemExit(main())
