#!/usr/bin/env python3
"""CI suite manifest, bounded execution, timings and build-artifact publication."""

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import tarfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FAST = ["workspace", "agents", "terminals", "ssh"]
EXTENDED = ["tools", "documents", "remote", "runtimes", "sandbox"]
TEST_BINARIES = {
    "runtimes",
    "ssh_roundtrip",
    "debugger",
    "notebooks",
    "project",
    "languages",
    "sandbox",
    "agents",
    "background_agents",
    "sessions",
}


def ui(name, **env):
    """Describe one isolated native UI command, its environment overrides and 240-second deadline."""
    return ([sys.executable, f"scripts/test-{name}-ui.py"], env, 240)


def rust(name, **env):
    """Describe one prebuilt integration-test executable and its 300-second deadline."""
    return ([f"target/ci/tests/{name}", "--nocapture"], env, 300)


def remote(group, transport=False):
    """Wrap a suite in the private SSH fixture, explicitly selecting whether transport tests run."""
    return (
        ["bash", "scripts/test-ssh.sh", sys.executable, "scripts/ci.py", "run", group],
        {"RODE_SSH_RUN_TRANSPORT": "1" if transport else "0"},
        900,
    )


# Each top-level suite runs in its own job/display. Remote subgroups share only
# their job's private sshd. Local and remote variants are deliberately separate.
SUITES = {
    "workspace": [
        ([sys.executable, "scripts/test-ui-harness.py"], {}, 30),
        ui("project", RODE_PROJECT_UI_CASE="failure"),
        ([sys.executable, "scripts/test-ui.py"], {}, 180),
        ui("workspace"),
        ui("terminal-keys"),
        ui("multiprocess"),
    ],
    "agents": [
        ui("agent"),
        ui("agent-history"),
        ui("agent-queue"),
        ui("agent-attachments"),
        ui("agent-persistence"),
    ],
    "terminals": [ui("sessions"), ui("slurm"), ui("codex-install")],
    "ssh": [remote("remote-smoke", transport=True)],
    "remote-smoke": [ui("project", RODE_PROJECT_UI_CASE="ssh")],
    "tools": [
        ui("environment"),
        ui("layout"),
        ui("languages"),
        ui("syntax-themes"),
        ui("terminal-colors", RODE_TERMINAL_COLORS_UI_PROMPT_ONLY="1"),
        ui("project", RODE_PROJECT_UI_CASE="sandbox"),
        ui("project", RODE_PROJECT_UI_CASE="fallback"),
    ],
    "documents": [
        ui("binary"),
        ui("workbench", RODE_WORKBENCH_UI_CASE="local"),
        ui("images", RODE_IMAGES_UI_CASE="local"),
        ui("markdown", RODE_MARKDOWN_UI_CASE="local"),
        ui("pdf", RODE_PDF_UI_CASE="local"),
        ui("marp"),
        ui("latex"),
        ui("vaults"),
    ],
    "remote": [remote("remote-documents")],
    "remote-documents": [
        ui("workbench", RODE_WORKBENCH_UI_CASE="ssh"),
        ui("images", RODE_IMAGES_UI_CASE="ssh"),
        ui("markdown", RODE_MARKDOWN_UI_CASE="ssh"),
        ui("pdf", RODE_PDF_UI_CASE="ssh"),
        ui("marp", RODE_MARP_UI_REMOTE_ONLY="1"),
        ui("latex", RODE_LATEX_UI_REMOTE_ONLY="1"),
        ui("vaults", RODE_VAULT_UI_REMOTE_ONLY="1"),
    ],
    "runtimes": [
        rust("runtimes"),
        rust("debugger"),
        rust("notebooks"),
        rust("sessions"),
        rust("project"),
        rust("languages"),
        ui("debugger"),
        ui("notebooks"),
        ui("slurm-notebooks"),
        remote("remote-runtimes", transport=True),
    ],
    "remote-runtimes": [
        ui("debugger", RODE_DEBUGGER_UI_REMOTE_ONLY="1"),
        ui("notebooks", RODE_NOTEBOOK_UI_REMOTE_ONLY="1"),
    ],
    "sandbox": [
        rust("sandbox", RODE_TEST_REQUIRE_SANDBOX="1"),
        rust("agents"),
        rust("background_agents"),
        rust("sessions"),
    ],
}


