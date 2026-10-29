# Project workspaces and optional sandboxing

Choose **Open project folder** from the File menu, the workspace banner, or Ctrl+Shift+O. When connected to SSH, this browser lists the remote host's directories. Choose **Open local folder** explicitly to switch to the desktop machine. The picker displays which machine it is browsing and supports absolute paths, `~/`, Up, and clicking subfolders. Enter browses a typed path; Ctrl+Enter opens it as the project.

The project is validated before replacing the current workspace. Invalid paths leave the existing project and buffers intact. Unsaved changes require a discard decision before switching. Once opened, the explorer, relative paths, file saves, and terminal working directory all use the chosen project. Remote folder changes create a new project shell channel through the existing authenticated SSH master, without another login.

## Choosing the execution mode

Enable **Sandbox project when bwrap is available** in the project picker, SSH connection form, or **View → Project settings / sandbox access**. The mode is optional; normal editing does not require bubblewrap. `--sandbox` requests it for CLI startup. Folder and project settings are flushed to local preferences when applied/saved, and sandbox policies are remembered per host/project. The folder browser recalls an existing folder's policy; CLI startup restores it, with `--sandbox` forcing the saved policy on. Connection profiles also store their own sandbox options through **Save profile**.

RODE probes bubblewrap on the project host, using the complete mount plan and a harmless `/bin/true` command. This checks more than whether the executable exists: remote host kernels or administrator policies may prohibit user namespaces. The probe and project I/O run off the UI thread.

The persistent workspace banner reports the actual result:

- **SANDBOX · bwrap**: the probe succeeded; editor file operations and project commands use that plan.
- **SANDBOX · bwrap · READ ONLY**: the project's base grant is read-only. More specific explicitly granted subfolders can have their own access level.
- **NORMAL MODE · sandbox unavailable**: the requested sandbox could not start. A persistent message explains whether bwrap is absent or why setup failed. Editing and project commands use normal host permissions.
- **NORMAL MODE**: sandboxing was not requested.

Sandbox preparation never installs bubblewrap or escalates privileges. The default `uvx ruff` command may populate its isolated tool cache on first use; warm that cache before disabling network access. After a sandbox has been selected successfully, a later sandbox operation failure is reported as an error; that operation is not retried outside it.

## Default policy

[Encrypted-project mount management](encrypted-projects.md) is another explicit host-level operation. gocryptfs unlocks before the sandbox is prepared; the sandbox grants the decrypted project root. Locking checks for outstanding sandbox mounts and requires their jobs to stop first.

The GUI and authenticated SSH master remain outside the project sandbox. Project terminal shells, Git, code tools, Python environment creation, and file helpers run inside it. The [native terminal service](terminals.md) and its control/attachment clients stay outside; it launches the actual shells with the prepared sandbox prefix and preserves that sandbox after the GUI detaches. Host monitoring is the explicit read-only exception: it reads real host counters/devices outside bwrap, only while visible. This avoids exposing X11/Wayland sockets to project commands just to display the editor, and lets the folder picker select a different project. Before project selection, folder browsing uses the host connection normally.

The local implementation uses a small private Rust file-helper entry point from the editor binary. A private temporary snapshot keeps that helper usable even if RODE's executable is rebuilt/replaced while the project is open; it is released when the workspace and its pending operations finish. The remote implementation runs the existing Python standard-library helper through bwrap. No helper files or editor server are installed remotely. Both implementations use the same mount policy:

| Access | Paths |
| --- | --- |
| Read-only system paths, if present | `/usr`, `/bin`, `/sbin`, `/lib`, `/lib64`, `/etc`, `/opt`, `/run/munge` |
| Read-only home paths, if present | `~/.local`, `~/.rustup`, `~/.bashrc`, `~/.profile`, `~/.gitconfig`, `~/.config/git` |
| Writable shared tool paths, if present | `~/.cargo`, `~/.cache/uv`, `~/.codex` |
| Writable project | The canonical selected project directory |
| Private temporary locations | Fresh `/tmp` and `/home`; the user's home path and an empty `.config` directory are created inside the sandbox |
| Process/device views | A new `/proc` and minimal `/dev` |

