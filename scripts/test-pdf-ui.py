#!/usr/bin/env python3
"""Native read-only PDFs, page navigation/reload and local/SSH sandbox reads."""

import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/pdf-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    click,
    close_window,
    display,
    focus_window,
    key,
    screenshot,
    stop_process,
    text,
    wait_for_project,
    wait_for_window,
    x11,
    xtest,
)

FIXTURE = Path(__file__).resolve().parent.parent / "tests/fixtures/pages.pdf"


def wait_for(check, message):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.12)
    raise AssertionError(message)


def wheel(up, ticks=1):
    xtest.XTestFakeMotionEvent(display, -1, 840, 520, 0)
    for _ in range(ticks):
        xtest.XTestFakeButtonEvent(display, 4 if up else 5, 1, 0)
        xtest.XTestFakeButtonEvent(display, 4 if up else 5, 0, 0)
        x11.XFlush(display)
        time.sleep(0.12)


def color(window, name, kind):
    screenshot(window, name)
    value = subprocess.check_output(
        ["convert", str(ARTIFACTS / name), "-format", "%[pixel:p{840,520}]", "info:"], text=True
    )
    rgb = tuple(map(int, re.findall(r"\d+", value)[:3]))
    if kind == "red":
        return rgb[0] > 180 and rgb[1] < 100 and rgb[2] < 100
    if kind == "blue":
        return rgb[2] > 180 and rgb[0] < 100
    return rgb[1] > 150 and rgb[0] < 100 and rgb[2] < 100


def run(remote=False):
    label = "ssh" if remote else "local"
    with tempfile.TemporaryDirectory(prefix="rode-pdf-ui-") as directory:
        base = Path(directory)
        root = base / "project"
        root.mkdir()
        pdf = root / "report.PDF"
        pdf.write_bytes(FIXTURE.read_bytes())
        notes = root / "notes.txt"
        notes.write_text("Keep this note.\n")
        private = base / "private.pdf"
        private.write_bytes(FIXTURE.read_bytes())
        (root / "escape.pdf").symlink_to(private)
        environment = dict(
            os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
        )
        args = [str(BINARY), "--sandbox", "--theme", "carbon"]
        if remote:
            tools = base / "ssh-fixture"
            tools.mkdir()
            wrapper = tools / "ssh"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os,sys\nbinary="
                + repr(shutil.which("ssh"))
                + "\nos.execv(binary,[binary]+['-T' if a=='-tt' else a for a in sys.argv[1:]])\n"
            )
            wrapper.chmod(0o755)
            environment["PATH"] = str(tools) + os.pathsep + os.environ["PATH"]
            args += [
                "--ssh",
                "rode-test",
                "--ssh-config",
                os.environ["RODE_TEST_SSH_CONFIG"],
                "--remote-path",
                str(root),
            ]
        else:
            args += [str(root)]
        environment["RODE_UI_TEST_STATE"] = str(base / "ui-state.json")
        log_path = ARTIFACTS / (label + ".log")
        with log_path.open("w") as log:
            process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
            window = None
            try:
                window = wait_for_window(process, log_path)
                wait_for_project(window, process, log_path, root)
                focus_window(window)
                ui = NativeUI(window, process, log_path, environment["RODE_UI_TEST_STATE"], root)
                ui.open_file("report.PDF")
                wait_for(
                    lambda: color(window, label + "-first-page.png", "red"),
                    "First PDF page missing",
                )
                # Wheel/trackpad navigation also works when a whole page fits.
                wheel(False, 4)
                wait_for(
                    lambda: color(window, label + "-wheel-next.png", "blue"),
                    "Wheel did not advance to the next PDF page",
                )
                wheel(True, 4)
                wait_for(
                    lambda: color(window, label + "-wheel-previous.png", "red"),
                    "Wheel did not return to the previous PDF page",
                )
                click(840, 520)
                key("equal", ("Control_L",))
                wheel(False)
                assert color(window, label + "-scroll-within-page.png", "red"), (
                    "Scrolling a zoomed page skipped its remaining content"
                )
                wheel(False, 20)
                wait_for(
                    lambda: color(window, label + "-scroll-next.png", "blue"),
                    "Scrolling past the bottom did not advance the PDF",
                )
                wheel(True, 20)
                wait_for(
                    lambda: color(window, label + "-scroll-back.png", "red"),
                    "Scrolling past the top did not go back in the PDF",
                )
                key("minus", ("Control_L",))
                key("Page_Down")
                wait_for(
                    lambda: color(window, label + "-second-page.png", "blue"),
                    "Second PDF page missing",
                )
                key("equal", ("Control_L",))
                screenshot(window, label + "-zoom.png")
                key("minus", ("Control_L",))
                original = pdf.read_bytes()
                ui.command("Save file")
                ui.notice("Image and PDF previews are read-only.")
                ui.dismiss_notice()
                ui.command("Save file as")
                ui.notice("Image and PDF previews are read-only.")
                ui.dismiss_notice()
                assert pdf.read_bytes() == original, "Read-only PDF was saved"
                ui.open_file("notes.txt")
                ui.control("source-editor", focused=True)
                key("End", ("Control_L",))
                text("\n// keep_dirty_text")
                ui.source("Keep this note.\n\n// keep_dirty_text")
                ui.open_file("report.PDF")
                assert notes.read_text() == "Keep this note.\n"
                pdf.write_bytes(original.replace(b"0.1 0.25 0.9 rg", b"0.1 0.80 0.1 rg"))
                ui.command("PDF: Reload preview")
                wait_for(
                    lambda: color(window, label + "-reload.png", "green"),
                    "Reload lost the selected page",
                )
                ui.command("Theme: Matrix")
                assert color(window, label + "-matrix.png", "green"), "Theme tinted PDF content"
                pdf.write_bytes(b"%PDF-1.4\nbroken\n")
                ui.command("PDF: Reload preview")
                ui.control("notice-dismiss")
                assert color(window, label + "-failed-reload.png", "green"), (
                    "Failed reload lost previous page"
                )
                key("w", ("Control_L",))
                ui.command("Save file")
                wait_for(
                    lambda: "keep_dirty_text" in notes.read_text(),
                    "Switching previews lost unsaved text",
                )
                close_window(window, process)
                print(
                    label
                    + ": PDF rendering, wheel scrolling/paging, navigation, zoom, read-only saves, reload and dirty-buffer preservation passed."
                )
            except Exception:
                if window and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                stop_process(process)


if __name__ == "__main__":
    case = os.environ.get("RODE_PDF_UI_CASE", "all")
    if case in ("all", "local"):
        run()
    if case in ("all", "ssh") and os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(remote=True)
