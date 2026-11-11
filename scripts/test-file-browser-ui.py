#!/usr/bin/env python3
"""Shared file/folder picker, keyboard/mouse navigation and host/sandbox scope."""

import hashlib
import os
import shutil
import subprocess
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/file-browser-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    close_window,
    fixture_directory,
    fixture_environment,
    focus_window,
    key,
    screenshot,
    text,
    wait_for_project,
    wait_for_window,
)


def wait_for(check, message):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def image_pixels(window, name, color, minimum):
    # Opening and rendering a document are asynchronous. Observe the result
    # rather than assuming that the previous tab has disappeared after a sleep.
    def rendered():
        screenshot(window, name)
        raw = subprocess.check_output(["convert", str(ARTIFACTS / name), "-depth", "8", "rgb:-"])
        # Ignore bars/explorer; inspect the editor body only.
        width = 1380
        return (
            sum(
                raw[(y * width + x) * 3 : (y * width + x) * 3 + 3] == bytes(color)
                for y in range(200, 760)
                for x in range(310, 1370)
            )
            >= minimum
        )

    wait_for(rendered, f"{name}: document did not render {minimum} pixels of {color}")


def run(remote=False, scenario="all"):
    label = ("ssh" if remote else "local") + "-" + scenario
    with fixture_directory(prefix="rode-file-browser-") as base:
        root = base / "project"
        root.mkdir()
        folder = root / "a-folder"
        folder.mkdir()
        (root / "z-longer-folder-name").mkdir()
        source = folder / "note.txt"
        source.write_text("original\n")
        binary = root / "sample.bin"
        binary.write_bytes(bytes(range(256)))
        subprocess.run(
            ["convert", "-size", "240x160", "xc:rgb(224,40,64)", str(root / "picture.png")],
            check=True,
        )
        shutil.copyfile(
            Path(__file__).resolve().parent.parent / "tests/fixtures/pages.pdf", root / "pages.pdf"
        )
        hidden = root / ".hidden.txt"
        hidden.write_text("hidden\n")
        environment = fixture_environment(
            base,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            RODE_UI_TEST_STATE=str(ARTIFACTS / (label + "-state.json")),
        )
        args = [str(BINARY), "--theme", "carbon", "--sandbox"]
        if remote:
            real_ssh = shutil.which("ssh")
            tools = base / "tools"
            tools.mkdir()
            wrapper = tools / "ssh"
            # The unprivileged fixture cannot allocate login TTYs. Native
            # project PTYs and normal authenticated file channels are unchanged.
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os,sys\nbinary="
                + repr(real_ssh)
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
            args.append(str(root))
        with (ARTIFACTS / (label + ".log")).open("w") as log:
            process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
            window = None
            try:
                window = wait_for_window(process, log.name)
                wait_for_project(window, process, log.name, root)
                focus_window(window)
                ui = NativeUI(
                    window, process, log.name, Path(environment["RODE_UI_TEST_STATE"]), root
                )

                def directory(path):
                    ui.value("file-dialog-directory", path)

                def choose(name, hex_mode=False):
                    ui.command(
                        "Hex: Open file / inspect binary" if hex_mode else "Open file by path"
                    )
                    ui.fill_path(name)
                    # Exercise the browser result rather than exact-path submission.
                    ui.value("file-picker-row", root / name)
                    key("Up")
                    key("Return")
                    ui.absent("file-dialog-path")
                    ui.value("active-document", root / name)

                if scenario in ("all", "navigation"):
                    ui.command("Open project folder")
                    ui.control("project-dialog-ready")
                    screenshot(window, label + "-project-folders.png")
                    ui.click("project-dialog-cancel")
                    ui.absent("project-dialog-path")
                    ui.command("Open file by path")
                    directory(root)
                    screenshot(window, label + "-open-files.png")
                    # Single click selects; Right enters. No guessed row coordinates.
                    ui.click("file-picker-row:" + hashlib.sha256(str(folder).encode()).hexdigest())
                    key("Right")
                    directory(folder)
                    ui.value("file-picker-row", source)
                    screenshot(window, label + "-subfolder.png")
                    key("Return")
                    ui.absent("file-dialog-path")
                    ui.value("active-document", source)
                    ui.control("source-editor", focused=True)
                    key("End", ("Control_L",))
                    text("browser-edit\n")
                    ui.source("original\nbrowser-edit\n")
                    key("s", ("Control_L",))
                    ui.control("source-editor-saved")
                    assert "browser-edit" in source.read_text()
                    key("s", ("Control_L", "Shift_L"))
                    ui.fill_path("copy.txt")
                    ui.click("file-dialog-up")
                    directory(root)
                    ui.value("file-dialog-path-value", "copy.txt")
                    ui.click("file-dialog-submit")
                    ui.absent("file-dialog-path")
                    ui.wait(
                        lambda: (root / "copy.txt").read_text() == source.read_text(),
                        "Save As lost its filename/folder",
                    )
                    ui.command("Open file by path")
                    ui.click("file-dialog-location")
                    ui.fill("file-dialog-location", str(base))
                    key("Return")
                    directory(base)
                    screenshot(window, label + "-sandbox-parent.png")
                    key("Escape")
                    ui.absent("file-dialog-path")
                if scenario in ("all", "previews"):
                    choose("sample.bin", hex_mode=True)
                    screenshot(window, label + "-hex.png")
                    assert binary.read_bytes() == bytes(range(256))
                    choose("picture.png")
                    image_pixels(window, label + "-image.png", (224, 40, 64), 10000)
                    choose("pages.pdf")
                    image_pixels(window, label + "-pdf.png", (255, 255, 255), 10000)
                close_window(window, process)
                print(label + ": file-browser assertions passed.")
            finally:
                if process.poll() is None:
                    if window:
                        screenshot(window, label + "-failure.png")
                    process.terminate()
                    process.wait(timeout=10)


if __name__ == "__main__":
    scenario = os.environ.get("RODE_FILE_BROWSER_UI_SCENARIO", "all")
    case = os.environ.get("RODE_FILE_BROWSER_UI_CASE", "all")
    assert case in ("all", "local", "ssh"), f"Unknown file-browser host case: {case}"
    assert scenario in ("all", "navigation", "previews"), (
        f"Unknown file-browser scenario: {scenario}"
    )
    if case == "ssh":
        assert os.environ.get("RODE_TEST_SSH_CONFIG"), "SSH case requires scripts/test-ssh.sh"
    if case in ("all", "local"):
        run(scenario=scenario)
    if case in ("all", "ssh") and os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(remote=True, scenario=scenario)
