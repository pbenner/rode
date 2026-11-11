#!/usr/bin/env python3
"""Native notebook execution, protected saves and cell editing, locally and over SSH."""

import json
import os
import shutil
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
    screenshot(ui.window, label + "-opened.png")
    ui.click("notebook-settings")
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
