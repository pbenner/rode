#!/usr/bin/env python3
"""Shared file/folder picker, keyboard/mouse navigation and host/sandbox scope."""

import os
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(
    Path(__file__).resolve().parent.parent / "target/file-browser-ui"
)
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    close_window,
    display,
    key,
    screenshot,
    text,
    x11,
)


def wait_for(check, message):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def command(name):
    key("p", ("Control_L", "Shift_L"))
    text(name)
    key("Return")
    time.sleep(0.5)


def choose(name, hex_mode=False):
    command("Hex: Open file / inspect binary" if hex_mode else "Open file by path")
    text(name)
    # Use an actual browser result rather than submitting the typed path.
    key("Up")
    key("Return")
    time.sleep(0.7)


def image_pixels(window, name, color, minimum):
    screenshot(window, name)
    raw = subprocess.check_output(["convert", str(ARTIFACTS / name), "-depth", "8", "rgb:-"])
    # Ignore bars/explorer; inspect the editor body only.
    width = 1380
    count = sum(
        raw[(y * width + x) * 3 : (y * width + x) * 3 + 3] == bytes(color)
        for y in range(200, 760)
        for x in range(310, 1370)
    )
    assert count >= minimum, (name, color, count)


def run(remote=False):
    label = "ssh" if remote else "local"
    with tempfile.TemporaryDirectory(prefix="rode-file-browser-") as directory:
        base = Path(directory)
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
        environment = dict(
            os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
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

                def mapped():
                    nonlocal window
                    assert process.poll() is None
                    found = re.search(
                        r'(0x[0-9a-f]+) "RODE',
                        subprocess.check_output(["xwininfo", "-root", "-tree"], text=True),
                    )
                    if found:
                        window = int(found[1], 16)
                    return found

                wait_for(mapped, "No native window")
                wait_for(
                    lambda: (
                        "[Opening project]"
                        not in subprocess.check_output(["xwininfo", "-id", hex(window)], text=True)
                    ),
                    "Project did not become ready",
                )
                time.sleep(1)
                x11.XSetInputFocus(display, window, 1, 0)
                x11.XFlush(display)
                command("Open project folder")
                screenshot(window, label + "-project-folders.png")
                key("Escape")
                command("Open file by path")
                screenshot(window, label + "-open-files.png")
                # Single click selects a folder; Right enters it. Enter in the
                # browser then opens its first file with editor focus restored.
                click(500, 399)
                key("Right")
                time.sleep(0.5)
                screenshot(window, label + "-subfolder.png")
                key("Return")
                time.sleep(0.5)
                key("End", ("Control_L",))
                text("browser-edit\n")
                key("s", ("Control_L",))
                wait_for(lambda: "browser-edit" in source.read_text(), "Browser did not open text")
                # Save As preserves the entered basename when changing folders.
                key("s", ("Control_L", "Shift_L"))
                time.sleep(0.4)
                key("a", ("Control_L",))
                text("copy.txt")
                click(438, 365)  # Up in the common location toolbar.
                time.sleep(0.5)
                click(600, 330)  # Destination name.
                key("Return")
                wait_for(lambda: (root / "copy.txt").exists(), "Save As lost its filename/folder")
                assert (root / "copy.txt").read_text() == source.read_text()
                choose("sample.bin", hex_mode=True)
                screenshot(window, label + "-hex.png")
                assert binary.read_bytes() == bytes(range(256))
                choose("picture.png")
                image_pixels(window, label + "-image.png", (224, 40, 64), 10000)
                choose("pages.pdf")
                image_pixels(window, label + "-pdf.png", (255, 255, 255), 10000)
                # In an active sandbox /tmp is private: this parent contains
                # the project mount, without the host's config/data siblings.
                # Capture the host-aware listing; integration tests assert scope.
                command("Open file by path")
                click(600, 365)
                key("a", ("Control_L",))
                text(str(base))
                key("Return")
                time.sleep(0.5)
                screenshot(window, label + "-sandbox-parent.png")
                key("Escape")
                close_window(window, process)
                print(label + ": folder/file rows, mouse/keyboard, Save As, hex, PNG, PDF passed.")
            finally:
                if process.poll() is None:
                    if window:
                        screenshot(window, label + "-failure.png")
                    process.terminate()
                    process.wait(timeout=10)


if __name__ == "__main__":
    run()
    if os.environ.get("RODE_TEST_SSH_CONFIG"):
        run(remote=True)
