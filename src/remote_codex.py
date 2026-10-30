"""Deploy the pinned native Codex runtime over an authenticated SSH channel.

Only RODE's private cache is modified. Publication is serialized and atomic;
the agent is launched separately inside the captured project sandbox.
"""

import fcntl
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import stat
import sys
import tempfile
import time
from pathlib import Path


def private_directory(path):
    """Require an owned private directory, never following a coordination symlink."""
    path.mkdir(mode=0o700, exist_ok=True)
    meta = path.lstat()
    if not stat.S_ISDIR(meta.st_mode) or meta.st_uid != os.geteuid() or meta.st_mode & 0o077:
        raise ValueError("Unsafe Codex cache directory")


def verified(path, size, digest):
    """Validate an opened regular executable before using a cached deployment."""
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    except FileNotFoundError:
        return False
    with os.fdopen(fd, "rb") as source:
        meta = os.fstat(source.fileno())
        if (
            not stat.S_ISREG(meta.st_mode)
            or meta.st_size != size
            or meta.st_uid != os.geteuid()
            or meta.st_nlink != 1
            or meta.st_mode & 0o022
        ):
            raise ValueError("Unsafe Codex runtime file")
        result = hashlib.sha256()
        total = 0
        while True:
            block = source.read(1024 * 1024)
            if not block:
                break
            total += len(block)
            if total > size:
                raise ValueError("Cached Codex file exceeds its pinned size")
            result.update(block)
        if result.hexdigest() != digest:
            raise ValueError("Cached Codex failed its SHA-256 check")
    return True


def cached_runtime(base, version, architecture, runtime, target, notices):
    """A sandbox grant must expose only owned, verified runtime files, never a symlinked tree."""
    for path in [base, base / version, base / version / architecture, runtime]:
        try:
            meta = path.lstat()
        except FileNotFoundError:
            return False
        if not stat.S_ISDIR(meta.st_mode) or meta.st_uid != os.geteuid() or meta.st_mode & 0o077:
            raise ValueError("Unsafe Codex cache directory")
    expected = {notice["file"] for notice in notices} | {"codex"}
    if {path.name for path in runtime.iterdir()} != expected:
        raise ValueError("Unexpected Codex runtime files")
    if not verified(runtime / "codex", target["size"], target["sha256"]):
        raise ValueError("Missing cached Codex executable")
    if not os.access(runtime / "codex", os.X_OK):
        raise ValueError("Cached Codex is not executable")
    for notice in notices:
        path = runtime / notice["file"]
        # Exact sizes come from the bounded, trusted notice contents on install;
        # subsequent probes hash the existing small file against the release pin.
        if path.stat().st_size > 1024 * 1024 or not verified(
            path, path.stat().st_size, notice["sha256"]
        ):
            raise ValueError("Invalid cached Codex notice")
    return True


def run(spec):
    """Probe or install one host architecture; streamed input is bounded by the pin."""
    architecture = {"arm64": "aarch64", "amd64": "x86_64"}.get(
        platform.machine(), platform.machine()
    )
    host_platform = platform.system().lower()
    local = Path.home() / ".local/bin/codex"
    installed = shutil.which("codex") or (str(local) if os.access(local, os.X_OK) else None)
    if spec["action"] == "host":
        return dict(architecture=architecture, platform=host_platform, installed=installed)
    version = spec["version"]
    target = spec["targets"].get(architecture)
    if not target or host_platform != "linux":
        if spec["action"] == "probe":
            return dict(architecture=architecture, platform=host_platform, installed=installed)
        raise ValueError("This host needs a configured Codex executable for its architecture")
    if not re.fullmatch(r"[0-9][0-9A-Za-z.+-]{0,63}", version):
        raise ValueError("Invalid Codex runtime version")
    size, digest = target["size"], target["sha256"]
    if not 0 < size <= 512 * 1024 * 1024 or not re.fullmatch(r"[a-f0-9]{64}", digest):
        raise ValueError("Invalid Codex runtime pin")
    base = Path(os.environ.get("RODE_CODEX_CACHE", "~/.cache/rode/codex")).expanduser()
    if not base.is_absolute():
        raise ValueError("Codex cache path must be absolute")
    folder = base / version / architecture
    runtime = folder / digest
    executable = runtime / "codex"
    notices = spec["notices"]
    for notice in notices:
        name = notice["file"]
        if Path(name).name != name or name in ("", ".", "..", "codex"):
            raise ValueError("Invalid Codex notice filename")
    cached = cached_runtime(base, version, architecture, runtime, target, notices)
    if spec["action"] == "probe":
        return dict(
            architecture=architecture,
            platform=host_platform,
            executable=str(executable),
            cached=cached,
            installed=installed,
        )
    if spec["action"] != "install":
        raise ValueError("Invalid Codex deployment action")
    # Permit an existing user-selected cache parent; every RODE-owned level is private.
    base.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    parent = base.parent.stat()
    if parent.st_uid != os.geteuid() or parent.st_mode & 0o022:
        raise ValueError("Unsafe Codex cache parent directory")
    for path in [base, base / version, folder]:
        private_directory(path)
    fd = os.open(
        folder / (digest + ".lock"), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600
    )
    with os.fdopen(fd, "r+b") as lock:
        meta = os.fstat(lock.fileno())
        if (
            not stat.S_ISREG(meta.st_mode)
            or meta.st_uid != os.geteuid()
            or meta.st_nlink != 1
            or meta.st_mode & 0o077
        ):
            raise ValueError("Unsafe Codex deployment lock")
        until = time.monotonic() + 30
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= until:
                    raise ValueError("Codex deployment is busy; retry shortly") from None
                time.sleep(0.05)
        # Always consume and verify this channel's input, even if another window
        # completed publication while we waited. Nothing overwrites a running copy.
        with tempfile.TemporaryDirectory(prefix=".incoming-", dir=folder) as work:
            stage = Path(work) / "runtime"
            stage.mkdir(mode=0o700)
            result = hashlib.sha256()
            total = 0
            with (stage / "codex").open("wb") as out:
                while True:
                    block = sys.stdin.buffer.read(min(1024 * 1024, size - total + 1))
                    if not block:
                        break
                    total += len(block)
                    if total > size:
                        raise ValueError("Codex transfer exceeds its pinned size")
                    result.update(block)
                    out.write(block)
                out.flush()
                os.fsync(out.fileno())
            if total != size or result.hexdigest() != digest:
                raise ValueError("Codex transfer failed its pinned size/SHA-256 check")
            (stage / "codex").chmod(0o755)
            for notice in notices:
                name = notice["file"]
                content = spec["notice_data"][name].encode("utf-8")
                if (
                    len(content) > 1024 * 1024
                    or hashlib.sha256(content).hexdigest() != notice["sha256"]
                ):
                    raise ValueError("Codex notice failed its SHA-256 check")
                (stage / name).write_bytes(content)
                (stage / name).chmod(0o644)
            if cached_runtime(base, version, architecture, runtime, target, notices):
                return dict(executable=str(executable))
            if runtime.exists():
                raise ValueError("Unexpected Codex runtime directory")
            os.rename(stage, runtime)
    return dict(executable=str(executable))


def interrupted(_signal, _frame):
    """Unwind only this upload's temporary files when its SSH channel closes."""
    raise InterruptedError("Codex deployment interrupted")


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGHUP, interrupted)
    try:
        print(json.dumps(run(json.loads(sys.argv[1]))), flush=True)
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
