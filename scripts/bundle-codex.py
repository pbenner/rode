#!/usr/bin/env python3
"""Prepare RODE's pinned native Codex runtime; no Node.js or npm is required."""

import argparse
import fcntl
import hashlib
import json
import os
import platform
import shutil
import stat
import tarfile
import tempfile
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "packaging/codex.json"


def digest(path):
    """Hash a file with bounded working memory."""
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, "rb") as source:
        metadata = os.fstat(source.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 512 * 1024 * 1024:
            raise ValueError(f"Unsupported Codex cache file: {path.name}")
        result = hashlib.sha256()
        total = 0
        while block := source.read(1024 * 1024):
            total += len(block)
            if total > 512 * 1024 * 1024:
                raise ValueError("Codex cache file exceeds its size limit")
            result.update(block)
        return result.hexdigest()


def prepare(architecture=None, destination=None):
    """Verify/cache one official archive and atomically publish its executable bundle."""
    architecture = architecture or platform.machine()
    architecture = {"arm64": "aarch64", "amd64": "x86_64"}.get(architecture, architecture)
    manifest = json.loads(MANIFEST.read_text())
    if architecture not in manifest["targets"]:
        raise ValueError("Bundled Codex supports Linux x86_64 and aarch64")
    spec = manifest["targets"][architecture]
    destination = Path(
        destination or ROOT / "target/codex" / manifest["version"] / architecture
    ).resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with (destination.parent / (destination.name + ".lock")).open("a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        payload = {"codex": spec} | spec.get("helpers", {})
        if any(
            name not in ("codex", "codex-code-mode-host", "codex-exec-server") for name in payload
        ):
            raise ValueError("Unexpected Codex helper pin")
        existing = destination.exists()
        if existing:
            expected = set(payload) | {"manifest.json"} | {n["file"] for n in manifest["notices"]}
            if any(
                p.name not in expected or p.is_symlink() or not p.is_file()
                for p in destination.iterdir()
            ):
                raise ValueError("Unexpected files in Codex bundle; refusing to replace it")
            previous = json.loads((destination / "manifest.json").read_text())
            old = previous["targets"].get(architecture, {})
            if (
                previous["version"] != manifest["version"]
                or any(old.get(k) != spec[k] for k in ("size", "sha256"))
                or previous["notices"] != manifest["notices"]
            ):
                raise ValueError("Existing Codex bundle differs from this release")
            for name, pin in payload.items():
                path = destination / name
                if name != "codex" and not path.exists():
                    continue
                if path.stat().st_size != pin["size"] or digest(path) != pin["sha256"]:
                    raise ValueError(f"Codex bundle has changed: {name}; refusing to replace it")
            if any(digest(destination / n["file"]) != n["sha256"] for n in manifest["notices"]):
                raise ValueError("Existing Codex license notices have changed")
            if (destination / "manifest.json").read_bytes() == MANIFEST.read_bytes() and all(
                (destination / name).is_file() for name in payload
            ):
                destination.chmod(0o755)
                for path in destination.iterdir():
                    path.chmod(0o755 if path.name in payload else 0o644)
                return destination
        with tempfile.TemporaryDirectory(prefix=".codex-", dir=destination.parent) as work:
            stage = Path(work) / "bundle"
            stage.mkdir(mode=0o755)
            for name, spec in payload.items():
                if existing and (destination / name).exists():
                    shutil.copyfile(destination / name, stage / name)
                    (stage / name).chmod(0o755)
                    continue
                cache = ROOT / "target/codex-downloads"
                cache.mkdir(parents=True, exist_ok=True)
                archive = cache / (spec["archive_sha256"] + ".tar.gz")
                if not archive.exists() or digest(archive) != spec["archive_sha256"]:
                    with tempfile.NamedTemporaryFile(dir=cache, delete=False) as out:
                        temporary = Path(out.name)
                        try:
                            with urllib.request.urlopen(spec["url"], timeout=60) as response:
                                total = 0
                                while block := response.read(1024 * 1024):
                                    total += len(block)
                                    if total > spec["archive_size"]:
                                        raise ValueError("Codex archive exceeds its pinned size")
                                    out.write(block)
                            out.flush()
                            if (
                                total != spec["archive_size"]
                                or digest(temporary) != spec["archive_sha256"]
                            ):
                                raise ValueError(
                                    "Codex archive failed its pinned size/SHA-256 check"
                                )
                            os.replace(temporary, archive)
                        finally:
                            temporary.unlink(missing_ok=True)
                with tarfile.open(archive) as package:
                    members = package.getmembers()
                    if (
                        len(members) != 1
                        or not members[0].isfile()
                        or members[0].name != spec["member"]
                    ):
                        raise ValueError("Unexpected Codex archive contents")
                    if members[0].size != spec["size"]:
                        raise ValueError("Codex executable exceeds its pinned size")
                    with (
                        package.extractfile(members[0]) as source,
                        (stage / name).open("wb") as out,
                    ):
                        shutil.copyfileobj(source, out, 1024 * 1024)
                if digest(stage / name) != spec["sha256"]:
                    raise ValueError("Codex executable failed its pinned SHA-256 check")
                (stage / name).chmod(0o755)
            shutil.copyfile(MANIFEST, stage / "manifest.json")
            (stage / "manifest.json").chmod(0o644)
            for notice in manifest["notices"]:
                source = ROOT / "packaging/codex-licenses" / notice["file"]
                if digest(source) != notice["sha256"]:
                    raise ValueError(f"Codex license snapshot changed: {source.name}")
                shutil.copyfile(source, stage / source.name)
                (stage / source.name).chmod(0o644)
            if existing:
                for name in payload:
                    if not (destination / name).exists():
                        os.link(stage / name, destination / name)
                os.replace(stage / "manifest.json", destination / "manifest.json")
            else:
                os.replace(stage, destination)
    return destination


def user_runtime_root():
    """Match the editor's stable user-data lookup, independent of Cargo's install root."""
    data_home = Path(os.environ.get("XDG_DATA_HOME", ""))
    if not data_home.is_absolute():
        home = Path(os.environ.get("HOME", ""))
        if not home.is_absolute():
            raise ValueError("Set HOME or an absolute XDG_DATA_HOME to install a user runtime")
        data_home = home / ".local/share"
    return data_home / "rode/codex"


def protected_directory(path):
    """Refuse existing symlinked/shared runtime directories instead of trusting their contents."""
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    metadata = path.lstat()
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o022
    ):
        raise ValueError(f"Unsafe Codex runtime directory: {path}")


def install(architecture=None):
    """Explicitly prepare a user runtime and atomically select it after verification.

    The shared install lock serializes architectures and versions, so a concurrent
    setup cannot publish a selector before its payload is complete. Existing editors
    keep using their selected executable; no process, credentials or settings change.
    """
    architecture = architecture or platform.machine()
    architecture = {"arm64": "aarch64", "amd64": "x86_64"}.get(architecture, architecture)
    manifest = json.loads(MANIFEST.read_text())
    if architecture not in manifest["targets"]:
        raise ValueError("Bundled Codex supports Linux x86_64 and aarch64")
    base = user_runtime_root()
    # Existing preferences may have created RODE's parent with umask 002.
    # Tighten that owned directory without touching its files or stable locks.
    if base.parent.exists():
        descriptor = os.open(
            base.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
        )
        try:
            metadata = os.fstat(descriptor)
            if metadata.st_uid != os.geteuid():
                raise ValueError("Codex application-data directory must belong to this user")
            if metadata.st_mode & 0o022:
                os.fchmod(descriptor, stat.S_IMODE(metadata.st_mode) & ~0o022)
        finally:
            os.close(descriptor)
    protected_directory(base.parent)
    protected_directory(base)
    descriptor = os.open(
        base / ".install.lock",
        os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
        0o600,
    )
    with os.fdopen(descriptor, "a+b") as lock:
        metadata = os.fstat(lock.fileno())
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_uid != os.geteuid()
            or metadata.st_mode & 0o022
        ):
            raise ValueError("Unsafe Codex runtime install lock")
        fcntl.flock(lock, fcntl.LOCK_EX)
        version = base / manifest["version"]
        protected_directory(version)
        destination = version / architecture
        if destination.exists() or destination.is_symlink():
            protected_directory(destination)
        bundle = prepare(architecture, destination)
        selector = (bundle / "manifest.json").read_bytes()
        if json.loads(selector) != manifest:
            raise ValueError("Codex pin changed during installation; retry with a stable checkout")
        with tempfile.NamedTemporaryFile(prefix=".manifest-", dir=base, delete=False) as out:
            temporary = Path(out.name)
            try:
                out.write(selector)
                out.flush()
                os.fchmod(out.fileno(), 0o644)
                os.fsync(out.fileno())
                os.replace(temporary, base / "manifest.json")
            finally:
                temporary.unlink(missing_ok=True)
    return bundle


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--architecture", choices=["x86_64", "aarch64"])
    destination = parser.add_mutually_exclusive_group()
    destination.add_argument("--output", type=Path)
    destination.add_argument(
        "--install",
        action="store_true",
        help="install for this user so Cargo-installed editors can discover the runtime",
    )
    options = parser.parse_args()
    print(
        install(options.architecture)
        if options.install
        else prepare(options.architecture, options.output)
    )
