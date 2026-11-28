#!/usr/bin/env python3
"""Native notebook execution, protected saves and cell editing, locally and over SSH."""

import base64
import json
import os
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
os.environ["RODE_UI_ARTIFACTS"] = str(ROOT / "target/notebook-ui")
from ui_fixture import native_session, project_args
from ui_harness import (
    ARTIFACTS,
    fixture_directory,
    fixture_environment,
    key,
    screenshot,
    text,
)

PYTHON = Path(
    os.environ.get("RODE_TEST_JUPYTER", ROOT / "target/notebook-env/bin/python")
).absolute()


def notebook(ui):
    return ui.layout(lambda state: state.get("notebook") is not None, "Notebook did not load")[
        "notebook"
    ]


def completed(ui, before, indices=None):
    """Observe completion; never press Save again while waiting for kernel results."""
    cells = before["cells"]
    indices = (
        indices
        if indices is not None
        else [i for i, cell in enumerate(cells) if cell["kind"] == "code"]
    )
    ui.layout(
        lambda state: (
            (current := state.get("notebook")) is not None
            and not current["busy"]
            and len(current["cells"]) == len(cells)
            and all(
                current["cells"][i]["execution_count"] is not None
                and current["cells"][i]["execution_count"] != cells[i]["execution_count"]
                for i in indices
            )
        ),
        "Notebook execution did not finish",
        timeout=45,
    )


def run_all(ui):
    before = notebook(ui)
    ui.command("Notebook: Run all cells")
    completed(ui, before)


def save(ui, path, check, message):
    key("s", ("Control_L",))
    ui.layout(
        lambda state: (state.get("notebook") or {}).get("saved") is True,
        "Notebook save was not acknowledged",
    )

    def saved():
        book = json.loads(path.read_text())
        return book if check(book) else None

    return ui.wait(saved, message)


def save_as(ui, name):
    key("s", ("Control_L", "Shift_L"))
    ui.fill_path(name)
    key("Return")
    ui.absent("file-dialog-path")
    ui.value("active-document", ui.project_root / name)
    ui.layout(
        lambda state: (state.get("notebook") or {}).get("saved") is True,
        "Notebook Save As was not acknowledged",
    )
    ui.wait((ui.project_root / name).exists, "Notebook Save As failed")


