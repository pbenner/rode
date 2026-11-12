#!/usr/bin/env python3
"""Native UI smoke test. Run on a headless X display of at least 1380x900,
e.g. python3 scripts/ui_display.py -- python3 scripts/test-ui.py.

Never use the user's desktop or a visible Xephyr fallback for automated input.

Requires Linux, libX11, libXtst, `xwininfo` (x11-utils), and ImageMagick's `import`.
Uses only Python's standard library. Keyboard events and screenshots target this test's window.
"""

import contextlib
import ctypes as c
import hashlib
import json
import os
import re
import signal
import subprocess
import sys
import time
from pathlib import Path

from fixture_support import (
    decode_ron_string as decode_ron_string,
)
from fixture_support import fixture_artifacts
from fixture_support import (
    fixture_directory as fixture_directory,
)
from fixture_support import (
    fixture_environment as fixture_environment,
)
from fixture_support import (
    read_preferences as read_preferences,
)
from fixture_support import (
    write_python_launcher as write_python_launcher,
)

ROOT = Path(__file__).resolve().parent.parent
BINARY = Path(os.environ.get("RODE_TEST_BINARY", ROOT / "target/debug/rode"))
ARTIFACTS = fixture_artifacts(
    ROOT,
    os.environ.get("RODE_UI_ARTIFACTS", ROOT / "target/ui-smoke"),
    os.environ.get("RODE_CI_RESULTS_DIR"),
)
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
    """Pace physical key transitions; callers must acknowledge focus and effects separately."""
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


def click(x, y, *, button=1):
    """Click absolute display coordinates; UI tests use a known window size/position."""
    move_pointer(x, y)
    xtest.XTestFakeButtonEvent(display, button, 1, 0)
    xtest.XTestFakeButtonEvent(display, button, 0, 0)
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


def viewport_state_path(process, log_path, state_path, title, timeout=12):
    """Find this process's snapshot for a named native child window.

    Child viewports publish sibling files so controls from equal-sized windows
    cannot be mistaken for one another. Input remains routed through XTest.
    """
    state_path = Path(state_path)
    deadline = time.monotonic() + timeout
    observed = []
    while time.monotonic() < deadline:
        assert_running(process, log_path)
        observed = []
        for path in sorted(state_path.parent.glob(state_path.name + ".*.json")):
            try:
                state = json.loads(path.read_text())
            except (FileNotFoundError, json.JSONDecodeError):
                continue
            if state.get("pid") == process.pid:
                observed.append(state.get("title"))
                if state.get("title") == title:
                    return path
        time.sleep(0.05)
    raise AssertionError(
        f"No UI snapshot for {title!r} within {timeout}s. Observed: {observed}. Log: {log_path}"
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
    enabled=True,
):
    """Wait for an enabled, visible registered control and resolve its current physical centre.

    RODE_UI_TEST_STATE opts the application into atomic, read-only layout snapshots.
    Input still goes through XTest; metadata cannot activate a control or start work.
    Optional focus/edit revision guards wait for dependent input to be processed,
    including when one busy native frame outlasts several keyboard shortcuts.
    after_pass also waits for cursor/selection changes that do not edit the buffer.
    hovered moves the pointer and requires the native hit test to accept it before
    returning. Follow moving controls without retrying any activating input.
    enabled=False resolves an explicitly disabled placeholder for negative-input tests.
    """
    if hovered and not enabled:
        raise ValueError("Disabled controls cannot acknowledge hover")
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
                if button["name"] != name or (button["enabled"] != enabled and not hovered):
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
        f"No {'enabled' if enabled else 'disabled'} {name!r} control within {timeout}s. "
        f"Last layout: {last}. Log: {log_path}"
    )


def click_button(window, process, log_path, state_path, name, timeout=20, *, mouse_button=1):
    """Click once after the real control acknowledges hover and input readiness."""
    focus_window(window)
    point = wait_for_button(window, process, log_path, state_path, name, timeout, hovered=True)
    click(*point, button=mouse_button)


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


