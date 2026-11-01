# Getting started

## Run

For Debian/Ubuntu, `scripts/build-deb.sh` creates an installable editor package with the desktop launcher and terminal helper. Codex is a separate optional package built with `scripts/build-deb.sh --component codex`. See [package prerequisites and installation](packaging.md).

Install a stable Rust toolchain (Rust 1.88 or newer), then:

```sh
cargo install --locked --path .
rode /path/to/project
# Or open one existing file directly:
rode /path/to/project/main.py
```

A directory opens as a project. A file opens immediately in its normal editor or preview, with its containing directory as the workspace. Relative paths work too, such as `rode main.py`; quote paths containing spaces. Saved sandbox settings for that directory still apply, and `--sandbox` requests isolation for it explicitly.

This installs **both `rode` and `rode-sessiond`** into Cargo's binary directory, normally `~/.cargo/bin`, without root access. RODE discovers the sibling helper for SSH persistent terminals on compatible Linux hosts. For older/different Linux hosts, build a static helper with `scripts/build-sessiond.sh`; see [terminal setup](terminals.md#remote-helper-and-packaging).

For Codex chat, click **Install Codex** in the chat panel, install the optional `rode-codex` package, or use an existing executable. Authenticate locally with `rode --codex login`, then leave the agent executable set to `auto`; see [agent setup](agents.md). VS Code, Node.js and npm are not needed. Editing and other tools work without Codex.

For development without installing, `cargo build --locked --release` builds `target/release/rode` and `target/release/rode-sessiond`; `cargo run --release -- .` runs only the editor target. Use the release build when evaluating responsiveness or memory use. Linux is the validated platform; the SSH backend uses Unix OpenSSH ControlMaster.

On Debian/Ubuntu, common build/runtime prerequisites are a C compiler, `pkg-config`, OpenGL, X11 or Wayland libraries, and the OpenSSH client. If development libraries are missing:

```sh
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev libgl1-mesa-dev openssh-client
```

RODE uses egui/eframe with the OpenGL renderer. It repaints on input, terminal output, and cursor/UI changes, rather than continuously running an animation loop. Directory listings and file transfers run on background threads. The explorer loads folders on demand, caches the visible tree, and renders only visible rows. Syntax highlighting is cached and disabled above 512 KiB. Release binaries use thin LTO and stripped symbols. On Linux installations with incompatible OpenGL drivers, startup retries once with Mesa software rendering if Mesa is available.

Earlier Linux release smoke measurements (before the persistent-terminal service): **13.7 MiB executable**, approximately **138.0 MiB RSS** after editing and opening a terminal under Xvfb/Mesa software rendering, and **0 CPU scheduler ticks during a one-second idle sample** with the project-tool panels hidden and no agent started. A separate session with small image previews and the terminal open measured **127.2 MiB RSS** and **0 idle ticks**. Larger previews use additional bounded pixel memory. Agents launch on demand and model servers use their own resources. These describe small test sessions, not a large-project benchmark or comparison with VS Code.

## First project

Launch `rode /path/to/project`, open a file directly with `rode /path/to/file`, or use **File → Open project folder**. Use **Ctrl+P** to open another file by path and **Ctrl+S** to save. **Ctrl+Shift+P** opens the command palette.

Use **New** in the terminal header for multiple native shells; **Sessions** reattaches them and shows [Slurm allocations](slurm.md). Enable **Persist** for shells that should survive exit; new shells default to ending when their last view closes.

For remote files, use [SSH connections](remote.md). For isolation, see [sandbox access](sandbox.md). Configuration paths and detailed theme settings are documented under [Themes](themes.md).
