# Project workspaces and optional sandboxing

Choose **Open project folder** from the File menu, the workspace banner, or Ctrl+Shift+O. When connected to SSH, this browser lists the remote host's directories. Choose **Open local folder** explicitly to switch to the desktop machine. The picker displays which machine it is browsing and supports absolute paths, `~/`, Up, and clicking subfolders. Enter browses a typed path; Ctrl+Enter opens it as the project.

The project is validated before replacing the current workspace. Invalid paths leave the existing project and buffers intact. Unsaved changes require a discard decision before switching. Once opened, the explorer, relative paths, file saves, and terminal working directory all use the chosen project. Remote folder changes create a new project shell channel through the existing authenticated SSH master, without another login.

## Choosing the execution mode

Enable **Sandbox project when available** and choose **bwrap** or **Landlock** in the project picker, SSH connection form, or **View → Project settings / sandbox access**. The mode is optional; normal editing requires neither backend. Bubblewrap remains the default for existing preferences. `--sandbox` enables the saved backend for CLI startup. Folder and project settings are flushed to local preferences when applied/saved, and sandbox policies are remembered per host/project. The folder browser recalls an existing folder's policy; CLI startup restores it, with `--sandbox` forcing the saved policy on. Connection profiles also store their own sandbox options through **Save profile**.

RODE probes the selected backend on the project host, using the complete policy and a harmless `/bin/true` command. This checks more than whether an executable exists: kernels or administrator policies may prohibit user namespaces or Landlock. The probe and project I/O run off the UI thread. RODE never switches backends automatically.

For **bwrap**, RODE prefers the executable on the project host's `PATH`. If none is available, it uses pinned bubblewrap bundled in the native RODE helper, locally or over SSH. No runtime download, package installation or Codex installation is needed. The same policy and availability probe apply. A failed system `bwrap` is not retried with the bundled copy, and the bundled copy cannot bypass restrictions on user namespaces or AppArmor. When it is selected, the status message identifies bundled bubblewrap. Update the native helper together with the editor; older installed helpers without the `bundled_bwrap` capability are not used for new connections.

The persistent workspace banner reports the actual result:

- **SANDBOX · bwrap**: the probe succeeded; editor file operations and project commands use that plan.
- **SANDBOX · bwrap · READ ONLY**: the project's base grant is read-only. More specific explicitly granted subfolders can have their own access level.
- **SANDBOX · Landlock**, optionally **· READ ONLY**: Landlock file-access rules are enforced; the limitations below apply.
- **NORMAL MODE · sandbox unavailable**: the requested sandbox could not start. A persistent message explains the missing prerequisite, unsupported policy or setup failure. Editing and project commands use normal host permissions.
- **NORMAL MODE**: sandboxing was not requested.

Sandbox preparation never installs tools or escalates privileges. The default `uvx ruff` command may populate its isolated tool cache on first use; warm that cache before disabling network access. After a sandbox has been selected successfully, a later sandbox operation failure is reported as an error; that operation is not retried outside it.

## Landlock without user namespaces

Landlock requires a Linux x86_64 or aarch64 project host and **Landlock ABI 4 or newer** enabled by the kernel. RODE probes the ABI rather than assuming support from the version string and requests only features that kernel supports:

| Host ABI | Enforced features |
| --- | --- |
| 4 (Linux 6.7+) | Filesystem access, cross-directory rename/link and truncation rules; TCP policy and the network-off seccomp filter |
| 5 (Linux 6.10+) | ABI 4 features plus device-ioctl restrictions |
| 6 and newer (Linux 6.12+) | ABI 5 features plus signal and abstract Unix socket scoping |

The workspace mode's tooltip reports the detected ABI and any missing isolation features. ABIs 4/5 do not scope signals or abstract Unix sockets; ABI 4 also cannot restrict device ioctls. These kernel limitations do not disable the filesystem or network-off policy. ABIs below 4 remain unavailable for project sandboxing. The separate command-free native agent file tools retain their ABI 1 minimum.

