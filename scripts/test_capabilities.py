"""Explicit optional coverage and required-lane capabilities, without installing tools."""

import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


def read_skip_records(paths):
    """Retain malformed/incomplete coverage evidence without aborting later commands."""
    for path in sorted(paths):
        try:
            record = json.loads(path.read_text())
            if not isinstance(record, dict) or not all(
                isinstance(record.get(key), str) for key in ("capability", "reason", "test")
            ):
                raise ValueError("Expected capability, reason and test strings")
        except (OSError, ValueError) as error:
            yield path, None, str(error)
        else:
            yield path, record, None


def skip(capability, reason, *, test=None, environment=None):
    """Record omitted coverage even when the test framework captures successful stdout."""
    env = os.environ if environment is None else environment
    if capability in env.get("RODE_TEST_REQUIRED_CAPABILITIES", "").split(","):
        raise AssertionError(f"Required capability {capability}: {reason}")
    record = dict(capability=capability, reason=reason, test=test or "fixture")
    print(f"SKIP {record['test']} [{capability}]: {reason}", flush=True)
    if directory := env.get("RODE_TEST_SKIP_DIR"):
        Path(directory).mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".json", dir=directory, delete=False
        ) as out:
            json.dump(record, out)


def probe(command, environment):
    """Return availability and a diagnostic tail with a 15-second command deadline.

    Startup errors and timeouts are unavailable results, not successful skips.
    The caller decides whether its suite requires the capability.
    """
    try:
        result = subprocess.run(
            command, env=environment, capture_output=True, text=True, timeout=15
        )
        return result.returncode == 0, (result.stderr or result.stdout).strip()[-1500:]
    except (OSError, subprocess.TimeoutExpired) as error:
        return False, str(error)


def landlock_probe(environment):
    """Ask the supplied host helper to prepare an actual private Landlock policy."""
    helper = environment.get("RODE_TEST_SESSION_HELPER")
    if not helper:
        return False, "Set RODE_TEST_SESSION_HELPER to the built host helper"
    try:
        temporary = Path(
            environment.get("RODE_TEST_TMPDIR")
            or environment.get("TMPDIR")
            or tempfile.gettempdir()
        ).resolve(strict=True)
        with tempfile.TemporaryDirectory(prefix="ll-probe-", dir=temporary) as directory:
            request = {
                "op": "prepare",
                "path": str(Path(directory).resolve()),
                "sandbox": {"enabled": True, "backend": "landlock"},
            }
            result = subprocess.run(
                [helper, "--file-helper"],
                input=json.dumps(request),
                env=environment,
                capture_output=True,
                text=True,
                timeout=15,
            )
            if result.returncode:
                return False, (result.stderr or result.stdout).strip()[-1500:]
            response = json.loads(result.stdout)
            if not isinstance(response, dict) or not isinstance(response.get("project"), dict):
                return False, "No Landlock preparation result returned"
            plan = response["project"]
            available = plan.get("active") is True and plan.get("backend") == "landlock"
            return available, str(plan.get("reason") or "No active Landlock policy returned")[
                -1500:
            ]
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        return False, str(error)


def inventory(environment=None):
    """Report availability separately from proof that a feature's tests ran."""
    env = os.environ if environment is None else environment
    results = {}

    def record(name, available, detail):
        results[name] = dict(available=available, detail=detail)

    for name, commands in {
        "headless-ui": (
            "Xvfb",
            "xvfb-run",
            "xauth",
            "xwininfo",
            "xprop",
            "xclip",
            "import",
            "convert",
        ),
        "ssh": ("sshd", "ssh", "ssh-keygen"),
        "kerberos": ("krb5kdc", "kadmin.local", "kinit"),
    }.items():
        missing = [
            command
            for command in commands
            if not shutil.which(command, path=env.get("PATH"))
            and not (command == "sshd" and os.access("/usr/sbin/sshd", os.X_OK))
        ]
        record(
            name, not missing, "Missing: " + ", ".join(missing) if missing else "Executables found"
        )
    for name, variable, fallback in (
        ("ruff", "RODE_TEST_RUFF", "ruff"),
        ("lldb", "RODE_TEST_LLDB", None),
        ("mise", "RODE_TEST_MISE", "mise"),
    ):
        value = env.get(variable) or (
            shutil.which(fallback, path=env.get("PATH")) if fallback else None
        )
        record(name, bool(value and os.access(value, os.X_OK)), value or f"Set {variable}")
    for name, variable, imports in (
        ("jupyter", "RODE_TEST_JUPYTER", "jupyter_client, ipykernel, matplotlib"),
        ("debugpy", "RODE_TEST_DEBUGPY", "debugpy"),
    ):
        python = env.get(variable) or (env.get("RODE_TEST_JUPYTER") if name == "debugpy" else None)
        if python:
            available, detail = probe([python, "-c", f"import {imports}"], env)
            record(name, available, detail or python)
        else:
            record(name, False, f"Set {variable} to an interpreter with {imports}")
    bwrap = shutil.which("bwrap", path=env.get("PATH"))
    helper = env.get("RODE_TEST_SESSION_HELPER")
    launcher = [bwrap] if bwrap else [helper, "--bwrap-launch"] if helper else ["bwrap"]
    available, detail = probe(
        [*launcher, "--unshare-all", "--ro-bind", "/", "/", "--", "/bin/true"], env
    )
    record("bwrap", available, detail or "Unprivileged namespace probe passed")
    available, detail = landlock_probe(env)
    record("landlock", available, detail)
    record(
        "fuse",
        os.access("/dev/fuse", os.R_OK | os.W_OK),
        "Requires usable /dev/fuse and mount permissions; test must still verify mounting",
    )
    return results
