#!/usr/bin/env python3
"""Capture the README tour from the current native editor on a private X display.

Run under scripts/ui_display.py and scripts/test-ssh.sh with RODE_SSH_RUN_TRANSPORT=0.
Uses installed Jupyter packages, an isolated demo project and an empty Slurm
fixture. No model inference or real scheduler allocation is requested.
"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import time
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path

from fixture_support import fixture_directory, fixture_environment

ROOT = Path(__file__).resolve().parent.parent
SOURCE = '''import math


def training_curve(steps=120, learning_rate=0.04):
    """Compare a baseline with a faster learning schedule."""
    baseline = [math.exp(-learning_rate * step) for step in range(steps)]
    improved = [math.exp(-learning_rate * 1.6 * step) for step in range(steps)]
    return baseline, improved


if __name__ == "__main__":
    baseline, improved = training_curve()
    print(f"Baseline final loss: {baseline[-1]:.4f}")
    print(f"Improved final loss: {improved[-1]:.4f}")
    print("Experiment complete. Ready to compare results.")
'''
PLOT = """%matplotlib inline
import matplotlib.pyplot as plt
from analysis import training_curve

baseline, improved = training_curve()
plt.style.use("dark_background")
fig, ax = plt.subplots(figsize=(7.2, 2.8), dpi=100)
ax.plot(baseline, color="#dcad42", label="Baseline")
ax.plot(improved, color="#35c9aa", label="Improved schedule")
ax.set(xlabel="Training step", ylabel="Loss", title="Compare experiment runs")
ax.legend(frameon=False)
fig.tight_layout()
plt.show()
"""
SCENES = (
    "01-editor",
    "02-ssh",
    "03-sandbox",
    "04-persist-off",
    "04-persist-on",
    "05-notebook",
    "06-agents",
    "07-slurm",
    "08-git",
)


def wait_for(check, message, timeout=30):
    """Poll a fixture condition until its deadline, raising the supplied diagnostic when it never becomes true."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.1)
    raise RuntimeError(message)


