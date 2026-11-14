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
import validation_report
from test_capabilities import inventory, read_skip_records

ROOT = Path(__file__).resolve().parent.parent


def publish_tests(logfile):
    """Publish Cargo's reported test executables into the repository CI runner directory."""
    ci.publish_tests(logfile, ROOT / "target/ci/tests")


def environment():
    """Capture explicit tool overrides and defaults for the validation run.

    Remove inherited enforcement requirements: each strict suite declares its
    own required capabilities without changing unrelated suites' coverage.
    """
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
    """Run selected validation phases and retain failures plus source/toolchain identities.

    Preflight reports capabilities without building or testing. Normal runs
    attempt each selected suite and keep earlier failures across repetitions.
    """
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
    source_before = validation_report.source_identity(ROOT)
    source_after = None
    artifacts_before = None
    artifacts_after = None
    tools = validation_report.toolchain(env)
    tools_after = None
    output = (
        ROOT / "target/ci-results" / (time.strftime("validation-%Y%m%d-%H%M%S-") + str(os.getpid()))
    )
    output.mkdir(parents=True)
    os.environ["RODE_CI_RESULTS_DIR"] = str(output)
    env["RODE_CI_RESULTS_DIR"] = str(output)
    (output / "capabilities.json").write_text(json.dumps(capabilities, indent=2) + "\n")
    results = []
    ui_runs = []

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
        skipped, metadata_errors = [], []
        for path, record, error in read_skip_records(output.rglob("*-skips/*.json")):
            location = str(path.relative_to(output))
            if error:
                metadata_errors.append(dict(report=location, error=error))
            else:
                skipped.append(record | {"report": location})
        source_unchanged = (
            source_before == source_after
            if source_before.get("available") and source_after is not None
            else None
        )
        artifacts_unchanged = (
            artifacts_before == artifacts_after if artifacts_after is not None else None
        )
        toolchain_unchanged = tools == tools_after if tools_after is not None else None
        comparable = (
            source_unchanged is True and artifacts_unchanged is True and toolchain_unchanged is True
        )
        repeated = validation_report.repeats(output, ui_runs, comparable=comparable)
        records_complete = not metadata_errors and not any(
            row["counts"].get("not-run", 0) for row in repeated
        )
        report = dict(
            schema_version=2,
            finished=finished,
            results=results,
            capabilities=capabilities,
            skipped=skipped,
            metadata_errors=metadata_errors,
            commands_passed=all(item["exit_code"] == 0 for item in results) and records_complete,
            records_complete=records_complete,
            unavailable_capabilities=[
                name for name, item in capabilities.items() if not item["available"]
            ],
            phases=args.phases,
            suites=args.suite or ci.FAST + ci.EXTENDED,
            repeat=args.repeat,
            stress=args.stress,
            provenance=dict(
                source_before=source_before,
                source_after=source_after,
                source_unchanged=source_unchanged,
                artifacts_before=artifacts_before,
                artifacts_after=artifacts_after,
                artifacts_unchanged=artifacts_unchanged,
                toolchain=tools,
                toolchain_after=tools_after,
                toolchain_unchanged=toolchain_unchanged,
            ),
            repeat_results=repeated,
            ui_runs=ui_runs,
            exclusions=["Release packaging", "Opt-in upstream downloads", "Manual benchmarks"],
        )
        (output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
        return report

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
        artifacts_before = validation_report.artifact_identity(ROOT, env, ci.TEST_BINARIES)
        for iteration in range(1, args.repeat + 1):
            for suite in args.suite or ci.FAST + ci.EXTENDED:
                label = f"{iteration:02d}-{suite}"
                mask = (0o002, 0o077, 0o022)[(iteration - 1) % 3] if args.stress else None
                ui_runs.append(
                    dict(
                        name=label,
                        suite=suite,
                        iteration=iteration,
                        condition=f"stress-umask-{mask:03o}" if args.stress else "baseline",
                        planned=[label for label, *_ in ci.suite_cases(suite)],
                    )
                )
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
                    mask=mask,
                )
        artifacts_after = validation_report.artifact_identity(ROOT, env, ci.TEST_BINARIES)
    source_after = validation_report.source_identity(ROOT)
    tools_after = validation_report.toolchain(env)
    if source_before.get("available") and source_after != source_before:
        results.append(
            dict(
                name="source-changed",
                exit_code=1,
                error="Checkout changed during validation; repeat results are not comparable.",
            )
        )
    if artifacts_after is not None and artifacts_after != artifacts_before:
        results.append(
            dict(
                name="artifacts-changed",
                exit_code=1,
                error="Test binaries changed during validation; repeat results are not comparable.",
            )
        )
    if tools_after != tools:
        results.append(
            dict(
                name="toolchain-changed",
                exit_code=1,
                error="Tool versions changed during validation; repeat results are not comparable.",
            )
        )
    report = write_report(finished=True)
    if ui_runs:
        summary = validation_report.repeat_summary(report["repeat_results"])
        (output / "repeats.md").write_text(summary)
        if path := os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(path, "a") as destination:
                destination.write("\n" + summary)
    print(f"Validation report: {output / 'summary.json'}", flush=True)
    if not report["records_complete"]:
        print("Incomplete case records; unexecuted or unreadable evidence cannot pass validation.")
    return int(not report["commands_passed"])


if __name__ == "__main__":
    raise SystemExit(main())