def extended_suites(paths, full=False):
    """Shared execution changes require every extended suite; docs alone do not."""
    if full:
        return EXTENDED
    selected = set()
    for path in paths:
        if path.startswith(("src/", "crates/", "tests/", ".github/")) or path in {
            "Cargo.toml",
            "Cargo.lock",
            "scripts/ci.py",
            "scripts/ci-deps.sh",
            "scripts/test-ci.py",
            "scripts/ui_harness.py",
            "scripts/test-ssh.sh",
        }:
            return EXTENDED
        if path.startswith("scripts/test-"):
            matched = False
            for group in EXTENDED:
                entries = SUITES[group]
                if group == "remote":
                    entries = SUITES["remote-documents"]
                elif group == "runtimes":
                    entries = entries + SUITES["remote-runtimes"]
                if any(path in command for command, _, _ in entries):
                    selected.add(group)
                    matched = True
            if not matched and not any(
                path in command for group in FAST for command, _, _ in SUITES[group]
            ):
                return EXTENDED
        elif path.startswith("scripts/"):
            return EXTENDED
    return [name for name in EXTENDED if name in selected]


def measure(label, command, env=None, timeout=300, group="commands"):
    """Retain failure logs and timings, and stop a timed-out command's children."""
    directory = ROOT / "target/ci-results" / group
    directory.mkdir(parents=True, exist_ok=True)
    logfile = directory / f"{label}.log"
    print(f"::group::{label}: {' '.join(command)}", flush=True)
    start = time.monotonic()
    code = 1
    state = "failed"
    with logfile.open("w") as output:
        try:
            process = subprocess.Popen(
                command,
                cwd=ROOT,
                env=env,
                stdout=output,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
        except OSError as error:
            output.write(f"Could not start {command[0]}: {error}\n")
            code = 127
        else:
            try:
                code = process.wait(timeout=timeout)
                state = "passed" if code == 0 else "failed"
            except (subprocess.TimeoutExpired, KeyboardInterrupt):
                state = "timeout" if time.monotonic() - start >= timeout else "interrupted"
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
                # Descendants may remain even if the immediate parent has exited.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
                code = 124
    elapsed = round(time.monotonic() - start, 2)
    with logfile.open("rb") as output:
        start_output = output.read(12000)
        rest = output.read(12001)
        if len(rest) > 12000:
            output.seek(-12000, os.SEEK_END)
            rest = b"\n... middle omitted; full output is in the log artifact ...\n" + output.read()
        print((start_output + rest).decode(errors="replace"), flush=True)
    print(f"{label}: {state} in {elapsed}s (log: {logfile.relative_to(ROOT)})", flush=True)
    print("::endgroup::", flush=True)
    record = dict(name=label, status=state, seconds=elapsed, command=command)
    with (directory / "timings.jsonl").open("a") as output:
        output.write(json.dumps(record) + "\n")
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a") as output:
            output.write(f"- `{group}/{label}`: **{state}**, {elapsed}s\n")
    if code:
        raise SystemExit(code)


def run_suite(name):
    """Validate the selected bundle tools, isolate host-specific settings and run each command with timing/deadline logs."""
    env = dict(os.environ)
    env.setdefault("RODE_TEST_BINARY", str(ROOT / "target/ci/rode"))
    env.setdefault("RODE_SSH_TEST_BINARY", str(ROOT / "target/ci/tests/ssh_roundtrip"))
    env.setdefault("RODE_TEST_SESSION_HELPER", str(ROOT / "target/ci/rode-sessiond-static"))
    # Transferred Rust tests live in tests/, away from the bundled helper.
    # Service-backed agents/history use application discovery, while direct
    # transport fixtures use RODE_TEST_SESSION_HELPER. Both need the same build.
    env.setdefault("RODE_SESSION_HELPER", env["RODE_TEST_SESSION_HELPER"])
    env.setdefault("RODE_TEST_RUFF", str(ROOT / "target/ci/tools/ruff"))
    env.setdefault("RODE_SESSION_DIR", str(ROOT / "target/ci-results" / name / "sessions"))
    env.setdefault("RODE_CI_SSH_LOG_DIR", str(ROOT / "target/ci-results" / name))
    required = ["RODE_TEST_BINARY", "RODE_TEST_SESSION_HELPER"]
    if name in {"documents", "remote", "remote-documents", "runtimes", "remote-runtimes"}:
        required.append("RODE_TEST_RUFF")
    if "RODE_SESSION_HELPER" in env:
        required.append("RODE_SESSION_HELPER")
    if any("scripts/test-ssh.sh" in command for command, _, _ in SUITES[name]):
        if any(overrides.get("RODE_SSH_RUN_TRANSPORT") == "1" for _, overrides, _ in SUITES[name]):
            required.append("RODE_SSH_TEST_BINARY")
    for variable in required:
        if not os.access(env[variable], os.X_OK):
            raise SystemExit(
                f"Missing executable {variable}={env[variable]}; build the CI bundle first"
            )
    # Do not accidentally add remote cases to a local-only shard.
    if name in FAST + EXTENDED:
        env.pop("RODE_TEST_SSH_CONFIG", None)
    for index, (command, overrides, timeout) in enumerate(SUITES[name], 1):
        measure(f"{index:02d}-{Path(command[0]).name}", command, env | overrides, timeout, name)


def bundle():
    """Use Cargo's executable paths, rather than guessing hashes from a stale cache."""
    destination = ROOT / "target/ci/tests"
    destination.mkdir(parents=True, exist_ok=True)
    found = set()
    for line in (ROOT / "target/ci-test-binaries.json").read_text().splitlines():
        entry = json.loads(line)
        target = entry.get("target", {})
        name = target.get("name")
        if target.get("kind") == ["test"] and name in TEST_BINARIES and entry.get("executable"):
            shutil.copy2(entry["executable"], destination / name)
            found.add(name)
    if found != TEST_BINARIES:
        raise SystemExit(f"Missing test artifacts: {sorted(TEST_BINARIES - found)}")
    shutil.copy2(
        ROOT / "target/release/rode-sessiond-static", ROOT / "target/ci/rode-sessiond-static"
    )
    tools = ROOT / "target/ci/tools"
    tools.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / "target/test-tools/bin/ruff", tools / "ruff")
    with tarfile.open(ROOT / "target/ci-bundle.tar.gz", "w:gz", compresslevel=1) as archive:
        for name in ["rode", "rode-sessiond", "rode-sessiond-static", "tests", "tools"]:
            archive.add(ROOT / "target/ci" / name, arcname=f"target/ci/{name}")