Local and SSH execution apply restrictions in a fresh, single-threaded Rust helper process with no Python requirement. The launcher sets `no_new_privs`, closes inherited descriptors and replaces itself with the command. No root access, bubblewrap executable or user namespace is required. Children inherit the policy, including persistent sessions and Slurm compute-node launches; an unsupported compute node fails without an unrestricted retry.

The same system, home, project and extra-path grants below control file contents and directory access. Executable helpers and bundled agents retain their real paths because Landlock cannot remap mounts. Read-only project roots can contain explicitly writable children. **A writable parent with a read-only child, or an allowed parent with a hidden child, is rejected:** Landlock's additive grants cannot subtract that child. Canonical aliases count too, such as hiding `/bin` while granting `/usr` on a merged-usr host. Use bubblewrap for these policies. A failed preparation is visibly normal mode, never a partially enforced active plan.

Grant aliases are resolved when the project is prepared. Each subsequent launch opens the captured paths without following symlinks in any component. Replacing a granted directory or an ancestor with a symlink therefore fails the launch instead of granting its new target. Reopen the project or apply its settings to prepare a changed path layout.

Launcher dependencies must remain outside writable grants. The Rust launcher protects its own executable and mapped libraries. A writable grant covering either is rejected; Python interpreters and their libraries are not part of the sandbox launcher.

Landlock differs materially from bubblewrap:

- It does not create mount, PID, user or network namespaces. Processes remain visible in host process listings, and `/proc` exposes the host process view read-only. **Hidden** denies content/directory access; it does not conceal path existence or metadata. The supported policy does not restrict operations such as `chmod`, ownership/timestamp changes and extended attributes. Read-only here protects file contents and directory entries, not all metadata mutations.
- It grants a small set of existing `/dev` devices for stdio, randomness and PTYs. It does not expose all host devices or create a private `/dev`.
- A private, owner-only scratch directory under `~/.cache/rode/landlock/<project-hash>` is granted writable and exported as `TMPDIR`, `TMP` and `TEMP`. The launcher validates and grants the same opened directory, without following replacement symlinks. A read-only rule on scratch or one of its children is rejected rather than overridden. Host `/tmp` is not broadly granted. Tools hard-coding `/tmp` may fail. Scratch survives persisted sessions and reopening the project; remove a project's scratch only after its jobs have ended. It is shared by that project's sessions, not an ephemeral mount.
- With **Allow network access** off, Landlock TCP denial is supplemented by a seccomp filter blocking socket creation/connect/bind/send and io_uring on every supported ABI. This includes UDP and pathname Unix sockets; tools needing IPC sockets, including notebook kernels, may therefore fail. With networking on, host networking and pathname Unix sockets remain reachable; filesystem grants do not filter those socket connections. On ABI 6+, abstract Unix sockets and signals outside the launch's Landlock domain are restricted in either mode. On ABI 4/5, abstract sockets remain reachable when networking is on, and signals follow ordinary host permissions.
- The launcher closes inherited descriptors above stderr. Stdin/stdout/stderr remain the deliberate editor/terminal transport capability. Existing environment variables and explicitly shared writable caches retain their usual risks.

The current native session service authenticates its abstract Unix control socket on Linux before accepting requests or attachments. A random capability stays in a sealed, non-executable, anonymous memory file held by the service; the runtime directory contains only its `/proc/<service-pid>/fd/<descriptor>` reference. Landlock's ptrace boundary prevents project tools reading that descriptor on all supported ABIs, even when the runtime directory itself is explicitly granted. ABI 6 additionally blocks the abstract connection. The filesystem discovery socket rejects control requests, and the existing same-UID/mount-namespace checks still protect bubblewrap. This prevents project tools using the current service as an unrestricted process broker, including when ordinary host processes already have `no_new_privs` set. The current session protocol is 18 and requires rebuilding/updating the native helper alongside RODE. Every control connection checks that protocol after authentication. Earlier service generations and unauthenticated endpoints are not supported. Updating does not add protection to earlier binaries or other host services; with networking enabled, reachable host services remain outside Landlock's filesystem boundary.