def wait_for_effect(check, message, *, timeout=20, process=None, log_path=None):
    """Observe an effect without resending input; allow expected post-exit cleanup checks."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process is not None:
            assert_running(process, log_path)
        try:
            value = check()
        except FileNotFoundError:
            value = None
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(message)


class NativeUI:
    """Drive native controls using focus, complete input and document acknowledgements.

    Polling only observes the application; activating actions are sent once via
    XTest. Timeouts report the process log and last accepted layout.
    """

    def __init__(self, window, process, log_path, state_path, project=None):
        self.window = window
        self.process = process
        self.log_path = log_path
        self.state_path = state_path
        self.project_root = Path(project) if project is not None else None

    def layout(self, check=lambda state: True, description="No rendered layout", timeout=12):
        return wait_for_ui_state(
            self.process, self.log_path, self.state_path, check, description, timeout
        )

    def control(self, name, **guards):
        return wait_for_button(
            self.window, self.process, self.log_path, self.state_path, name, **guards
        )

    def click(self, name, *, mouse_button=1):
        click_button(
            self.window,
            self.process,
            self.log_path,
            self.state_path,
            name,
            mouse_button=mouse_button,
        )

    def absent(self, name, after_pass=None):
        return self.layout(
            lambda state: (
                (after_pass is None or state["pass"] > after_pass)
                and all(button["name"] != name for button in state["buttons"])
            ),
            name + " did not close",
        )

    def command(self, value, *, keyboard=False):
        palette_command(
            self.window, self.process, self.log_path, self.state_path, value, keyboard=keyboard
        )

    def navigate(self, name, modifiers=(), *, leader="space", leader_modifiers=("Control_L",)):
        """Send a leader chord once; wait until each half is processed, without clicking."""
        before = self.layout()["pass"]
        with delayed_input(self.process):
            key(leader, leader_modifiers)
        self.control("navigation-leader", after_pass=before)
        before = self.layout()["pass"]
        with delayed_input(self.process):
            key(name, modifiers)
        self.absent("navigation-leader", after_pass=before)

    def document(self, path, *, focused=True):
        """Observe the active path and editor focus without changing either."""
        document = Path(path)
        if not document.is_absolute():
            if self.project_root is None:
                raise ValueError("Relative documents require a project path")
            document = self.project_root / document
        self.value("active-document", str(document))
        if focused:
            self.control("source-editor", focused=True)

    def notice(self, value):
        digest = hashlib.sha256(value.encode()).hexdigest()
        self.control("notice-value:" + digest)

    def value(self, name, value, **guards):
        return self.control(name + ":" + hashlib.sha256(str(value).encode()).hexdigest(), **guards)

    def source(self, value, *, changed_after=None):
        """Wait for complete small-buffer text before the next modal can consume its keys."""
        if len(value.encode()) > 64 * 1024:
            raise ValueError("Source value acknowledgements are limited to 64 KiB")
        digest = hashlib.sha256(value.encode()).hexdigest()
        self.control("source-editor-value:" + digest, focused=True, changed_after=changed_after)

    def dismiss_notice(self):
        if any(button["name"] == "notice-dismiss" for button in self.layout()["buttons"]):
            before = self.layout()["pass"]
            self.click("notice-dismiss")
            self.absent("notice-dismiss", after_pass=before)

    def fill_path(self, value):
        self.fill("file-dialog-path", value)

    def fill(self, name, value):
        """Replace a probe-backed input and wait for its complete value and focus."""
        self.control(name, focused=True)
        before = self.layout()["pass"]
        key("a", ("Control_L",))
        self.control(name, focused=True, after_pass=before)
        with delayed_input(self.process):
            text(str(value))
        digest = hashlib.sha256(str(value).encode()).hexdigest()
        self.control(name + "-value:" + digest, focused=True)

    def open_file(self, path, *, expect_error=False):
        if expect_error:
            self.dismiss_notice()
        self.command("Open file by path")
        self.fill_path(path)
        before = self.layout()["pass"]
        key("Return")
        self.absent("file-dialog-path", after_pass=before)
        if expect_error:
            self.control("notice-dismiss")
        else:
            self.document(path, focused=False)

    def resize(self, width, height):
        x11.XResizeWindow.argtypes = [c.c_void_p, c.c_ulong, c.c_uint, c.c_uint]
        before = self.layout()["pass"]
        x11.XResizeWindow(display, self.window, width, height)
        x11.XFlush(display)
        return self.layout(
            lambda state: (
                state["pass"] > before
                and all(
                    abs(actual - logical * state["pixels_per_point"]) <= 2
                    for actual, logical in zip((width, height), state["size"])
                )
            ),
            f"Window did not render at {width} × {height}",
        )

    def wait(self, check, message, *, timeout=20):
        """Poll an effect; missing files are transient, other fixture errors remain failures."""
        return wait_for_effect(
            check,
            f"{message}. Log: {self.log_path}; layout: {self.state_path}",
            timeout=timeout,
            process=self.process,
            log_path=self.log_path,
        )

    def edit(self, name, value):
        self.click(name)
        self.control(name, focused=True)
        before = self.layout()["pass"]
        key("a", ("Control_L",))
        if value:
            text(value)
        else:
            key("BackSpace")
        self.control(name, focused=True, changed_after=before)
        self.copy(name, value)

    def clipboard(self):
        """An absent or not-yet-serving clipboard owner is a pending acknowledgement."""
        try:
            result = subprocess.run(
                ["xclip", "-selection", "clipboard", "-o"],
                capture_output=True,
                text=True,
                timeout=1,
            )
        except subprocess.TimeoutExpired:
            return None
        return result.stdout if result.returncode == 0 else None

    def copy(self, name, expected):
        """Verify the full native field value without publishing document text in metadata."""
        self.control(name, focused=True)
        before = self.layout()["pass"]
        key("a", ("Control_L",))
        self.control(name, focused=True, after_pass=before)
        key("c", ("Control_L",))
        self.wait(
            lambda: self.clipboard() == expected, name + " did not contain the complete value"
        )
        before = self.layout()["pass"]
        key("End", ("Control_L",))
        self.control(name, focused=True, after_pass=before)

    def press(self, name, key_name, modifiers=()):
        """Send one key to a focused control; callers must acknowledge its actual effect."""
        focus_window(self.window)
        self.control(name, focused=True)
        key(key_name, modifiers)

    def terminal_input(self, *, focused=True):
        """Observe an attached terminal; keyboard-only tests must retain their focus evidence."""
        state = self.layout(
            lambda state: any(
                button["name"].startswith("terminal-input:")
                and button["enabled"]
                and (not focused or button["focused"])
                for button in state["buttons"]
            ),
            "Project terminal did not become ready",
        )
        name = next(
            button["name"]
            for button in state["buttons"]
            if button["name"].startswith("terminal-input:")
            and button["enabled"]
            and (not focused or button["focused"])
        )
        return name

    def terminal(self):
        """Select the terminal surface and acknowledge its attached, focused input."""
        self.click("workspace-tab:Terminal")
        name = self.terminal_input(focused=False)
        self.click(name)
        self.control(name, focused=True)
        return name


@contextlib.contextmanager
def delayed_input(process):
    """Queue input while the owned GUI is paused; resume before waiting for its acknowledgement."""
    if os.environ.get("RODE_TEST_SLOW_UI") != "1":
        yield
        return
    descriptor = os.pidfd_open(process.pid)
    try:
        signal.pidfd_send_signal(descriptor, signal.SIGSTOP)
        try:
            yield
        finally:
            try:
                signal.pidfd_send_signal(descriptor, signal.SIGCONT)
            except ProcessLookupError:
                pass
    finally:
        os.close(descriptor)


def palette_command(window, process, log_path, state_path, value, timeout=12, *, keyboard=False):
    """Submit once, after focus and the complete query reach the native palette.

    The exact-match control is registered only after all query characters were
    processed. A matching prefix or a new paint alone cannot acknowledge typing.
    """
    focus_window(window)
    with delayed_input(process):
        key("p", ("Control_L", "Shift_L"))
    wait_for_button(
        window, process, log_path, state_path, "command-palette-input", timeout, focused=True
    )
    with delayed_input(process):
        text(value)
    if keyboard:
        wait_for_button(window, process, log_path, state_path, "command-match:" + value, timeout)
        wait_for_button(
            window, process, log_path, state_path, "command-palette-input", timeout, focused=True
        )
        key("Return")
    else:
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
            owner = window_pid(int(window, 16))
            if owner != process.pid:
                states[window] = f"different process: {owner}"
                continue
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


def window_pid(window):
    """Resolve the native owner so another RODE process cannot satisfy readiness."""
    result = subprocess.run(
        ["xprop", "-id", hex(window), "_NET_WM_PID"],
        capture_output=True,
        text=True,
        env=dict(os.environ, LC_ALL="C.UTF-8"),
        timeout=3,
    )
    match = re.search(r"_NET_WM_PID\([^)]*\)\s*=\s*(\d+)", result.stdout)
    return int(match[1]) if result.returncode == 0 and match else None


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
        if title == expected or title.startswith(expected + " ["):
            # SSH names the candidate root before authentication/preparation.
            # Mapping that window is not an acknowledgement of a usable host.
            suffix = title[len(expected) :]
            if "[Opening project]" not in suffix and " · offline]" not in suffix:
                return
        time.sleep(0.05)
    raise AssertionError(
        f"Project {project} was not ready within {timeout}s. Title: {title!r}. Log: {log_path}"
    )


class XError(c.Structure):
    """Xlib error event used to catch windows disappearing between native requests."""

    _fields_ = [
        ("type", c.c_int),
        ("display", c.c_void_p),
        ("resourceid", c.c_ulong),
        ("serial", c.c_ulong),
        ("error_code", c.c_ubyte),
        ("request_code", c.c_ubyte),
        ("minor_code", c.c_ubyte),
    ]


x11.XSetErrorHandler.argtypes = [c.c_void_p]
x11.XSetErrorHandler.restype = c.c_void_p
x11.XSync.argtypes = [c.c_void_p, c.c_int]
x11.XGetInputFocus.argtypes = [c.c_void_p, c.POINTER(c.c_ulong), c.POINTER(c.c_int)]


def focus_request(window):
    """Trap only expected mapping races; Xlib's default handler otherwise exits Python."""
    errors = []

    @c.CFUNCTYPE(c.c_int, c.c_void_p, c.POINTER(XError))
    def on_error(_display, error):
        errors.append(error.contents.error_code)
        return 0

    previous = x11.XSetErrorHandler(c.cast(on_error, c.c_void_p))
    try:
        x11.XRaiseWindow(display, window)
        x11.XSetInputFocus(display, window, 1, 0)
        x11.XSync(display, 0)
        if any(code not in (3, 8) for code in errors):  # BadWindow, BadMatch
            raise AssertionError(f"Unexpected X11 errors focusing {hex(window)}: {errors}")
        if errors:
            return False
        focused, revert = c.c_ulong(), c.c_int()
        x11.XGetInputFocus(display, c.byref(focused), c.byref(revert))
        return focused.value == window
    finally:
        x11.XSetErrorHandler(previous)


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
        if state == "IsViewable" and focus_request(window):
            return
        time.sleep(0.05)
    raise AssertionError(f"Cannot focus RODE window {hex(window)} within {timeout}s: {state}")


