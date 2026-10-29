#!/usr/bin/env python3
"""Build an upstream Debian binary package without installing into the host."""

import argparse
import datetime
import gzip
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CARGO = os.environ.get("CARGO", "cargo")


def output(*command, cwd=ROOT):
    """Run an explicit build/inspection tool and return stripped UTF-8 stdout."""
    return subprocess.check_output(command, cwd=cwd, text=True).strip()


def build(package, binary, target=None):
    """Build a selected Cargo binary and resolve its reported artifact instead of assuming a target path."""
    command = [
        CARGO,
        "build",
        "--locked",
        "--release",
        "-p",
        package,
        "--bin",
        binary,
        "--message-format=json-render-diagnostics",
    ]
    if target:
        command += ["--target", target]
    executable = None
    with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, text=True) as process:
        for line in process.stdout:
            try:
                event = json.loads(line)
            except ValueError:
                print(line, end="")
                continue
            if event.get("reason") == "compiler-message":
                print(event["message"].get("rendered", ""), file=sys.stderr, end="")
            if event.get("reason") == "compiler-artifact" and event["target"]["name"] == binary:
                executable = event.get("executable") or executable
        if process.wait():
            raise ValueError(f"Cargo failed while building {binary}")
    if not executable:
        raise ValueError(f"Cargo did not report the {binary} executable")
    return Path(executable)


def check_elf(path, architecture, static=False):
    """Verify the target ELF architecture and, for helpers, absence of a dynamic interpreter."""
    with path.open("rb") as source:
        header = source.read(20)
    machines = {"amd64": 62, "arm64": 183}
    if (
        header[:6] != b"\x7fELF\x02\x01"
        or int.from_bytes(header[18:20], "little") != machines[architecture]
    ):
        raise ValueError(f"{path} does not match Debian architecture {architecture}")
    if static and "INTERP" in output("readelf", "-lW", str(path)):
        raise ValueError("The terminal helper must be static; build the musl rode-sessiond target")


def shared_dependencies(binary, maintainer):
    """Resolve Debian dependencies with dpkg-shlibdeps, limiting fallback to verified host libc/libgcc links."""
    with tempfile.TemporaryDirectory(prefix="rode-shlibdeps-") as directory:
        root = Path(directory)
        (root / "debian").mkdir()
        (root / "debian/control").write_text(
            f"Source: rode\nMaintainer: {maintainer}\n\nPackage: rode\nArchitecture: any\nDescription: native Rust editor\n"
        )
        process = subprocess.run(
            ["dpkg-shlibdeps", "-O", "-e" + str(binary)], cwd=root, text=True, capture_output=True
        )
        if process.returncode:
            if not Path("/var/lib/dpkg/status").is_file():
                # Project sandboxes often hide /var. A native GUI that links
                # only libc/libgcc can conservatively require this host's libc.
                # Never infer package names for any other linked libraries.
                needed = set(
                    re.findall(
                        r"Shared library: \[([^\]]+)\]", output("readelf", "-dW", str(binary))
                    )
                )
                known = {
                    "libc.so.6",
                    "libm.so.6",
                    "libgcc_s.so.1",
                    "ld-linux-x86-64.so.2",
                    "ld-linux-aarch64.so.1",
                }
                if "libc.so.6" in needed and needed <= known:
                    libc = output("getconf", "GNU_LIBC_VERSION")
                    if re.fullmatch(r"glibc [0-9]+(?:\.[0-9]+)+", libc):
                        print(
                            "Debian package database unavailable; using a conservative host-libc dependency.",
                            file=sys.stderr,
                        )
                        return f"libc6 (>= {libc.split()[1]}), libgcc-s1"
            raise ValueError(
                "dpkg-shlibdeps failed. Install dpkg-dev and ensure its package database is available.\n"
                "For an isolated build environment, --shlibs-depends can supply verified dependency constraints.\n"
                + process.stderr.strip()
            )
        for line in process.stdout.splitlines():
            if line.startswith("shlibs:Depends="):
                return line.partition("=")[2]
    raise ValueError("dpkg-shlibdeps did not return shared-library dependencies")


