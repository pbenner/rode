"""Host-side encrypted-project lease. Only Python's standard library is used.

Standalone host-level peer of vault.rs, intentionally outside project isolation solely to
manage verified gocryptfs mounts. Passwords pass over stdin and never enter arguments,
preferences or diagnostics. Stable operation/view locks coordinate windows; normal unmount
refuses other leases/jobs/namespaces and never claims a failed lock succeeded.
"""

import fcntl
import hashlib
import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
from pathlib import Path


def private_directory(path):
    """Create or verify an owner-only runtime directory without accepting symlinks."""
    path.mkdir(mode=0o700, exist_ok=True)
    meta = path.lstat()
    if not stat.S_ISDIR(meta.st_mode) or meta.st_uid != os.geteuid() or meta.st_mode & 0o077:
        raise ValueError(
            "Encrypted-project runtime must be an owner-only directory without symlinks."
        )


def mount_table(path):
    """Read a bounded host mount table, failing visibly if identity cannot be checked."""
    with Path(path).open("rb") as stream:
        data = stream.read(8 * 1024 * 1024 + 1)
    if len(data) > 8 * 1024 * 1024:
        raise ValueError("Mount information exceeds the verification limit.")
    return data.decode("utf-8")


def mount_entry(text, point):
    """Decode a mount-table entry for an exact mountpoint."""

    def decode(value):
        return re.sub(r"\\([0-7]{3})", lambda m: chr(int(m[1], 8)), value)

    for line in text.splitlines():
        left, separator, right = line.partition(" - ")
        fields, tail = left.split(), right.split()
        if separator and len(fields) >= 6 and len(tail) >= 2 and decode(fields[4]) == point:
            return fields[2], tail[0], decode(tail[1])
    return None


def foreign_mount(name, point):
    """Detect references to this encrypted mount in other process namespaces."""

    def references(table, ignored_point=None):
        for line in table.splitlines():
            left, separator, right = line.partition(" - ")
            fields, tail = left.split(), right.split()
            if (
                separator
                and len(fields) >= 5
                and len(tail) >= 2
                and tail[:2] == ["fuse.gocryptfs", name]
            ):
                mounted_point = re.sub(r"\\([0-7]{3})", lambda m: chr(int(m[1], 8)), fields[4])
                if mounted_point != ignored_point:
                    return True
        return False

    if references(mount_table("/proc/self/mountinfo"), point):
        return True
    seen = {os.readlink("/proc/self/ns/mnt")}
    for index, path in enumerate(Path("/proc").iterdir()):
        if index >= 65536:
            raise ValueError("Too many processes to verify encrypted-project lock state safely.")
        if not path.name.isdigit():
            continue
        try:
            if path.stat().st_uid != os.geteuid():
                continue
            namespace = os.readlink(path / "ns/mnt")
            if namespace in seen:
                continue
            seen.add(namespace)
            table = mount_table(path / "mountinfo")
        except FileNotFoundError:
            continue
        except OSError as error:
            raise ValueError(
                "Cannot verify other mount namespaces; project remains unlocked."
            ) from error
        if references(table):
            return True
    return False


def run_tool(program, args, password="", timeout=60):
    # Quiet mode avoids printing the master key during initialization. Tool
    # diagnostics are bounded; neither passwords nor tool stdout are logged.
    """Run a bounded filesystem tool; send transient passwords on stdin and redact them from errors."""
    import threading

    process = subprocess.Popen(
        [program, *args],
        stdin=subprocess.PIPE,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        cwd="/",
        start_new_session=True,
    )
    errors = bytearray()

    def read_errors():
        while True:
            block = process.stderr.read(4096)
            if not block:
                return
            if len(errors) + len(block) > 65536:
                os.killpg(process.pid, signal.SIGKILL)
                return
            errors.extend(block)

    reader = threading.Thread(target=read_errors, daemon=True)
    reader.start()
    try:
        # A fast failure (or a tool that never reads stdin) still has an
        # exit status and diagnostics. Match communicate() and the Rust
        # peer rather than replacing that result with a pipe error.
        try:
            process.stdin.write(password.encode())
        except BrokenPipeError:
            pass
        try:
            process.stdin.close()
        except BrokenPipeError:
            pass
        process.wait(timeout=timeout)
    except BaseException:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        raise
    finally:
        reader.join(timeout=1)
    message = errors.decode(errors="replace")
    if password:
        message = message.replace(password, "[redacted]")
    return process.returncode, message


