#!/usr/bin/env python3
"""Native raster tabs, zoom, reload, and edit protection, locally or over SSH."""

import hashlib
import os
import re
import shutil
import struct
import subprocess
import time
import zlib
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/images-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    NativeUI,
    click_button,
    close_window,
    fixture_directory,
    focus_window,
    key,
    screenshot,
    stop_process,
    text,
    wait_for_project,
    wait_for_ui_state,
    wait_for_window,
)


def png(path, alternate=False):
    width, height = 240, 160
    rows = []
    for y in range(height):
        row = bytearray(b"\0")
        for x in range(width):
            color = (
                (224, 40, 64, 255)
                if x < 120
                else (32, 192, 96, 255)
                if y < 80
                else (40, 96, 224, 255)
            )
            if alternate:
                color = (240, 160, 24, 255)
            if 100 <= x < 140 and 60 <= y < 100:
                color = (0, 0, 0, 0)
            row.extend(color)
        rows.append(row)

    def chunk(name, value):
        return (
            struct.pack(">I", len(value))
            + name
            + value
            + struct.pack(">I", zlib.crc32(name + value))
        )

    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(b"".join(rows)))
        + chunk(b"IEND", b"")
    )


def wait_for(check, message):
    until = time.monotonic() + 10
    while time.monotonic() < until:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def pixels(window, name, color, size=(1380, 900), check=lambda bounds: True):
    # Observe until the expected frame is painted; never repeat the zoom/reload input.
    until = time.monotonic() + 12
    last = None
    while time.monotonic() < until:
        last = pixel_bounds(window, name, color, size)
        if last is not None and check(last):
            return last
        time.sleep(0.05)
    raise AssertionError(f"Image pixels/bounds did not become ready: {name}: {last}")


def pixel_bounds(window, name, color, size):
    screenshot(window, name)
    raw = subprocess.check_output(["convert", str(ARTIFACTS / name), "-depth", "8", "rgb:-"])
    width, height = size
    assert len(raw) == width * height * 3
    points = []
    for y in range(160, height - 40):
        for x in range(310, width - 10):
            if raw[(y * width + x) * 3 : (y * width + x) * 3 + 3] == bytes(color):
                points.append((x, y))
    if len(points) <= 100:
        return None
    return (
        min(x for x, _ in points),
        min(y for _, y in points),
        max(x for x, _ in points),
        max(y for _, y in points),
    )


