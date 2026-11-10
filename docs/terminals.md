# Persistent terminals

RODE renders the line-drawing borders used by ncurses terminal applications with
its bundled monospace font. Borders and character-set selection are preserved
when detaching and reattaching a session.

RODE owns terminal sessions through a small native Rust service on the computer running the shell. It needs neither tmux nor screen. Local and SSH sessions support multiple terminals and simultaneous attachment from several RODE windows.

## Everyday use

- Drag across terminal text to select it, including output visible in scrollback or after a process exits. **Ctrl+C** copies the selection; with nothing selected it interrupts the shell/program. **Ctrl+Shift+C** (or **Cmd+C** on macOS), **Copy** in the terminal header, and **Terminal: Copy output** in the command palette copy the selection, or the visible output when nothing is selected. Right-click for **Copy selection** and **Copy visible output**. Clicking the terminal or pressing Escape clears the selection. Selections also clear when the displayed text changes or the viewport scrolls/resizes, so copying cannot pick up unrelated output.
- **New** in the terminal header creates another native terminal with **Persist: off** in the current project, using the remembered terminal sandbox choice and selected environment. The initial choice follows the project sandbox. Switch between views using the terminal tabs.
- **Sandbox: Landlock / bwrap / off** in the terminal header reports the selected shell’s actual execution mode. Open its menu and choose **New sandboxed terminal** or **New unsandboxed terminal**. Switching opens a separate shell on the same project host and leaves existing shells and jobs running. New terminals remember this choice per host/project. A sandboxed shell requires an active project sandbox; enable it in project settings first. Run, Build, project tasks and Slurm allocations keep the project policy, opening a separate terminal when needed. Agent permissions are unchanged.
- **Sessions**, or **View → Terminals & Slurm**, lists your running terminals on the current host, including attachment count, project path, and sandbox mode. Use **Attach**, **Rename**, or **End** there. Exited terminals are cleared automatically; the visible list refreshes every two seconds.
- The same manager lists [notebook kernels](notebooks.md) and [debuggers](debugger.md) under **Notebook kernels & debuggers**. Their persistence controls retain execution and inspection state on the host; reattach from the original project and sandbox, or use **End runtime** to stop them explicitly.
- **Persist: off / on** at the far right of the terminal header controls whether that session keeps running after its last view closes. The button uses the theme's accent fill when on; click it again to switch off. New terminals start off. Change it at any time without restarting the shell or its commands. The setting belongs to the session and updates in every attached RODE window.
- **Slurm…**, beside Persist, requests an [interactive allocation](slurm.md) in a new terminal. It shows pending status, then a filled allocated-GPU indicator, and provides confirmed cancellation/release. Slurm environment variables are available in the compute-node shell; enable Persist separately to retain it after exit.
- **Detach** in the workspace toolbar opens the terminal in a separate window. **Dock back**, or closing that window, reintegrates it without ending the shell.
- **Detach tab** opens only Terminal in its own window, leaving other tools in the sidebar. **Reattach**, or closing that window, returns it with the same shell. The Terminal tab stays visible and disabled while detached.
- **Detach session** in the terminal header closes a view while its shell and commands continue; enable **Persist** first. In a narrow pane, Detach and Copy are under **More**. Hiding the tools sidebar with **Ctrl+~** (or **Ctrl+`**) keeps the view attached and remembers its tab. **Ctrl+Shift+T** selects the terminal directly.
- Reopen the original project on the same host, with the same sandbox policy for sandboxed sessions, and choose **Sessions → Attach** to resume. Changing projects closes the old views. Persistent sessions continue on their original host and sandbox; nonpersistent sessions end when their last view closes.

Several windows may attach to the same session: their keystrokes go to the same shell, and output appears in every attached view. The terminal uses the smallest attached view's dimensions so text wraps consistently. Detaching one view leaves the others connected. Separate terminals have independent shells, working directories, and environments.

New terminals receive the selected Python environment plus applied runtime paths and project variables from **Environment**. Reattachment preserves the original running environment; RODE does not send activation commands into an existing program. **Activate in current terminal** remains an explicit action.

SSH terminals support [X11 applications](remote.md#x11-applications) when the
desktop and remote server provide X11 forwarding. The connection's native
gateway keeps the display tunnel alive when a terminal view is detached.
After an SSH reconnect, use a new terminal for X applications; persistent
shells retain their original display environment. Sandbox access restrictions
continue to apply.

Terminal text and prompt colours follow the selected theme’s palette, including bright entries for bold PS1 segments. Default text matches the theme’s UI text. PS1 paths using the bold-blue ANSI slot render in warm gold for Carbon/Amber, green for Phosphor/Matrix, cool blue for Ice/Code Dark and violet for Neon. Red/green status indicators remain distinct. Open **View → Terminal colors…** to adjust these swatches or foreground/background; optional minimum contrast is separate. Settings apply immediately without restarting the shell. See [themes](themes.md). Each attached RODE window renders with its own selected theme; the shared session's output stays unchanged.

## Bash prompts and colors

RODE uses your shell's existing prompt configuration (`PS1` for Bash). It does not replace `.bashrc`, rewrite prompt commands or inject a different prompt. Bash expands host/path/status/Git segments as usual; RODE renders the resulting ANSI sequences with the selected theme.

RODE follows [xterm's default boldColors behavior](https://www.invisible-island.net/xterm/manpage/xterm.html): a bold basic foreground uses the corresponding bright color. For example, `\[\e[1;30m\]` in PS1 uses **Bright black**, rather than ordinary black that can disappear against a dark background; `\[\e[1;34m\]` uses the theme’s **Bright blue** slot (warm gold in Carbon/Amber). The `\[...\]` markers belong to Bash/readline and keep invisible escape sequences out of its cursor-width calculation. `\e[22m` clears bold, and `\e[0m` resets attributes. Backgrounds, explicit RGB and extended color indices retain their color mapping.

Under **View → Terminal colors… → ANSI colors**, adjust the **Bright black** or **Bright blue** swatch if those prompt segments need a different shade for your theme. These changes immediately affect existing local/SSH terminal views without restarting commands. Minimum contrast defaults to **1:1** (off); its optional global text correction is separate from prompt escape handling. Explicit saved contrast overrides remain in effect until changed or reset to theme defaults.

## Closing RODE

RODE protects unsaved source buffers, then closes without asking about terminals:

- **Persist: on** leaves that session running on its host, discoverable through **Sessions → Attach**.
- **Persist: off** ends the session when its last attached view closes. Closing one RODE window preserves a session still attached in another window.
- Temporary shells stop with their owning window or project. They cannot enable persistence; use a native terminal for work that needs to continue.

The same rule applies on project changes, desktop crashes, lost SSH connections and **Disconnect** in the SSH banner. The native service owns this choice; no whole-file editor configuration is involved. **Terminal: New persistent terminal** in the command palette (or the equivalent session-manager button) explicitly creates a session with persistence already on.

The editor and native helper must use the same current session protocol. RODE discovers and controls only that protocol's service; there is no attachment or routing support for earlier services. Updating does not terminate earlier jobs, but the updated editor cannot manage them. Finish or end those jobs with their original client before switching versions. New terminals still default to **Persist: off**.

If startup or a persistence change is still in flight, closing waits briefly for its result instead of asking for a policy. A failed persistence change reports the error; if closing was waiting for it, the window stays open so you can check the session state. Persistent-startup errors remain visible rather than silently substituting a temporary shell.

Ending a terminal interrupts its shell/programs. A managed allocation's `salloc` normally releases resources when its shell ends; use the confirmed [allocation release control](slurm.md) to explicitly request cancellation and verify cleanup. Closing RODE never automatically calls `scancel`.

## Remote helper and packaging

The source installer includes a static headless helper automatically:

```sh
cargo xtask install
rode-sessiond-static protocol
```

The installer builds and verifies the matching musl helper, installs `rode` and `rode-sessiond` through Cargo, and atomically publishes `rode-sessiond-static` beside them, normally in `~/.cargo/bin`. When remote deployment is needed, desktop artifact discovery prefers the static sibling, so older remote glibc versions need no manual setup. Repeat `cargo xtask install` to update the editor and helper together, or add `--root /path/to/prefix` for another installation. This native Linux installer uses Rust and the native build prerequisites; Python, Bash and binutils are not required for installation. Existing jobs keep their original helper inode; no running service is stopped.

Plain `cargo install --locked --path .` and `cargo build --release` still build the ordinary `rode` and `rode-sessiond` pair. That helper targets the build machine's architecture and libc and requires compatible Linux hosts. Local terminals continue to use the editor's matching embedded service.

For hosts with a different/older libc, build a static Linux helper:

```sh
cargo xtask build-helper
# Produces target/release/rode-sessiond-static on the matching architecture.
```

Package `rode-sessiond-static` beside `rode`, or set desktop `RODE_SESSION_HELPER` to the helper file. Static helpers take precedence over the ordinary sibling. The `build-helper` task accepts `x86_64` or `aarch64` as an optional argument; cross compilation requires a suitable linker/toolchain. It builds the opt-in `rode-sessiond-standalone` target in the `rode-session` library package, keeping desktop dependencies out of the musl build, then copies it to the discovery name `rode-sessiond-static`. The helper must match the **remote** architecture and current terminal protocol. A stale explicit or packaged helper produces a rebuild instruction before starting new work. The static build needs no remote Rust installation, GUI libraries, or compatible glibc. Linux x86_64 is the tested combination.

A compatible preinstalled remote `rode-sessiond` is discovered automatically, or selected with the SSH profile’s **Remote helper** / `--remote-helper PATH`. It is copied on the host into the private cache after version/capability checks, so no desktop artifact for that architecture is needed. Explicit remote paths fail on incompatibility; automatic discovery can fall back to desktop deployment.

The [native SSH helper](remote.md#host-service-backends) is verified and published at `~/.rode-host/host-<sha256>` for both ordinary host work and session control. The cache is private to the user. Session control uses no separate TCP listener or application port forwarding; optional X11 uses OpenSSH's display forwarding. Helper images referenced by live jobs are retained; service ownership remains independent of the connection gateway.

## Lifetime and limits

The service owns the PTYs independently of the desktop and SSH master. It runs outside the project sandbox; the actual shell and descendants run **inside the project's original prepared sandbox**. Control/attachment clients run on the host. The Linux service checks UID and mount namespace so sandboxed tools cannot use an exposed control socket to start unrestricted host commands.

The default runtime is `/tmp/rode-session-v<protocol>-<uid>/`, with mode `0700`,
a stable singleton lock, and `service.log`. `rode_session::PROTOCOL` defines the
generation. Linux control uses an authenticated abstract Unix socket bound to
the runtime directory's identity; `control.capability` contains a reference to
the service's sealed memory descriptor, rather than the secret itself. Its mode
`0600` filesystem socket is a discovery marker and accepts no control commands.
Other Unix hosts use that filesystem socket for control. Optional `RODE_SESSION_DIR`
selects a private absolute runtime directory; its parent must exist, and remote
hosts must be able to create/use the same path. Do not remove its lock or control
metadata while sessions run. Windows using the same host, UID, mount namespace,
and runtime discover the same service. Removing the last session stops that
service only when no retained agent, notebook/debugger runtime or SSH tunnel
remains.

The service also exits automatically after five seconds without a retained terminal, agent, notebook/debugger runtime or SSH tunnel, including an unused startup or the last shell exiting naturally. A live shell counts even at an idle prompt or with no attached windows. Explicitly started [SSH return tunnels](remote.md#compute-node-return-tunnels) also keep the service alive, including tunnels awaiting reconnection or credential/host-key attention; stop them explicitly to allow shutdown. Session-list requests and completed tabs do not keep an empty service running. Shutdown removes its socket and releases its singleton lock; creating a terminal starts a fresh service automatically. The stable lock file stays in place for safe concurrent startup.

Control connections authenticate and require the current session protocol. An explicit `RODE_SESSION_DIR` must point to a matching current service or an unused private directory. A protocol mismatch is an error; RODE does not search historical subdirectories or start a compatibility service.

At most 32 clients can be in the authentication/protocol handshake at once.
Excess connections close immediately. A completed handshake releases its slot;
terminal views and other established attachments retain their separate limits.

There are at most 32 live sessions per service, 8 attached views per session, and 16 views per window. Scrollback is bounded to 1,000 rows, with a bounded screen/history replay when reattaching to a running shell. Output is consumed even with no viewers; a slow viewer is disconnected rather than blocking the job. On process exit, the service reaps its child, drains final output and releases the session record and terminal slot automatically, even while other shells keep running. Already displayed output remains in its editor tab; exited sessions cannot be reattached. This is a terminal session service, not a durable log or editor crash-recovery system.

**Persist: on** survives closing/crashing the desktop or losing SSH. It cannot survive reboot, service termination, scheduler time limits, or a host policy that kills user processes at logout. Run the persistent terminal on a stable login/working host **before** allocating compute resources; see [interactive Slurm work](slurm.md).