class Mount:
    """Verified encrypted mount and shared per-window lease; all mutations use the operation lock."""

    def __init__(self, settings, create):
        """Validate storage/runtime identity without unlocking or persisting a password."""
        self.settings = settings
        values = [settings[key] for key in ("cipherdir", "program", "unmount_program")]
        if not settings["cipherdir"].strip() or not settings["program"].strip():
            raise ValueError("Choose an encrypted folder and gocryptfs executable.")
        if any(len(value.encode()) > 4096 or "\0" in value for value in values):
            raise ValueError("Invalid encrypted-project path or executable.")
        cipher = Path(settings["cipherdir"]).expanduser()
        if create and not cipher.exists():
            cipher.mkdir(mode=0o700)
        cipher = cipher.resolve(strict=True)
        if not cipher.is_dir():
            raise ValueError("Encrypted storage must be a directory.")
        digest = hashlib.sha256(str(cipher).encode()).hexdigest()
        base = Path(f"/tmp/rode-vaults-{os.geteuid()}")
        private_directory(base)
        base = base.resolve(strict=True)
        self.runtime = base / digest
        private_directory(self.runtime)
        point = self.runtime / "plain"
        if not point.exists():
            private_directory(point)
        if point.is_symlink():
            raise ValueError("The decrypted mountpoint cannot be a symlink.")
        self.info = dict(cipherdir=str(cipher), mountpoint=str(point))
        self.name = "rode-gocryptfs-" + digest
        self.lease = self.directory = None
        self.attempted = False

    def lock_file(self, name):
        """Open a stable private coordination inode with ownership/link checks."""
        fd = os.open(
            self.runtime / name, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600
        )
        meta = os.fstat(fd)
        if (
            not stat.S_ISREG(meta.st_mode)
            or meta.st_uid != os.geteuid()
            or meta.st_mode & 0o077
            or meta.st_nlink != 1
        ):
            os.close(fd)
            raise ValueError("Unsafe encrypted-project coordination file.")
        return os.fdopen(fd, "r+b", buffering=0)

    def operation(self):
        """Acquire the timed exclusive mount-operation lock, released by the returned file context."""
        import time

        lock = self.lock_file("operation.lock")
        until = time.monotonic() + 3
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return lock
            except BlockingIOError:
                if time.monotonic() >= until:
                    lock.close()
                    raise ValueError("Encrypted project is busy in another process.") from None
                time.sleep(0.01)

    def mounted(self):
        """Return the verified gocryptfs mount device, or None when no mount exists."""
        entry = mount_entry(mount_table("/proc/self/mountinfo"), self.info["mountpoint"])
        if entry:
            device, kind, name = entry
            if kind != "fuse.gocryptfs" or name != self.name:
                raise ValueError(
                    "Refusing to manage an unrelated filesystem at the encrypted-project mountpoint."
                )
            return device
        return None

    def acquire(self):
        """Hold a shared view lease and decrypted-directory descriptor while the project is open."""
        self.lease = self.lock_file("views.lock")
        fcntl.flock(self.lease, fcntl.LOCK_SH | fcntl.LOCK_NB)
        self.directory = os.open(
            self.info["mountpoint"], os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC
        )

    def tool(self, args, password):
        """Invoke installed gocryptfs with transient stdin credentials and sanitized failure output."""
        code, message = run_tool(os.path.expanduser(self.settings["program"]), args, password)
        if code:
            raise ValueError(f"gocryptfs failed ({code}): {message}")

    def open(self, password, create):
        """Serialize initialization/unlock, verify the FUSE identity and acquire this view's lease."""
        with self.operation():
            if self.mounted():
                if create:
                    raise ValueError("This encrypted project is already initialized and unlocked.")
                self.acquire()
                return
            os.chmod(self.info["mountpoint"], 0o700)
            covered = os.open(self.info["mountpoint"], os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
            try:
                if next(Path(self.info["mountpoint"]).iterdir(), None) is not None:
                    raise ValueError("The decrypted mountpoint must be empty.")
                if (
                    not password
                    or len(password.encode()) > 1000
                    or any(c in password for c in "\n\r\0")
                ):
                    raise ValueError("Enter a password of 1–1000 bytes without line breaks.")
                if create:
                    if next(Path(self.info["cipherdir"]).iterdir(), None) is not None:
                        raise ValueError(
                            "New encrypted storage must be empty. Existing files are never converted or removed."
                        )
                    self.tool(["-init", "-q", "--", self.info["cipherdir"]], password)
                if not (Path(self.info["cipherdir"]) / "gocryptfs.conf").is_file():
                    raise ValueError(
                        "No gocryptfs.conf found. Select existing encrypted storage or enable creation for an empty folder."
                    )
                self.attempted = True
                self.tool(
                    [
                        "-q",
                        "-fsname",
                        self.name,
                        "--",
                        self.info["cipherdir"],
                        self.info["mountpoint"],
                    ],
                    password,
                )
                if not self.mounted():
                    raise ValueError(
                        "gocryptfs exited without creating a verified encrypted mount. FUSE must be available on the project host."
                    )
                self.acquire()

            finally:
                os.fchmod(covered, 0)
                os.close(covered)

    def release(self, retain_on_failure=False, close_shared=False):
        """Release this view and attempt normal lock; optionally reacquire its lease after a busy failure."""
        with self.operation():
            if self.directory is not None:
                os.close(self.directory)
                self.directory = None
            if self.lease is not None:
                self.lease.close()
                self.lease = None
            try:
                self.unmount_locked()
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                shared = str(error).startswith(
                    "Another RODE window still has this encrypted project open."
                )
                if retain_on_failure and not (close_shared and shared) and self.mounted():
                    self.acquire()
                raise

    def unmount_locked(self):
        """Require exclusive view ownership and no foreign namespace references before verified normal unmount."""
        with self.lock_file("views.lock") as lease:
            try:
                fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise ValueError(
                    "Another RODE window still has this encrypted project open."
                ) from None
            if foreign_mount(self.name, self.info["mountpoint"]):
                raise ValueError(
                    "The decrypted filesystem is still mounted in another process namespace, possibly a sandbox or running job. Stop that work before locking."
                )
            if not self.mounted():
                return
            program = (
                self.settings["unmount_program"].strip()
                or shutil.which("fusermount3")
                or shutil.which("fusermount")
            )
            if not program:
                raise ValueError(
                    "Install FUSE's fusermount3/fusermount on the project host to lock this project."
                )
            code, message = run_tool(
                os.path.expanduser(program), ["-u", "--", self.info["mountpoint"]], timeout=10
            )
            if code:
                raise ValueError("Project remains unlocked (it may be busy): " + message)
            if self.mounted():
                raise ValueError("Encrypted filesystem is still mounted.")
            if foreign_mount(self.name, self.info["mountpoint"]):
                raise ValueError(
                    "A process still holds the decrypted filesystem. Stop that work before locking."
                )


def emit(reply):
    """Write a secret-free lease-protocol reply to stdout."""
    print(json.dumps(reply), flush=True)


def close(mount, retain_on_failure=False, close_shared=False):
    """Return verified lock status; a busy/failed close remains explicitly unlocked."""
    try:
        mount.release(retain_on_failure, close_shared)
        return dict(kind="closed", info=mount.info, locked=True, message="")
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        return dict(kind="closed", info=mount.info, locked=False, message=str(error))


def main():
    """Serve one unlock/lease lifecycle and perform best-effort normal cleanup on input loss."""
    mount = None

    def terminate(_signal, _frame):
        raise EOFError

    signal.signal(signal.SIGTERM, terminate)
    signal.signal(signal.SIGHUP, terminate)
    try:
        request = json.loads(sys.stdin.buffer.readline(32768))
        mount = Mount(request["settings"], request["create"])
        if request.get("lock_only"):
            if not (Path(mount.info["cipherdir"]) / "gocryptfs.conf").is_file():
                raise ValueError("No gocryptfs.conf found in the selected encrypted folder.")
            emit(close(mount))
            mount = None
            return
        mount.open(request["password"], request["create"])
        request["password"] = ""
        emit(dict(kind="opened", info=mount.info))
        while True:
            line = sys.stdin.buffer.readline(4096)
            try:
                action = json.loads(line).get("action")
                lock_requested = action in ("lock", "close")
                close_shared = action == "close"
            except (ValueError, AttributeError):
                lock_requested = close_shared = False
            reply = close(mount, retain_on_failure=lock_requested, close_shared=close_shared)
            emit(reply)
            shared_closed = (
                close_shared
                and not reply["locked"]
                and reply["message"].startswith(
                    "Another RODE window still has this encrypted project open."
                )
            )
            if not lock_requested or reply["locked"] or shared_closed:
                mount = None
                break
    except EOFError:
        pass
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        emit(dict(kind="error", message=str(error)))
    finally:
        if mount is not None and (
            mount.attempted or mount.lease is not None or mount.directory is not None
        ):
            reply = close(mount)
            try:
                emit(reply)
            except (BrokenPipeError, OSError):
                pass


if __name__ == "__main__":
    main()
