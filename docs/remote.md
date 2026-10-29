# Remote connections

Click **Connect SSH**. Every connection can configure:

- SSH config alias, hostname, or `user@hostname`
- Username, port, identity file, and remote folder
- One or more comma-separated ProxyJump hosts
- An alternative SSH config file
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

Remote files require **Python 3** on the target host and a POSIX-compatible login shell. Each file operation sends a small standard-library Python program through an existing SSH channel, with paths and contents as JSON on stdin. File access needs no uploaded helper files, Python packages, persistent editor daemon, application port forwarding, or project indexing process. [Persistent terminals](terminals.md) separately deploy a small native helper and use a per-user PTY service on Linux; file editing and temporary terminals remain available without it. Optional code tools have their own [host requirements](project-tools.md#formatting-issues-and-python-environments).

Saved profiles live in `${XDG_CONFIG_HOME:-~/.config}/rode/connections.json`. **Save profile** persists connection settings, not passwords. **Remote: Reconnect SSH** in the command palette restores the last connection. A dedicated OpenSSH master owns the authenticated connection; project terminals use separate channels. Hiding or detaching a project terminal leaves the connection available. **New** creates an independent shell with persistence off; **Sessions** discovers/reattaches existing ones. Enable **Persist: on** in the terminal header to leave work running when RODE closes without an exit prompt; tmux/screen is not required. See [terminal lifetime and remote helper setup](terminals.md).

Use **Disconnect** next to the SSH host in the top-left connection banner, or **Remote: Disconnect SSH** in the command palette. Disconnecting, losing the connection, or a failed login clears the selected host and project. The title and explorer return to the local, no-folder state; **Open project folder…** and **Open file…** work locally immediately. Open remote tabs and unsaved edits stay in memory as disconnected buffers. Use **Save As** for a local copy, or **Remote: Reconnect SSH** to reconnect to their original project and restore their file bindings. Their original paths are never used for local file access. Opening a local project follows the usual unsaved-change protection. Disconnect stops project agents, kernels and builds, and detaches terminal views: **Persist: on** sessions continue, while **Persist: off** sessions end after their last view closes. Disconnect is briefly unavailable while a terminal starts or its persistence setting is being saved.

## Project folders and sandbox mode

[Encrypted projects](encrypted-projects.md) can be unlocked on the connected host through **File → Open encrypted project…**. gocryptfs and FUSE run there; the desktop uses the existing SSH channels. Lock the encrypted project before explicitly disconnecting.

The SSH host and connection state are displayed in a persistent banner, the status bar, and the window title. **File → Open project folder…** / **Ctrl+Shift+O** browses the currently selected host: remote folders when connected over SSH, local folders otherwise. Navigate with the path field, **Browse**, **Up**, and the folder list, then choose **Open project** (or **Ctrl+Enter**). The explorer, relative file paths, saves, and project terminal then use that folder. Switching remote projects reuses the authenticated connection and protects unsaved buffers. **File → Open local folder…** explicitly switches back to this machine.

Enable **Sandbox project when bwrap is available** in the folder picker, or enable the equivalent option in an SSH connection profile. You can allow/block network access and grant additional read-only or read-write paths. A requested sandbox is probed on the machine hosting the project. If bubblewrap is missing or unusable, the project still opens with a persistent **NORMAL MODE · sandbox unavailable** indicator and the reason; it is not presented as isolated.

```sh
./target/release/rode /path/to/project --sandbox
./target/release/rode --ssh workstation --remote-path '~/projects/simulation' --sandbox
```

When available, bubblewrap isolates both editor file operations and project terminals. The desktop UI and SSH transport stay outside the sandbox. System/tool files are read-only, the project and selected tool caches are writable, and the rest of the home directory is hidden. Sandbox preparation does not install tools; see the [Ruff runner](project-tools.md#formatting-issues-and-python-environments) for its cache behavior. See [sandbox behavior, shared paths, and limitations](sandbox.md).

**View → Project settings / sandbox access** opens the access editor. Set each built-in path to **Hidden**, **Read only**, or **Read + write**; add paths with their own access level; control networking and the project's base read-only mode. **Apply sandbox settings** probes the new policy, preserves editor buffers, and opens a shell under the new policy. Existing persistent shells detach and retain their original policy; they are not silently migrated or stopped. Settings are saved locally per host/project. The folder browser recalls a saved folder's policy, and CLI startup restores it; `--sandbox` forces sandboxing on with that project's saved rules.
