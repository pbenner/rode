# Project workspaces and optional sandboxing

Choose **Open project folder** from the File menu, the workspace banner, or Ctrl+Shift+O. When connected to SSH, this browser lists the remote host's directories. Choose **Open local folder** explicitly to switch to the desktop machine. The picker displays which machine it is browsing and supports absolute paths, `~/`, Up, and clicking subfolders. Enter browses a typed path; Ctrl+Enter opens it as the project.

The project is validated before replacing the current workspace. Invalid paths leave the existing project and buffers intact. Unsaved changes require a discard decision before switching. Once opened, the explorer, relative paths, file saves, and terminal working directory all use the chosen project. Remote folder changes create a new project shell channel through the existing authenticated SSH master, without another login.

## Choosing the execution mode

Enable **Sandbox project when available** and choose **bwrap** or **Landlock** in the project picker, SSH connection form, or **View → Project settings / sandbox access**. The mode is optional; normal editing requires neither backend. Bubblewrap remains the default for existing preferences. `--sandbox` enables the saved backend for CLI startup. Folder and project settings are flushed to local preferences when applied/saved, and sandbox policies are remembered per host/project. The folder browser recalls an existing folder's policy; CLI startup restores it, with `--sandbox` forcing the saved policy on. Connection profiles also store their own sandbox options through **Save profile**.

RODE probes the selected backend on the project host, using the complete policy and a harmless `/bin/true` command. This checks more than whether an executable exists: kernels or administrator policies may prohibit user namespaces or Landlock. The probe and project I/O run off the UI thread. RODE never switches backends automatically.

The persistent workspace banner reports the actual result:

- **SANDBOX · bwrap**: the probe succeeded; editor file operations and project commands use that plan.
- **SANDBOX · bwrap · READ ONLY**: the project's base grant is read-only. More specific explicitly granted subfolders can have their own access level.
- **SANDBOX · Landlock**, optionally **· READ ONLY**: Landlock file-access rules are enforced; the limitations below apply.
- **NORMAL MODE · sandbox unavailable**: the requested sandbox could not start. A persistent message explains the missing prerequisite, unsupported policy or setup failure. Editing and project commands use normal host permissions.
- **NORMAL MODE**: sandboxing was not requested.

Sandbox preparation never installs tools or escalates privileges. The default `uvx ruff` command may populate its isolated tool cache on first use; warm that cache before disabling network access. After a sandbox has been selected successfully, a later sandbox operation failure is reported as an error; that operation is not retried outside it.

## Landlock without user namespaces

Landlock requires a Linux x86_64 or aarch64 project host and **Landlock ABI 6 or newer** enabled by the kernel. ABI 6 first shipped with Linux 6.12; RODE probes the ABI rather than assuming support from the version string. Local and SSH execution apply restrictions in a fresh, single-threaded Rust helper process with no Python requirement. The launcher sets `no_new_privs`, closes inherited descriptors and replaces itself with the command. No root access, bubblewrap executable or user namespace is required. Children inherit the policy, including persistent sessions and Slurm compute-node launches; an unsupported compute node fails without an unrestricted retry.

The same system, home, project and extra-path grants below control file contents and directory access. Executable helpers and bundled agents retain their real paths because Landlock cannot remap mounts. Read-only project roots can contain explicitly writable children. **A writable parent with a read-only child, or an allowed parent with a hidden child, is rejected:** Landlock's additive grants cannot subtract that child. Canonical aliases count too, such as hiding `/bin` while granting `/usr` on a merged-usr host. Use bubblewrap for these policies. A failed preparation is visibly normal mode, never a partially enforced active plan.

Grant aliases are resolved when the project is prepared. Each subsequent launch opens the captured paths without following symlinks in any component. Replacing a granted directory or an ancestor with a symlink therefore fails the launch instead of granting its new target. Reopen the project or apply its settings to prepare a changed path layout.

Launcher dependencies must remain outside writable grants. The Rust launcher protects its own executable and mapped libraries. A writable grant covering either is rejected; Python interpreters and their libraries are not part of the sandbox launcher.

Landlock differs materially from bubblewrap:

- It does not create mount, PID, user or network namespaces. Processes remain visible in host process listings, and `/proc` exposes the host process view read-only. **Hidden** denies content/directory access; it does not conceal path existence or metadata. Landlock ABI 6 does not restrict operations such as `chmod`, ownership/timestamp changes and extended attributes. Read-only here protects file contents and directory entries, not all metadata mutations.
- It grants a small set of existing `/dev` devices for stdio, randomness and PTYs. It does not expose all host devices or create a private `/dev`.
- A private, owner-only scratch directory under `~/.cache/rode/landlock/<project-hash>` is granted writable and exported as `TMPDIR`, `TMP` and `TEMP`. The launcher validates and grants the same opened directory, without following replacement symlinks. A read-only rule on scratch or one of its children is rejected rather than overridden. Host `/tmp` is not broadly granted. Tools hard-coding `/tmp` may fail. Scratch survives persisted sessions and reopening the project; remove a project's scratch only after its jobs have ended. It is shared by that project's sessions, not an ephemeral mount.
- With **Allow network access** off, Landlock TCP denial is supplemented by a seccomp filter blocking socket creation/connect/bind/send and io_uring. This includes UDP and pathname Unix sockets; tools needing IPC sockets, including notebook kernels, may therefore fail. With networking on, host networking and pathname Unix sockets remain reachable; filesystem grants do not filter those socket connections. Abstract Unix sockets and signals outside the launch's Landlock domain are restricted in either mode.
- The launcher closes inherited descriptors above stderr. Stdin/stdout/stderr remain the deliberate editor/terminal transport capability. Existing environment variables and explicitly shared writable caches retain their usual risks.

