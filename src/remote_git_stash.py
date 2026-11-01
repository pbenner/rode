"""Remove the captured stash entry under Git's files-backend writer locks.

Peer of project/stash.rs. Apply uses an immutable object ID; pop keeps that
saved object if the subsequent locked removal detects a changed stash list.
"""


class StashTransaction:
    """A Git-owned lock lease which aborts when its controlling stdin disappears."""

    def __init__(self, root, tip, deleting):
        import queue
        import subprocess
        import tempfile
        import threading

        command, env = globals()["project_git_command"](
            [
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.filesRefLockTimeout=0",
                "-c",
                "core.packedRefsTimeout=0",
                "update-ref",
                "--stdin",
            ]
        )
        self.errors = tempfile.TemporaryFile()
        try:
            self.process = subprocess.Popen(
                command,
                cwd=root,
                env=env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=self.errors,
                start_new_session=True,
            )
        except BaseException:
            self.errors.close()
            raise
        self.operation = "delete" if deleting else "verify"
        self.tip = tip
        self.replies = queue.Queue(maxsize=4)

        def read():
            for _ in range(4):
                line = self.process.stdout.readline(256)
                self.replies.put(line)
                if not line:
                    break

        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def __enter__(self):
        try:
            self.send(f"option no-deref\nstart\n{self.operation} refs/stash {self.tip}\nprepare\n")
            self.acknowledge("start: ok")
            self.acknowledge("prepare: ok")
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def send(self, command):
        self.process.stdin.write(command.encode())
        self.process.stdin.flush()

    def acknowledge(self, expected):
        reply = self.replies.get(timeout=120)
        if reply.rstrip() != expected.encode():
            self.errors.seek(0)
            detail = self.errors.read(8192).decode("utf-8", errors="replace")
            raise ValueError(f"Git metadata is busy or the stash transaction failed: {detail}")

    def finish(self, commit):
        operation = "commit" if commit else "abort"
        self.send(operation + "\n")
        self.acknowledge(operation + ": ok")
        self.__exit__(None, None, None)

    def __exit__(self, *_args):
        import os
        import signal
        import subprocess

        if not self.process.stdin.closed:
            self.process.stdin.close()
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            os.killpg(self.process.pid, signal.SIGTERM)
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        self.errors.close()
        self.reader.join(timeout=2)
        self.process.stdout.close()


def git_stash(request, root):
    """Run one bounded, identity-checked stash action on the project host."""
    import os
    import re
    import stat
    import tempfile
    from pathlib import Path

    git, checked = globals()["project_git"], globals()["project_checked"]
    action, reference, oid = request["action"], request["reference"], request["oid"]
    if action not in ("apply", "pop", "drop") or not re.fullmatch(
        r"(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})", oid
    ):
        raise ValueError("Invalid stash action/object ID")
    selector = re.fullmatch(r"stash@\{([0-9]{1,20})\}", reference)
    if not selector:
        raise ValueError("Invalid stash selector")
    index = int(selector[1])
    output = dict(code=0, stdout="", stderr="")
    if action != "drop":
        output = git(root, ["stash", "apply", oid])  # noqa: F821 - concatenated peer
        if output["code"] or action == "apply":
            return output

    def read(path):
        try:
            if not stat.S_ISREG(path.lstat().st_mode):
                raise ValueError("Git metadata must be a regular file")
            with open_regular(path) as source:  # noqa: F821 - concatenated peer
                data = source.read(8 * 1024 * 1024 + 1)
            if len(data) > 8 * 1024 * 1024:
                raise ValueError("Git metadata exceeds 8 MiB")
            return data
        except FileNotFoundError:
            return None

    def replace(path, data):
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            metadata = Path(str(path) + ".lock").lstat()
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError("Git metadata must be a regular file")
        descriptor, name = tempfile.mkstemp(prefix=".rode-stash-", dir=path.parent)
        try:
            with os.fdopen(descriptor, "wb") as stream:
                os.fchmod(stream.fileno(), stat.S_IMODE(metadata.st_mode))
                stream.write(data)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(name, path)
        finally:
            Path(name).unlink(missing_ok=True)

    def drop():
        if os.environ.get("GIT_NAMESPACE"):
            raise ValueError("Namespaced stash removal is unsupported; use the Git terminal")
        storage = git(root, ["config", "--get", "extensions.refStorage"])  # noqa: F821
        if not (
            (storage["code"] == 1 and not storage["stdout"])
            or (storage["code"] == 0 and storage["stdout"].strip() == "files")
        ):
            raise ValueError(
                "Safe stash removal requires Git's files reference backend; use the Git terminal for this repository"
            )
        common = Path(
            checked(git(root, ["rev-parse", "--path-format=absolute", "--git-common-dir"])).rstrip(
                "\n"
            )
        )
        ref, log = common / "refs/stash", common / "logs/refs/stash"
        before = read(log)
        if before is None:
            raise ValueError("Stash history disappeared; refresh before removing it")
        deleting = len(before.splitlines()) == 1
        tip = checked(git(root, ["rev-parse", "--verify", "refs/stash"])).strip()
        if not re.fullmatch(r"(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})", tip):
            raise ValueError("Invalid stash reference")
        with StashTransaction(root, tip, deleting) as transaction:
            if log.with_name("stash.lock").exists():
                raise ValueError("Git metadata is busy; refresh and retry")
            loose = read(ref)
            if loose is not None and loose.decode("ascii").strip() != tip:
                raise ValueError("Symbolic or changed stash reference; refresh before removing it")
            data = read(log)
            if data is None:
                raise ValueError("Stash history disappeared; refresh before removing it")
            lines, width = data.splitlines(keepends=True), len(oid)
            for line in lines:
                if (
                    len(line) <= 2 * width + 2
                    or not line.endswith(b"\n")
                    or not re.fullmatch(
                        rb"[0-9a-fA-F]{%d} [0-9a-fA-F]{%d} " % (width, width), line[: 2 * width + 2]
                    )
                ):
                    raise ValueError("Invalid stash reflog; no entry was removed")
            if (
                index >= len(lines)
                or lines[-index - 1][width + 1 : 2 * width + 1] != oid.encode()
                or lines[-1][width + 1 : 2 * width + 1] != tip.encode()
            ):
                raise ValueError("Stash list changed; refresh before removing it")
            del lines[-index - 1]
            previous, rewritten = b"0" * width, bytearray()
            for line in lines:
                rewritten.extend(previous + line[width:])
                previous = line[width + 1 : 2 * width + 1]
            if (not lines) != deleting:
                raise ValueError("Stash list changed; refresh before removing it")
            if deleting:
                update = f"{tip} {'0' * width} refs/stash\n"

                def hook(phase):
                    return git(
                        root,
                        ["hook", "run", "--ignore-missing", "reference-transaction", "--", phase],
                        update,
                    )

                try:
                    checked(hook("prepared"))
                    transaction.finish(True)
                except Exception:
                    hook("aborted")
                    raise
                try:
                    hook("committed")
                except Exception:
                    pass
            else:
                replace(log, rewritten)
                replace(ref, previous + b"\n")
                transaction.finish(False)

    try:
        drop()
    except Exception as error:
        if action != "pop":
            raise
        output["code"] = 1
        output["stderr"] += (
            f"Stash was applied, but removal did not complete; refresh the stash list before retrying: {error}\n"
        )
        return output
    output["stdout"] += f"Dropped {reference} ({oid})\n"
    return output
