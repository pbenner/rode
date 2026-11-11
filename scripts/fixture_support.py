"""Private fixture paths, literal launchers and serialized preferences.

These helpers require no display, daemon, model or account configuration.
"""

import contextlib
import os
import re
import shlex
import sys
import tempfile
from pathlib import Path


def decode_ron_string(literal):
    """Decode the quoted strings emitted by RON's char::escape_debug serializer.

    Storage values contain JSON inside RON strings. RON additionally escapes
    apostrophes, NUL and Unicode scalars; treating that outer layer as JSON fails
    for otherwise valid project paths. This parses no expressions or RON objects.
    """
    if len(literal) < 2 or literal[0] != '"' or literal[-1] != '"':
        raise ValueError("Expected a quoted RON storage string")
    escapes = {"n": "\n", "r": "\r", "t": "\t", "0": "\0", "\\": "\\", "'": "'", '"': '"'}
    result, index = [], 1
    while index < len(literal) - 1:
        char = literal[index]
        index += 1
        if char == '"':
            raise ValueError("Unexpected quote inside RON string")
        if char != "\\":
            result.append(char)
            continue
        if index >= len(literal) - 1:
            raise ValueError("Truncated RON escape")
        escaped = literal[index]
        index += 1
        if escaped in escapes:
            result.append(escapes[escaped])
        elif escaped == "u" and literal[index : index + 1] == "{":
            end = literal.find("}", index + 1)
            if end < 0:
                raise ValueError("Truncated RON Unicode escape")
            digits = literal[index + 1 : end]
            if not re.fullmatch(r"[0-9a-fA-F]{1,6}", digits):
                raise ValueError("Invalid RON Unicode escape")
            scalar = int(digits, 16)
            if scalar > 0x10FFFF or 0xD800 <= scalar <= 0xDFFF:
                raise ValueError("Invalid RON Unicode scalar")
            result.append(chr(scalar))
            index = end + 1
        else:
            raise ValueError("Unsupported RON escape: " + escaped)
    return "".join(result)


@contextlib.contextmanager
def fixture_directory(*, prefix):
    """Own a private fixture directory using the same canonical paths as the host."""
    with tempfile.TemporaryDirectory(prefix=prefix) as directory:
        yield Path(directory).resolve()


def fixture_environment(base, **overrides):
    """Isolate each native fixture while preserving explicit tools and SSH settings."""
    base = Path(base).resolve()
    for name in ("home", "config", "data", "cache", "run"):
        (base / name).mkdir(mode=0o700, exist_ok=True)
    environment = dict(
        os.environ,
        HOME=str(base / "home"),
        XDG_CONFIG_HOME=str(base / "config"),
        XDG_DATA_HOME=str(base / "data"),
        XDG_CACHE_HOME=str(base / "cache"),
        XDG_RUNTIME_DIR=str(base / "run"),
        RODE_SESSION_DIR=str(base / "run/sessions"),
        RODE_VAULT_DIR=str(base / "run/vaults"),
        RODE_UI_TEST_STATE=str(base / "ui-state.json"),
    )
    environment.pop("CODEX_HOME", None)
    environment.update(overrides)
    return environment


def write_python_launcher(path, source, *, python=None):
    """Create a literal Python launcher even when interpreter or fixture paths contain spaces."""
    path = Path(path)
    program = path.with_name(path.name + ".py")
    program.write_text(source, encoding="utf-8")
    interpreter = str(Path(python or sys.executable).absolute())
    path.write_text(
        "#!/bin/sh\nexec " + shlex.quote(interpreter) + " " + shlex.quote(str(program)) + ' "$@"\n'
    )
    path.chmod(0o700)