def epoch():
    """Choose SOURCE_DATE_EPOCH, the latest commit time or current time for package metadata."""
    if "SOURCE_DATE_EPOCH" in os.environ:
        value = int(os.environ["SOURCE_DATE_EPOCH"])
        if value < 0:
            raise ValueError("SOURCE_DATE_EPOCH must be nonnegative")
        return value
    if shutil.which("git"):
        result = subprocess.run(
            ["git", "log", "-1", "--format=%ct"], cwd=ROOT, text=True, capture_output=True
        )
        if result.returncode == 0:
            return int(result.stdout.strip())
    return int(time.time())


def compressed(path, data, timestamp):
    """Write gzip bytes with an explicit reproducible timestamp and no source filename."""
    with (
        path.open("wb") as destination,
        gzip.GzipFile(fileobj=destination, filename="", mode="wb", mtime=timestamp) as archive,
    ):
        archive.write(data)


def notices(binary):
    # The built dependency graph supplies exact source directories. Only notice
    # files are included; no repository tree or private development files.
    """Collect notices from the exact offline Cargo dependency graph and embedded syntax/theme acknowledgements."""
    rustc = os.environ.get("RUSTC", "rustc")
    host = next(
        line.removeprefix("host: ")
        for line in output(rustc, "-vV").splitlines()
        if line.startswith("host: ")
    )
    metadata = json.loads(
        output(
            CARGO,
            "metadata",
            "--locked",
            "--offline",
            "--format-version=1",
            "--filter-platform",
            host,
        )
    )
    # metadata also lists unused optional/dev dependencies. Match the normal and
    # build graph of the workspace binaries instead of attributing unused code.
    tree = output(
        CARGO,
        "tree",
        "--locked",
        "--offline",
        "--workspace",
        "--target",
        host,
        "--edges",
        "normal,build",
        "--prefix",
        "none",
        "--format",
        "{p}",
    )
    active = set()
    for line in tree.splitlines():
        if not line.strip():
            continue
        match = re.match(r"^(\S+) v(\S+)(?: |$)", line)
        if not match:
            raise ValueError("Cannot identify a dependency in cargo tree output")
        active.add(match.groups())
    license_root = ROOT / "packaging/licenses"
    supplements = {
        package: group["files"]
        for group in json.loads((license_root / "manifest.json").read_text())
        for package in group["packages"]
    }
    sections = ["RODE third-party notices\n========================\n"]
    for package in sorted(
        metadata["packages"], key=lambda value: (value["name"], value["version"])
    ):
        if not package.get("source") or (package["name"], package["version"]) not in active:
            continue
        source = Path(package["manifest_path"]).parent
        candidates = [
            path
            for path in source.iterdir()
            if path.is_file()
            and re.match(r"^(LICENSE|LICENCE|COPYING|COPYRIGHT|NOTICE)(?:[.\-]|$)", path.name, re.I)
        ]
        for directory in [source / "LICENSES", source / "licenses"]:
            if directory.is_dir():
                candidates.extend(path for path in directory.rglob("*") if path.is_file())
        if package.get("license_file"):
            candidates.append(source / package["license_file"])
        crate_has_license = bool(candidates)
        if package["name"] == "epaint_default_fonts":
            # The fonts have their own terms, separate from egui's Rust code.
            # Keep the complete upstream texts, including embedded copyrights.
            for name in [
                "Hack-Regular.txt",
                "OFL.txt",
                "UFL.txt",
                "emoji-icon-font-mit-license.txt",
            ]:
                path = source / "fonts" / name
                if not path.is_file():
                    raise ValueError(f"Missing bundled font license: {path}")
                candidates.append(path)
        sections.append(
            f"\n{package['name']} {package['version']} — {package.get('license') or 'see upstream license'}\n"
        )
        for path in sorted(set(candidates)):
            name = path.relative_to(source) if path.is_relative_to(source) else path.name
            sections.append(f"\n--- {name} ---\n" + path.read_text(errors="replace") + "\n")
        extra = supplements.get(f"{package['name']}@{package['version']}", [])
        for entry in extra:
            path = license_root / entry["file"]
            data = path.read_bytes()
            if hashlib.sha256(data).hexdigest() != entry["sha256"]:
                raise ValueError(f"Upstream license snapshot checksum mismatch: {path.name}")
            sections.append(f"\n--- {entry['source']} ---\n" + data.decode() + "\n")
        if not crate_has_license and not extra:
            raise ValueError(
                f"Missing full license text for {package['name']} {package['version']}; "
                "review its upstream notices before packaging"
            )
        # Generic license texts need the concrete source-header attributions too.
        copyright_sources = {source / "src/lib.rs", source / "lib.rs"}
        if (source / "LICENSES").is_dir():
            copyright_sources.update(source.rglob("*.rs"))
        copyrights = sorted(
            {
                line.strip().removeprefix("//").strip()
                for path in copyright_sources
                if path.is_file()
                for line in path.read_text(errors="replace").splitlines()
                if re.match(r"^//\s*(?:Copyright\b|SPDX-FileCopyrightText:)", line)
            }
        )
        sections.extend(line + "\n" for line in copyrights)
    sysroot = Path(output(os.environ.get("RUSTC", "rustc"), "--print", "sysroot"))
    rust_docs = sysroot / "share/doc/rust"
    for path in sorted((rust_docs / "licenses").glob("*")) + [rust_docs / "COPYRIGHT-library.html"]:
        if path.is_file():
            sections.append(
                f"\nRust standard library — {path.name}\n" + path.read_text(errors="replace") + "\n"
            )
    for flag in ["--theme-licenses", "--syntax-licenses"]:
        sections.append(f"\nEmbedded definitions ({flag})\n" + output(str(binary), flag) + "\n")
    return "".join(sections).encode()