The current native session service uses an abstract Unix control socket on Linux, which the kernel blocks from a separate Landlock domain. Its filesystem discovery socket rejects control requests, and the existing mount-namespace check still protects bubblewrap. This prevents project tools using the current service as an unrestricted process broker, including when ordinary host processes already have `no_new_privs` set. Protocol 10 requires rebuilding/updating the native helper alongside RODE. Older service binaries and other host services do not acquire this protection automatically; with networking enabled, reachable host services are outside Landlock's filesystem boundary. End old jobs and restart an old service explicitly when upgrading; RODE preserves their existing service generations.

See the kernel's [Landlock interface and limitations](https://docs.kernel.org/6.12/userspace-api/landlock.html) and [seccomp filter interface](https://docs.kernel.org/userspace-api/seccomp_filter.html). Neither backend supplies CPU or memory quotas, and these development policies are not a guarantee for arbitrary hostile code.

## Default bubblewrap policy

[Encrypted-project mount management](encrypted-projects.md) is another explicit host-level operation. gocryptfs unlocks before the sandbox is prepared; the sandbox grants the decrypted project root. Locking checks for outstanding sandbox mounts and requires their jobs to stop first.

The GUI and authenticated SSH master remain outside the project sandbox. Project terminal shells, Git, code tools, Python environment creation, and file helpers run inside it. The [native terminal service](terminals.md) and its control/attachment clients stay outside; it launches the actual shells with the prepared sandbox prefix and preserves that sandbox after the GUI detaches. Host monitoring is the explicit read-only exception: it reads real host counters/devices outside bwrap, only while visible. This avoids exposing X11/Wayland sockets to project commands just to display the editor, and lets the folder picker select a different project. Before project selection, folder browsing uses the host connection normally.

The local implementation uses a private Rust file-helper entry point from the editor binary. A temporary snapshot keeps it usable if the executable is rebuilt while the project is open; its lease lasts through pending operations. Native SSH uses the verified headless helper with captured project contexts and starts each file/tool worker under that policy. Both use the same mount policy:

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

Bubblewrap project/extra grants are applied from parent to child so the most specific rule takes effect. For example, a writable project with a read-only `data/` grant protects that dataset; a read-only project with an explicitly writable `results/` grant allows only that subfolder's writes. Landlock supports the latter and rejects the former as described above. Duplicate canonical extra paths, including a duplicate project-root grant, are rejected. Paths refer to the project host, not the desktop when using SSH. A missing extra path is a configuration error; correct it or open the project in normal mode. At most 32 additional paths are accepted, with a 16 KiB total and 4,096 bytes per path.

Saved SSH profile JSON retains `read_only`/`read_write` arrays, plus `project_read_only`, `backend` (`"bwrap"` or `"landlock"`) and a preset `overrides` map. For example, `"overrides": {"~/.cargo": "hidden", "~/.codex": "read_only"}` changes those built-in grants. The access table edits the same configuration. The baseline mount names/defaults live in [crates/rode-host-core/src/sandbox_policy.json](../crates/rode-host-core/src/sandbox_policy.json); ordinary access changes belong in settings, without editing source code.

Git worktrees can need the main repository's `.git` directory outside their project root. The Worktrees panel's **Open + share main .git** makes this grant explicit for standard layouts, without sharing the rest of the main project. Nonstandard metadata locations can be added manually. Git network authentication may need separate explicit identity/agent grants and network access; these are not enabled automatically.

Open an actual project directory, not `/`, the whole home directory, or an ancestor of home. These broad project/extra mounts are rejected because they would expose the home directory the policy is intended to hide. Symlink paths are resolved for planning, and symlinks from the project to hidden locations remain inaccessible inside a working sandbox.

This is a development sandbox with explicit shared resources. Network access, inherited environment variables, writable tool caches, `.codex`, and any extra mounts retain access to the resources they contain. It is not an assurance that arbitrary hostile code cannot access credentials or influence future tool runs. Bubblewrap does not add a seccomp policy; Landlock's network-off filter is described above. Grant only the paths required by your project, and use normal host/remote host job controls for resource limits.

## Checks

`cargo test --test sandbox` verifies local/remote policy agreement, missing/unusable-bwrap fallback, network/path arguments, and real namespace isolation when available. The active isolation check verifies project writes, hidden outside files, and symlink escapes through the actual Rust helper and PTY.

`RODE_TEST_REQUIRE_LANDLOCK=1 cargo test --test landlock` requires real Landlock enforcement: desktop/headless agreement, native and remote file helpers, inherited child restrictions, read-only/truncate protection, nested writable grants, restrictive-grant rejection, TCP/UDP/Unix network denial, inherited descriptor cleanup and failure without an unsandboxed retry. Without that environment variable, unavailable-kernel tests report a skip; preference and fallback checks still run.

`cargo test --test agents` checks whole-agent sandbox scope, read-only writes, streamed protocols, approvals/questions, interruption, bounded output, and child cleanup without inference. SSH roundtrips also exercise agents through the actual multiplexed channels.

`bash scripts/test-ssh.sh` checks direct and ProxyJump sessions, remote project switching, command working directories, and remote sandbox file access. Add `RODE_SSH_UI=1` on an isolated X display to exercise the native project browser in local sandbox mode, without bwrap, and over the temporary SSH server. The test daemon runs without privilege and cannot allocate/login a remote TTY, so the GUI fixture uses a local SSH wrapper that substitutes `-T` for `-tt`. Its authentication, multiplexing, file transfers, project shells, and local PTY remain real. Production code requests a remote TTY and contains no special fixture mode. The local sandbox UI check also replaces a temporary copy of the running editor executable and verifies that file helpers continue to work.
