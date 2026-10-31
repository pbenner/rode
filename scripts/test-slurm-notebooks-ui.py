#!/usr/bin/env python3
"""Reserve through native controls, select a Slurm notebook target and change it."""

import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/slurm-notebook-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    ROOT,
    click,
    close_window,
    focus_window,
    key,
    screenshot,
    text,
)


def wait_for(check, message):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.1)
    raise AssertionError(message)


def command(name):
    key("p", ("Control_L", "Shift_L"))
    text(name)
    key("Return")
    time.sleep(0.4)


with tempfile.TemporaryDirectory(prefix="rode-slurm-notebook-ui-") as directory:
    base = Path(directory)
    root = base / "project"
    tools = root / "tools"
    tools.mkdir(parents=True)
    python = Path(os.environ["RODE_TEST_JUPYTER"]).absolute()
    (root / ".venv").symlink_to(python.parent.parent)
    for name in ["salloc", "srun", "squeue", "scancel"]:
        shutil.copy2(ROOT / "tests/fixtures/slurm.py", tools / name)
        (tools / name).chmod(0o755)
    state = root / "slurm-state"
    state.mkdir()
    (state / "grant").touch()
    book = json.loads((ROOT / "tests/fixtures/notebook.ipynb").read_text())
    book["cells"] = [
        dict(
            cell_type="code",
            metadata={},
            outputs=[],
            execution_count=None,
            source="import json, os\nfrom pathlib import Path\n"
            "counter = globals().get('counter', 0) + 1\n"
            "Path('executed.json').write_text(json.dumps(dict(job=os.environ.get('SLURM_JOB_ID'), "
            "gpu=os.environ.get('CUDA_VISIBLE_DEVICES'), counter=counter)))\nprint(counter)",
        )
    ]
    (root / "demo.ipynb").write_text(json.dumps(book))
    env = dict(
        os.environ,
        PATH=str(tools) + os.pathsep + os.environ["PATH"],
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        RODE_SESSION_DIR=str(base / "runtime"),
    )

    def rpc(action, **values):
        result = json.loads(
            subprocess.check_output(
                [str(BINARY), "--session-service", "request"],
                input=json.dumps(dict(action=action, **values)).encode(),
                env=env,
            )
        )
        assert result["kind"] != "error", result
        return result.get("data")

    process, window = None, None
    with (ARTIFACTS / "native.log").open("w") as log:
        try:
            process = subprocess.Popen(
                [str(BINARY), str(root), "--theme", "carbon"], env=env, stdout=log, stderr=log
            )

            def mapped():
                global window
                assert process.poll() is None
                match = re.search(
                    r'(0x[0-9a-f]+) "RODE',
                    subprocess.check_output(["xwininfo", "-root", "-tree"], text=True),
                )
                if match:
                    window = int(match[1], 16)
                return window

            wait_for(mapped, "No RODE window")
            time.sleep(0.8)
            focus_window(window)
            command("Terminal: Allocate Slurm resources")
            screenshot(window, "reserve-form.png")
            click(660, 602)
            wait_for(lambda: (state / "job.json").exists(), "Reserve button did not submit")
            wait_for(
                lambda: json.loads((state / "job.json").read_text())["state"] == "RUNNING",
                "Reservation was not granted",
            )
            owner = next(info for info in rpc("list") if info.get("allocation"))
            assert owner["persistent"]
            screenshot(window, "reserved.png")
            command("Open file by path")
            text("demo.ipynb")
            key("Return")
            time.sleep(0.6)
            screenshot(window, "notebook.png")
            click(612, 213)  # Refresh the target list.
            time.sleep(0.7)
            click(477, 213)
            screenshot(window, "targets.png")
            click(487, 281)
            command("Notebook: Run all cells")
            marker = root / "executed.json"
            wait_for(marker.exists, "Notebook did not execute on selected target")
            result = json.loads(marker.read_text())
            assert result == dict(job="4242", gpu="0,1", counter=1), result
            screenshot(window, "compute-kernel.png")
            # Toggle persistence on the existing kernel, then destroy the entire desktop.
            click(385, 183)
            wait_for(
                lambda: any(i["persistent"] for i in rpc("runtimes")),
                "Kernel persistence was not acknowledged",
            )
            kernel = next(i for i in rpc("runtimes") if i["persistent"])
            key("s", ("Control_L",))
            time.sleep(0.3)
            close_window(window, process)
            wait_for(
                lambda: any(i["id"] == kernel["id"] and not i["attached"] for i in rpc("runtimes")),
                "Kernel did not detach",
            )
            # Reattach with a different UI interpreter selection. The retained
            # kernel must keep its captured interpreter and Python variables.
            preferences = base / "data/rode/app.ron"
            content = preferences.read_text().rstrip()
            storage = json.loads(content[:-1].rstrip().removesuffix(",") + "}")
            saved = json.loads(storage.get("project_preferences", "{}"))
            profile = json.dumps([None, str(root)], separators=(",", ":"))
            saved.setdefault(profile, {})["python"] = str(python)
            storage["project_preferences"] = json.dumps(saved)
            preferences.write_text(json.dumps(storage))
            process = subprocess.Popen(
                [str(BINARY), str(root), "--theme", "carbon"], env=env, stdout=log, stderr=log
            )
            window = None
            wait_for(mapped, "No replacement RODE window")
            time.sleep(0.8)
            focus_window(window)
            command("Open file by path")
            text("demo.ipynb")
            key("Return")
            time.sleep(0.6)
            screenshot(window, "before-reattach.png")
            click(644, 183)
            wait_for(
                lambda: any(i["id"] == kernel["id"] and i["attached"] for i in rpc("runtimes")),
                "Kernel did not reattach",
            )
            time.sleep(0.5)
            marker.unlink()
            command("Notebook: Run all cells")
            wait_for(marker.exists, "Reattached kernel did not execute")
            result = json.loads(marker.read_text())
            assert result == dict(job="4242", gpu="0,1", counter=2), result
            screenshot(window, "reattached-kernel.png")
            # Changing to the host stops the old kernel and resets its live variables.
            marker.unlink()
            click(477, 213)
            click(477, 242)
            command("Notebook: Run all cells")
            wait_for(marker.exists, "Host kernel did not start after changing target")
            result = json.loads(marker.read_text())
            assert result["job"] is None and result["counter"] == 1, result
            host_kernel = next(i for i in rpc("runtimes") if i["alive"] and i["target"] is None)
            assert host_kernel["python"] == str(python), host_kernel
            assert (state / "job.json").exists(), "Changing kernels released the reservation"
            assert any(info["id"] == owner["id"] for info in rpc("list"))
            screenshot(window, "host-kernel.png")
            key("s", ("Control_L",))
            close_window(window, process)
        except Exception:
            if process and process.poll() is None and window:
                screenshot(window, "failure.png")
            raise
        finally:
            if process and process.poll() is None:
                process.terminate()
                process.wait(timeout=5)
            for info in rpc("runtimes") or []:
                rpc("end_runtime", id=info["id"], policy=info["policy"])
            for info in rpc("list") or []:
                rpc("end", id=info["id"], policy=info["policy"])
print(
    "Notebook reservation, compute execution, desktop restart, kernel reattachment and target restart passed"
)
