#!/usr/bin/env python3
"""Check component independence without compilers, network access or package installation."""

import argparse
import contextlib
import hashlib
import io
import json
import os
import runpy
import sys
import tarfile
import tempfile
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from unittest.mock import patch

BUILDER = runpy.run_path(str(Path(__file__).with_name("build-deb.py")))
BUNDLER = runpy.run_path(str(Path(__file__).with_name("bundle-codex.py")))


class VendoredSandbox(unittest.TestCase):
    def test_pinned_sandbox_sources_match_the_recorded_release_snapshot(self):
        root = Path(__file__).resolve().parent.parent / "crates/rode-bwrap"
        manifest = json.loads((root / "vendor.json").read_text())
        paths = {
            str(path.relative_to(root)) for path in (root / "vendor").rglob("*") if path.is_file()
        }
        self.assertEqual(paths, set(manifest["files"]))
        for name, digest in manifest["files"].items():
            self.assertEqual(hashlib.sha256((root / name).read_bytes()).hexdigest(), digest, name)
        for name in ["bubblewrap", "libcap"]:
            self.assertTrue(manifest[name]["url"].startswith("https://"))
            self.assertEqual(len(manifest[name]["sha256"]), 64)


@contextlib.contextmanager
def runtime_fixture():
    """Exercise the real preparer with a tiny pinned archive and no internet/user data."""
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        (root / "packaging/codex-licenses").mkdir(parents=True)
        (root / "packaging/codex-licenses/LICENSE").write_bytes(b"fixture license")
        executable = b"#!/bin/sh\necho user-runtime-fixture\n"
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w:gz") as package:
            member = tarfile.TarInfo("codex")
            member.size = len(executable)
            package.addfile(member, io.BytesIO(executable))
        archive = archive.getvalue()
        archive_hash = hashlib.sha256(archive).hexdigest()
        spec = {
            "member": "codex",
            "url": "https://invalid.example/codex.tar.gz",
            "sha256": hashlib.sha256(executable).hexdigest(),
            "size": len(executable),
            "archive_sha256": archive_hash,
            "archive_size": len(archive),
        }
        manifest = root / "packaging/codex.json"
        manifest.write_text(
            json.dumps(
                {
                    "version": "9-fixture",
                    "targets": {"x86_64": spec, "aarch64": spec},
                    "notices": [
                        {
                            "file": "LICENSE",
                            "sha256": hashlib.sha256(b"fixture license").hexdigest(),
                        }
                    ],
                }
            )
        )
        cache = root / "target/codex-downloads"
        cache.mkdir(parents=True)
        (cache / (archive_hash + ".tar.gz")).write_bytes(archive)
        with (
            patch.dict(BUNDLER["install"].__globals__, ROOT=root, MANIFEST=manifest),
            patch.dict(os.environ, HOME=str(root / "home"), XDG_DATA_HOME=str(root / "user data")),
            patch("urllib.request.urlopen", side_effect=AssertionError("Unexpected download")),
        ):
            yield root, manifest


