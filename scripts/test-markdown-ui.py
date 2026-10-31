#!/usr/bin/env python3
"""Native Markdown previews, undo/save protection, and sandboxed local/SSH images."""

import ctypes
import json
import os
import re
import shutil
import struct
import subprocess
import tempfile
import time
import zlib
from pathlib import Path

os.environ["RODE_UI_ARTIFACTS"] = str(Path(__file__).resolve().parent.parent / "target/markdown-ui")
from ui_harness import (
    ARTIFACTS,
    BINARY,
    close_window,
    display,
    key,
    screenshot,
    text,
    theme,
    x11,
)

x11.XResizeWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_uint, ctypes.c_uint]


def png(path, color):
    def chunk(kind, data):
        return (
            struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
        )

    path.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", 160, 60, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress((b"\0" + bytes(color) * 160) * 60))
        + chunk(b"IEND", b"")
    )


def wait_for(check, message):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.1)
    raise AssertionError(message)


def colors(window, name):
    screenshot(window, name)
    raw = subprocess.check_output(["convert", str(ARTIFACTS / name), "-depth", "8", "rgb:-"])
    counts = {}
    for y in range(160, 850):
        for x in range(310, 1370):
            color = raw[(y * 1380 + x) * 3 : (y * 1380 + x) * 3 + 3]
            counts[color] = counts.get(color, 0) + 1
    return counts


def run(remote=False):
    label = "ssh" if remote else "local"
    with tempfile.TemporaryDirectory(prefix="rode-markdown-ui-") as temporary:
        base = Path(temporary)
        root = base / "project"
        root.mkdir()
        png(root / "signal.png", (224, 40, 64))
        png(base / "private.png", (241, 91, 132))
        (root / "notes.txt").write_text("Linked file on the same host.\n")
        source = root / "README.md"
        original = """# Mission report

Native **Markdown**, *emphasis*, ~~old text~~, and `inline code`.

![Signal](signal.png)

## Checklist
- [x] Keep edits safe
- [ ] Review [notes](notes.txt)

| Host | State |
| --- | --- |
| local or SSH | ready |

> Preview the current buffer before saving.

```rust
fn main() { println!("ready"); }
```

![Hidden outside image](../private.png)
![External image](https://example.invalid/no-download.png)
"""
        source.write_text(original)
        environment = dict(
            os.environ, XDG_CONFIG_HOME=str(base / "config"), XDG_DATA_HOME=str(base / "data")
        )
        # Probe the same default policy with the actual executable, not the test process.
        probe = subprocess.run(
            [str(BINARY), "--file-helper"],
            input=json.dumps({"op": "prepare", "path": str(root), "sandbox": {"enabled": True}}),
            text=True,
            capture_output=True,
            check=True,
        )
        isolated = json.loads(probe.stdout)["project"]["active"]
        args = [str(BINARY), "--sandbox"]
        if remote:
            # The unprivileged fixture cannot allocate a login TTY on the host.
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
                time.sleep(1)
                x11.XSetInputFocus(display, window, 1, 0)
                x11.XFlush(display)
                key("p", ("Control_L", "Shift_L"))
                text("Open file by path")
                key("Return")
                text("README.md")
                key("Return")
                time.sleep(0.7)
                key("End", ("Control_L",))
                text("\n## Unsaved section\n")
                key(
                    "t", ("Control_L", "Shift_L")
                )  # Hide the terminal before rendering the fixture.
                key("r", ("Control_L", "Shift_L"))
                wait_for(
                    lambda: (
                        colors(window, label + "-preview.png").get(bytes((224, 40, 64)), 0) > 500
                    ),
                    "The Markdown preview did not render its relative image",
                )
                assert source.read_text() == original, "Preview silently saved the source"
                if isolated:
                    assert (
                        colors(window, label + "-sandbox.png").get(bytes((241, 91, 132)), 0) == 0
                    ), "Markdown image bypassed the sandbox"
                expected = original + "\n## Unsaved section\n"
                key("s", ("Control_L",))
                wait_for(
                    lambda: source.read_text() == expected,
                    "Preview save did not save Markdown source",
                )
                key("r", ("Control_L", "Shift_L"))
                key("z", ("Control_L",))
                key("s", ("Control_L",))
                wait_for(
                    lambda: source.read_text() != expected,
                    "Preview lost the text editor's undo history",
                )
                key("y", ("Control_L",))
                key("s", ("Control_L",))
                wait_for(
                    lambda: source.read_text() == expected, "Redo failed after returning to Edit"
                )
                text("cursor-kept\n")
                key("s", ("Control_L",))
                expected += "cursor-kept\n"
                wait_for(lambda: source.read_text() == expected, "Preview reset the source cursor")
                key("r", ("Control_L", "Shift_L"))
                theme("carbon")
                colors(window, label + "-carbon.png")
                x11.XResizeWindow(display, window, 850, 560)
                x11.XFlush(display)
                screenshot(window, label + "-compact.png")
                key("f", ("Control_L",))
                text("Mission")
                key("Escape")
                screenshot(window, label + "-find-returns-to-edit.png")
                close_window(window, process)
                print(
                    label
                    + ": Markdown rendering, images/sandbox scope, unsaved preview, save, undo/redo, cursor, themes, and find passed."
                )
            except Exception:
                if window is not None and process.poll() is None:
                    screenshot(window, label + "-failure.png")
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)


case = os.environ.get("RODE_MARKDOWN_UI_CASE", "all")
if case in ("all", "local"):
    run()
if case in ("all", "ssh") and os.environ.get("RODE_TEST_SSH_CONFIG"):
    run(remote=True)
