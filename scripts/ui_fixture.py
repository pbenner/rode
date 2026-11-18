"""Owned native UI sessions shared by document fixtures.

Fixtures supply their private environment and project data. This module owns
only the launched process, its window, and its diagnostic artifacts.
"""

import contextlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    close_window,
    focus_window,
    screenshot,
    stop_process,
    wait_for_project,
    wait_for_window,
)


def project_args(project, *, remote=False, sandbox=False):
    """Build a local or fixture-SSH launch command with explicit theme and sandbox choice.

    Remote launches require RODE_TEST_SSH_CONFIG and its rode-test alias;
    the project path is interpreted on the selected host.
    """
    args = [str(BINARY), "--theme", "carbon"]
    if remote:
        args += [
            "--ssh",
            "rode-test",
            "--ssh-config",
            os.environ["RODE_TEST_SSH_CONFIG"],
            "--remote-path",
            str(project),
        ]
    else:
        args.append(str(project))
    if sandbox:
        args.append("--sandbox")
    return args


def project_ui(process, environment, project, log_path):
    """Attach observations only after this process has mapped and prepared its project."""
    window = wait_for_window(process, log_path)
    wait_for_project(
        window, process, log_path, project, state_path=environment["RODE_UI_TEST_STATE"]
    )
    focus_window(window)
    ui = NativeUI(window, process, log_path, Path(environment["RODE_UI_TEST_STATE"]), project)
    ui.layout()
    return ui


def capture_failure(label, process, window=None, *, files=(), observations=()):
    """Retain independent, read-only evidence without replacing the scenario failure.

    Each (source, suffix) file and (name, callable) observation is independent of
    screenshot capture. Callers must bound any observation that performs I/O.
    Console excerpts are bounded; the manifest records missing or failed captures
    as well as saved paths, so a missing PNG has an explicit explanation.
    """
    report = dict(pid=process.pid, exit_code=process.poll(), files={}, observations={})

    def excerpt(name, value):
        print(f"{label} failure {name}:\n{value[-16 * 1024 :]}", file=sys.stderr, flush=True)

    for source, suffix in files:
        destination = ARTIFACTS / f"{label}-failure{suffix}"
        try:
            shutil.copyfile(source, destination)
            report["files"][suffix] = dict(path=str(destination))
            with destination.open("rb") as contents:
                contents.seek(0, os.SEEK_END)
                contents.seek(max(0, contents.tell() - 16 * 1024))
                excerpt(suffix, contents.read().decode("utf-8", errors="replace"))
        except Exception as error:
            report["files"][suffix] = dict(error=str(error))

    for name, observe in observations:
        try:
            value = observe()
            excerpt(name, json.dumps(value, ensure_ascii=True, indent=2))
            report["observations"][name] = dict(value=value)
        except Exception as error:
            report["observations"][name] = dict(error=str(error))

    if window is None or report["exit_code"] is not None:
        report["screenshot"] = dict(skipped="No live, mapped fixture window")
    else:
        try:
            screenshot(window, f"{label}-failure.png")
            report["screenshot"] = dict(path=str(ARTIFACTS / f"{label}-failure.png"))
        except Exception as error:
            report["screenshot"] = dict(error=str(error))

    manifest = ARTIFACTS / f"{label}-failure-diagnostics.json"
    try:
        manifest.write_text(json.dumps(report, indent=2) + "\n")
    except Exception as error:
        print(f"Cannot write {manifest}: {error}", file=sys.stderr, flush=True)
    excerpt(f"artifacts ({manifest})", json.dumps(report, ensure_ascii=True, indent=2))
    return report


@contextlib.contextmanager
def native_session(args, environment, project, label):
    """Wait for the owning window/project and retain diagnostics even on startup failure.

    A successful scenario must close normally, including application shutdown.
    Failure cleanup never retries UI actions or replaces the original exception.
    """
    state = Path(environment["RODE_UI_TEST_STATE"])
    log_path = ARTIFACTS / f"{label}.log"
    window = None
    with log_path.open("w") as log:
        process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
        try:
            ui = project_ui(process, environment, project, log_path)
            window = ui.window
            yield ui
            if process.poll() is None:
                close_window(window, process)
            else:
                assert process.returncode == 0, f"RODE exited with {process.returncode}: {log_path}"
        except BaseException:
            capture_failure(label, process, window, files=((state, ".json"), (log_path, ".log")))
            raise
        finally:
            stop_process(process)