def screenshot(window, name):
    """Capture the owning window; scenario readiness must be established by its caller."""
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


def theme(name, *, ui):
    """Select an exact theme after the complete query is rendered; spelling is case-insensitive."""
    query = "Theme: " + name
    key("p", ("Control_L", "Shift_L"))
    ui.control("command-palette-input", focused=True)
    ui.fill("command-palette-input", query)
    target = "command-match:" + query
    state = ui.layout(
        lambda state: any(
            button["name"].casefold() == target.casefold() and button["enabled"]
            for button in state["buttons"]
        ),
        "Theme did not match the complete query: " + name,
    )
    actual = next(
        button["name"]
        for button in state["buttons"]
        if button["name"].casefold() == target.casefold()
    )
    ui.click(actual)
    ui.absent("command-palette-input")


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
    """Require the theme's painted pixel, rather than sleeping before one screenshot."""
    actual = None

    def matches():
        nonlocal actual
        screenshot(window, filename)
        pixel = subprocess.check_output(
            ["convert", str(ARTIFACTS / filename), "-format", "%[pixel:p{1300,400}]", "info:"],
            text=True,
        )
        actual = tuple(map(int, re.findall(r"\d+", pixel)[:3]))
        return actual == expected

    try:
        wait_for_effect(matches, f"Theme background did not become {expected}", timeout=10)
    except AssertionError as error:
        raise AssertionError(f"{filename}: background {actual}, expected {expected}") from error


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