def main():
    """Create isolated demo projects and deterministic service fixtures, capture native views and clean up owned processes."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target/readme-demo/captures")
    parser.add_argument("--sandbox-backend", choices=("landlock", "bwrap"), default="landlock")
    options = parser.parse_args()
    binary = Path(os.environ.get("RODE_TEST_BINARY", ROOT / "target/release/rode")).resolve()
    helper = Path(os.environ.get("RODE_SESSION_HELPER", binary.parent / "rode-sessiond")).resolve()
    python = Path(
        os.environ.get("RODE_TEST_JUPYTER", ROOT / "target/notebook-env/bin/python")
    ).absolute()
    ssh_config = os.environ.get("RODE_TEST_SSH_CONFIG")
    if not ssh_config or not os.environ.get("DISPLAY"):
        parser.error("Run inside the isolated X display and SSH fixture; see docs/development.md")
    for executable in (binary, helper, python):
        if not executable.is_file():
            parser.error(f"Required installed executable not found: {executable}")
    subprocess.run([str(python), "-c", "import jupyter_client, ipykernel, matplotlib"], check=True)
    options.output.mkdir(parents=True, exist_ok=True)

    with fixture_directory(prefix="rode-tour-") as base:
        project = base / "signal-lab"
        project.mkdir()
        captures = base / "captures"
        captures.mkdir()
        os.environ["RODE_UI_ARTIFACTS"] = str(captures)
        os.environ["RODE_SESSION_DIR"] = str(base / "sessions")
        import ui_harness as ui

        # The shared harness may relocate artifacts into a configured CI run.
        # Publish from its actual directory rather than the requested path.
        captures = ui.ARTIFACTS
        (project / "analysis.py").write_text(SOURCE)
        (project / "README.md").write_text(
            "# Signal Lab\n\nA synthetic experiment for the RODE feature tour.\n\n"
            "Edit analysis.py, compare results.ipynb, and keep jobs in a persistent terminal.\n"
        )
        (project / "data").mkdir()
        (project / "data/README.txt").write_text("Synthetic data only.\n")
        (project / "pyproject.toml").write_text(
            '[project]\nname = "signal-lab"\nversion = "0.1.0"\nrequires-python = ">=3.11"\n'
        )
        (project / ".gitignore").write_text(".venv\n__pycache__/\n.tour-*\n")
        (project / ".venv").symlink_to(python.parent.parent)
        notebook = project / "results.ipynb"
        notebook.write_text(
            json.dumps(
                {
                    "nbformat": 4,
                    "nbformat_minor": 5,
                    "metadata": {
                        "kernelspec": {
                            "display_name": "Python 3",
                            "language": "python",
                            "name": "python3",
                        }
                    },
                    "cells": [
                        {
                            "id": "experiment",
                            "cell_type": "code",
                            "metadata": {},
                            "source": PLOT.splitlines(True),
                            "execution_count": None,
                            "outputs": [],
                        }
                    ],
                },
                indent=2,
            )
        )
        git_env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null")
        for args in (
            ["init", "-q", "-b", "main"],
            ["add", "."],
            [
                "-c",
                "user.name=RODE demo",
                "-c",
                "user.email=demo@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-qm",
                "Add reproducible experiment",
            ],
        ):
            subprocess.run(["git", "-C", str(project), *args], env=git_env, check=True)
        (project / "analysis.py").write_text(
            SOURCE.replace("learning_rate=0.04", "learning_rate=0.05")
        )
        tools = base / "tools"
        tools.mkdir()
        for name in ("squeue", "salloc", "srun", "scancel"):
            shutil.copyfile(ROOT / "tests/fixtures/slurm.py", tools / name)
            (tools / name).chmod(0o755)
        shell = tools / "demo-shell"
        shell.write_text(
            '#!/bin/sh\nexport PS1="signal-lab $ "\nexec /bin/bash --noprofile --norc\n'
        )
        shell.chmod(0o755)
        environment = fixture_environment(
            base,
            MPLCONFIGDIR=str(base / "matplotlib"),
            RODE_SESSION_HELPER=str(helper),
            SHELL=str(shell),
            PATH=str(tools) + os.pathsep + os.environ["PATH"],
            GIT_CONFIG_NOSYSTEM="1",
            GIT_CONFIG_GLOBAL="/dev/null",
        )
        # The local scenes intentionally demonstrate ordinary project mode.
        # The remote scene below requests and verifies an active sandbox.
        preferences = Path(environment["XDG_DATA_HOME"]) / "rode/app.ron"
        preferences.parent.mkdir(parents=True, exist_ok=True)
        preferences.write_text(json.dumps({"sandbox_defaults": json.dumps({"enabled": False})}))

        def capture(native, name):
            """Capture an acknowledged native scene with its pointer outside the content."""
            before = native.layout()["pass"]
            x, y, _, _ = ui.window_geometry(native.window)
            ui.move_pointer(x + 4, y + 4)
            native.layout(lambda state: state["pass"] > before, "Pointer move was not acknowledged")
            # The first capture lets pending presentation finish; retain the
            # second capture, then inspect every scene before publication.
            ui.screenshot(native.window, name + ".png")
            ui.screenshot(native.window, name + ".png")
            print(f"Captured {name}", flush=True)

        def sessions():
            """Read this isolated fixture host service metadata without attaching to or creating a terminal."""
            reply = subprocess.check_output(
                [str(binary), "--session-service", "request"],
                input='{"action":"list"}',
                text=True,
                env=environment,
                timeout=10,
            )
            return json.loads(reply).get("data", [])

        @contextmanager
        def editor(args, env, name):
            """Own one demo process/window, capturing failures and reaping only that process on every exit path."""
            with (options.output / f"{name}.log").open("w") as log:
                state_path = base / f"{name}-ui-state.json"
                process = subprocess.Popen(
                    [str(binary), *args, "--theme", "carbon"],
                    env=dict(env, RODE_UI_TEST_STATE=str(state_path)),
                    stdout=log,
                    stderr=log,
                )
                window = None
                try:
                    window = ui.wait_for_window(process, options.output / f"{name}.log")
                    ui.wait_for_project(window, process, log.name, project)
                    ui.focus_window(window)
                    native = ui.NativeUI(window, process, log.name, state_path, project)
                    native.resize(1380, 900)
                    yield native
                    ui.close_window(window, process)
                except Exception:
                    try:
                        if window and process.poll() is None:
                            ui.screenshot(window, name + "-failure.png")
                            shutil.copyfile(
                                captures / f"{name}-failure.png",
                                options.output / f"{name}-failure.png",
                            )
                        if state_path.exists():
                            shutil.copyfile(state_path, options.output / f"{name}-failure.json")
                    except Exception as error:
                        print(f"Could not retain {name} failure diagnostics: {error}")
                    raise
                finally:
                    ui.stop_process(process)

        try:
            with editor([str(project)], environment, "local") as native:
                native.open_file("README.md")
                native.open_file("analysis.py")
                native.control("git-change-modified:4")
                capture(native, "01-editor")
                native.command("Terminal: New terminal")
                wait_for(lambda: len(sessions()) == 1, "Demo terminal did not start")
                native.terminal_input()
                ui.text("clear; python3 analysis.py; printf ready > .tour-terminal-ready\n")
                native.wait(
                    lambda: (project / ".tour-terminal-ready").exists(),
                    "Terminal command did not finish",
                )
                native.button_fill(
                    "terminal-persist:off", "terminal-new-menu", "04-persist-off.png", filled=False
                )
                native.click("terminal-persist:off")
                wait_for(lambda: sessions()[0]["persistent"], "Persistence was not enabled")
                native.button_fill(
                    "terminal-persist:on", "terminal-new-menu", "04-persist-on.png", filled=True
                )
                native.click("terminal-persist:on")
                wait_for(lambda: not sessions()[0]["persistent"], "Persistence was not disabled")
                native.open_file("results.ipynb")
                native.layout(
                    lambda state: state.get("notebook") is not None, "Notebook did not load"
                )
                native.command("Notebook: Run all cells")
                native.layout(
                    lambda state: (
                        (current := state.get("notebook")) is not None
                        and not current["busy"]
                        and current["cells"][0]["execution_count"] is not None
                    ),
                    "Notebook execution did not finish",
                    timeout=45,
                )
                ui.key("s", ("Control_L",))
                native.layout(
                    lambda state: (state.get("notebook") or {}).get("saved") is True,
                    "Notebook save was not acknowledged",
                )

                def plot_saved():
                    """Observe the acknowledged save; never resubmit Save while waiting for output."""
                    outputs = json.loads(notebook.read_text())["cells"][0]["outputs"]
                    errors = [output for output in outputs if output["output_type"] == "error"]
                    if errors:
                        raise RuntimeError(f"Notebook execution failed: {errors}")
                    return any("image/png" in output.get("data", {}) for output in outputs)

                wait_for(plot_saved, "The real notebook kernel did not return a plot", timeout=45)
                capture(native, "05-notebook")
                native.command("Agent: Chat")
                native.click("agent-provider-menu")
                native.control("agent-provider-Claude")
                capture(native, "06-agents")
                ui.key("Escape")
                native.absent("agent-provider-Claude")
                native.command("Terminal: Allocate Slurm resources")
                native.control("menu-file", enabled=False)
                capture(native, "07-slurm")
                assert not (base / "slurm-state/job.json").exists(), (
                    "Demo requested a scheduler allocation"
                )
                ui.key("Escape")
                native.open_file("analysis.py")
                native.click("git-change-modified:4")
                native.control("git-peek-diff")
                capture(native, "08-git")
        finally:
            # Only this fixture's private service and its owned terminals.
            for session in sessions():
                subprocess.run(
                    [str(binary), "--session-service", "request"],
                    input=json.dumps(
                        {"action": "end", "id": session["id"], "policy": session["policy"]}
                    ),
                    text=True,
                    env=environment,
                    stdout=subprocess.DEVNULL,
                    check=True,
                    timeout=10,
                )

        remote_environment = fixture_environment(base / "remote", environment=environment)
        policy = {"enabled": True, "backend": options.sandbox_backend}
        project_key = json.dumps(
            [["rode-test", "", "", ssh_config, ""], str(project)], separators=(",", ":")
        )
        preferences = Path(remote_environment["XDG_DATA_HOME"]) / "rode/app.ron"
        preferences.parent.mkdir(parents=True, exist_ok=True)
        preferences.write_text(
            json.dumps(
                {
                    "sandbox_defaults": json.dumps(policy),
                    "project_preferences": json.dumps({project_key: {"sandbox": policy}}),
                }
            )
        )
        with editor(
            [
                "--ssh",
                "rode-test",
                "--ssh-config",
                ssh_config,
                "--remote-path",
                str(project),
                "--sandbox",
            ],
            remote_environment,
            "ssh",
        ) as native:

            def connected():
                """Wait for the SSH fixture title to confirm prepared sandbox state rather than an opening project."""
                info = subprocess.check_output(
                    ["xwininfo", "-id", hex(native.window)], text=True, timeout=5
                )
                return "[Sandbox]" in info and "[Opening project]" not in info

            wait_for(
                connected, f"SSH project did not enter an actual {options.sandbox_backend} sandbox"
            )
            native.open_file("analysis.py")
            native.control("source-editor", focused=True)
            capture(native, "02-ssh")
            native.command("Project: Settings and sandbox access")
            native.click("settings-category-Sandbox")
            native.control("settings-save")
            capture(native, "03-sandbox")

        manifest = {
            "captured_at": datetime.now(timezone.utc).isoformat(),
            "revision": subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
            ).strip(),
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "sandbox_backend": options.sandbox_backend,
            "captures": {},
        }
        for name in SCENES:
            path = captures / f"{name}.png"
            manifest["captures"][path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
            shutil.copyfile(path, options.output / path.name)
        (options.output / "capture.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"Saved {len(SCENES)} fresh native captures to {options.output}", flush=True)


if __name__ == "__main__":
    main()
