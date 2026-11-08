#!/usr/bin/env python3
"""Native UI smoke test. Run on a headless X display of at least 1380x900,
e.g. python3 scripts/ui_display.py -- python3 scripts/test-ui.py.

Never use the user's desktop or a visible Xephyr fallback for automated input.

Requires Linux, libX11, libXtst, `xwininfo` (x11-utils), and ImageMagick's `import`.
Uses only Python's standard library. Keyboard events and screenshots target this test's window.
"""

import ctypes as c
import json
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
x11.XRaiseWindow.argtypes = [c.c_void_p, c.c_ulong]
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
        "@": ("2", True),
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


def move_pointer(x, y):
    """Move without activating anything, allowing the native hit test to acknowledge the target."""
    xtest.XTestFakeMotionEvent(display, -1, x, y, 0)
    x11.XFlush(display)


def click(x, y):
    """Click absolute display coordinates; UI tests use a known window size/position."""
    move_pointer(x, y)
    xtest.XTestFakeButtonEvent(display, 1, 1, 0)
    xtest.XTestFakeButtonEvent(display, 1, 0, 0)
    x11.XFlush(display)
    time.sleep(0.2)


def read_json_lines(path):
    """Read complete fixture acknowledgements while a peer may still be appending.

    Ignore only the unterminated final record, including a partial UTF-8 code
    point. Malformed completed records remain errors rather than disappearing.
    """
    try:
        data = Path(path).read_bytes()
    except FileNotFoundError:
        return []
    complete = data[: data.rfind(b"\n") + 1]
    return [json.loads(line) for line in complete.splitlines()]


def window_geometry(window):
    """Return client origin and physical size, including windows moved by a window manager."""
    info = subprocess.check_output(
        ["xwininfo", "-id", hex(window)],
        text=True,
        env=dict(os.environ, LC_ALL="C.UTF-8"),
        timeout=3,
    )
    return tuple(
        int(re.search(rf"{label}:\s*(-?\d+)", info)[1])
        for label in ("Absolute upper-left X", "Absolute upper-left Y", "Width", "Height")
    )


