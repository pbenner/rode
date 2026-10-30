# Persistent terminals

RODE owns terminal sessions through a small native Rust service on the computer running the shell. It needs neither tmux nor screen. Local and SSH sessions support multiple terminals and simultaneous attachment from several RODE windows.

## Everyday use

- **New** in the terminal header creates another native terminal with **Persist: off** in the current project, using its prepared sandbox and selected environment. Switch between views using the terminal tabs.
- **Sessions**, or **View → Terminals & Slurm**, lists your running terminals on the current host, including attachment count, project path, and sandbox mode. Use **Attach**, **Rename**, or **End** there. Exited terminals are cleared automatically; the visible list refreshes every two seconds.
- **Persist: off / on** at the far right of the terminal header controls whether that session keeps running after its last view closes. The button uses the theme's accent fill when on; click it again to switch off. New terminals start off. Change it at any time without restarting the shell or its commands. The setting belongs to the session and updates in every attached RODE window.
- **Detach** closes a view while its shell and commands continue; enable **Persist** first. In a narrow pane, Detach and Copy are under **More**. Hiding the terminal with **Ctrl+`** keeps the view attached.
- Reopen the original project on the same host, with the same sandbox policy, and choose **Sessions → Attach** to resume. Changing projects closes the old views. Persistent sessions continue on their original host and sandbox; nonpersistent sessions end when their last view closes.

Several windows may attach to the same session: their keystrokes go to the same shell, and output appears in every attached view. The terminal uses the smallest attached view's dimensions so text wraps consistently. Detaching one view leaves the others connected. Separate terminals have independent shells, working directories, and environments.

New terminals receive the selected Python environment. Reattachment preserves the original running environment; RODE does not send activation commands into an existing program. **Activate in current terminal** remains an explicit action.

Prompt colors follow the selected theme's ANSI palette, including bright entries for bold PS1 segments. Open **View → Terminal colors…** to adjust these swatches or foreground/background; optional minimum contrast is separate. Settings apply immediately without restarting the shell. See [themes](themes.md). Each attached RODE window renders with its own selected theme; the shared session's output stays unchanged.

## Bash prompts and colors

RODE uses your shell's existing prompt configuration (`PS1` for Bash). It does not replace `.bashrc`, rewrite prompt commands or inject a different prompt. Bash expands host/path/status/Git segments as usual; RODE renders the resulting ANSI sequences with the selected theme.

RODE follows [xterm's default boldColors behavior](https://www.invisible-island.net/xterm/manpage/xterm.html): a bold basic foreground uses the corresponding bright color. For example, `\[\e[1;30m\]` in PS1 uses **Bright black**, rather than ordinary black that can disappear against a dark background; `\[\e[1;34m\]` uses **Bright blue**. The `\[...\]` markers belong to Bash/readline and keep invisible escape sequences out of its cursor-width calculation. `\e[22m` clears bold, and `\e[0m` resets attributes. Backgrounds, explicit RGB and extended color indices retain their color mapping.

Under **View → Terminal colors… → ANSI colors**, adjust the **Bright black** or **Bright blue** swatch if those prompt segments need a different shade for your theme. These changes immediately affect existing local/SSH terminal views without restarting commands. Minimum contrast defaults to **1:1** (off); its optional global text correction is separate from prompt escape handling. Explicit saved contrast overrides remain in effect until changed or reset to theme defaults.

## Closing RODE

RODE protects unsaved source buffers, then closes without asking about terminals:

- **Persist: on** leaves that session running on its host, discoverable through **Sessions → Attach**.
- **Persist: off** ends the session when its last attached view closes. Closing one RODE window preserves a session still attached in another window.
- Temporary shells stop with their owning window or project. They cannot enable persistence; use a native terminal for work that needs to continue.

The same rule applies on project changes, desktop crashes, lost SSH connections and **Disconnect** in the SSH banner. The native service owns this choice; no whole-file editor configuration is involved. **Terminal: New persistent terminal** in the command palette (or the equivalent session-manager button) explicitly creates a session with persistence already on.

After an editor update, older services keep their existing shells and jobs. Updated RODE windows list and attach to both old and new sessions, and route rename/end actions to the owning service. An older session keeps its original lifetime setting; its disabled Persist button explains that a new terminal is needed for persistence controls. New terminals use a compatible service alongside the old one and still default to **Persist: off**. No jobs are stopped or migrated during the update.

If startup or a persistence change is still in flight, closing waits briefly for its result instead of asking for a policy. A failed persistence change reports the error; if closing was waiting for it, the window stays open so you can check the session state. Persistent-startup errors remain visible rather than silently substituting a temporary shell.

Ending a terminal interrupts its shell/programs. It does not explicitly cancel Slurm jobs. Use the separate [allocation release control](slurm.md) to request that GPUs be freed. Closing RODE never automatically calls `scancel`.

## Remote helper and packaging

The easy Cargo installation includes the standalone headless helper:

```sh
cargo install --locked --path .
rode-sessiond protocol
```

Both `rode` and `rode-sessiond` are installed beside each other, normally in `~/.cargo/bin`. A normal `cargo build --release` also builds both binaries under `target/release/`. No separate crate installation is needed. Local terminals continue to use the editor's matching embedded service, and SSH deployment discovers the sibling helper automatically. The Cargo-installed helper targets the build machine's architecture and libc; it is usable on compatible Linux hosts.

For hosts with a different/older libc, build a static Linux helper:

```sh
scripts/build-sessiond.sh
# Produces target/release/rode-sessiond-static on the matching architecture.
```

Package `rode-sessiond-static` beside `rode`, or set desktop `RODE_SESSION_HELPER` to the helper file. Static helpers take precedence over the ordinary sibling. The script supports `x86_64` and `aarch64`; cross compilation requires a suitable linker/toolchain. It builds the opt-in `rode-sessiond-standalone` target in the `rode-session` library package, keeping desktop dependencies out of the musl build, then copies it to the discovery name `rode-sessiond-static`. The helper must match the **remote** architecture and current terminal protocol. A stale explicit or packaged helper produces a rebuild instruction before starting new work. The static build needs no remote Rust installation, GUI libraries, or compatible glibc. Linux x86_64 is the tested combination.

On first use, RODE transfers this checksum-verified helper through its authenticated SSH channel to `~/.local/share/rode/session-helpers/sessiond-<sha256>`. Files/directories are owner-only. No separate TCP listener or application port forwarding is used. Remote file editing itself retains its existing Python-only transport and remains usable without this helper. Temporary terminals are also available without it.

## Lifetime and limits

The service owns the PTYs independently of the desktop and SSH master. It runs outside the project sandbox; the actual shell and descendants run **inside the project's original prepared sandbox**. Control/attachment clients run on the host. The Linux service checks UID and mount namespace so sandboxed tools cannot use an exposed control socket to start unrestricted host commands.

The default runtime is `/tmp/rode-session-v1-<uid>/`, with mode `0700`, a mode `0600` Unix socket, a stable singleton lock, and `service.log`. Optional `RODE_SESSION_DIR` selects a private absolute runtime directory; its parent must exist, and remote hosts must be able to create/use the same path. Do not remove its lock/socket while sessions run. Windows using the same host, UID, mount namespace, and runtime discover the same service. Removing the last session stops that service.

The service also exits automatically after five seconds without a running shell, including an unused startup or the last shell exiting naturally. A live shell counts even at an idle prompt or with no attached windows. Session-list requests and completed tabs do not keep an empty service running. Shutdown removes its socket and releases its singleton lock; creating a terminal starts a fresh service automatically. The stable lock file stays in place for safe concurrent startup.

When an older service is still running, the compatible service uses the private `protocol-2/` subdirectory with its own socket, lock, and log. Both remain discoverable through the same runtime setting. Do not remove either service's files while it owns sessions.

There are at most 32 live sessions per service, 8 attached views per session, and 16 views per window. Scrollback is bounded to 1,000 rows, with a bounded screen/history replay when reattaching to a running shell. Output is consumed even with no viewers; a slow viewer is disconnected rather than blocking the job. On process exit, the service reaps its child, drains final output and releases the session record and terminal slot automatically, even while other shells keep running. Already displayed output remains in its editor tab; exited sessions cannot be reattached. Updated clients also hide completed records reported by older running services. This is a terminal session service, not a durable log or editor crash-recovery system.

**Persist: on** survives closing/crashing the desktop or losing SSH. It cannot survive reboot, service termination, scheduler time limits, or a host policy that kills user processes at logout. Run the persistent terminal on a stable login/working host **before** allocating compute resources; see [interactive Slurm work](slurm.md).
