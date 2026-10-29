# Getting started

## Run

For Debian/Ubuntu, `scripts/build-deb.sh` creates an installable package with the desktop launcher and terminal helper. See [package prerequisites and installation](packaging.md).

Install a stable Rust toolchain (Rust 1.88 or newer), then:

```sh
cargo run --release -- .
```

For remote persistent terminals, also run `scripts/build-sessiond.sh` and package `target/release/rode-sessiond-static` beside the editor. See [terminal setup](terminals.md#remote-helper-and-packaging).

The compiled application is `target/release/rode`. Use the release build when evaluating responsiveness or memory use. Linux is the validated platform; the SSH backend uses Unix OpenSSH ControlMaster.

On Debian/Ubuntu, common build/runtime prerequisites are a C compiler, `pkg-config`, OpenGL, X11 or Wayland libraries, and the OpenSSH client. If development libraries are missing:

```sh
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev libgl1-mesa-dev openssh-client
```

RODE uses egui/eframe with the OpenGL renderer. It repaints on input, terminal output, and cursor/UI changes, rather than continuously running an animation loop. Directory listings and file transfers run on background threads. The explorer loads folders on demand, caches the visible tree, and renders only visible rows. Syntax highlighting is cached and disabled above 512 KiB. Release binaries use thin LTO and stripped symbols. On Linux installations with incompatible OpenGL drivers, startup retries once with Mesa software rendering if Mesa is available.

Earlier Linux release smoke measurements (before the persistent-terminal service): **13.7 MiB executable**, approximately **138.0 MiB RSS** after editing and opening a terminal under Xvfb/Mesa software rendering, and **0 CPU scheduler ticks during a one-second idle sample** with the project-tool panels hidden and no agent started. A separate session with small image previews and the terminal open measured **127.2 MiB RSS** and **0 idle ticks**. Larger previews use additional bounded pixel memory. Agents launch on demand and model servers use their own resources. These describe small test sessions, not a large-project benchmark or comparison with VS Code.

## First project

Launch `rode /path/to/project`, or use **File → Open project folder**. Use **Ctrl+P** to open a file by path and **Ctrl+S** to save. **Ctrl+Shift+P** opens the command palette.

Use **New** in the terminal header for multiple native persistent shells; **Sessions** reattaches them and shows [Slurm allocations](slurm.md). On exit, choose whether attached terminal work should keep running.

For remote files, use [SSH connections](remote.md). For isolation, see [sandbox access](sandbox.md). Configuration paths and detailed theme settings are documented under [Themes](themes.md).