def main():
    """Dispatch suite inspection, PR selection, bundle publication or one bounded command from the CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    run = commands.add_parser("run")
    run.add_argument("suite", choices=SUITES)
    matrix = commands.add_parser("matrix")
    matrix.add_argument("--base")
    matrix.add_argument("--full", action="store_true")
    commands.add_parser("bundle")
    commands.add_parser("list")
    timed = commands.add_parser("time")
    timed.add_argument("label")
    timed.add_argument("--timeout", type=int, default=600)
    timed.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.action == "run":
        run_suite(args.suite)
    elif args.action == "bundle":
        bundle()
    elif args.action == "matrix":
        if not args.base and not args.full:
            parser.error("matrix needs --base or --full")
        paths = (
            []
            if args.full
            else subprocess.check_output(
                ["git", "diff", "--name-only", f"{args.base}...HEAD"],
                cwd=ROOT,
                text=True,
            ).splitlines()
        )
        print(json.dumps(extended_suites(paths, args.full)))
    elif args.action == "list":
        print(json.dumps({name: SUITES[name] for name in FAST + EXTENDED}, indent=2))
    else:
        command = args.command
        if command and command[0] == "--":
            command = command[1:]
        if not command:
            parser.error("time needs a command")
        measure(args.label, command, timeout=args.timeout)


if __name__ == "__main__":
    main()
