#!/usr/bin/env python3
"""Native notebooks, stateful kernels, protected saves and local/SSH sandbox scope."""

import json
import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/notebook-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    text,
    x11,
    xtest,
)

ROOT = Path(__file__).resolve().parent.parent
PYTHON = Path(
    os.environ.get("RODE_TEST_JUPYTER", ROOT / "target/notebook-env/bin/python")
).absolute()


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
    time.sleep(0.35)


def open_file(name):
    command("Open file by path")
    text(name)
    key("Return")
    time.sleep(0.6)


def wheel(ticks):
    xtest.XTestFakeMotionEvent(display, -1, 1180, 580, 0)
    for _ in range(ticks):
        xtest.XTestFakeButtonEvent(display, 5, 1, 0)
        xtest.XTestFakeButtonEvent(display, 5, 0, 0)
        x11.XFlush(display)
        time.sleep(0.06)


def run(remote=False):
    assert PYTHON.is_file(), (
        "Set RODE_TEST_JUPYTER to an interpreter with jupyter_client, ipykernel and matplotlib"
    )
    label = "ssh" if remote else "local"
    with tempfile.TemporaryDirectory(prefix="rode-notebook-ui-") as directory:
        base = Path(directory)
        project = base / "project"
        project.mkdir()
        (project / ".venv").symlink_to(PYTHON.parent.parent)
        source = project / "demo.ipynb"
        shutil.copyfile(ROOT / "tests/fixtures/notebook.ipynb", source)
        original = json.loads(source.read_text())
        (project / "notes.txt").write_text("preserve text\n")
        env = dict(
            os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
        )
        if remote:
            tools = base / "tools"
            tools.mkdir()
            wrapper = tools / "ssh"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os,sys\nbinary="
                + repr(shutil.which("ssh"))
                + "\nos.execv(binary,[binary]+['-T' if a=='-tt' else a for a in sys.argv[1:]])\n"
            )
            wrapper.chmod(0o755)
            env["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
            # Grants apply to the selected remote interpreter; no package upload.
            config = base / "data/rode"
            config.mkdir(parents=True)
            policy = json.dumps(
                {"enabled": True, "share_network": False, "read_only": [str(PYTHON.parent.parent)]}
            )
            host_key = json.dumps(
                [["rode-test", "", "", os.environ["RODE_TEST_SSH_CONFIG"], ""], str(project)],
                separators=(",", ":"),
            )
            project_preferences = json.dumps({host_key: {"sandbox": json.loads(policy)}})
            (config / "app.ron").write_text(
                json.dumps({"sandbox_defaults": policy, "project_preferences": project_preferences})
            )
            args = [
                str(BINARY),
                "--ssh",
                "rode-test",
                "--ssh-config",
                os.environ["RODE_TEST_SSH_CONFIG"],
                "--remote-path",
                str(project),
                "--sandbox",
                "--theme",
                "carbon",
            ]
        else:
            args = [str(BINARY), str(project), "--theme", "carbon"]
        with (ARTIFACTS / (label + ".log")).open("w") as log:
            process = subprocess.Popen(args, env=env, stdout=log, stderr=log)
            window = None
            try:

                def mapped():
                    nonlocal window
                    assert process.poll() is None
                    tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True)
                    match = re.search(r'(0x[0-9a-f]+) "RODE', tree)
                    if match:
                        window = int(match[1], 16)
                    return match

                wait_for(mapped, "No notebook window")
                time.sleep(1)
                focus_window(window)
                open_file("demo.ipynb")
                title = subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                host_offset = (44 if "[Sandbox]" in title else 74) if remote else 0

                def document_click(x, y):
                    # Remote projects add the host banner and, on fallback,
                    # the sandbox-unavailable reason above notebook controls.
                    click(x, y + host_offset)

                # Select Code explicitly: opening the notebook may already have
                # hidden the SSH terminal, so toggling it can cover the notebook.
                key("space", ("Control_L",))
                key("e")
                screenshot(window, label + "-opened.png")
                document_click(390, 208)
                screenshot(window, label + "-settings.png")
                document_click(390, 208)
                assert json.loads(source.read_text()) == original, (
                    "Opening a notebook changed its source"
                )
                command("Notebook: Run all cells")
                # Runtime outputs become dirty; a save records them through normal file protection.
                time.sleep(5)
                key("s", ("Control_L",))

                def executed():
                    key("s", ("Control_L",))
                    book = json.loads(source.read_text())
                    return [
                        c.get("execution_count") for c in book["cells"] if c["cell_type"] == "code"
                    ] == [1, 2, 3] and any(
                        "image/png" in o.get("data", {}) for o in book["cells"][3]["outputs"]
                    )

                wait_for(executed, "Notebook run/PNG output was not saved")
                book = json.loads(source.read_text())
                assert book["cells"][2]["outputs"][0]["text"] == "42\n", (
                    "Cells did not share kernel state"
                )
                assert book["metadata"]["custom"] == original["metadata"]["custom"]
                assert book["cells"][0]["attachments"] == original["cells"][0]["attachments"]
                screenshot(window, label + "-executed.png")
                wheel(8)
                screenshot(window, label + "-plot.png")
                # Ctrl+F finds a cell source and selects its matching text.
                key("f", ("Control_L",))
                text("counter = 40")
                key("Return")
                key("Escape")
                text("counter = 41")
                key("Return", ("Shift_L",))
                time.sleep(0.8)
                key("s", ("Control_L",))
                wait_for(
                    lambda: "counter = 41" in json.loads(source.read_text())["cells"][1]["source"],
                    "Cell edit / Shift+Enter was not saved",
                )
                command("Notebook: Run all cells")
                time.sleep(1.5)
                key("s", ("Control_L",))
                wait_for(
                    lambda: (
                        json.loads(source.read_text())["cells"][2]["outputs"][0]["text"] == "43\n"
                    ),
                    "Rerun did not use edited source",
                )
                # A source modification outside RODE must win over an outdated notebook revision.
                external = json.loads(source.read_text())
                external["metadata"]["external"] = True
                source.write_text(json.dumps(external))
                command("Notebook: Run all cells")
                time.sleep(1.2)
                key("s", ("Control_L",))
                time.sleep(0.4)
                assert json.loads(source.read_text())["metadata"]["external"] is True, (
                    "Notebook save conflict overwrote external data"
                )
                screenshot(window, label + "-conflict.png")
                key("s", ("Control_L", "Shift_L"))
                key("a", ("Control_L",))
                text("copy.ipynb")
                key("Return")
                wait_for(lambda: (project / "copy.ipynb").exists(), "Notebook Save As failed")
                command("Notebook: Toggle JSON source view")
                screenshot(window, label + "-json-source.png")
                command("Notebook: Toggle JSON source view")
                screenshot(window, label + "-cells-restored.png")
                open_file("notes.txt")
                key("End", ("Control_L",))
                text("dirty_text")
                command("Notebook: New Jupyter notebook")
                screenshot(window, label + "-new.png")
                key("w", ("Control_L",))
                time.sleep(0.2)
                screenshot(window, label + "-dirty-guard.png")
                key("Escape")
                key("s", ("Control_L",))
                key("a", ("Control_L",))
                text("new.ipynb")
                key("Return")
                wait_for(lambda: (project / "new.ipynb").exists(), "New notebook save failed")
                document_click(610, 149)
                text("inserted_code = 7")
                document_click(710, 149)
                text("Inserted markdown")
                key("s", ("Control_L",))

                def inserted():
                    cells = json.loads((project / "new.ipynb").read_text())["cells"]
                    return (
                        len(cells) == 3
                        and [c["cell_type"] for c in cells] == ["code", "code", "markdown"]
                        and "inserted_code = 7" in "".join(cells[1]["source"])
                        and "Inserted markdown" in "".join(cells[2]["source"])
                    )

                wait_for(inserted, "Toolbar cell insertion or editor focus failed")
                screenshot(window, label + "-inserted-cells.png")
                # Insert between existing cells, rather than appending to the notebook.
                document_click(465, 359)
                text("Between cells")
                key("s", ("Control_L",))

                def between():
                    cells = json.loads((project / "new.ipynb").read_text())["cells"]
                    return (
                        len(cells) == 4
                        and [c["cell_type"] for c in cells]
                        == ["code", "markdown", "code", "markdown"]
                        and "Between cells" in "".join(cells[1]["source"])
                        and "inserted_code = 7" in "".join(cells[2]["source"])
                    )

                wait_for(between, "Inline cell insertion changed the wrong position")
                screenshot(window, label + "-between-cells.png")
                open_file("notes.txt")
                key("s", ("Control_L",))
                time.sleep(0.3)
                close_window(window, process)
            except Exception:
                if source.exists():
                    shutil.copyfile(source, ARTIFACTS / (label + "-failure.ipynb"))
                if window and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
    print(
        label
        + " notebook cells, stateful kernels, PNGs, metadata, edits, protected saves and source view passed"
    )


if __name__ == "__main__":
    if not os.environ.get("RODE_NOTEBOOK_UI_REMOTE_ONLY"):
        run()
    if os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(True)