def execution(ui, source, original, label):
    ui.open_file("demo.ipynb")
    notebook(ui)
    ui.control("markdown-math-inline")
    ui.control("markdown-math-display")
    screenshot(ui.window, label + "-opened.png")
    ui.click("notebook-settings")
    ui.resize(850, 560)
    ui.value("notebook-output-budget-value", "256")
    ui.click("notebook-output-budget")
    ui.click("notebook-output-budget-64")
    ui.value("notebook-output-budget-value", "64")
    ui.click("notebook-output-budget")
    ui.click("notebook-output-budget-256")
    ui.value("notebook-output-budget-value", "256")
    ui.resize(1380, 900)
    screenshot(ui.window, label + "-settings.png")
    ui.click("notebook-settings")
    assert json.loads(source.read_text()) == original, "Opening a notebook changed its source"
    run_all(ui)
    book = save(
        ui,
        source,
        lambda book: (
            [c.get("execution_count") for c in book["cells"] if c["cell_type"] == "code"]
            == [1, 2, 3]
        ),
        "Notebook output was not saved",
    )
    assert any("image/png" in o.get("data", {}) for o in book["cells"][3]["outputs"])
    assert book["cells"][2]["outputs"][0]["text"] == "42\n", "Cells did not share kernel state"
    assert book["metadata"]["custom"] == original["metadata"]["custom"]
    assert book["cells"][0]["attachments"] == original["cells"][0]["attachments"]
    screenshot(ui.window, label + "-executed.png")

    # Find selects the matching source in the actual cell editor, including after scrolling.
    key("f", ("Control_L",))
    ui.fill("notebook-find", "plt.show()")
    key("Return")
    plot = notebook(ui)["cells"][3]
    ui.control(f"notebook-cell:{plot['uid']}", focused=True)
    key("Escape")
    ui.control("notebook-output-image")
    output_index = next(
        i for i, o in enumerate(book["cells"][3]["outputs"]) if "image/png" in o.get("data", {})
    )
    original_image = book["cells"][3]["outputs"][output_index]["data"]["image/png"]
    if isinstance(original_image, list):
        original_image = "".join(original_image)
    assert len(original_image) > 2 * 1024 * 1024, "Plot must exceed the old transport/preview limit"
    image_path = ui.project_root / "exported-plot.png"
    image_control = f"notebook-output-image:{plot['uid']}:{output_index}"
    # Seed text so a stale image clipboard cannot satisfy the copy assertion.
    ui.copy(f"notebook-cell:{plot['uid']}", "".join(book["cells"][3]["source"]))
    ui.click(image_control, mouse_button=3)
    ui.control("notebook-image-menu-save")
    ui.click("notebook-image-menu-copy")

    def clipboard_image():
        try:
            reply = subprocess.run(
                ["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
                capture_output=True,
                timeout=1,
            )
            return (
                reply.stdout
                if reply.returncode == 0 and reply.stdout.startswith(b"\x89PNG")
                else None
            )
        except subprocess.TimeoutExpired:
            return None

    copied = ui.wait(clipboard_image, "Notebook image was not copied to the desktop clipboard")
    original_pixels = subprocess.check_output(
        ["convert", "-", "-depth", "8", "rgba:-"],
        input=base64.b64decode(original_image),
        timeout=5,
    )
    copied_pixels = subprocess.check_output(
        ["convert", "-", "-depth", "8", "rgba:-"], input=copied, timeout=5
    )
    assert copied_pixels == original_pixels, (
        "Clipboard image changed the plot's pixels or dimensions"
    )
    ui.click(image_control)
    ui.control(image_control, focused=True)
    key("F10", ("Shift_L",))
    ui.control("notebook-image-menu-copy")
    key("Escape")
    ui.absent("notebook-image-menu-copy")
    ui.click(image_control, mouse_button=3)
    screenshot(ui.window, label + "-image-menu.png")
    ui.click("notebook-image-menu-save")
    ui.fill_path(str(image_path))
    key("Return")
    ui.absent("file-dialog-path")
    ui.wait(image_path.exists, "Plot image was not saved on its project host")
    assert image_path.read_bytes() == base64.b64decode(original_image), (
        "Export changed the original image"
    )
    assert json.loads(source.read_text()) == book, "Image export changed the notebook"
    # Existing destinations stay intact and a failed export leaves the notebook usable.
    ui.dismiss_notice()
    ui.click(f"notebook-save-image:{plot['uid']}:{output_index}")
    ui.fill_path(str(image_path))
    key("Return")
    ui.notice("Image not saved: the destination already exists. Choose a new filename.")
    assert image_path.read_bytes() == base64.b64decode(original_image)
    ui.dismiss_notice()
    screenshot(ui.window, label + "-plot.png")
    key("f", ("Control_L",))
    ui.fill("notebook-find", "counter = 40")
    key("Return")
    first = notebook(ui)["cells"][1]
    ui.control(f"notebook-cell:{first['uid']}", focused=True)
    key("Escape")
    text("counter = 41")
    expected = "".join(original["cells"][1]["source"]).replace("counter = 40", "counter = 41")
    ui.value(f"notebook-cell:{first['uid']}-value", expected, focused=True)
    before = notebook(ui)
    key("Return", ("Shift_L",))
    completed(ui, before, [1])
    save(
        ui,
        source,
        lambda book: "counter = 41" in book["cells"][1]["source"],
        "Cell edit / Shift+Enter was not saved",
    )
    run_all(ui)
    save(
        ui,
        source,
        lambda book: book["cells"][2]["outputs"][0]["text"] == "43\n",
        "Rerun did not use edited source",
    )


def protected_saves(ui, source, label):
    external = json.loads(source.read_text())
    external["metadata"]["external"] = True
    source.write_text(json.dumps(external))
    run_all(ui)
    key("s", ("Control_L",))
    ui.notice(
        "Save conflict: this file changed on disk. Your edits are still open; "
        "use Save As to keep a separate copy, or close and reopen to load the disk version."
    )
    assert json.loads(source.read_text()) == external, (
        "Notebook save conflict overwrote external data"
    )
    screenshot(ui.window, label + "-conflict.png")
    ui.dismiss_notice()
    save_as(ui, "copy.ipynb")
    copy = json.loads((ui.project_root / "copy.ipynb").read_text())
    assert "external" not in copy["metadata"], "Save As discarded the retained notebook buffer"
    assert copy["cells"][2]["outputs"][0]["text"] == "43\n"
    for source_mode, suffix in [(True, "json-source"), (False, "cells-restored")]:
        ui.command("Notebook: Toggle JSON source view")
        ui.layout(
            lambda state: (state.get("notebook") or {}).get("source_mode") is source_mode,
            "Notebook view did not switch",
        )
        screenshot(ui.window, label + "-" + suffix + ".png")


def insertion(ui, label):
    ui.open_file("notes.txt")
    ui.control("source-editor", focused=True)
    key("End", ("Control_L",))
    text("dirty_text")
    ui.source("preserve text\ndirty_text")
    ui.command("Notebook: New Jupyter notebook")
    notebook(ui)
    screenshot(ui.window, label + "-new.png")
    key("w", ("Control_L",))
    ui.control("guard-keep-editing")
    screenshot(ui.window, label + "-dirty-guard.png")
    ui.click("guard-keep-editing")
    save_as(ui, "new.ipynb")
    for button, index, value in [
        ("notebook-add-code", 1, "inserted_code = 7"),
        ("notebook-add-markdown", 2, "Inserted markdown"),
    ]:
        ui.click("notebook-insert")
        ui.click(button)
        current = ui.layout(
            lambda state: len((state.get("notebook") or {}).get("cells", [])) == index + 1,
            "Cell was not inserted",
        )["notebook"]
        name = f"notebook-cell:{current['cells'][index]['uid']}"
        ui.control(name, focused=True)
        text(value)
        ui.value(name + "-value", value, focused=True)
    target = ui.project_root / "new.ipynb"
    book = save(ui, target, lambda book: len(book["cells"]) == 3, "Cell insertion was not saved")
    assert [c["cell_type"] for c in book["cells"]] == ["code", "code", "markdown"]
    assert "inserted_code = 7" in "".join(book["cells"][1]["source"])
    assert "Inserted markdown" in "".join(book["cells"][2]["source"])
    screenshot(ui.window, label + "-inserted-cells.png")
    # The inline insertion control is distinct from the toolbar's append-after-selection action.
    ui.click("notebook-insert-markdown:0")
    current = ui.layout(
        lambda state: len((state.get("notebook") or {}).get("cells", [])) == 4,
        "Inline cell was not inserted",
    )["notebook"]
    name = f"notebook-cell:{current['cells'][1]['uid']}"
    ui.control(name, focused=True)
    text("Between cells")
    ui.value(name + "-value", "Between cells", focused=True)
    book = save(ui, target, lambda book: len(book["cells"]) == 4, "Inline insertion was not saved")
    assert [c["cell_type"] for c in book["cells"]] == ["code", "markdown", "code", "markdown"]
    assert "Between cells" in "".join(book["cells"][1]["source"])
    assert "inserted_code = 7" in "".join(book["cells"][2]["source"])
    screenshot(ui.window, label + "-between-cells.png")
    ui.open_file("notes.txt")
    ui.source("preserve text\ndirty_text")
    key("s", ("Control_L",))
    ui.control("source-editor-saved")
    assert (ui.project_root / "notes.txt").read_text() == "preserve text\ndirty_text"


def run(remote=False):
    assert PYTHON.is_file(), (
        "Set RODE_TEST_JUPYTER to an interpreter with jupyter_client, ipykernel and matplotlib"
    )
    label = "ssh" if remote else "local"
    with fixture_directory(prefix="rode-notebook-ui-") as base:
        project = base / "project"
        project.mkdir()
        (project / ".venv").symlink_to(PYTHON.parent.parent)
        source = project / "demo.ipynb"
        shutil.copyfile(ROOT / "tests/fixtures/notebook.ipynb", source)
        original = json.loads(source.read_text())
        original["cells"][0]["source"].extend(
            ["\n\nInline $E=mc^2$.\n\n", "$$x_i^2 + y_i^2 = z_i^2$$\n"]
        )
        # A detailed wide plot crosses the former 1 MiB output and 2 MiB
        # preview/transport limits while fitting the existing viewport layout.
        original["cells"][3]["source"] = [
            "import matplotlib.pyplot as plt\n",
            "import numpy as np\n",
            "plt.figure(figsize=(20.48, 5.12), dpi=100)\n",
            "plt.axes([0, 0, 1, 1]); plt.axis('off')\n",
            "plt.imshow(np.random.default_rng(7).integers(0, 256, (512, 2048, 3), dtype=np.uint8), interpolation='nearest')\n",
            "plt.show()\n",
        ]
        # Include partial transparency so copying cannot silently premultiply
        # the clipboard's straight RGBA pixels and darken pasted plots.
        original["cells"][3]["source"].insert(
            -1, "plt.gcf().patch.set_alpha(0.5)\nplt.gca().patch.set_alpha(0.5)\n"
        )
        source.write_text(json.dumps(original))
        (project / "notes.txt").write_text("preserve text\n")
        environment = fixture_environment(base)
        if remote:
            policy = {
                "enabled": True,
                "share_network": False,
                "read_only": sorted(
                    {str(PYTHON.parent.parent), str(PYTHON.resolve().parent.parent)}
                ),
            }
            host = ["rode-test", "", "", os.environ["RODE_TEST_SSH_CONFIG"], ""]
            context = json.dumps([host, str(project)], separators=(",", ":"))
            config = base / "data/rode"
            config.mkdir()
            (config / "app.ron").write_text(
                json.dumps(
                    {
                        "sandbox_defaults": json.dumps(policy),
                        "project_preferences": json.dumps({context: {"sandbox": policy}}),
                    }
                )
            )
        try:
            with native_session(
                project_args(project, remote=remote, sandbox=remote), environment, project, label
            ) as ui:
                execution(ui, source, original, label)
                protected_saves(ui, source, label)
                insertion(ui, label)
        except BaseException:
            shutil.copyfile(source, ARTIFACTS / (label + "-failure.ipynb"))
            raise
    print(label + ": notebook execution, protected saves and cell editing passed")


if __name__ == "__main__":
    if not os.environ.get("RODE_NOTEBOOK_UI_REMOTE_ONLY"):
        run()
    if os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(True)
