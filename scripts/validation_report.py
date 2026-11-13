"""Validation provenance and command-level repeat outcomes; never retry or erase a failure."""

import hashlib
import json
import os
import platform
import shutil
import stat
import subprocess
import sys
from collections import Counter
from pathlib import Path


def file_digest(path):
    """Hash bytes without retaining executable contents in the report or memory."""
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def source_identity(root):
    """Include dirty/untracked source and executable bits, excluding Git-ignored artifacts.

    Report only aggregate hashes, never diffs, source contents or arbitrary environment.
    An unavailable Git checkout makes repeat comparability unknown.
    """

    def git(*args):
        return subprocess.check_output(["git", *args], cwd=root, stderr=subprocess.PIPE, timeout=10)

    try:
        head = git("rev-parse", "HEAD").decode().strip()
        paths = sorted(
            set(git("ls-files", "-z", "--cached", "--others", "--exclude-standard").split(b"\0"))
            - {b""}
        )
        digest = hashlib.sha256()
        for name in paths:
            path = Path(root) / os.fsdecode(name)
            digest.update(len(name).to_bytes(8, "big") + name)
            try:
                mode = path.lstat().st_mode
            except FileNotFoundError:
                digest.update(b"missing")
                continue
            digest.update(str(stat.S_IFMT(mode) | (mode & 0o111)).encode() + b"\0")
            if stat.S_ISLNK(mode):
                digest.update(os.fsencode(os.readlink(path)))
            elif stat.S_ISREG(mode):
                digest.update(file_digest(path).encode())
            else:
                # Do not follow directories, devices or substituted FIFOs.
                return dict(available=False, reason="Unsupported source entry type")
        dirty = bool(git("status", "--porcelain", "--untracked-files=normal"))
        return dict(
            available=True, head=head, sha256=digest.hexdigest(), dirty=dirty, files=len(paths)
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        return dict(available=False, reason=str(error))


def artifact_identity(root, environment, test_binaries):
    """Identify supplied binaries, including prebuilt test executables, without claiming build provenance."""
    paths = {
        key: Path(environment[key])
        for key in (
            "RODE_TEST_BINARY",
            "RODE_TEST_SESSION_HELPER",
            "RODE_HOST_HELPER",
            "RODE_SESSION_HELPER",
            "RODE_TEST_SSH_CLIENT",
            "RODE_TEST_RUFF",
            "RODE_TEST_MISE",
            "RODE_TEST_JUPYTER",
            "RODE_TEST_DEBUGPY",
            "RODE_TEST_LLDB",
            "RODE_TEST_PRETTIER",
        )
        if environment.get(key)
    }
    paths.update({"test:" + name: Path(root) / "target/ci/tests" / name for name in test_binaries})
    results, digests = {}, {}
    for name, path in sorted(paths.items()):
        try:
            resolved = path.resolve(strict=True)
            if resolved not in digests:
                digests[resolved] = file_digest(resolved)
            results[name] = dict(path=str(path), sha256=digests[resolved])
        except OSError as error:
            results[name] = dict(path=str(path), unavailable=str(error))
    return results


def toolchain(environment):
    """Capture bounded versions, using stderr only for tools without a stdout version.

    Maintenance/update warnings on stderr do not identify a different toolchain.
    Stderr-only version commands such as OpenSSH still retain their identity.
    """
    commands = {
        "python": [sys.executable, "--version"],
        "rustc": ["rustc", "-Vv"],
        "cargo": ["cargo", "--version"],
        "ssh": ["ssh", "-V"],
    }
    for name, variable, fallback in (
        ("mise", "RODE_TEST_MISE", "mise"),
        ("ruff", "RODE_TEST_RUFF", "ruff"),
        ("lldb", "RODE_TEST_LLDB", None),
        ("prettier", "RODE_TEST_PRETTIER", None),
    ):
        path = environment.get(variable) or (
            shutil.which(fallback, path=environment.get("PATH")) if fallback else None
        )
        if path:
            commands[name] = [path, "--version"]
    for name, variable, packages in (
        ("jupyter", "RODE_TEST_JUPYTER", ("jupyter-client", "ipykernel", "matplotlib")),
        ("debugpy", "RODE_TEST_DEBUGPY", ("debugpy",)),
    ):
        python = environment.get(variable) or (
            environment.get("RODE_TEST_JUPYTER") if name == "debugpy" else None
        )
        if python:
            commands[name] = [
                python,
                "-c",
                f"import importlib.metadata as m; print({{p: m.version(p) for p in {packages!r}}})",
            ]
    results = {}
    for name, command in commands.items():
        try:
            result = subprocess.run(
                command, env=environment, capture_output=True, text=True, timeout=5
            )
            results[name] = dict(
                command=command,
                exit_code=result.returncode,
                version=(result.stdout.strip() or result.stderr.strip())[:2000],
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            results[name] = dict(command=command, unavailable=str(error))
    return dict(
        platform=platform.platform(),
        runner={
            key: environment[key]
            for key in ("ImageOS", "ImageVersion", "RUNNER_OS", "RUNNER_ARCH")
            if key in environment
        },
        tools=results,
    )


def repeats(output, runs, *, comparable):
    """Group the same case/condition across attempts; missing coverage never enters a pass denominator.

    Rates describe command failures, not individual assertions or skipped features.
    Mixed outcomes are only candidates for investigation, never proof of a false alarm.
    """

    def read_record(path):
        try:
            record = json.loads(path.read_text())
            if not isinstance(record, dict):
                raise ValueError("Expected a JSON object")
            return record, None
        except (OSError, ValueError) as error:
            # A deadline may kill a child halfway through publishing metadata.
            # Preserve other attempts and explicitly mark this evidence missing.
            return {}, str(error)

    def valid_labels(value):
        return (
            isinstance(value, list)
            and all(
                isinstance(label, str) and label and Path(label).name == label for label in value
            )
            and len(value) == len(set(value))
        )

    grouped = {}
    for run in runs:
        parent = output / run["name"] / run["suite"] / "suite.json"
        # Child suites cannot substitute for a missing top-level suite record.
        paths = sorted(set((output / run["name"]).glob("*/suite.json")) | {parent})
        for path in paths:
            suite, suite_error = read_record(path)

            if not (
                suite.get("suite") == path.parent.name
                and valid_labels(suite.get("planned"))
                and valid_labels(suite.get("completed"))
                and isinstance(suite.get("status"), str)
                and suite.get("status") in {"running", "blocked", "failed", "passed", "interrupted"}
            ):
                suite_error = suite_error or "Incomplete suite metadata"
            elif not set(suite["completed"]).issubset(suite["planned"]):
                suite_error = "Completed cases are outside the suite plan"
            elif path == parent and suite["planned"] != run["planned"]:
                suite_error = "Suite plan does not match the expected cases"
            if suite_error:
                suite = dict(
                    suite=path.parent.name,
                    status="not-run",
                    planned=run["planned"] if path == parent else ["suite-metadata"],
                    completed=[],
                )
            completed = set(suite.get("completed", []))
            for label in suite.get("planned", []):
                key = (suite["suite"] + "/" + label, run["condition"])
                record_path = path.parent / (label + ".json")
                error = suite_error
                if label in completed and record_path.is_file():
                    record, error = read_record(record_path)
                    status = record.get("status", "not-run")
                    if not isinstance(status, str) or status not in {
                        "passed",
                        "passed-with-skips",
                        "failed",
                        "timeout",
                        "interrupted",
                    }:
                        error = error or "Unknown command status"
                        status = "not-run"
                elif suite["status"] == "blocked":
                    status = "blocked"
                else:
                    status = "not-run"
                grouped.setdefault(key, []).append(
                    dict(
                        iteration=run["iteration"],
                        status=status,
                        **({"error": error} if error else {}),
                        report=str(record_path.relative_to(output))
                        if label in completed and record_path.is_file()
                        else str(path.relative_to(output)),
                    )
                )
    results = []
    for (case, condition), attempts in sorted(grouped.items()):
        counts = Counter(attempt["status"] for attempt in attempts)
        passed = counts["passed"] + counts["passed-with-skips"]
        failed = counts["failed"] + counts["timeout"]
        executed = passed + failed
        mixed = bool(passed and failed)
        results.append(
            dict(
                case=case,
                condition=condition,
                attempts=attempts,
                counts=dict(counts),
                executed=executed,
                command_failure_rate=failed / executed if executed and comparable else None,
                mixed_outcomes=mixed,
                intermittent_failure_candidate=bool(comparable and mixed),
            )
        )
    return results


def repeat_summary(rows):
    """Render observed samples with their denominators and coverage limits."""
    lines = [
        "## Repeated validation commands",
        "",
        "Rates count executed commands. Blocked/unexecuted cases are excluded; optional skips remain separate.",
        "Mixed outcomes require investigation and do not establish that a failure is a false positive.",
        "",
        "| Case | Condition | Passed | With skips | Failed/timeout | Blocked/not run | Failure rate |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in rows:
        counts = Counter(row["counts"])
        rate = row["command_failure_rate"]
        rate = f"{rate:.1%}" if rate is not None else "unavailable"
        values = [
            row["case"],
            row["condition"],
            counts["passed"],
            counts["passed-with-skips"],
            counts["failed"] + counts["timeout"],
            counts["blocked"] + counts["not-run"] + counts["interrupted"],
            rate,
        ]
        lines.append("| " + " | ".join(str(value).replace("|", r"\|") for value in values) + " |")
    return "\n".join(lines) + "\n"
