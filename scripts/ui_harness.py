#!/usr/bin/env python3
"""Native UI smoke test. Run on an isolated X display of at least 1380x900,
e.g. xvfb-run -a -s "-screen 0 1600x1100x24".

Requires Linux, libX11, libXtst, `xwininfo` (x11-utils), and ImageMagick's `import`.
Uses only Python's standard library. Keyboard events and screenshots target this test's window.
"""

import ctypes as c
import os
import re
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("RODE_TEST_BINARY", ROOT / "target/debug/rode"))
ARTIFACTS = Path(os.environ.get("RODE_UI_ARTIFACTS", ROOT / "target/ui-smoke"))
ARTIFACTS.mkdir(parents=True, exist_ok=True)
os.environ.setdefault("RODE_SESSION_DIR", str(ARTIFACTS / "session-runtime"))

x11 = c.CDLL("libX11.so.6")
xtest = c.CDLL("libXtst.so.6")
x11.XOpenDisplay.restype = c.c_void_p
x11.XOpenDisplay.argtypes = [c.c_char_p]
x11.XStringToKeysym.restype = c.c_ulong
x11.XStringToKeysym.argtypes = [c.c_char_p]
x11.XKeysymToKeycode.restype = c.c_uint
x11.XKeysymToKeycode.argtypes = [c.c_void_p, c.c_ulong]
x11.XSetInputFocus.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_ulong]
x11.XFlush.argtypes = [c.c_void_p]
xtest.XTestFakeKeyEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
xtest.XTestFakeMotionEvent.argtypes = [c.c_void_p, c.c_int, c.c_int, c.c_int, c.c_ulong]
xtest.XTestFakeButtonEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
display = x11.XOpenDisplay(None)
assert display, (
    'No X display. Use xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-ui.py'
)


def event(name, pressed):
    """Queue one XTest key transition on the explicitly selected isolated display."""
    code = x11.XKeysymToKeycode(display, x11.XStringToKeysym(name.encode()))
    assert code, name
    xtest.XTestFakeKeyEvent(display, code, pressed, 0)


def key(name, modifiers=()):
    """Press/release modifiers and a key, flush X events and allow a native frame to process them."""
    for modifier in modifiers:
        event(modifier, 1)
    event(name, 1)
    event(name, 0)
    for modifier in reversed(modifiers):
        event(modifier, 0)
    x11.XFlush(display)
    time.sleep(0.08)


def text(value):
    """Type supported test text using real key events rather than mutating application buffers."""
    specials = {
        " ": ("space", False),
        "\n": ("Return", False),
        "_": ("minus", True),
        ":": ("semicolon", True),
        "'": ("apostrophe", False),
        '"': ("apostrophe", True),
        "(": ("9", True),
        ")": ("0", True),
        "{": ("bracketleft", True),
        "}": ("bracketright", True),
        "+": ("equal", True),
        ".": ("period", False),
        "-": ("minus", False),
        "/": ("slash", False),
        ">": ("period", True),
        "=": ("equal", False),
        "$": ("4", True),
        "#": ("3", True),
        "!": ("1", True),
        "[": ("bracketleft", False),
        "]": ("bracketright", False),
        "\\": ("backslash", False),
        "|": ("backslash", True),
        "&": ("7", True),
        ";": ("semicolon", False),
        ",": ("comma", False),
        "%": ("5", True),
        "*": ("8", True),
        "?": ("slash", True),
    }
    for char in value:
        name, shift = specials.get(char, (char.lower(), char.isupper()))
        key(name, ("Shift_L",) if shift else ())


def click(x, y):
    """Click absolute display coordinates; UI tests use a known window size/position."""
    xtest.XTestFakeMotionEvent(display, -1, x, y, 0)
    xtest.XTestFakeButtonEvent(display, 1, 1, 0)
    xtest.XTestFakeButtonEvent(display, 1, 0, 0)
    x11.XFlush(display)
    time.sleep(0.2)


def assert_running(process, log_path):
    """Report early native startup failure with bounded stderr, rather than a bare assert."""
    code = process.poll()
    if code is None:
        return
    path = Path(log_path)
    with path.open("rb") as log:
        log.seek(0, os.SEEK_END)
        log.seek(max(0, log.tell() - 16 * 1024))
        output = log.read().decode("utf-8", errors="replace").strip()
    raise AssertionError(
        f"RODE exited before its window appeared (exit code {code}).\n"
        f"Startup log: {path}\n{output or '(no startup output)'}"
    )


