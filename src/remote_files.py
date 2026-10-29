"""RODE's per-request helper. Python 3 standard library only; input is JSON on stdin.

Final entry point in files::REMOTE_HELPER. Rust prepends sandbox, project, session, extension,
Marp and LaTeX peers plus shared policy/catalogue globals. One bounded JSON request yields one
reply; stdout is protocol-only. Source writes follow the same inode-lock, content-revision and
atomic-publication contract as files.rs.
"""

import base64
import errno
import fcntl
import hashlib
import json
import os
import stat
import sys
import tempfile
import time

LIMIT = 8 * 1024 * 1024


def open_regular(path, writable=False):
    """Open without waiting for FIFO writers, then require a regular file descriptor."""
    flags = os.O_RDWR if writable else os.O_RDONLY
    descriptor = os.open(path, flags | os.O_NONBLOCK | os.O_CLOEXEC)
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise ValueError("Only regular files can be accessed.")
        return os.fdopen(descriptor, "r+b" if writable else "rb")
    except BaseException:
        os.close(descriptor)
        raise


def lock_existing(path):
    """Lock the current file inode, reopening after replacements; return None if absent."""
    deadline = time.monotonic() + 3
    while True:
        try:
            stream = open_regular(path)
        except FileNotFoundError:
            return None
        try:
            original = os.fstat(stream.fileno())
            if not stat.S_ISREG(original.st_mode):
                raise ValueError("Only regular files can be written.")
            while True:
                try:
                    fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    break
                except BlockingIOError:
                    if time.monotonic() >= deadline:
                        raise ValueError(
                            "File is busy in another process; changes were not saved."
                        ) from None
                    time.sleep(0.01)
                except OSError as error:
                    if error.errno != errno.EBADF or stream.writable():
                        raise
                    stream.close()
                    stream = open_regular(path, writable=True)
                    original = os.fstat(stream.fileno())
                    if not stat.S_ISREG(original.st_mode):
                        raise ValueError("Only regular files can be written.") from None
            try:
                current = os.stat(path)
            except FileNotFoundError:
                current = None
            if current and (current.st_dev, current.st_ino) == (original.st_dev, original.st_ino):
                return stream
        except BaseException:
            stream.close()
            raise
        stream.close()
        if time.monotonic() >= deadline:
            raise ValueError("File keeps changing in another process; changes were not saved.")


def write(path, content, expected):
    """Check the expected digest under inode locks, then atomically publish bytes without a permanent source lock file."""
    locked = lock_existing(path)
    try:
        mode = None
        if locked:
            original = read(path)
            if hashlib.sha256(original).hexdigest() != expected:
                raise ValueError(
                    "Save conflict: this file changed on disk. Your edits are still open; save to a new path."
                )
            mode = stat.S_IMODE(os.fstat(locked.fileno()).st_mode)
        elif expected is not None:
            raise ValueError(
                "Save conflict: this file was removed on disk. Your edits are still open."
            )
        fd, temporary = tempfile.mkstemp(prefix=".rode-", dir=os.path.dirname(path))
        try:
            with os.fdopen(fd, "wb") as stream:
                fcntl.flock(stream, fcntl.LOCK_EX)
                stream.write(content)
                if mode is not None:
                    os.fchmod(stream.fileno(), mode)
                stream.flush()
                os.fsync(stream.fileno())
                if locked:
                    os.replace(temporary, path)
                else:
                    try:
                        os.link(temporary, path)
                    except FileExistsError:
                        raise ValueError(
                            "Save conflict: another process created this file. Your edits are still open."
                        ) from None
                    os.unlink(temporary)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
    finally:
        if locked:
            locked.close()


def read(path):
    """Read bounded regular-file bytes, rejecting unsupported file shapes."""
    with open_regular(path) as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError("Only regular files can be edited.")
        content = stream.read(LIMIT + 1)
    if len(content) > LIMIT:
        raise ValueError("File exceeds the 8 MiB editing limit.")
    return content


def handle(request):
    """Dispatch one typed file/project request and return the Rust-compatible reply shape."""
    if request["op"] == "tool":
        return {"path": "", "tool": project_handle(request["request"])}  # noqa: F821 - concatenated peer
    path = os.path.realpath(os.path.expanduser(request["path"]))
    response = {"path": path}
    if request["op"] == "prepare":
        plan = prepare_project(path, request.get("sandbox", {}))  # noqa: F821 - concatenated peer
        response = handle({"op": "list", "path": path})
        response["project"] = plan
    elif request["op"] == "list":
        entries = []
        with os.scandir(path) as scan:
            for entry in scan:
                # JSON cannot round-trip Unix filenames containing invalid UTF-8.
                try:
                    entry.name.encode("utf-8")
                except UnicodeEncodeError:
                    continue
                entries.append({"path": entry.path, "name": entry.name, "dir": entry.is_dir()})
                if len(entries) > 10000:
                    raise ValueError(
                        "Directory has more than 10000 entries. Open a smaller folder."
                    )
        response["entries"] = sorted(entries, key=lambda e: (not e["dir"], e["name"].lower()))
    elif request["op"] in ("read_auto", "read_hex"):
        content = read(path)
        response["revision"] = hashlib.sha256(content).hexdigest()
        try:
            if request["op"] == "read_hex" or b"\0" in content:
                raise ValueError("Binary")
            response["text"] = content.decode("utf-8")
        except (UnicodeDecodeError, ValueError):
            response["binary"] = base64.b64encode(content).decode("ascii")
    elif request["op"] == "read":
        content = read(path)
        if b"\0" in content:
            raise ValueError("Binary files cannot be edited.")
        response.update(text=content.decode("utf-8"), revision=hashlib.sha256(content).hexdigest())
    elif request["op"] in ("read_image", "read_binary"):
        kind = "Preview" if request["op"] == "read_binary" else "Image"
        if not stat.S_ISREG(os.stat(path).st_mode):
            raise ValueError("Only regular preview files can be opened.")
        with open_regular(path) as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode):
                raise ValueError("Only regular image files can be opened.")
            if metadata.st_size > 32 * 1024 * 1024:
                raise ValueError(f"{kind} file exceeds 32 MiB.")
            content = stream.read(32 * 1024 * 1024 + 1)
        if len(content) > 32 * 1024 * 1024:
            raise ValueError(f"{kind} file exceeds 32 MiB.")
        response.update(revision=hashlib.sha256(content).hexdigest())
        response["binary" if request["op"] == "read_binary" else "image"] = base64.b64encode(
            content
        ).decode("ascii")
    elif request["op"] in ("write", "write_binary"):
        if request["op"] == "write_binary":
            if len(request["data"]) > ((LIMIT + 2) // 3) * 4:
                raise ValueError("Binary file exceeds 8 MiB.")
            content = base64.b64decode(request["data"], validate=True)
        else:
            content = request["text"].encode("utf-8")
        if len(content) > LIMIT:
            raise ValueError("File exceeds the 8 MiB editing limit.")
        write(path, content, request.get("expected"))
        response["revision"] = hashlib.sha256(content).hexdigest()
    else:
        raise ValueError("Unknown file operation")
    return response


if __name__ == "__main__":
    try:
        result = (
            latex_coordinator()  # noqa: F821 - concatenated peer
            if "--latex-helper" in sys.argv
            else marp_coordinator()  # noqa: F821 - concatenated peer
            if "--marp-helper" in sys.argv
            else extension_coordinator()  # noqa: F821 - concatenated peer
            if "--extension-helper" in sys.argv
            else handle(json.load(sys.stdin))
        )  # noqa: F821
        print(json.dumps(result, ensure_ascii=True))
    except Exception as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