def stop_process(process, timeout=5):
    """Reap an owned fixture after failure without retrying UI actions or masking its error."""
    if process.poll() is not None:
        return True
    process.terminate()
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        process.kill()
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            print(f"Fixture process {process.pid} did not exit after SIGKILL", file=sys.stderr)
            return False
    return True


def stop_fixture_agents(environment, project):
    """End agents owned by this project in an explicitly private fixture service."""
    runtime = environment.get("RODE_SESSION_DIR")
    if not runtime or not Path(runtime).resolve().is_relative_to(Path(project).parent.resolve()):
        raise ValueError("Agent cleanup requires a runtime under the temporary fixture root")
    if not Path(runtime).exists():
        return True

    def request(value):
        reply = subprocess.run(
            [str(BINARY), "--session-service", "request"],
            input=json.dumps(value).encode(),
            env=environment,
            capture_output=True,
            check=True,
            timeout=5,
        )
        return json.loads(reply.stdout)

    try:
        response = request({"action": "agents"})
        if response["kind"] != "agents":
            raise ValueError("Unexpected fixture service reply: " + response["kind"])
        for agent in response["data"]:
            if Path(agent["root"]).resolve() == Path(project).resolve():
                response = request(dict(action="end_agent", id=agent["id"], policy=agent["policy"]))
                if response["kind"] == "error":
                    raise ValueError("Fixture agent did not stop")
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError) as error:
        print(f"Could not end fixture agents: {error}", file=sys.stderr)
        return False
    return True
