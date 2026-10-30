#!/usr/bin/env python3
"""Inspect/extract a .deb and exercise its packaged binaries without installing."""

import argparse
import gzip
import hashlib
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import time
import uuid
from pathlib import Path


def field(package, name):
    return subprocess.check_output(["dpkg-deb", "--field", str(package), name], text=True).strip()


def checksum(path):
    """Keep verification memory bounded even for the optional native runtime."""
    result = hashlib.md5()
    with path.open("rb") as source:
        while block := source.read(1024 * 1024):
            result.update(block)
    return result.hexdigest()


def test_codex_package(package, extracted, binary, environment, architecture):
    """Overlay the optional package and check discovery without installing into the host."""
    assert field(package, "Package") == "rode-codex"
    assert field(package, "Architecture") == architecture
    assert not field(package, "Depends"), "The static runtime must not depend on the editor"
    with (
        subprocess.Popen(
            ["dpkg-deb", "--fsys-tarfile", str(package)], stdout=subprocess.PIPE
        ) as payload,
        tarfile.open(fileobj=payload.stdout, mode="r|") as archive,
    ):
        for member in archive:
            assert member.uid == member.gid == 0 and not member.mode & 0o022, member.name
            assert not member.issym() and not member.islnk(), member.name
            name = member.name.removeprefix("./")
            assert member.isdir() or name.startswith(
                ("usr/lib/rode/codex/", "usr/share/doc/rode-codex/")
            ), name
    assert payload.returncode == 0
    control = extracted.parent / "codex-control"
    subprocess.run(["dpkg-deb", "--control", str(package), str(control)], check=True)
    subprocess.run(["dpkg-deb", "--extract", str(package), str(extracted)], check=True)
    for line in (control / "md5sums").read_text().splitlines():
        digest, name = line.split("  ", 1)
        assert checksum(extracted / name) == digest, name
    manifest = extracted / "usr/lib/rode/codex/manifest.json"
    pin = json.loads(manifest.read_text())
    codex_version = subprocess.check_output(
        [str(binary), "--codex", "--version"], text=True, env=environment
    )
    assert pin["version"] in codex_version, codex_version
    architecture = {"amd64": "x86_64", "arm64": "aarch64"}[field(package, "Architecture")]
    directory = manifest.parent / pin["version"] / architecture
    assert (directory / "manifest.json").read_bytes() == manifest.read_bytes()
    with (directory / "codex").open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    assert digest == pin["targets"][architecture]["sha256"]
    assert "INTERP" not in subprocess.check_output(
        ["readelf", "-lW", str(directory / "codex")], text=True
    )
    assert "--listen" in subprocess.check_output(
        [str(binary), "--codex", "app-server", "--help"], text=True, env=environment
    )
    with gzip.open(extracted / "usr/share/doc/rode-codex/CODEX-NOTICES.gz", "rt") as source:
        text = source.read()
        assert "Apache License" in text and "OpenAI Codex" in text
    print(
        f"{package.name}: optional payload, independent version, checksums, static runtime and editor discovery passed."
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument(
        "--codex-package",
        type=Path,
        help="also validate and overlay an optional rode-codex package",
    )
    parser.add_argument(
        "--gui",
        action="store_true",
        help="also launch the extracted GUI on DISPLAY, e.g. under xvfb-run",
    )
    args = parser.parse_args()
    package = args.package.resolve()
    assert field(package, "Package") == "rode"
    assert "rode-codex" in field(package, "Suggests")
    assert "rode-codex" not in field(package, "Depends") + field(package, "Recommends")
    assert (
        field(package, "Architecture")
        == subprocess.check_output(["dpkg", "--print-architecture"], text=True).strip()
    )
    for dependency in [
        "libc6",
        "libgcc-s1",
        "libgl1",
        "libegl1",
        "libx11-6",
        "libwayland-client0",
        "libxkbcommon0",
    ]:
        assert dependency in field(package, "Depends"), dependency
    with (
        subprocess.Popen(
            ["dpkg-deb", "--fsys-tarfile", str(package)], stdout=subprocess.PIPE
        ) as payload,
        tarfile.open(fileobj=payload.stdout, mode="r|") as archive,
    ):
        names = set()
        for member in archive:
            assert member.uid == member.gid == 0, member.name
            assert not (member.mode & 0o022), member.name
            assert not member.issym() and not member.islnk(), member.name
            name = member.name.removeprefix("./")
            assert not any(
                part in name.split("/") for part in ["AGENTS.md", "DESIGN.md", ".git", ".env"]
            ), name
            assert member.isdir() or name.startswith(
                (
                    "usr/bin/",
                    "usr/share/applications/",
                    "usr/share/icons/",
                    "usr/share/doc/rode/",
                )
            ), name
            names.add(name)
        assert {
            "usr/bin/rode",
            "usr/bin/rode-sessiond",
            "usr/share/applications/RODE.desktop",
            "usr/share/icons/hicolor/scalable/apps/rode.svg",
            "usr/share/doc/rode/copyright",
            "usr/share/doc/rode/ARTWORK-LICENSING.md",
            "usr/share/doc/rode/THIRD-PARTY-NOTICES.gz",
        } <= names
        assert not any(
            name.startswith("usr/lib/rode/codex") or name.endswith("CODEX-NOTICES.gz")
            for name in names
        )
    assert payload.returncode == 0
    with tempfile.TemporaryDirectory(prefix="rode-deb-test-") as directory:
        root = Path(directory)
        extracted = root / "unpacked"
        subprocess.run(["dpkg-deb", "--raw-extract", str(package), str(extracted)], check=True)
        for line in (extracted / "DEBIAN/md5sums").read_text().splitlines():
            digest, name = line.split("  ", 1)
            assert checksum(extracted / name) == digest, name
        binary = extracted / "usr/bin/rode"
        helper = extracted / "usr/bin/rode-sessiond"
        help_text = subprocess.check_output([str(binary), "--help"], text=True)
        assert "native Rust editor" in help_text
        codex_environment = dict(os.environ)
        codex_environment.pop("RODE_CODEX_BUNDLE", None)
        codex_home = root / "codex-home"
        codex_home.mkdir(mode=0o700)
        codex_environment["CODEX_HOME"] = str(codex_home)
        missing = subprocess.run(
            [str(binary), "--codex", "--version"],
            text=True,
            capture_output=True,
            env=dict(codex_environment, HOME=str(codex_home), PATH=""),
        )
        assert missing.returncode != 0 and "optional rode-codex" in missing.stderr, missing.stderr
        assert "INTERP" not in subprocess.check_output(["readelf", "-lW", str(helper)], text=True)
        if shutil.which("desktop-file-validate"):
            subprocess.run(
                ["desktop-file-validate", str(extracted / "usr/share/applications/RODE.desktop")],
                check=True,
            )
        with gzip.open(extracted / "usr/share/doc/rode/THIRD-PARTY-NOTICES.gz", "rt") as notices:
            text = notices.read()
            assert "serde " in text and "MIT" in text and "--syntax-licenses" in text
            for notice in [
                "fonts/Hack-Regular.txt",
                "fonts/OFL.txt",
                "fonts/UFL.txt",
                "fonts/emoji-icon-font-mit-license.txt",
                "Copyright (c) 2018 Source Foundry Authors",
                "SIL OPEN FONT LICENSE Version 1.1",
                "UBUNTU FONT LICENCE Version 1.0",
                "Copyright (c) 2014 John Slegers",
            ]:
                assert notice in text, f"Missing bundled font notice: {notice}"
            for notice in [
                "Copyright (c) 2018-2021 Emil Ernerfeldt",
                "Copyright (C) 2018-present iced project and contributors",
                "Copyright (c) 2020 Philip Degarmo and other contributors",
                "Copyright 2015 Brendan Zabarauskas and the gl-rs developers",
                "LICENSES/MIT.txt",
                "SPDX-FileCopyrightText: 2017 - 2023 Kamila Borowska",
                "Apache License",
                "http://www.apache.org/licenses/",
            ]:
                assert notice in text, f"Missing dependency license or attribution: {notice}"
        runtime = root / "sessions"
        environment = dict(os.environ, RODE_SESSION_DIR=str(runtime))
        project = root / "project"
        project.mkdir()
        session = "s-" + uuid.uuid4().hex
        policy = "a" * 64

        def request(action, **values):
            output = subprocess.check_output(
                [str(helper), "request"],
                input=json.dumps(dict(action=action, **values)).encode(),
                env=environment,
            )
            reply = json.loads(output)
            assert reply["kind"] != "error", reply
            return reply.get("data")

        try:
            request(
                "create",
                id=session,
                name="Packaged helper test",
                root=str(project),
                policy=policy,
                mode="NORMAL MODE",
                argv=["/bin/sh", "-c", "printf packaged > result; exec sleep 60"],
                initial_input="",
                env={"PATH": "/usr/bin:/bin"},
            )
            deadline = time.monotonic() + 5
            while not (project / "result").exists():
                assert time.monotonic() < deadline, "Packaged helper did not run its shell"
                time.sleep(0.02)
            assert (project / "result").read_text() == "packaged"
            assert any(info["id"] == session for info in request("list"))
            if args.gui:
                assert os.environ.get("DISPLAY"), "--gui needs an X display; use xvfb-run"
                home = root / "home"
                home.mkdir()
                gui = subprocess.run(
                    [str(binary)],
                    cwd=project,
                    env=dict(
                        environment,
                        HOME=str(home),
                        XDG_CONFIG_HOME=str(home / "config"),
                        XDG_DATA_HOME=str(home / "data"),
                        RODE_SMOKE_TEST="1",
                    ),
                    capture_output=True,
                    text=True,
                    timeout=20,
                )
                assert gui.returncode == 0, gui.stderr + gui.stdout

            if args.codex_package:
                optional = args.codex_package.resolve()
                test_codex_package(
                    optional, extracted, binary, codex_environment, field(package, "Architecture")
                )

        finally:
            request("end", id=session, policy=policy)
            deadline = time.monotonic() + 8
            while (runtime / "socket").exists():
                assert time.monotonic() < deadline, "Fixture terminal service did not exit"
                time.sleep(0.02)
    checks = "metadata, root ownership/modes, payload, checksums, launcher, licenses, CLI, and native helper"
    if args.gui:
        checks += ", GUI launch"
    print(f"{package.name}: {checks} passed.")


if __name__ == "__main__":
    main()
