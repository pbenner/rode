"""Project-host sandbox policy construction and capability probing.

Prepended by files::REMOTE_HELPER, with RODE_SANDBOX_POLICY supplied first. This
standard-library peer constructs and probes the same ordered grants as sandbox.rs. Only
preparation may choose visible normal-mode fallback; a failed operation in an active plan
never retries outside it.
"""

# Prepended to the per-request file helper; only Python's standard library.
import os
import shutil
import subprocess
import sys

# Supplied by Rust before these standard-library peers are concatenated.
POLICY = globals()["RODE_SANDBOX_POLICY"]


def sandbox_validate_root(root, home):
    """Reject whole-home grants even when HOME names a symlink to the real directory."""
    actual_home = os.path.realpath(home)
    if not os.path.isdir(actual_home):
        raise ValueError("Cannot resolve the home directory while checking sandbox grants.")
    if root == "/" or any(
        os.path.commonpath([root, spelling]) == root for spelling in (home, actual_home)
    ):
        raise ValueError("Sandbox paths must not expose the whole home directory or its parent.")


def sandbox_arguments(binary, root, home, options):
    """Validate grants and build ordered bubblewrap mounts without executing the sandbox."""
    overrides = options.get("overrides", {})
    defaults = POLICY["system_read_only"] + [
        "~/" + p for p in POLICY["home_read_only"] + POLICY["home_read_write"]
    ]
    if any(
        path not in defaults or access not in ("hidden", "read_only", "read_write")
        for path, access in overrides.items()
    ):
        raise ValueError("Invalid sandbox mount override.")
    flags = {"hidden": None, "read_only": "--ro-bind", "read_write": "--bind"}
    readonly = options.get("read_only", [])
    writable = options.get("read_write", [])
    if len(readonly) + len(writable) > 32:
        raise ValueError("At most 32 additional sandbox paths are supported.")
    if sum(len(path.encode("utf-8")) for path in readonly + writable) > 16 * 1024:
        raise ValueError("Additional sandbox paths exceed the 16 KiB total limit.")
    sandbox_validate_root(root, home)
    args = [binary, "--unshare-all", "--die-with-parent"]
    if options.get("share_network", True):
        args += ["--share-net"]
    for path in POLICY["system_read_only"]:
        flag = flags[overrides.get(path, "read_only")]
        if flag and os.path.exists(path):
            args += [flag, path, path]
    args += [
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
        "--tmpfs",
        "/home",
        "--dir",
        home,
        "--dir",
        os.path.join(home, ".config"),
    ]
    for flag, paths in [
        ("--ro-bind", POLICY["home_read_only"]),
        ("--bind", POLICY["home_read_write"]),
    ]:
        for relative in paths:
            path = os.path.join(home, relative)
            override = flags[overrides["~/" + relative]] if "~/" + relative in overrides else flag
            if override and os.path.exists(path):
                args += [override, path, path]
    mounts = [("--ro-bind" if options.get("project_read_only", False) else "--bind", root)]
    seen = {root}
    for flag, paths in [("--ro-bind", readonly), ("--bind", writable)]:
        for path in paths:
            if (
                not isinstance(path, str)
                or not path
                or len(path.encode("utf-8")) > 4096
                or "\0" in path
            ):
                raise ValueError("Invalid sandbox path.")
            if not (path.startswith("/") or path == "~" or path.startswith("~/")):
                raise ValueError("Additional sandbox paths must be absolute or start with ~/.")
            path = os.path.realpath(os.path.expanduser(path))
            if not os.path.exists(path):
                raise ValueError("Additional sandbox path is unavailable: " + path)
            sandbox_validate_root(path, home)
            if path in seen:
                raise ValueError(
                    "Duplicate sandbox grant. Use project access for its root and one rule per additional path."
                )
            seen.add(path)
            mounts.append((flag, path))
    for flag, path in sorted(mounts, key=lambda item: item[1].count("/")):
        args += [flag, path, path]
    args += ["--setenv", "HOME", home, "--setenv", "UV_LINK_MODE", "copy", "--chdir", root, "--"]
    return args


def prepare_project(path, options):
    """Canonicalize the root and probe the complete plan; report actual isolation or explicit fallback."""
    if not os.path.isdir(path):
        raise ValueError("Choose a directory to open as a project.")
    plan = {
        "root": path,
        "requested": options.get("enabled", False),
        "active": False,
        "project_read_only": options.get("project_read_only", False),
        "reason": "",
        "prefix": [],
    }
    if not plan["requested"]:
        return plan
    home = os.path.expanduser("~")
    sandbox_validate_root(path, home)
    binary = shutil.which("bwrap") if sys.platform == "linux" else None
    if binary is None:
        plan["reason"] = "bwrap is not installed on this host; this project is open in normal mode."
        return plan
    prefix = sandbox_arguments(binary, path, home, options)
    try:
        result = subprocess.run(
            prefix + ["/bin/true"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            timeout=10,
        )
        if result.returncode != 0:
            plan["reason"] = (
                "bwrap cannot start on this host; normal mode: "
                + result.stderr.decode("utf-8", "replace")[:2048].strip()
            )
        else:
            plan.update(active=True, prefix=prefix)
    except (OSError, subprocess.TimeoutExpired) as error:
        plan["reason"] = "bwrap cannot start on this host; normal mode: " + str(error)
    return plan