See the kernel's [Landlock interface and limitations](https://docs.kernel.org/6.12/userspace-api/landlock.html) and [seccomp filter interface](https://docs.kernel.org/userspace-api/seccomp_filter.html). Neither backend supplies CPU or memory quotas, and these development policies are not a guarantee for arbitrary hostile code.

## Default bubblewrap policy

[Encrypted-project mount management](encrypted-projects.md) is another explicit host-level operation. gocryptfs unlocks before the sandbox is prepared; the sandbox grants the decrypted project root. Locking checks for outstanding sandbox mounts and requires their jobs to stop first.

The GUI and authenticated SSH master remain outside the project sandbox. Agents, code tools, Python environment creation and file helpers use the project policy. User-operated Git defaults to normal host permissions; **Git sandbox: off / on** in Source control selects normal execution or the project sandbox. Interactive terminals initially follow the project policy. Their **Sandbox** menu opens a new sandboxed or unsandboxed shell on the same host, retaining existing shells and their jobs. Both choices are remembered per host/project. Run, Build and project tasks still use the project policy and open another terminal if the selected shell has a different policy. These controls do not change agent permissions or the workspace policy. The [native terminal service](terminals.md) and its control/attachment clients stay outside; it launches each shell with its selected policy and preserves that policy after the GUI detaches. Host monitoring is the explicit read-only exception: it reads real host counters/devices outside bwrap, only while visible. This avoids exposing X11/Wayland sockets to project commands just to display the editor, and lets the folder picker select a different project. Before project selection, folder browsing uses the host connection normally.

The local implementation uses a private Rust file-helper entry point from the editor binary. A temporary snapshot keeps it usable if the executable is rebuilt while the project is open; its lease lasts through pending operations. Native SSH uses the verified headless helper with captured project contexts and starts each file/tool worker under that policy. Both use the same mount policy:

| Access | Paths |
| --- | --- |
| Read-only system paths, if present | `/usr`, `/bin`, `/sbin`, `/lib`, `/lib64`, `/etc`, `/etc/resolv.conf`, `/opt`, `/run/munge`, `/var/lib/command-not-found` |
| Read-only home paths, if present | `~/.local`, `~/.rustup`, `~/.bashrc`, `~/.bash_profile`, `~/.bash_login`, `~/.bash_aliases`, `~/.profile`, `~/.gitconfig`, `~/.config/git` |
| Writable shared tool paths, if present | `~/.cargo`, `~/.cache/uv`, `~/.codex` |
| Writable project | The canonical selected project directory |
| Private temporary locations | Fresh `/tmp` and `/home`; the user's home path and an empty `.config` directory are created inside the sandbox |
| Process/device views | A new `/proc` and minimal `/dev` |

