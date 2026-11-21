"""Private fixture paths, literal launchers and serialized preferences.

These helpers require no display, daemon, model or account configuration.
"""

import contextlib
import hashlib
import os
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path


def fixture_artifacts(root, path, results=None):
    """Keep each fixture's artifact namespace when relocating output into a run."""
    path = Path(path)
    if not results:
        return path
    root = Path(root)
    absolute = (root / path).resolve()
    try:
        relative = absolute.relative_to((root / "target").resolve())
    except ValueError:
        digest = hashlib.sha256(str(absolute).encode()).hexdigest()[:12]
        relative = Path(f"{path.name}-{digest}")
    return Path(results) / "ui" / relative


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


def read_preferences(path):
    """Read RODE's atomic string map, decoding RON before any nested JSON.

    Accept the serializer's quoted string keys/values and optional final comma.
    Missing storage is pending; malformed storage is a fixture failure. This is
    deliberately not a general RON parser and never evaluates configuration.
    """
    try:
        contents = Path(path).read_text().strip()
    except FileNotFoundError:
        return {}
    if not (contents.startswith("{") and contents.endswith("}")):
        raise ValueError("Expected a RON preference string map")
    remaining = contents[1:-1].strip()
    literal = r'"(?:\\.|[^"\\])*"'
    entry = re.compile(rf"({literal})\s*:\s*({literal})\s*(,|$)", re.DOTALL)
    values = {}
    while remaining:
        match = entry.match(remaining)
        if not match:
            raise ValueError("Invalid RON preference entry")
        key, value = (decode_ron_string(match[index]) for index in (1, 2))
        if key in values:
            raise ValueError("Duplicate RON preference key: " + key)
        values[key] = value
        remaining = remaining[match.end() :].lstrip()
    return values


def temporary_root():
    """Use the explicit fixture root, then Python's TMPDIR/TEMP/TMP-aware default.

    Resolve aliases before constructing host paths or Unix socket names. An
    invalid explicit override is an error, never a reason to use another disk.
    """
    configured = os.environ.get("RODE_TEST_TMPDIR")
    root = Path(configured if configured else tempfile.gettempdir()).resolve(strict=True)
    if not root.is_dir():
        raise NotADirectoryError(f"Fixture temporary root is not a directory: {root}")
    return root


@contextlib.contextmanager
def fixture_directory(*, prefix, awkward=True):
    """Own a private fixture directory using the same canonical paths as the host."""
    if awkward and os.environ.get("RODE_TEST_AWKWARD_PATHS") == "1":
        prefix += "space ' -"
    directory = tempfile.TemporaryDirectory(prefix=prefix, dir=temporary_root())
    try:
        yield Path(directory.name).resolve()
    except BaseException:
        try:
            directory.cleanup()
        except OSError as error:
            print(f"Fixture cleanup failed at {directory.name}: {error}", file=sys.stderr)
        raise
    else:
        directory.cleanup()


def cleanup_fixture_mounts(directory, destinations):
    """Detach declared RODE SSHFS mounts before deleting a failed fixture's directories.

    Run after its processes stop. Successful scenarios must assert ordinary unmount/shutdown
    before this last-resort cleanup; a lazy detach cannot count as a passing lifecycle check.
    Inspect mountinfo without traversing potentially disconnected mount paths.
    """
    root = Path(directory).resolve()
    paths = {Path(os.path.abspath(path)) for path in destinations}
    if any(path == root or not path.is_relative_to(root) for path in paths):
        raise ValueError("Mount cleanup requires destinations inside its private fixture")
    if not paths:
        return
    entries = {path: [] for path in paths}
    try:
        for line in Path("/proc/self/mountinfo").read_text(errors="surrogateescape").splitlines():
            before, separator, after = line.partition(" - ")
            fields, kind = before.split(), after.split()
            if not separator or len(fields) < 5 or len(kind) < 2:
                continue
            path = Path(re.sub(r"\\([0-7]{3})", lambda m: chr(int(m[1], 8)), fields[4]))
            if path in entries:
                entries[path].append(kind[:2])
    except OSError as error:
        print(f"Cannot inspect fixture mounts: {error}", file=sys.stderr)
        return
    tool = shutil.which("fusermount3") or shutil.which("fusermount")
    for path, mounted in entries.items():
        if not mounted:
            continue
        if (
            len(mounted) != 1
            or mounted[0][0] != "fuse.sshfs"
            or not re.fullmatch(r"rode-share-[0-9a-fA-F]{32}", mounted[0][1])
        ):
            print(f"Refusing cleanup of an unrelated or stacked mount at {path}", file=sys.stderr)
            continue
        try:
            if not tool:
                raise OSError("fusermount3/fusermount is unavailable")
            result = subprocess.run(
                [tool, "-uz", "--", str(path)], capture_output=True, text=True, timeout=10
            )
            if result.returncode:
                raise OSError(result.stderr.strip() or f"Unmount exited with {result.returncode}")
        except (OSError, subprocess.TimeoutExpired) as error:
            print(f"Cannot detach fixture mount {path}: {error}", file=sys.stderr)


def private_directory(path):
    """Create fixture storage or reject unsafe existing storage; never chmod a caller's path."""
    path = Path(path)
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    metadata = path.lstat()
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or stat.S_IMODE(metadata.st_mode) != 0o700
    ):
        raise ValueError(f"Fixture directory must be an owned, non-symlink 0700 directory: {path}")
    return path


def fixture_environment(base, *, environment=None, **overrides):
    """Isolate each native fixture while preserving explicit tools and SSH settings."""
    base = Path(base).resolve()
    for name in ("home", "config", "data", "cache", "run"):
        private_directory(base / name)
    environment = dict(
        os.environ if environment is None else environment,
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
    # Overrides are allowed for fixtures with deliberate shared state, but
    # must meet exactly the same contract as the default directories.
    for name in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR"):
        private_directory(environment[name])
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
