# Debian packages

RODE can build a native `.deb` for Debian/Ubuntu on **amd64** or **arm64**. The amd64 package has been built and exercised; arm64 requires a native arm64 build machine. No Cargo packaging plugin or root permissions are needed to assemble the archive.

## GitHub Actions

The **Package Debian** workflow (`.github/workflows/package.yml`) builds packages for release version tags or explicit manual runs. Regular commit and pull-request checks run code, SSH and UI validation without building or uploading Debian packages.

For a manual build:

1. Put the workflow on the repository's default branch so GitHub exposes [manual workflow runs](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow).
2. Set the repository Actions variable **DEB_MAINTAINER** to your package-maintainer identity, such as `Your Name <you@example.org>`. Alternatively, enter it in the manual run's **maintainer** field; that value takes precedence.
3. Open **Actions → Package Debian → Run workflow**, select a branch and start the run.
4. Download **rode-debian-amd64** from the completed run's artifacts or summary link. Optionally download **rode-codex-debian-amd64** for Codex chat. Extract each ZIP separately and verify it with `sha256sum -c SHA256SUMS`.

Pushing a version tag such as `v0.1.0` also triggers packaging. The tag must exactly match `v` plus the version in `Cargo.toml`; this catches accidentally mislabelled packages. Tag builds use **DEB_MAINTAINER**. Set that variable before pushing a version tag. Package versions retain the builder's Debian `-1` revision and prerelease conversion described below.

