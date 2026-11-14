"""Owned native UI sessions shared by document fixtures.

Fixtures supply their private environment and project data. This module owns
only the launched process, its window, and its diagnostic artifacts.
"""

import contextlib
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
    wait_for_project(window, process, log_path, project)
    focus_window(window)
    ui = NativeUI(window, process, log_path, Path(environment["RODE_UI_TEST_STATE"]), project)
    ui.layout()
    return ui


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
            try:
                if state.is_file():
                    shutil.copyfile(state, ARTIFACTS / f"{label}-failure.json")
                if window is not None and process.poll() is None:
                    screenshot(window, f"{label}-failure.png")
            except Exception as error:
                print(f"Cannot capture {label} failure artifacts: {error}", file=sys.stderr)
            raise
        finally:
            stop_process(process)