The namespace/mount flags follow [bubblewrap's documented interface](https://github.com/containers/bubblewrap). RODE uses `--unshare-all`, `--die-with-parent`, and the selected project's directory as the working directory. Network access is shared by default, matching the supplied example; turn off **Allow network access** to retain a private network namespace. `HOME` points to the isolated home path and `UV_LINK_MODE=copy` avoids cross-mount hard-link assumptions.

Both backends share the canonical resolver file separately. On Ubuntu, `/etc/resolv.conf` commonly points into `/run/systemd/resolve`; sharing its target file keeps DNS available without sharing the rest of `/run`. Networked agents still require **Allow network access**. Ubuntu's command lookup database is shared read-only so `command-not-found` can report missing commands normally.

Native project terminals start login shells. The standard Bash profiles and alias file are shared read-only so a login profile can load `~/.bashrc` and its aliases. Custom files sourced by these profiles require their own explicit extra-path grants. After updating the editor and native helper, reapply sandbox settings and open a fresh terminal/chat to use the updated grants; existing persistent jobs keep their captured policy.

Helper discovery requires the `sandbox_system_lookups` capability so an older preinstalled helper cannot silently retain the broken defaults. Automatic discovery can deploy the updated helper; an explicit remote helper override must point to a compatible build.

The sandbox deliberately does not mount VS Code directories, the entire `/run`, or desktop display sockets. RODE's UI/preferences live outside the sandbox. `/run/munge` is included for remote hosts that use Munge authentication. Other remote host-specific software or data locations can be added explicitly.

## Extra paths and scope

**Model connection only** in Codex's Access menu is an explicit exception for the trusted model controller. It requires an active offline project sandbox, keeps its filesystem grants, disables built-in/external tools, and routes native commands through the original offline plan. Terminals and other project tools keep their existing policy. See [Agent networking](agents.md#model-connection-with-offline-tools) for setup, command limits and provider support. The following paragraph describes the default **Use project network policy** mode.

Agent chat uses this same plan for the entire agent process and its tool children. Installed executables, runtimes, configuration/authentication, and caches must be reachable through these grants; for example, `~/.codex` is shared by default. Other agents may need additional explicit paths. Approving a command or file change in chat does not widen the outer sandbox. Local model servers such as Ollama run separately; their endpoints need network access, and localhost refers to the project host when using SSH. See [Agent chat](agents.md) for setup and supported protocols. Switching projects or applying a new sandbox stops the old chat process independently of SSH authentication.

Use **Sandbox access rules** in the folder/SSH form, or the full **Project settings** panel. Each built-in shared path has **Hidden**, **Read only**, and **Read + write** choices. **+ Add folder or file** adds an absolute or `~/` path with its access level. Remove an extra grant with ×. **Project is read only** controls the project's base grant. **Apply sandbox settings** probes the edited rules and replaces the project environment while preserving buffers; new sandboxed shells use the edited policy; explicitly unsandboxed shells continue to use normal host permissions. Existing persistent sessions detach and keep their original sandbox. Restore the original project/policy to reattach sandboxed sessions; unsandboxed sessions require the original project on the same host. You can explicitly end either in the session manager.

Bubblewrap project/extra grants are applied from parent to child so the most specific rule takes effect. For example, a writable project with a read-only `data/` grant protects that dataset; a read-only project with an explicitly writable `results/` grant allows only that subfolder's writes. Landlock supports the latter and rejects the former as described above. Duplicate canonical extra paths, including a duplicate project-root grant, are rejected. Paths refer to the project host, not the desktop when using SSH. A missing extra path is a configuration error; correct it or open the project in normal mode. At most 32 additional paths are accepted, with a 16 KiB total and 4,096 bytes per path.

Saved SSH profile JSON retains `read_only`/`read_write` arrays, plus `project_read_only`, `backend` (`"bwrap"` or `"landlock"`) and a preset `overrides` map. For example, `"overrides": {"~/.cargo": "hidden", "~/.codex": "read_only"}` changes those built-in grants. The access table edits the same configuration. The baseline mount names/defaults live in [crates/rode-host-core/src/sandbox_policy.json](../crates/rode-host-core/src/sandbox_policy.json); ordinary access changes belong in settings, without editing source code.

### Adapting access to your host

Custom shell configuration, site toolchains and caches belong in user-defined grants. RODE does not assume personal filenames or evaluate shell scripts to discover dependencies. A standard login profile can source other files; add those files, or their configuration directory, under **Additional shared paths**. Granting a file makes it accessible; your shell's normal startup configuration still decides whether to source it.

For example, a host might need these additional rules (choose paths that actually exist on your host):

| Path | Access | Purpose |
| --- | --- | --- |
| `~/.config/dev-shell` | Read only | Custom shell fragments and their configuration |
| `/srv/site-tools` | Read only | A site-managed tool installation |
| `~/.cache/dev-tools` | Read + write | State a tool needs to update |

Use **Reusable sandbox presets** in project settings, the folder picker or the SSH form to save this setup under a descriptive name. **Save current rules** records the complete draft, including the enabled mode, backend, network setting, project access, built-in overrides and extra paths. **Replace saved preset** explicitly updates an existing name. In another project or SSH form, select the preset and click **Load into form**, review its paths, then open/connect or **Apply sandbox settings**. Loading replaces the draft rather than merging grants. Save the SSH connection profile as well if you want it to retain the loaded rules.

Presets are stored in the desktop's shared preferences and can be reused across local and remote projects. Paths remain literal: `~/` expands to the project host's home, and absolute paths refer to that host. A preset is a template, not a live policy link: saving, replacing or deleting one does not change an existing project's access or running processes. No preset is loaded automatically. Up to 64 named presets are supported; the usual extra-path limits still apply. Nothing is added to the project's repository or to shell startup files.

Git worktrees can need the main repository's `.git` directory outside their project root. The Worktrees panel's **Open + share main .git** makes this grant explicit for standard layouts, without sharing the rest of the main project. Nonstandard metadata locations can be added manually. Git network authentication may need separate explicit identity/agent grants and network access; these are not enabled automatically.

Open an actual project directory, not `/`, the whole home directory, or an ancestor of home. These broad project/extra mounts are rejected because they would expose the home directory the policy is intended to hide. Symlink paths are resolved for planning, and symlinks from the project to hidden locations remain inaccessible inside a working sandbox.

This is a development sandbox with explicit shared resources. Network access, inherited environment variables, writable tool caches, `.codex`, and any extra mounts retain access to the resources they contain. It is not an assurance that arbitrary hostile code cannot access credentials or influence future tool runs. Bubblewrap does not add a seccomp policy; Landlock's network-off filter is described above. Grant only the paths required by your project, and use normal host/remote host job controls for resource limits.

## Checks

`cargo test --test sandbox` verifies local/remote policy agreement, missing/unusable-bwrap fallback, network/path arguments, and real namespace isolation when available. The active isolation check verifies project writes, hidden outside files, and symlink escapes through the actual Rust helper and PTY.

`RODE_TEST_REQUIRE_LANDLOCK=1 cargo test --test landlock --test sessions` requires real Landlock enforcement on ABI 4+: desktop/headless agreement, native and remote file helpers, inherited child restrictions, read-only/truncate protection, nested writable grants, restrictive-grant rejection, TCP/UDP/Unix network denial, inherited descriptor cleanup, authenticated host control and failure without an unsandboxed retry. Ruleset unit tests cover ABI 4, 5, 6 and newer masks/layouts. Without that environment variable, unavailable-kernel tests report a skip; preference and fallback checks still run.

Landlock compatibility checks also open the host resolver and available Ubuntu command database, verify their parent directories remain restricted, and start login Bash shells with isolated profile/alias fixtures. They retain private-home and explicit-hidden-path denial checks. The real Codex loopback check in [Development](development.md#agent-checks) includes an enforced Landlock case; it uses no real account or model.

`cargo test --test agents` checks whole-agent sandbox scope, read-only writes, streamed protocols, approvals/questions, interruption, bounded output, and child cleanup without inference. SSH roundtrips also exercise agents through the actual multiplexed channels.

`bash scripts/test-ssh.sh` checks direct and ProxyJump sessions, remote project switching, command working directories, and remote sandbox file access. Add `RODE_SSH_UI=1` on an isolated X display to exercise the native project browser in local sandbox mode, without bwrap, and over the temporary SSH server. The test daemon runs without privilege and cannot allocate/login a remote TTY, so the GUI fixture uses a local SSH wrapper that substitutes `-T` for `-tt`. Its authentication, multiplexing, file transfers, project shells, and local PTY remain real. Production code requests a remote TTY and contains no special fixture mode. The local sandbox UI check also replaces a temporary copy of the running editor executable and verifies that file helpers continue to work.