The workflow uses a fixed **Ubuntu 24.04 amd64** runner, builds the release editor and static terminal helper with the locked Cargo dependencies, and validates metadata, licenses, extracted binaries, a real terminal session and GUI startup before uploading. Each artifact contains the `.deb` and `SHA256SUMS`, with a requested retention of 30 days. Runtime library requirements reflect that build system; see [compatibility and dependencies](#contents-and-dependencies).

The optional Codex job runs after the editor artifact has been published. It builds and validates `rode-codex` separately, checks discovery alongside the extracted editor, and exercises the real agent protocol without an account or model. A missing runtime/download failure does not discard the editor artifact. Manual runs can disable **build_codex**; tags build both components. Package assembly remains release-only.

This produces downloadable workflow artifacts. Installing packages or publishing GitHub Releases remains a separate action; the workflow only requests read access to repository contents.

## Build

Install Rust 1.88+ with rustup and the native [build prerequisites](getting-started.md). Debian packaging additionally uses `dpkg-dev`, binutils, and Python 3. Desktop validation is provided by `desktop-file-utils`:

```sh
sudo apt install build-essential pkg-config dpkg-dev binutils python3 \
  libxkbcommon-dev libwayland-dev libgl1-mesa-dev desktop-file-utils
scripts/build-deb.sh
```

The default builder compiles the release editor, ensures the matching Rust musl target is available and builds the static terminal helper. It never resolves or downloads Codex assets and includes no Codex executable. Cargo/rustup may download build dependencies. Output goes to `target/debian/rode_<version>_<architecture>.deb`, or the corresponding Cargo target directory if configured. For the current version on amd64:

```sh
sudo apt install ./target/debian/rode_0.1.0-1_amd64.deb
rode /path/to/project
```

APT resolves runtime dependencies. Installation is a separate, explicit action; the build script never installs the package. Launch **RODE** from the desktop Development menu, or open a local directory with RODE from a file manager.

The version comes from `Cargo.toml`, with Debian revision `-1`; Cargo prereleases use `~` for Debian ordering. Before redistributing, supply a real maintainer identity in place of the local-build placeholder:

```sh
scripts/build-deb.sh --version 0.1.0-2 \
  --maintainer 'Your Name <you@example.org>' --output target/packages
```

### Optional Codex runtime

Build Codex separately when needed:

```sh
scripts/build-deb.sh --component codex
sudo apt install ./target/debian/rode-codex_0.166.0-1_amd64.deb
rode --codex login
```

This component needs Python 3, `dpkg-deb`, `dpkg` and `readelf`, with no Rust compiler or editor build. It prepares the exact upstream asset pinned in `packaging/codex.json`, verifies archive/executable/notice hashes and packages the static runtime. It requires no Node.js/npm. Its version comes from the Codex manifest, independently of `Cargo.toml`; `--version` can override its Debian revision too. Compatible runtime releases require real protocol checks.

The editor lists `rode-codex` only in **Suggests**, so ordinary installation does not pull it in automatically. Users may instead click **Install Codex** in the chat panel for rootless installation in their user data directory, or select an existing compatible executable. A missing upstream architecture affects only this explicitly requested runtime build; it is never consulted by the core package builder. The current native Debian builder remains limited to amd64/arm64; separating Codex does not claim new editor platforms.

## Contents and dependencies

The package installs:

- `/usr/bin/rode` and the static `/usr/bin/rode-sessiond`. The editor discovers the sibling helper for SSH deployment; local terminals default to the matching embedded service.
- A desktop launcher and the scalable `[r]` application icon.
- The project license, [artwork licensing notes](licensing.md), upstream dependency/Rust/embedded-definition notices, README, and package changelog under `/usr/share/doc/rode/`. The notices include the complete Hack, Noto Emoji, Ubuntu and emoji-icon-font license texts supplied by egui; these are separate from the Rust code license. Missing crate license files use versioned upstream snapshots with verified hashes; missing license coverage stops packaging.

Only these explicit assets are packaged. Local development guides, project files, credentials, preferences and build caches are excluded. Installation/removal does not start a system service, change user preferences, or stop persistent terminal sessions.

The separate `rode-codex` package installs native Codex under `/usr/lib/rode/codex/VERSION/ARCH/`, with its manifest and complete upstream notices. A protected `/usr/lib/rode/codex/manifest.json` selects that package's pinned version without an editor rebuild. Its documentation and assembled `CODEX-NOTICES.gz` live under `/usr/share/doc/rode-codex/`. It owns no editor, launcher, terminal helper or settings files, and has no dependency on `rode`. `rode --codex` forwards CLI commands such as `login` and `--version` when a runtime is available.

Linked-library version requirements come from [`dpkg-shlibdeps`](https://manpages.debian.org/trixie/dpkg-dev/dpkg-shlibdeps.1.en.html). X11, Wayland, keyboard and OpenGL libraries loaded dynamically are declared separately. OpenSSH, Git and Poppler are recommended packages; bubblewrap, Python/venv and process-monitor tools are suggested. Individual project toolchains, agents, model servers and Ruff/uv remain user-managed.

Build on the oldest Debian/Ubuntu release you intend to support. Building on a newer system can require newer libc/library versions; the package metadata records those requirements. A `.deb` extension alone does not make a binary compatible with every Debian-derived distribution. The SSH helper is static and does not impose the desktop's glibc requirement on the remote host.

## Existing binaries and isolated builds

After building release binaries, CI can package without compiling again. `--skip-build` skips Rust compilation and still packages only the editor:

```sh
scripts/build-sessiond.sh
cargo build --locked --release
scripts/build-deb.sh --skip-build
python3 scripts/test-deb.py target/debian/rode_0.1.0-1_amd64.deb
```

`--binary PATH` and `--helper PATH`, used with `--skip-build`, select other existing release outputs. The helper must be static, and both ELF architectures must match the native Debian architecture; mismatches are rejected. This is a native packager, not a cross-build system.

For an optional portable runtime, run `python3 scripts/bundle-codex.py` and copy its `VERSION/ARCH` directory into `rode-codex/VERSION/ARCH` beside the editor. Also copy `packaging/codex.json` to `rode-codex/manifest.json` to select that runtime version independently. Keep directories/files protected from writes by other users, and retain all manifest and notice files with the executable. The runtime is a separate on-demand process; its substantial disk size does not add an always-running model or Node.js runtime. Model servers and models remain separately managed.

`dpkg-shlibdeps` normally uses `/var/lib/dpkg`. Restricted project sandboxes sometimes hide it. For the current native GUI, which links only libc/libgcc, the builder then uses a conservative requirement for the build host's glibc version and reports this choice. This can require a newer target system than a normal symbol-based scan. Unknown additional linked libraries cause an error rather than guessed dependencies.

An advanced build environment can explicitly supply **verified constraints** through `--shlibs-depends`. For example, a conservative local build on a glibc 2.41 system can use:

```sh
scripts/build-deb.sh --skip-build \
  --shlibs-depends 'libc6 (>= 2.41), libgcc-s1'
```

The override replaces only automatic ELF dependency scanning; the declared display-library dependencies remain. Do not lower version constraints to make an incompatible system accept a package. Normal Debian/Ubuntu builds should use the default scanner.

`SOURCE_DATE_EPOCH` controls archive/document timestamps. When unset, the builder uses the last Git commit timestamp, or the current time for a source tree without Git history. Given identical binaries, source notices, metadata and build tools, setting this variable allows reproducible package assembly. [`dpkg-deb --root-owner-group`](https://manpages.debian.org/trixie/dpkg/dpkg-deb.1.en.html) records root ownership without requiring root during the build.

## Validation

`scripts/test-deb.py` inspects metadata, architecture, payload allowlists, ownership/modes, checksums, launcher syntax, notices, the extracted editor CLI, and a real shell started by the extracted native helper. It verifies that the core contains no Codex files and works without a runtime. Add `--gui` under `xvfb-run` to exercise the extracted desktop application too. Use `--codex-package PATH` to additionally inspect the optional component, overlay it and check verified discovery, version and app-server support. Everything runs in temporary directories; no system package is installed or removed. The **Package Debian** workflow runs this validation before uploading release-tag or manually requested packages.