class UserRuntime(unittest.TestCase):
    def test_user_install_rejects_writable_ancestors_before_downloading(self):
        with runtime_fixture() as (root, _manifest):
            shared = root / "shared"
            data = shared / "data"
            data.mkdir(parents=True, mode=0o700)
            shared.chmod(0o777)
            with patch.dict(os.environ, XDG_DATA_HOME=str(data)):
                with self.assertRaisesRegex(ValueError, "ancestor is writable"):
                    BUNDLER["install"]("x86_64")
            self.assertFalse((data / "rode").exists())
            # Even an alias whose target returns to a protected directory must
            # not conceal a replaceable component of the resolution path.
            (data / "back").symlink_to(root, target_is_directory=True)
            alias = root / "alias"
            alias.symlink_to(data / "back", target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "ancestor is writable"):
                BUNDLER["trusted_ancestors"](alias)

    def test_install_repairs_missing_helpers_without_replacing_the_main_executable(self):
        with runtime_fixture() as (root, manifest):
            destination = BUNDLER["install"]("x86_64")
            inode = (destination / "codex").stat().st_ino
            pin = json.loads(manifest.read_text())
            body = b"#!/bin/sh\necho helper-fixture\n"
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode="w:gz") as package:
                member = tarfile.TarInfo("helper")
                member.size = len(body)
                package.addfile(member, io.BytesIO(body))
            data = archive.getvalue()
            digest = hashlib.sha256(data).hexdigest()
            (root / "target/codex-downloads" / (digest + ".tar.gz")).write_bytes(data)
            pin["targets"]["x86_64"]["helpers"] = {
                "codex-code-mode-host": dict(
                    member="helper",
                    url="https://invalid.example/helper",
                    size=len(body),
                    sha256=hashlib.sha256(body).hexdigest(),
                    archive_size=len(data),
                    archive_sha256=digest,
                )
            }
            manifest.write_text(json.dumps(pin))
            BUNDLER["install"]("x86_64")
            self.assertEqual((destination / "codex").stat().st_ino, inode)
            self.assertEqual((destination / "codex-code-mode-host").read_bytes(), body)
            self.assertTrue((destination / "codex-code-mode-host").stat().st_mode & 0o111)
            self.assertEqual(json.loads((destination / "manifest.json").read_text()), pin)
            BUNDLER["install"]("x86_64")

    def test_user_data_lookup_ignores_empty_or_relative_xdg_paths(self):
        with runtime_fixture() as (root, _manifest):
            self.assertEqual(BUNDLER["user_runtime_root"](), root / "user data/rode/codex")
            for value in [None, "", "relative"]:
                with patch.dict(os.environ):
                    if value is None:
                        os.environ.pop("XDG_DATA_HOME", None)
                    else:
                        os.environ["XDG_DATA_HOME"] = value
                    self.assertEqual(
                        BUNDLER["user_runtime_root"](), root / "home/.local/share/rode/codex"
                    )
                    os.environ.pop("HOME", None)
                    with self.assertRaisesRegex(ValueError, "Set HOME"):
                        BUNDLER["user_runtime_root"]()

    def test_concurrent_user_installs_select_complete_reusable_runtimes(self):
        with runtime_fixture() as (root, manifest):
            # The installer must not repair the user's data directory. Create
            # this ancestor privately regardless of the caller's umask.
            (root / "user data").mkdir(mode=0o700)
            parent = root / "user data/rode"
            parent.mkdir(parents=True)
            parent.chmod(0o775)
            (parent / "existing-settings").write_text("preserve")
            architectures = ["x86_64", "aarch64", "x86_64", "aarch64"]
            with ThreadPoolExecutor(max_workers=4) as pool:
                bundles = list(pool.map(BUNDLER["install"], architectures))
            base = root / "user data/rode/codex"
            self.assertEqual((base / "manifest.json").read_bytes(), manifest.read_bytes())
            self.assertEqual((base / "manifest.json").stat().st_mode & 0o777, 0o644)
            for architecture, bundle in zip(architectures, bundles, strict=True):
                self.assertEqual(bundle, base / "9-fixture" / architecture)
                self.assertEqual((bundle / "manifest.json").read_bytes(), manifest.read_bytes())
                self.assertTrue(os.access(bundle / "codex", os.X_OK))
                self.assertEqual((bundle / "LICENSE").read_bytes(), b"fixture license")
            self.assertFalse(list(base.glob(".manifest-*")))
            self.assertEqual((parent / "existing-settings").read_text(), "preserve")
            self.assertEqual(parent.stat().st_mode & 0o022, 0)

    def test_failed_verification_preserves_the_previous_selection(self):
        with runtime_fixture() as (root, manifest):
            first = BUNDLER["install"]("x86_64")
            selector = root / "user data/rode/codex/manifest.json"
            original = selector.read_bytes()
            changed = json.loads(manifest.read_text())
            changed["targets"]["aarch64"]["sha256"] = "0" * 64
            manifest.write_text(json.dumps(changed))
            with self.assertRaisesRegex(ValueError, "SHA-256"):
                BUNDLER["install"]("aarch64")
            self.assertEqual(selector.read_bytes(), original)
            self.assertTrue((first / "codex").is_file())
            self.assertFalse(first.with_name("aarch64").exists())

    def test_unsafe_paths_and_unavailable_architectures_do_not_publish_a_selector(self):
        for unsafe in ["symlink", "writable", "lock"]:
            with self.subTest(unsafe=unsafe), runtime_fixture() as (root, _manifest):
                base = root / "user data/rode/codex"
                base.parent.mkdir(parents=True)
                base.parent.chmod(0o700)
                victim = root / "unrelated"
                victim.mkdir()
                if unsafe == "symlink":
                    base.symlink_to(victim, target_is_directory=True)
                else:
                    base.mkdir(mode=0o700)
                    if unsafe == "writable":
                        base.chmod(0o777)
                    else:
                        (base / ".install.lock").symlink_to(victim / "must-not-be-created")
                with self.assertRaises((ValueError, OSError)):
                    BUNDLER["install"]("x86_64")
                self.assertFalse((base / "manifest.json").exists())
                self.assertEqual(list(victim.iterdir()), [])
        with runtime_fixture() as (root, _manifest):
            with self.assertRaisesRegex(ValueError, "supports Linux"):
                BUNDLER["install"]("unsupported-fixture")
            self.assertFalse((root / "user data").exists())


