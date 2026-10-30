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
        if destination.exists():
            expected = {"codex", "manifest.json"} | {n["file"] for n in manifest["notices"]}
            if any(
                p.name not in expected or p.is_symlink() or not p.is_file()
                for p in destination.iterdir()
            ):
                raise ValueError("Unexpected files in Codex bundle; refusing to replace it")
            if digest(destination / "codex") != spec["sha256"]:
                raise ValueError(f"Codex bundle has changed: {destination}; remove it and rebuild")
            if (destination / "manifest.json").read_bytes() == MANIFEST.read_bytes() and all(
                digest(destination / n["file"]) == n["sha256"] for n in manifest["notices"]
            ):
                destination.chmod(0o755)
                for path in destination.iterdir():
                    path.chmod(0o755 if path.name == "codex" else 0o644)
                return destination
            # License-only updates can replace an otherwise unchanged bundle.
            shutil.rmtree(destination)
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
                    if total != spec["archive_size"] or digest(temporary) != spec["archive_sha256"]:
                        raise ValueError("Codex archive failed its pinned size/SHA-256 check")
                    os.replace(temporary, archive)
                finally:
                    temporary.unlink(missing_ok=True)
        with tempfile.TemporaryDirectory(prefix=".codex-", dir=destination.parent) as work:
            stage = Path(work) / "bundle"
            stage.mkdir(mode=0o755)
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
                with package.extractfile(members[0]) as source, (stage / "codex").open("wb") as out:
                    shutil.copyfileobj(source, out, 1024 * 1024)
            if digest(stage / "codex") != spec["sha256"]:
                raise ValueError("Codex executable failed its pinned SHA-256 check")
            (stage / "codex").chmod(0o755)
            shutil.copyfile(MANIFEST, stage / "manifest.json")
            (stage / "manifest.json").chmod(0o644)
            for notice in manifest["notices"]:
                source = ROOT / "packaging/codex-licenses" / notice["file"]
                if digest(source) != notice["sha256"]:
                    raise ValueError(f"Codex license snapshot changed: {source.name}")
                shutil.copyfile(source, stage / source.name)
                (stage / source.name).chmod(0o644)
            os.replace(stage, destination)
    return destination


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--architecture", choices=["x86_64", "aarch64"])
    parser.add_argument("--output", type=Path)
    options = parser.parse_args()
    print(prepare(options.architecture, options.output))
