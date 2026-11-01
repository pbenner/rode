#!/usr/bin/env python3
"""Capture the README tour from the current native editor on a private X display.

Run under xvfb-run and scripts/test-ssh.sh with RODE_SSH_RUN_TRANSPORT=0.
Uses installed Jupyter packages, an isolated demo project and an empty Slurm
fixture. No model inference or real scheduler allocation is requested.
"""

import argparse
import ctypes
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
import time
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path

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
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.1)
    raise RuntimeError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target/readme-demo/captures")
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

    with tempfile.TemporaryDirectory(prefix="rode-tour-") as directory:
        base = Path(directory)
        project = base / "signal-lab"
        project.mkdir()
        captures = base / "captures"
        captures.mkdir()
        os.environ["RODE_UI_ARTIFACTS"] = str(captures)
        os.environ["RODE_SESSION_DIR"] = str(base / "sessions")
        import ui_harness as ui

        ui.x11.XResizeWindow.argtypes = [
            ctypes.c_void_p,
            ctypes.c_ulong,
            ctypes.c_uint,
            ctypes.c_uint,
        ]
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
        (project / ".gitignore").write_text(".venv\n__pycache__/\n")
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
        environment = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            XDG_CACHE_HOME=str(base / "cache"),
            MPLCONFIGDIR=str(base / "matplotlib"),
            RODE_SESSION_HELPER=str(helper),
            SHELL=str(shell),
            PATH=str(tools) + os.pathsep + os.environ["PATH"],
        )

        def command(value):
            ui.key("p", ("Control_L", "Shift_L"))
            ui.text(value)
            ui.key("Return")
            time.sleep(0.4)

        def open_file(name):
            ui.key("space", ("Control_L",))
            ui.key("e")
            ui.key("p", ("Control_L",))
            ui.text(name)
            ui.key("Return")
            time.sleep(0.5)

        def capture(window, name):
            ui.xtest.XTestFakeMotionEvent(ui.display, -1, 1500, 1020, 0)
            ui.x11.XFlush(ui.display)
            ui.screenshot(window, name + ".png")
            print(f"Captured {name}", flush=True)

        def sessions():
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
            with (options.output / f"{name}.log").open("w") as log:
                process = subprocess.Popen(
                    [str(binary), *args, "--theme", "carbon"], env=env, stdout=log, stderr=log
                )
                window = None
                try:
                    window = ui.wait_for_window(process, options.output / f"{name}.log")
                    ui.focus_window(window)
                    ui.x11.XResizeWindow(ui.display, window, 1380, 900)
                    ui.x11.XFlush(ui.display)
                    time.sleep(0.8)
                    yield window
                    ui.close_window(window, process)
                except Exception:
                    if window and process.poll() is None:
                        capture(window, name + "-failure")
                        shutil.copyfile(
                            captures / f"{name}-failure.png", options.output / f"{name}-failure.png"
                        )
                    raise
                finally:
                    if process.poll() is None:
                        process.terminate()
                        try:
                            process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            process.kill()
                            process.wait()

        try:
            with editor([str(project)], environment, "local") as window:
                open_file("README.md")
                open_file("analysis.py")
                capture(window, "01-editor")
                command("Terminal: New terminal")
                wait_for(lambda: len(sessions()) == 1, "Demo terminal did not start")
                ui.text("clear\npython3 analysis.py\n")
                time.sleep(0.5)
                capture(window, "04-persist-off")
                ui.click(1325, 108)
                wait_for(lambda: sessions()[0]["persistent"], "Persistence was not enabled")
                capture(window, "04-persist-on")
                ui.click(1325, 108)
                wait_for(lambda: not sessions()[0]["persistent"], "Persistence was not disabled")
                open_file("results.ipynb")
                command("Notebook: Run all cells")

                def plot_saved():
                    ui.key("s", ("Control_L",))
                    outputs = json.loads(notebook.read_text())["cells"][0]["outputs"]
                    errors = [output for output in outputs if output["output_type"] == "error"]
                    if errors:
                        raise RuntimeError(f"Notebook execution failed: {errors}")
                    return any("image/png" in output.get("data", {}) for output in outputs)

                wait_for(plot_saved, "The real notebook kernel did not return a plot", timeout=45)
                time.sleep(0.5)
                capture(window, "05-notebook")
                command("Agent: Chat")
                ui.click(1173, 116)
                capture(window, "06-agents")
                ui.key("Escape")
                command("Terminal: Allocate Slurm resources")
                time.sleep(0.5)
                capture(window, "07-slurm")
                assert not (base / "slurm-state/job.json").exists(), (
                    "Demo requested a scheduler allocation"
                )
                ui.key("Escape")
                open_file("analysis.py")
                command("Git: Source control")
                time.sleep(0.8)
                ui.click(337, 325)
                capture(window, "08-git")
                ui.click(1226, 142)
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

        ssh_tools = base / "ssh-tools"
        ssh_tools.mkdir()
        wrapper = ssh_tools / "ssh"
        wrapper.write_text(
            "#!/usr/bin/python3\nimport os,sys\nbinary="
            + repr(shutil.which("ssh"))
            + '\nos.execv(binary,[binary]+["-T" if a=="-tt" else a for a in sys.argv[1:]])\n'
        )
        wrapper.chmod(0o755)
        remote_environment = dict(
            environment,
            PATH=str(ssh_tools) + os.pathsep + environment["PATH"],
            XDG_CONFIG_HOME=str(base / "remote-config"),
            XDG_DATA_HOME=str(base / "remote-data"),
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
        ) as window:

            def connected():
                info = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                return "[Sandbox]" in info and "[Opening project]" not in info

            wait_for(connected, "SSH project did not enter an actual bubblewrap sandbox")
            time.sleep(1)
            ui.click(115, 265)
            time.sleep(0.8)
            ui.key("space", ("Control_L",))
            ui.key("e")
            capture(window, "02-ssh")
            command("Project: Settings and sandbox access")
            ui.xtest.XTestFakeMotionEvent(ui.display, -1, 1160, 700, 0)
            for _ in range(15):
                ui.xtest.XTestFakeButtonEvent(ui.display, 5, 1, 0)
                ui.xtest.XTestFakeButtonEvent(ui.display, 5, 0, 0)
                ui.x11.XFlush(ui.display)
                time.sleep(0.06)
            capture(window, "03-sandbox")

        manifest = {
            "captured_at": datetime.now(timezone.utc).isoformat(),
            "revision": subprocess.check_output(
                ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
            ).strip(),
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
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
