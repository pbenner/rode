# Remote connections

Click **Connect SSH**. Every connection can configure:

- SSH config alias, hostname, or `user@hostname`
- Username, port, identity file, and remote folder
- One or more comma-separated ProxyJump hosts
- An alternative SSH config file
- An optional preinstalled remote helper path
- A friendly name for a saved connection profile

Blank optional fields defer to your OpenSSH configuration. Passwords, key passphrases, MFA, and host-key confirmation are handled interactively in the terminal. RODE does not store credentials or disable host-key verification. SSH agent forwarding is governed by your own SSH config; it is not enabled by the editor.

An existing alias works directly:

```sh
./target/release/rode --ssh workstation --remote-path '~/projects/simulation'
```

Or configure a direct connection with a jump host:

```sh
./target/release/rode --ssh compute.internal \
  --user researcher --port 22 \
  --identity ~/.ssh/id_ed25519 \
  --jump researcher@login.example.org \
  --remote-path /scratch/researcher/simulation
```

OpenSSH reads `~/.ssh/config`, including `Include` and `ProxyJump` settings. The connection picker discovers literal aliases in the main config file; aliases defined in included files can be entered manually. Use `--ssh-config FILE` to select an alternative configuration.

## Host-service backends

All SSH connections use the native Rust `rode-sessiond` helper. Files, protected saves, Git, tools, telemetry, mise integration, terminal/process/debugger coordination, runtime-cache deployment and sandbox preparation share Rust services with local execution. Python is required only for Python-specific features such as interpreter inspection, debugpy and the Jupyter kernel adapter. Other installed tools retain their own [host requirements](project-tools.md#formatting-issues-and-python-environments).

RODE first looks for a compatible **preinstalled `rode-sessiond` on the remote host**: in its non-interactive `PATH`, then `~/.cargo/bin`, `~/.local/bin`, `/usr/local/bin` and `/usr/bin`. Discovery checks at most five candidates within a bounded deadline. Incompatible or older installations are skipped; if none is compatible, RODE uses the existing verified deployment of a desktop-supplied helper. It never overwrites the installed package or downloads/builds a helper automatically.

To select one installation explicitly, set **Remote helper** in the SSH connection form (saved with that profile), or use:

```sh
rode --ssh workstation --remote-helper '~/.cargo/bin/rode-sessiond'
rode --ssh workstation --remote-helper /opt/rode/bin/rode-sessiond
```

Paths refer to the **remote host**, allow spaces, and must be absolute or start with `~/`. They are passed as literal arguments. An explicit missing, unsafe or incompatible helper fails with an error; it never silently selects another installation or uploads a replacement. With a compatible installed helper, the desktop needs no binary for the remote architecture.

### Compatibility and upgrades

`rode-sessiond --host-info` prints bounded JSON containing its package version, OS/architecture, metadata format, helper API revision, host and session protocol versions, capabilities, executable path, size and SHA-256. The package version identifies a release; compatibility requires matching metadata/API/protocol versions and every required capability. Different package releases can work together when those contracts match. Helpers predating this metadata are not accepted as installed candidates. Update the remote helper, use automatic deployment, or point RODE at another compatible installation.

Before executing a discovered helper, RODE resolves symlinks and checks that the executable and its canonical ancestors are owned by the user or root and not writable by another user (sticky ancestor directories such as `/tmp` are allowed). Setuid/setgid executables are rejected. A compatible helper is **copied on the remote host** into the private immutable cache at `~/.rode-host/host-<sha256>`; no executable bytes are transferred from the desktop. Size and digest are checked before publication, and the live gateway handshake repeats the identity and compatibility checks before any project operation. Existing cached generations are verified and reused. Upgrading the installed package therefore does not replace the executable used by a live connection or persistent job. Reconnect to discover the new installation; keep cached generations while jobs reference them.

### Supported hosts and deployment fallback

Remote hosts still need **Linux x86_64 or aarch64**, a POSIX shell, `uname`, and GNU-compatible `id`, `stat`, `sha256sum`, `wc`, `head`, `mktemp`, `ln`, `chmod`, `cut`, `mkdir`, `rm` and `rmdir`. Installed-helper discovery also uses `readlink -f`. A preinstalled helper does not add support for macOS, BSD or Windows. x86_64 is locally validated; aarch64 selection is supported but does not establish execution coverage there.

The recommended desktop source installation, `cargo xtask install`, builds and installs a static Linux helper beside the editor; Debian packages supply one too. When deployment is needed, this avoids older-glibc startup failures. Plain `cargo install` builds a helper linked against the build host's libc. Build a matching [static helper](terminals.md#remote-helper-and-packaging) for a different architecture or libc; cross-compilation requires a suitable toolchain.

Desktop `RODE_HOST_HELPER` selects a file or a directory containing `<architecture>/rode-sessiond`; it takes precedence over `RODE_SESSION_HELPER`. These explicit desktop overrides bypass automatic remote discovery. A profile/CLI **Remote helper** path takes precedence over both. Without overrides, deployment checks `host-helpers/<architecture>/rode-sessiond` beside the editor, matching musl build output and same-architecture sibling helpers.

Both installed and uploaded helpers use the same verified private cache. Its home/cache must be writable by the owner and executable; no root access, remote compiler, GUI libraries or application port forwarding is needed. A `noexec` home, unsafe paths, missing utilities or incompatible runtime libraries produce explicit errors. Use a static helper for libc differences. A damaged cache entry is preserved and rejected, never silently replaced.

One authenticated stdio channel owns the native gateway and up to four concurrent project operations. Project contexts capture the actual sandbox and cannot be weakened by an ordinary request. Terminal and process streams use separate channels on the same OpenSSH master. Disconnect cancels transient work; [persistent sessions](terminals.md) retain their separate owner. Native failure never retries through Python or outside an active sandbox. After a lost write acknowledgement, the write may have completed: reconnect and reconcile the remote revision before trying another save. A damaged helper cache fails verification without overwriting the suspect file.


## Connection and workspace lifetime

Saved profiles live in `${XDG_CONFIG_HOME:-~/.config}/rode/connections.json`. **Save profile** persists connection settings, not passwords. Clicking a saved connection in the explorer connects immediately using that profile; unsaved edits still require a decision before switching. Use **Connect SSH** to open the form for new connections or editing profiles. **Remote: Reconnect SSH** in the command palette restores the last connection. A dedicated OpenSSH master owns the authenticated connection; project terminals use separate channels. Hiding or detaching a project terminal leaves the connection available. **New** creates an independent shell with persistence off; **Sessions** discovers/reattaches existing ones. Enable **Persist: on** in the terminal header to leave work running when RODE closes without an exit prompt; tmux/screen is not required. See [terminal lifetime and remote helper setup](terminals.md).

Use **Disconnect** next to the SSH host in the top-left connection banner, or **Remote: Disconnect SSH** in the command palette. Disconnecting, losing the connection, or a failed login clears the selected host and project. The title and explorer return to the local, no-folder state; **Open project folder…** and **Open file…** work locally immediately. Open remote tabs and unsaved edits stay in memory as disconnected buffers. Use **Save As** for a local copy, or **Remote: Reconnect SSH** to reconnect to their original project and restore their file bindings. Their original paths are never used for local file access. Opening a local project follows the usual unsaved-change protection. Disconnect stops project kernels/builds and detaches agent/terminal views: **Persist: on** sessions continue, while **Persist: off** sessions end after their last view closes. Reattach agents through **Agent chat → Running** under their original project policy. Disconnect is briefly unavailable while terminal/agent startup or a persistence change is pending.

If login fails or the connection ends unexpectedly, the terminal shows **SSH ERROR / host** and retains the login's output, including OpenSSH errors such as `Permission denied` or `Connection refused`. It is read-only: the SSH process is stopped, and this is not a local shell. Scroll to earlier output, use **Copy**, and choose **New** when you want a local shell. Folder/file opening already uses the local computer; **Remote: Reconnect SSH** starts a fresh attempt. The bounded terminal output stays only in this window's memory and is cleared when replaced by a new terminal/project or the window closes. RODE does not save the login output to preferences or a log file.

## Project folders and sandbox mode

[Encrypted projects](encrypted-projects.md) can be unlocked on the connected host through **File → Open encrypted project…**. gocryptfs and FUSE run there; the desktop uses the existing SSH channels. Lock the encrypted project before explicitly disconnecting.

The SSH host and connection state are displayed in a persistent banner, the status bar, and the window title. **File → Open project folder…** / **Ctrl+Shift+O** browses the currently selected host: remote folders when connected over SSH, local folders otherwise. Navigate with the path field, **Browse**, **Up**, and the folder list, then choose **Open project** (or **Ctrl+Enter**). The explorer, relative file paths, saves, and project terminal then use that folder. Switching remote projects reuses the authenticated connection and protects unsaved buffers. **File → Open local folder…** explicitly switches back to this machine.

Enable sandboxing and choose **bwrap** or **Landlock** in the folder picker or SSH profile. You can allow/block network access and grant additional read-only or read-write paths. The selected backend is probed on the project host. If it is unavailable, preparation returns a persistent **NORMAL MODE · sandbox unavailable** indicator and the reason; it is not presented as isolated. Once active, an operation failure never triggers an unrestricted retry.

```sh
./target/release/rode /path/to/project --sandbox
./target/release/rode --ssh workstation --remote-path '~/projects/simulation' --sandbox
```

When available, bubblewrap isolates both editor file operations and project terminals. The desktop UI and SSH transport stay outside the sandbox. System/tool files are read-only, the project and selected tool caches are writable, and the rest of the home directory is hidden. Sandbox preparation does not install tools; see the [Ruff runner](project-tools.md#formatting-issues-and-python-environments) for its cache behavior. See [sandbox behavior, shared paths, and limitations](sandbox.md).

**View → Project settings / sandbox access** opens the access editor. Set each built-in path to **Hidden**, **Read only**, or **Read + write**; add paths with their own access level; control networking and the project's base read-only mode. **Apply sandbox settings** probes the new policy, preserves editor buffers, and opens a shell under the new policy. Existing persistent shells detach and retain their original policy; they are not silently migrated or stopped. Settings are saved locally per host/project. The folder browser recalls a saved folder's policy, and CLI startup restores it; `--sandbox` forces sandboxing on with that project's saved rules.