def run(remote=False):
    label = "ssh" if remote else "local"
    with fixture_directory(prefix="rode-images-ui-") as base:
        root = base / "pictures"
        root.mkdir()
        source = root / "notes.txt"
        source.write_text("text remains editable\n")
        image = root / "alpha.png"
        png(image)
        subprocess.run(
            [
                "convert",
                str(image),
                "-background",
                "black",
                "-alpha",
                "remove",
                str(root / "photo.JPG"),
            ],
            check=True,
        )
        (root / "corrupt.png").write_bytes(b"not a PNG")
        original = hashlib.sha256(image.read_bytes()).digest()
        state_path = base / "ui-state.json"
        environment = dict(
            os.environ,
            XDG_CONFIG_HOME=str(base / "config"),
            XDG_DATA_HOME=str(base / "data"),
            RODE_UI_TEST_STATE=str(state_path),
        )
        args = [str(BINARY), "--sandbox"]
        if remote:
            real_ssh = shutil.which("ssh")
            tools = base / "tools"
            tools.mkdir()
            wrapper = tools / "ssh"
            wrapper.write_text(
                "#!/usr/bin/python3\nimport os, sys\nbinary = "
                + repr(real_ssh)
                + "\nos.execv(binary, [binary] + ['-T' if a == '-tt' else a for a in sys.argv[1:]])\n"
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
        log_path = ARTIFACTS / (label + ".log")
        with log_path.open("w") as log:
            process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
            window = None
            try:
                window = wait_for_window(process, log_path)
                wait_for_project(window, process, log_path, root)
                focus_window(window)
                ui = NativeUI(window, process, log_path, state_path, root)
                ui.open_file("alpha.png")
                normal = pixels(
                    window,
                    label + "-png-phosphor.png",
                    (224, 40, 64),
                    check=lambda b: 118 <= b[2] - b[0] <= 121 and 158 <= b[3] - b[1] <= 161,
                )
                assert 118 <= normal[2] - normal[0] <= 121 and 158 <= normal[3] - normal[1] <= 161
                key("equal", ("Control_L",))
                pixels(
                    window,
                    label + "-first-zoom.png",
                    (224, 40, 64),
                    check=lambda b: b[2] - b[0] > (normal[2] - normal[0]) * 1.2,
                )
                key("equal", ("Control_L",))
                enlarged = pixels(
                    window,
                    label + "-zoom.png",
                    (224, 40, 64),
                    check=lambda b: b[2] - b[0] > (normal[2] - normal[0]) * 1.4,
                )
                assert enlarged[2] - enlarged[0] > (normal[2] - normal[0]) * 1.4
                ui.dismiss_notice()
                key("s", ("Control_L",))
                ui.notice("Image and PDF previews are read-only.")
                ui.dismiss_notice()
                key("s", ("Control_L", "Shift_L"))
                ui.notice("Image and PDF previews are read-only.")
                ui.dismiss_notice()
                assert hashlib.sha256(image.read_bytes()).digest() == original
                ui.open_file("notes.txt")
                ui.control("source-editor", focused=True)
                key("End", ("Control_L",))
                text("preserved while viewing images\n")
                ui.source("text remains editable\npreserved while viewing images\n")
                ui.open_file("photo.JPG")
                screenshot(window, label + "-jpeg.png")
                ui.open_file("alpha.png")
                png(image, alternate=True)
                ui.command("Image: Reload preview")
                pixels(window, label + "-reloaded.png", (240, 160, 24))
                ui.command("Theme: Carbon")
                screenshot(window, label + "-carbon.png")
                ui.resize(850, 560)
                screenshot(window, label + "-compact.png")
                # Exercise a persistent notice at the compact size, where it can
                # cover the image. Acknowledge it before measuring rendered bounds.
                ui.open_file("corrupt.png", expect_error=True)
                pixels(
                    window,
                    label + "-corrupt-keeps-current-tab.png",
                    (240, 160, 24),
                    size=(850, 560),
                )
                click_button(window, process, log_path, state_path, "notice-dismiss")
                wait_for_ui_state(
                    process,
                    log_path,
                    state_path,
                    lambda state: all(
                        button["name"] != "notice-dismiss" for button in state["buttons"]
                    ),
                    "Image error notification did not close",
                )
                click_button(window, process, log_path, state_path, "image-fit")
                fitted = pixels(
                    window,
                    label + "-compact-fit.png",
                    (240, 160, 24),
                    size=(850, 560),
                    check=lambda b: 238 <= b[2] - b[0] <= 241 and 158 <= b[3] - b[1] <= 161,
                )
                assert 238 <= fitted[2] - fitted[0] <= 241, fitted
                assert 158 <= fitted[3] - fitted[1] <= 161, fitted
                before = Path(f"/proc/{process.pid}/stat").read_text().split()
                time.sleep(1)
                after = Path(f"/proc/{process.pid}/stat").read_text().split()
                ticks = sum(map(int, after[13:15])) - sum(map(int, before[13:15]))
                memory = re.search(
                    r"VmRSS:\s+(\d+) kB", Path(f"/proc/{process.pid}/status").read_text()
                )
                print(f"{label}: image idle {ticks} ticks/s; RSS {int(memory[1]) / 1024:.1f} MiB")
                ui.resize(1380, 900)
                ui.open_file("notes.txt")
                key("s", ("Control_L",))
                wait_for(
                    lambda: "preserved while viewing images" in source.read_text(),
                    "Image tabs lost the text buffer",
                )
                close_window(window, process)
                print(
                    label
                    + ": native image tabs, zoom, reload, themes, read-only saves, and text preservation passed."
                )
            except Exception:
                if window is not None and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                stop_process(process)


case = os.environ.get("RODE_IMAGES_UI_CASE", "all")
if case in ("all", "local"):
    run()
if case in ("all", "ssh") and os.environ.get("RODE_TEST_SSH_CONFIG"):
    run(remote=True)