def main():
    """Validate build/package options and create a rootless archive without installing into the host."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--skip-build", action="store_true", help="package existing release binaries"
    )
    parser.add_argument("--binary", type=Path, help="existing GUI executable (with --skip-build)")
    parser.add_argument(
        "--helper", type=Path, help="existing static terminal helper (with --skip-build)"
    )
    parser.add_argument(
        "--output", type=Path, help="output directory (default: Cargo target directory/debian)"
    )
    parser.add_argument(
        "--version", help="Debian version override, including optional revision/epoch"
    )
    parser.add_argument(
        "--maintainer",
        default="RODE contributors <maintainer@example.invalid>",
        help="package maintainer Name <email>; set before redistribution",
    )
    parser.add_argument(
        "--shlibs-depends", help="override dpkg-shlibdeps output for a verified build environment"
    )
    args = parser.parse_args()
    if (args.binary or args.helper) and not args.skip_build:
        parser.error("--binary/--helper require --skip-build")
    for program in [CARGO, "rustc", "dpkg", "dpkg-deb", "readelf"]:
        if not shutil.which(program):
            raise ValueError(f"Missing build tool: {program}")
    if not args.shlibs_depends and not shutil.which("dpkg-shlibdeps"):
        raise ValueError("Install dpkg-dev to provide dpkg-shlibdeps")
    if not re.fullmatch(r"[^\r\n<>]+ <[^\s<>]+@[^\s<>]+>", args.maintainer):
        raise ValueError("Maintainer must be one line: Name <email>")
    if args.shlibs_depends and ("\n" in args.shlibs_depends or "\r" in args.shlibs_depends):
        raise ValueError("Dependencies must be a single control-field value")
    metadata = json.loads(output(CARGO, "metadata", "--locked", "--no-deps", "--format-version=1"))
    package = next(value for value in metadata["packages"] if value["name"] == "rode")
    target = Path(metadata["target_directory"])
    architecture = output("dpkg", "--print-architecture")
    if architecture not in ["amd64", "arm64"]:
        raise ValueError("Native Debian packaging supports amd64 and arm64")
    helper_target = {"amd64": "x86_64", "arm64": "aarch64"}[architecture] + "-unknown-linux-musl"
    if args.skip_build:
        binary = (args.binary or target / "release/rode").resolve()
        helper = (args.helper or target / helper_target / "release/rode-sessiond").resolve()
    else:
        binary = build("rode", "rode")
        subprocess.run(
            [os.environ.get("RUSTUP", "rustup"), "target", "add", helper_target], check=True
        )
        helper = build("rode-session", "rode-sessiond", helper_target)
    check_elf(binary, architecture)
    check_elf(helper, architecture, static=True)
    version = args.version or package["version"].replace("-", "~", 1) + "-1"
    subprocess.run(["dpkg", "--validate-version", version], check=True)
    if "/" in version or any(character.isspace() for character in version):
        raise ValueError("Invalid package version")
    shlibs = args.shlibs_depends or shared_dependencies(binary, args.maintainer)
    if not shlibs.strip():
        raise ValueError("Shared-library dependency list must not be empty")
    destination = (args.output or target / "debian").resolve()
    destination.mkdir(parents=True, exist_ok=True)
    timestamp = epoch()
    environment = dict(os.environ, SOURCE_DATE_EPOCH=str(timestamp))
    artifact = destination / f"rode_{version.split(':', 1)[-1]}_{architecture}.deb"
    with tempfile.TemporaryDirectory(prefix=".rode-deb-", dir=destination) as directory:
        work = Path(directory)
        stage = work / "root"
        files = {
            "usr/bin/rode": binary,
            "usr/bin/rode-sessiond": helper,
            "usr/share/applications/RODE.desktop": ROOT / "packaging/debian/RODE.desktop",
            "usr/share/icons/hicolor/scalable/apps/rode.svg": ROOT / "assets/rode.svg",
            "usr/share/doc/rode/copyright": ROOT / "LICENSE",
            "usr/share/doc/rode/README.Debian": ROOT / "packaging/debian/README.Debian",
            "usr/share/doc/rode/ARTWORK-LICENSING.md": ROOT / "docs/licensing.md",
        }
        for name, source in files.items():
            path = stage / name
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, path)
        documentation = stage / "usr/share/doc/rode"
        compressed(documentation / "README.md.gz", (ROOT / "README.md").read_bytes(), timestamp)
        compressed(documentation / "THIRD-PARTY-NOTICES.gz", notices(binary), timestamp)
        date = datetime.datetime.fromtimestamp(timestamp, datetime.timezone.utc).strftime(
            "%a, %d %b %Y %H:%M:%S +0000"
        )
        changelog = f"rode ({version}) unstable; urgency=medium\n\n  * Build the upstream native editor and static terminal helper.\n\n -- {args.maintainer}  {date}\n"
        compressed(documentation / "changelog.Debian.gz", changelog.encode(), timestamp)
        control = stage / "DEBIAN"
        control.mkdir()
        installed_size = sum(path.stat().st_size for path in stage.rglob("*") if path.is_file())
        template = (ROOT / "packaging/debian/control.in").read_text()
        (control / "control").write_text(
            template.format(
                version=version,
                architecture=architecture,
                maintainer=args.maintainer,
                installed_size=(installed_size + 1023) // 1024,
                shlibs=shlibs,
            )
        )
        sums = [
            f"{hashlib.md5(path.read_bytes()).hexdigest()}  {path.relative_to(stage)}\n"
            for path in sorted(stage.rglob("*"))
            if path.is_file() and control not in path.parents
        ]
        (control / "md5sums").write_text("".join(sums))
        for path in stage.rglob("*"):
            path.chmod(0o755 if path.is_dir() or path.parent == stage / "usr/bin" else 0o644)
            os.utime(path, (timestamp, timestamp))
        stage.chmod(0o755)
        os.utime(stage, (timestamp, timestamp))
        temporary = work / "package.deb"
        subprocess.run(
            [
                "dpkg-deb",
                "--root-owner-group",
                "-Zxz",
                "-z6",
                "--build",
                str(stage),
                str(temporary),
            ],
            env=environment,
            check=True,
            stdout=sys.stderr,
        )
        os.replace(temporary, artifact)
    print(artifact)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f"Debian package build failed: {error}")
