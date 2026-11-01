"""Terminate private stash workers and verify Git releases their transaction locks."""

import ctypes
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

spec = json.load(sys.stdin)
if sys.platform.startswith("linux"):
    assert ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0  # PR_SET_CHILD_SUBREAPER


def wait(check, message):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.02)
    raise AssertionError(message)


def reap():
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        try:
            pid, _ = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            return
        if not pid:
            time.sleep(0.02)
    raise AssertionError("Owned Git descendants did not exit")


for peer in (False, True):
    for packed in (False, True):
        for stop in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT, signal.SIGKILL):
            with tempfile.TemporaryDirectory(prefix="rode-stash-interrupt-") as temp:
                root = Path(temp)
                environment = dict(
                    os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1"
                )

                def git(*args):
                    return subprocess.check_output(
                        ["git", *args],
                        cwd=root,
                        env=environment,
                        stderr=subprocess.DEVNULL,
                        text=True,
                    ).strip()

                git("init", "-b", "main")
                git("config", "user.name", "RODE Test")
                git("config", "user.email", "rode@example.invalid")
                (root / "file").write_text("baseline\n")
                git("add", "file")
                git("commit", "-qm", "Baseline")
                (root / "file").write_text("saved\n")
                git("stash", "push", "-qm", "Saved")
                oid = git("rev-parse", "refs/stash")
                if packed:
                    git("pack-refs", "--all")
                before = (root / ".git/logs/refs/stash").read_bytes()
                hook = root / ".git/hooks/reference-transaction"
                hook.write_text(
                    "#!/usr/bin/env python3\nimport os, sys, time\nfrom pathlib import Path\n"
                    "if sys.argv[1] == 'prepared':\n"
                    " Path('hook-ready').write_text(str(os.getpgrp()))\n"
                    " while not Path('release-hook').exists(): time.sleep(.02)\n"
                )
                hook.chmod(0o755)
                source = root / "peer.py"
                source.write_text(spec["peer"])
                command = (
                    [sys.executable, str(source)] if peer else [spec["binary"], "--file-helper"]
                )
                request = json.dumps(
                    dict(
                        op="tool",
                        request=dict(
                            task="git_stash",
                            root=str(root),
                            action="drop",
                            reference="stash@{0}",
                            oid=oid,
                        ),
                    )
                ).encode()
                worker = subprocess.Popen(
                    command,
                    cwd=root,
                    env=environment,
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    start_new_session=True,
                )
                worker.stdin.write(request)
                worker.stdin.close()
                try:
                    wait(
                        lambda: (root / "hook-ready").exists() or worker.poll() is not None,
                        "Stash hook never started",
                    )
                    assert worker.poll() is None, worker.stderr.read().decode()
                    os.kill(worker.pid, stop)
                    worker.wait(timeout=5)
                    wait(
                        lambda: not list((root / ".git").rglob("*.lock")),
                        f"Stale Git locks after {stop}: peer={peer}, packed={packed}",
                    )
                    assert (root / ".git/logs/refs/stash").read_bytes() == before
                    assert git("rev-parse", "refs/stash") == oid
                finally:
                    (root / "release-hook").touch()
                    if worker.poll() is None:
                        os.killpg(worker.pid, signal.SIGKILL)
                        worker.wait()
                    if (root / "hook-ready").exists():
                        try:
                            os.killpg(int((root / "hook-ready").read_text()), signal.SIGTERM)
                        except ProcessLookupError:
                            pass
                    reap()
                result = subprocess.run(
                    command,
                    input=request,
                    cwd=root,
                    env=environment,
                    capture_output=True,
                    timeout=8,
                )
                assert result.returncode == 0, result.stderr.decode()
                assert not git("stash", "list")
                assert not list((root / ".git").rglob("*.lock"))
                reap()

print("Native/Python stash interruptions release loose and packed Git locks for TERM/HUP/INT/KILL")