The namespace/mount flags follow [bubblewrap's documented interface](https://github.com/containers/bubblewrap). RODE uses `--unshare-all`, `--die-with-parent`, and the selected project's directory as the working directory. Network access is shared by default, matching the supplied example; turn off **Allow network access** to retain a private network namespace. `HOME` points to the isolated home path and `UV_LINK_MODE=copy` avoids cross-mount hard-link assumptions.

The sandbox deliberately does not mount VS Code directories, the entire `/run`, or desktop display sockets. RODE's UI/preferences live outside the sandbox. `/run/munge` is included for remote hosts that use Munge authentication. Other remote host-specific software or data locations can be added explicitly.

## Extra paths and scope

Agent chat uses this same plan for the entire agent process and its tool children. Installed executables, runtimes, configuration/authentication, and caches must be reachable through these grants; for example, `~/.codex` is shared by default. Other agents may need additional explicit paths. Approving a command or file change in chat does not widen the outer sandbox. Local model servers such as Ollama run separately; their endpoints need network access, and localhost refers to the project host when using SSH. See [Agent chat](agents.md) for setup and supported protocols. Switching projects or applying a new sandbox stops the old chat process independently of SSH authentication.

Use **Sandbox access rules** in the folder/SSH form, or the full **Project settings** panel. Each built-in shared path has **Hidden**, **Read only**, and **Read + write** choices. **+ Add folder or file** adds an absolute or `~/` path with its access level. Remove an extra grant with ×. **Project is read only** controls the project's base grant. **Apply sandbox settings** probes the edited rules and replaces the project environment while preserving buffers; a new shell uses the edited policy. Existing persistent sessions detach and keep their original sandbox. Restore the original project/policy to reattach, or explicitly end them in the session manager.

Project/extra grants are applied from parent to child so the most specific rule takes effect. For example, a writable project with a read-only `data/` grant protects that dataset; a read-only project with an explicitly writable `results/` grant allows only that subfolder's writes. Duplicate canonical extra paths, including a duplicate project-root grant, are rejected. Paths refer to the project host, not the desktop when using SSH. A missing extra path is a configuration error; correct it or open the project in normal mode. At most 32 additional paths are accepted, with a 16 KiB total and 4,096 bytes per path.

Saved SSH profile JSON retains `read_only`/`read_write` arrays, plus `project_read_only` and a preset `overrides` map. For example, `"overrides": {"~/.cargo": "hidden", "~/.codex": "read_only"}` changes those built-in grants. The access table edits the same configuration. The baseline mount names/defaults live in [src/sandbox_policy.json](../src/sandbox_policy.json); ordinary access changes belong in settings, without editing source code.

Git worktrees can need the main repository's `.git` directory outside their project root. The Worktrees panel's **Open + share main .git** makes this grant explicit for standard layouts, without sharing the rest of the main project. Nonstandard metadata locations can be added manually. Git network authentication may need separate explicit identity/agent grants and network access; these are not enabled automatically.

Open an actual project directory, not `/`, the whole home directory, or an ancestor of home. These broad project/extra mounts are rejected because they would expose the home directory the policy is intended to hide. Symlink paths are resolved for planning, and symlinks from the project to hidden locations remain inaccessible inside a working sandbox.

This is a development sandbox with explicit shared resources. Network access, inherited environment variables, writable tool caches, `.codex`, and any extra mounts retain access to the resources they contain. It is not an assurance that arbitrary hostile code cannot access credentials or influence future tool runs. It also does not impose CPU/memory quotas or a seccomp syscall policy. Grant only the paths required by your project, and use normal host/remote host job controls for resource limits.

## Checks

`cargo test --test sandbox` verifies local/remote policy agreement, missing/unusable-bwrap fallback, network/path arguments, and real namespace isolation when available. The active isolation check verifies project writes, hidden outside files, and symlink escapes through the actual Rust helper and PTY.

`cargo test --test agents` checks whole-agent sandbox scope, read-only writes, streamed protocols, approvals/questions, interruption, bounded output, and child cleanup without inference. SSH roundtrips also exercise agents through the actual multiplexed channels.

`bash scripts/test-ssh.sh` checks direct and ProxyJump sessions, remote project switching, command working directories, and remote sandbox file access. Add `RODE_SSH_UI=1` on an isolated X display to exercise the native project browser in local sandbox mode, without bwrap, and over the temporary SSH server. The test daemon runs without privilege and cannot allocate/login a remote TTY, so the GUI fixture uses a local SSH wrapper that substitutes `-T` for `-tt`. Its authentication, multiplexing, file transfers, project shells, and local PTY remain real. Production code requests a remote TTY and contains no special fixture mode. The local sandbox UI check also replaces a temporary copy of the running editor executable and verifies that file helpers continue to work.
