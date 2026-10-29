#!/usr/bin/env python3
"""Native raster tabs, zoom, reload, and edit protection, locally or over SSH."""

import ctypes
import hashlib
import os
import re
import shutil
import struct
import subprocess
import tempfile
import time
import zlib
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/images-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    click,
    close_window,
    display,
    key,
    screenshot,
    text,
    theme,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


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


def command(name):
    key("p", ("Control_L", "Shift_L"))
    text(name)
    key("Return")
    time.sleep(0.2)


def open_file(name):
    command("Open file by path")
    text(name)
    key("Return")
    time.sleep(0.6)


def wait_for(check, message):
    until = time.monotonic() + 10
    while time.monotonic() < until:
        if check():
            return
        time.sleep(0.05)
    raise AssertionError(message)


def pixels(window, name, color, size=(1380, 900)):
    screenshot(window, name)
    raw = subprocess.check_output(["convert", str(ARTIFACTS / name), "-depth", "8", "rgb:-"])
    width, height = size
    assert len(raw) == width * height * 3
    points = []
    for y in range(160, height - 40):
        for x in range(310, width - 10):
            if raw[(y * width + x) * 3 : (y * width + x) * 3 + 3] == bytes(color):
                points.append((x, y))
    assert len(points) > 100, "Image pixels did not render: " + name
    return (
        min(x for x, _ in points),
        min(y for _, y in points),
        max(x for x, _ in points),
        max(y for _, y in points),
    )


def run(remote=False):
    label = "ssh" if remote else "local"
    with tempfile.TemporaryDirectory(prefix="rode-images-ui-") as directory:
        base = Path(directory)
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
        environment = dict(
            os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
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
        with (ARTIFACTS / (label + ".log")).open("w") as log:
            process = subprocess.Popen(args, env=environment, stdout=log, stderr=log)
            window = None
            try:
                for _ in range(150):
                    found = re.search(
                        r'(0x[0-9a-f]+) "RODE',
                        subprocess.check_output(["xwininfo", "-root", "-tree"], text=True),
                    )
                    if found:
                        window = int(found[1], 16)
                        break
                    assert process.poll() is None
                    time.sleep(0.05)
                assert window
                time.sleep(0.8)
                x11.XSetInputFocus(display, window, 1, 0)
                x11.XFlush(display)
                open_file("alpha.png")
                normal = pixels(window, label + "-png-phosphor.png", (224, 40, 64))
                assert 118 <= normal[2] - normal[0] <= 121 and 158 <= normal[3] - normal[1] <= 161
                key("equal", ("Control_L",))
                key("equal", ("Control_L",))
                enlarged = pixels(window, label + "-zoom.png", (224, 40, 64))
                assert enlarged[2] - enlarged[0] > (normal[2] - normal[0]) * 1.4
                key("s", ("Control_L",))
                key("s", ("Control_L", "Shift_L"))
                assert hashlib.sha256(image.read_bytes()).digest() == original
                open_file("notes.txt")
                key("End", ("Control_L",))
                text("preserved while viewing images\n")
                open_file("photo.JPG")
                screenshot(window, label + "-jpeg.png")
                open_file("alpha.png")
                png(image, alternate=True)
                command("Image: Reload preview")
                pixels(window, label + "-reloaded.png", (240, 160, 24))
                theme("carbon")
                screenshot(window, label + "-carbon.png")
                x11.XResizeWindow(display, window, 850, 560)
                x11.XFlush(display)
                screenshot(window, label + "-compact.png")
                click(319, 186)  # Fit remains accessible with the terminal open.
                fitted = pixels(window, label + "-compact-fit.png", (240, 160, 24), size=(850, 560))
                assert fitted[2] - fitted[0] < 130 and fitted[3] - fitted[1] < 80
                time.sleep(0.4)
                before = Path(f"/proc/{process.pid}/stat").read_text().split()
                time.sleep(1)
                after = Path(f"/proc/{process.pid}/stat").read_text().split()
                ticks = sum(map(int, after[13:15])) - sum(map(int, before[13:15]))
                memory = re.search(
                    r"VmRSS:\s+(\d+) kB", Path(f"/proc/{process.pid}/status").read_text()
                )
                print(f"{label}: image idle {ticks} ticks/s; RSS {int(memory[1]) / 1024:.1f} MiB")
                x11.XResizeWindow(display, window, 1380, 900)
                x11.XFlush(display)
                time.sleep(0.3)
                open_file("corrupt.png")
                pixels(window, label + "-corrupt-keeps-current-tab.png", (240, 160, 24))
                open_file("notes.txt")
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
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)


case = os.environ.get("RODE_IMAGES_UI_CASE", "all")
if case in ("all", "local"):
    run()
if case in ("all", "ssh") and os.environ.get("RODE_TEST_SSH_CONFIG"):
    run(remote=True)