def wait_for_window(process, log_path, timeout=20):
    """Wait for a viewable RODE window, not merely its creation in the X tree."""
    deadline = time.monotonic() + timeout
    environment = dict(os.environ, LC_ALL="C.UTF-8")
    states = {}
    while time.monotonic() < deadline:
        assert_running(process, log_path)
        tree = subprocess.check_output(
            ["xwininfo", "-root", "-tree"], text=True, env=environment, timeout=3
        )
        states = {}
        for window in re.findall(r'(0x[0-9a-f]+) "RODE', tree):
            attributes = subprocess.run(
                ["xwininfo", "-id", window],
                capture_output=True,
                text=True,
                env=environment,
                timeout=3,
            )
            # A window may disappear between the tree and attribute queries.
            if attributes.returncode:
                states[window] = "disappeared"
                continue
            state = re.search(r"Map State:\s*(\w+)", attributes.stdout)
            states[window] = state[1] if state else "unknown"
            if states[window] == "IsViewable":
                return int(window, 16)
        time.sleep(0.05)
    raise AssertionError(
        f"No viewable RODE window within {timeout}s. Map states: {states}. Startup log: {log_path}"
    )


def screenshot(window, name):
    """Capture this test window after a render opportunity into its artifact directory."""
    time.sleep(0.25)
    subprocess.run(
        [
            "import",
            "-display",
            os.environ["DISPLAY"],
            "-window",
            hex(window),
            str(ARTIFACTS / name),
        ],
        check=True,
    )


def theme(name):
    """Select an interface theme through the real command palette."""
    key("p", ("Control_L", "Shift_L"))
    text("theme: " + name)
    key("Return")
    time.sleep(0.3)


def saved_theme(directory, expected):
    # Check the actual preference file while the app is still running, before
    # normal exit can invoke its save hook.
    """Wait for an immediate preference-file write while the application remains open."""
    preferences = Path(directory) / "data-home/rode/app.ron"
    deadline = time.monotonic() + 3.0
    while time.monotonic() < deadline:
        if preferences.exists():
            match = re.search(r'"color_theme"\s*:\s*"([^"]+)"', preferences.read_text())
            if match and match[1] == expected:
                return
        time.sleep(0.05)
    raise AssertionError(f"Theme {expected!r} was not saved immediately to {preferences}")


def background(window, filename, expected):
    """Capture a frame and assert the known editor-background sample pixel."""
    screenshot(window, filename)
    pixel = subprocess.check_output(
        ["convert", str(ARTIFACTS / filename), "-format", "%[pixel:p{1300,400}]", "info:"],
        text=True,
    )
    actual = tuple(map(int, re.findall(r"\d+", pixel)[:3]))
    assert actual == expected, (filename, pixel, expected)


class ClientData(c.Union):
    """ctypes representation of XClientMessageEvent data with native Xlib alignment."""

    _fields_ = [("b", c.c_char * 20), ("s", c.c_short * 10), ("l", c.c_long * 5)]


class ClientMessage(c.Structure):
    """ctypes layout required for a correctly aligned WM_DELETE_WINDOW event."""

    _fields_ = [
        ("type", c.c_int),
        ("serial", c.c_ulong),
        ("send_event", c.c_int),
        ("display", c.c_void_p),
        ("window", c.c_ulong),
        ("message_type", c.c_ulong),
        ("format", c.c_int),
        ("data", ClientData),
    ]


class XEvent(c.Union):
    """Native-size Xlib event union for sending a window-manager close request."""

    _fields_ = [("client", ClientMessage), ("padding", c.c_long * 24)]


x11.XInternAtom.argtypes = [c.c_void_p, c.c_char_p, c.c_int]
x11.XInternAtom.restype = c.c_ulong
x11.XSendEvent.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_long, c.POINTER(XEvent)]


def request_close(window):
    """Ask this window to close through its normal WM protocol, preserving application guards."""
    event = XEvent()
    event.client.type = 33
    event.client.send_event = 1
    event.client.display = display
    event.client.window = window
    event.client.message_type = x11.XInternAtom(display, b"WM_PROTOCOLS", 0)
    event.client.format = 32
    event.client.data.l[0] = x11.XInternAtom(display, b"WM_DELETE_WINDOW", 0)
    x11.XSendEvent(display, window, 0, 0, c.byref(event))
    x11.XFlush(display)


def close_window(window, process):
    """Request normal close and verify successful exit; session persistence remains the app's decision."""
    request_close(window)
    # Each session's explicit Persist setting determines its lifetime. Tests
    # never answer a terminal-policy prompt; source-buffer guards still apply.
    assert process.wait(timeout=15) == 0