def wait_for_button(
    window,
    process,
    log_path,
    state_path,
    name,
    timeout=20,
    *,
    focused=False,
    changed_after=None,
    after_pass=None,
    hovered=False,
):
    """Wait for an enabled, visible registered control and resolve its current physical centre.

    RODE_UI_TEST_STATE opts the application into atomic, read-only layout snapshots.
    Input still goes through XTest; metadata cannot activate a control or start work.
    Optional focus/edit revision guards wait for dependent input to be processed,
    including when one busy native frame outlasts several keyboard shortcuts.
    after_pass also waits for cursor/selection changes that do not edit the buffer.
    hovered moves the pointer and requires the native hit test to accept it before
    returning. Follow moving controls without retrying any activating input.
    """
    deadline = time.monotonic() + timeout
    last = "No layout snapshot received"
    pointer = None
    pointer_pass = None
    while time.monotonic() < deadline:
        assert_running(process, log_path)
        try:
            state = json.loads(Path(state_path).read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            time.sleep(0.05)
            continue
        last = repr(state)
        if state.get("pid") == process.pid and (after_pass is None or state["pass"] > after_pass):
            for button in state["buttons"]:
                if button["name"] != name or (not button["enabled"] and not hovered):
                    continue
                if focused and not button.get("focused", False):
                    continue
                if changed_after is not None and (
                    button.get("changed_pass") is None or button["changed_pass"] <= changed_after
                ):
                    continue
                x, y, width, height = window_geometry(window)
                scale = state["pixels_per_point"]
                # Wait for the resize to be rendered before using its control bounds.
                if any(
                    abs(actual - logical * scale) > 2
                    for actual, logical in zip((width, height), state["size"])
                ):
                    break
                left, top, right, bottom = button["rect"]
                if right > left and bottom > top:
                    point = (
                        round(x + (left + right) * scale / 2),
                        round(y + (top + bottom) * scale / 2),
                    )
                    if hovered:
                        if point != pointer:
                            move_pointer(*point)
                            pointer = point
                            pointer_pass = state["pass"]
                        if (
                            state["pass"] <= pointer_pass
                            or not button["enabled"]
                            or not button.get("hovered", False)
                        ):
                            break
                    return point
        time.sleep(0.05)
    raise AssertionError(
        f"No enabled {name!r} control within {timeout}s. Last layout: {last}. Log: {log_path}"
    )


def click_button(window, process, log_path, state_path, name, timeout=20):
    """Click once after the real control acknowledges hover and input readiness."""
    focus_window(window)
    point = wait_for_button(window, process, log_path, state_path, name, timeout, hovered=True)
    click(*point)


def wait_for_ui_state(process, log_path, state_path, check, description, timeout=12):
    """Require a valid snapshot from this process before testing an observable UI condition."""
    deadline = time.monotonic() + timeout
    last = "No layout snapshot received"
    while time.monotonic() < deadline:
        assert_running(process, log_path)
        try:
            state = json.loads(Path(state_path).read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            time.sleep(0.05)
            continue
        last = repr(state)
        if state.get("pid") == process.pid and check(state):
            return state
        time.sleep(0.05)
    raise AssertionError(f"{description} within {timeout}s. Last layout: {last}. Log: {log_path}")


def palette_command(window, process, log_path, state_path, value, timeout=12):
    """Submit once, after focus and the complete query reach the native palette.

    The exact-match control is registered only after all query characters were
    processed. A matching prefix or a new paint alone cannot acknowledge typing.
    """
    focus_window(window)
    key("p", ("Control_L", "Shift_L"))
    wait_for_button(
        window, process, log_path, state_path, "command-palette-input", timeout, focused=True
    )
    text(value)
    click_button(window, process, log_path, state_path, "command-match:" + value, timeout)
    wait_for_ui_state(
        process,
        log_path,
        state_path,
        lambda state: all(button["name"] != "command-palette-input" for button in state["buttons"]),
        "Command palette did not close after submitting " + value,
        timeout,
    )


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
        f"RODE exited while waiting for the UI (exit code {code}).\n"
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


def wait_for_project(window, process, log_path, project, timeout=20):
    """Wait for project preparation before sending shortcuts or using layout coordinates."""
    deadline = time.monotonic() + timeout
    environment = dict(os.environ, LC_ALL="C.UTF-8")
    expected = f"RODE · {Path(project).name}"
    title = ""
    while time.monotonic() < deadline:
        assert_running(process, log_path)
        attributes = subprocess.run(
            ["xwininfo", "-id", hex(window)],
            capture_output=True,
            text=True,
            env=environment,
            timeout=3,
        )
        match = re.search(r'^xwininfo: Window id: \S+ "(.*)"$', attributes.stdout, re.MULTILINE)
        title = match[1] if match and attributes.returncode == 0 else ""
        if (
            title == expected or title.startswith(expected + " [")
        ) and "[Opening project]" not in title:
            return
        time.sleep(0.05)
    raise AssertionError(
        f"Project {project} was not ready within {timeout}s. Title: {title!r}. Log: {log_path}"
    )


def focus_window(window, timeout=20):
    """Raise and focus a mapped window so another fixture view cannot intercept mouse input."""
    deadline = time.monotonic() + timeout
    environment = dict(os.environ, LC_ALL="C.UTF-8")
    state = "unknown"
    while time.monotonic() < deadline:
        attributes = subprocess.run(
            ["xwininfo", "-id", hex(window)],
            capture_output=True,
            text=True,
            env=environment,
            timeout=3,
        )
        mapped = re.search(r"Map State:\s*(\w+)", attributes.stdout)
        state = mapped[1] if attributes.returncode == 0 and mapped else "disappeared"
        if state == "IsViewable":
            x11.XRaiseWindow(display, window)
            x11.XSetInputFocus(display, window, 1, 0)
            x11.XFlush(display)
            return
        time.sleep(0.05)
    raise AssertionError(f"Cannot focus RODE window {hex(window)} within {timeout}s: {state}")


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
