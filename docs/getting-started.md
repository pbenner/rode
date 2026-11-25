# Getting started

## Run

For Debian/Ubuntu, `scripts/build-deb.sh` creates an installable editor package with the desktop launcher and terminal helper. Codex is a separate optional package built with `scripts/build-deb.sh --component codex`. See [package prerequisites and installation](packaging.md).

Install Rust (1.93 or newer) with rustup and the native build prerequisites below, including a musl C toolchain (`musl-tools` on Debian/Ubuntu). Then run the source installer on Linux x86_64 or aarch64:

```sh
cargo xtask install
rode /path/to/project
# Or open one existing file directly:
rode /path/to/project/main.py
```

A directory opens as a project. Launching `rode` without a path opens the current working directory in **Normal** mode; saved sandbox settings are retained but are not applied to this implicit startup. Add `--sandbox` to request isolation explicitly. The window starts on Welcome with Explorer hidden. Select **Explorer** or **Project tools** in the left activity bar to enter the workspace. Click the RODE logo, choose **View → Welcome**, or run **Application: Show Welcome** in the command palette to return to Welcome without closing files or stopping work.

A file argument opens immediately in its normal editor or preview, normally with its containing directory as the workspace. Relative paths work too, such as `rode main.py`; quote paths containing spaces. Saved sandbox settings for that directory still apply, and `--sandbox` requests isolation for it explicitly.

If global sandbox defaults would reject a file's containing directory because it is your home directory or an ancestor, RODE opens just the file without a project. The window title and bottom status bar show **File mode**: file reads and saves are unsandboxed, no project tools or terminal start automatically, and sandbox preferences are unchanged. This exception does not override `--sandbox`, an explicitly saved folder policy, or directory arguments. For sandboxed editing, keep files in a project subdirectory and open that project.

This installs **`rode`, `rode-sessiond` and `rode-sessiond-static`** into Cargo's binary directory, normally `~/.cargo/bin`, without root access. The installer builds and verifies the static Linux helper before updating the editor. RODE automatically selects it for SSH, so remote hosts do not need the desktop's glibc version or a helper environment variable. Repeat the same command to update both. Cargo/rustup may download build dependencies on the desktop; no remote compiler or Rust installation is needed.

Use `cargo xtask install --root /path/to/prefix` for a custom install directory; binaries go in its `bin/` subdirectory. Defaults honor `CARGO_INSTALL_ROOT` and `CARGO_HOME`. Plain `cargo install --locked --path .` remains available, but its ordinary helper requires a compatible remote libc. See [SSH setup](remote.md#host-service-backends) and [helper setup](terminals.md#remote-helper-and-packaging).

For Codex chat, click **Install Codex** in the chat panel, install the optional `rode-codex` package, or use an existing executable. Authenticate locally with `rode --codex login`, then leave the agent executable set to `auto`; see [agent setup](agents.md). VS Code, Node.js and npm are not needed. Editing and other tools work without Codex.

For development without installing, `cargo build --locked --release` builds `target/release/rode`, `target/release/rode-sessiond` and the independent `target/release/rode-stocks` extension; `cargo run --release -- .` runs only the editor target. Use the release build when evaluating responsiveness or memory use. Linux is the validated platform; the default SSH client is embedded Rust (`russh`), with an optional OpenSSH backend. Remote transport currently requires Unix.

On Debian/Ubuntu, common build/runtime prerequisites are a C compiler, `pkg-config`, OpenGL, X11 or Wayland libraries. The OpenSSH client is optional for compatibility mode and external tools such as Git-over-SSH. If development libraries are missing:

```sh
sudo apt install build-essential pkg-config musl-tools \
  libxkbcommon-dev libwayland-dev libgl1-mesa-dev
```

`musl-tools` is required by `cargo xtask install` and `cargo xtask build-helper` to compile the bundled bubblewrap C sources for the static helper. Rustup installs Rust's musl libraries, but not the musl C compiler or headers. If the build reports a missing `musl-gcc` or `x86_64-linux-musl-gcc`, install this prerequisite and retry. A custom or cross toolchain can instead provide the target compiler and sysroot through `CC_<target>` and `CFLAGS_<target>`; an ordinary glibc compiler is insufficient.

RODE uses egui/eframe with the OpenGL renderer. It repaints for input, output and active UI work. Directory listings and file transfers run on background threads. Explorer loads folders on demand and renders visible rows. Source highlighting runs incrementally on a background worker, without a file-size cutoff; edited lines use plain text until their highlighting is ready. Release binaries use thin LTO and stripped symbols. On Linux installations with incompatible OpenGL drivers, startup retries once with Mesa software rendering if Mesa is available.

## First project

Launch `rode /path/to/project`, open a file directly with `rode /path/to/file`, or use **Open project…** in the top header (**File → Open project folder** / **Ctrl+Shift+O**). Use **Ctrl+P** to open another file by path and **Ctrl+S** to save. **Ctrl+Shift+P** opens the command palette.

The header action becomes **Close project** for an explicitly opened project,
or **Close & lock** for an encrypted project. Closing checks unsaved edits in
every editor window and returns to Welcome. Local projects return to the startup
CWD in Normal mode; remote projects keep their SSH connection available for
opening another folder. **Disconnect** closes the remote project first, including
verified encrypted unmount, then releases SSH. Failed saves or locks keep the
project available. Use **File → Open project folder…**, a saved project in **Connections**, or
**Ctrl+Shift+O** to open another folder while a project is already open.

Open **Project tools** in the left activity bar. It opens Chat first, then remembers your selected tab. Available tabs are Chat, Terminal, Git, Problems, Tasks, Shared folders, Debugger and Settings, subject to enabled extensions. Close tools with the same button, or open a file in Explorer, to return to the editor.

Use **Connections** for saved projects, SSH profiles and tunnel setups. **System monitor**, **File transfers**, **Stocks** and **App settings** are separate application views in the left rail. Disabled extensions hide their controls. See [workspace layout](editing.md#workspace-layout).

Use **New terminal** in the terminal header for multiple native shells; **Sessions** reattaches them and shows [Slurm allocations](slurm.md). Enable **Persist** for shells that should survive exit; new shells default to ending when their last view closes.

For remote files, use [SSH connections](remote.md). For isolation, see [sandbox access](sandbox.md). Configuration paths and detailed theme settings are documented under [Themes](themes.md).