class Components(unittest.TestCase):
    def test_editor_does_not_consult_codex_or_download_a_runtime(self):
        """Even a tree with no Codex manifest must assemble the core component."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "README.md").write_text("editor")
            scope = BUILDER["main"].__globals__
            metadata = {
                "packages": [{"name": "rode", "version": "1.2.3"}],
                "target_directory": str(root / "target"),
            }
            helper = root / "target/x86_64-unknown-linux-musl/release/rode-sessiond-standalone"
            helper.parent.mkdir(parents=True)
            helper.write_bytes(b"static helper fixture")

            def check_elf(path, _architecture, static=False):
                self.assertEqual(path, helper if static else root / "target/release/rode")

            def output(*command):
                if "metadata" in command:
                    return json.dumps(metadata)
                self.assertEqual(command, ("dpkg", "--print-architecture"))
                return "amd64"

            def write_package(
                _args,
                name,
                version,
                _architecture,
                _destination,
                files,
                _executables,
                _documents,
                **_fields,
            ):
                self.assertEqual((name, version), ("rode", "1.2.3-1"))
                self.assertFalse(any("codex" in name.lower() for name in files))
                return root / "rode.deb"

            with (
                patch.dict(
                    scope,
                    ROOT=root,
                    output=output,
                    check_elf=check_elf,
                    notices=lambda _binary: b"licenses",
                    write_package=write_package,
                ),
                patch.object(
                    sys, "argv", ["build-deb.py", "--skip-build", "--shlibs-depends", "libc6"]
                ),
                patch("shutil.which", return_value="/build-tool"),
                patch("runpy.run_path", side_effect=AssertionError("Core must not prepare Codex")),
                contextlib.redirect_stdout(io.StringIO()),
            ):
                BUILDER["main"]()

    def test_skip_build_does_not_silently_package_an_obsolete_helper_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            legacy = root / "target/x86_64-unknown-linux-musl/release/rode-sessiond"
            legacy.parent.mkdir(parents=True)
            legacy.write_bytes(b"stale pre-migration helper")
            metadata = {
                "packages": [{"name": "rode", "version": "1.2.3"}],
                "target_directory": str(root / "target"),
            }

            def output(*command):
                return json.dumps(metadata) if "metadata" in command else "amd64"

            with (
                patch.dict(BUILDER["main"].__globals__, ROOT=root, output=output),
                patch.object(
                    sys, "argv", ["build-deb.py", "--skip-build", "--shlibs-depends", "libc6"]
                ),
                patch("shutil.which", return_value="/build-tool"),
            ):
                with self.assertRaisesRegex(ValueError, "Run cargo xtask build-helper"):
                    BUILDER["main"]()
            self.assertEqual(legacy.read_bytes(), b"stale pre-migration helper")
            self.assertFalse((root / "target/debian").exists())

    def test_missing_codex_target_fails_only_the_explicit_runtime_component(self):
        """An unsupported runtime target must not run Cargo or download anything."""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "packaging").mkdir()
            (root / "packaging/codex.json").write_text('{"version":"9.0.0","targets":{}}')
            scope = BUILDER["package_codex"].__globals__
            with (
                patch.dict(scope, ROOT=root, output=lambda *_a: "amd64"),
                patch("runpy.run_path", side_effect=AssertionError("No download")),
            ):
                with self.assertRaisesRegex(
                    ValueError, "build the editor without --component codex"
                ):
                    BUILDER["package_codex"](argparse.Namespace())


if __name__ == "__main__":
    unittest.main()
