# Project tools

## Git source control

Use the **Source control** activity icon, **View → Source control**, or **Ctrl+Shift+G**. Git runs on the project host using its installed Git and configuration. The native panel provides:

- Status, branch/upstream information, working/staged diffs, and file or individual text-hunk staging/unstaging.
- Commits, amend, the latest 100 commits with patches, blame, cherry-pick, and revert.
- Branch creation/switching/deletion, merge/rebase, stash push/apply/pop/drop, and conflict-resolution controls.
- Fetch, fast-forward-only pull, push/upstream setup, remotes, tags, and submodule initialization/update.
- Worktree listing, creation with new or existing branches, opening, removal, and metadata pruning.
- Repository initialization and cloning into an empty selected project directory.

Open conflicted files, edit the conflict markers, save, and **Mark resolved**, then continue the merge/rebase. Changes shown by Git are on disk; save modified editor buffers before staging or changing branches. Discard, branch/tag deletion, stash removal, revert, rebase, and worktree removal expose a confirmation; normal Git protections remain in force. Hunk staging is available for ordinary tracked text patches; use whole-file staging for new/deleted/renamed/binary files.

Git operations run in background workers, with bounded output/time and no automatic fetch. The Changes panel refreshes while visible. Network actions use existing host credentials and report failures; **Git terminal** provides interactive authentication and arbitrary Git commands. Within bwrap, enable network access and explicitly grant any identity/agent paths your Git authentication requires. Global Git configuration is shared read-only by default and can be hidden in the access editor.

Linked worktrees share repository metadata. **Open + share main .git** explicitly grants that metadata directory when opening a separate worktree sandbox; **Open project** keeps the current/saved policy. Standard repository layouts are supported by the shortcut; nonstandard Git directories can be granted through the path editor. Worktree creation needs a destination accessible to the current sandbox.

## Formatting, issues, and Python environments

The **Python environment** activity icon or the Python status-bar control selects an interpreter. Auto mode detects `.venv`, `venv`, `env`, or `.env` directly in the project, then uses system Python. Choose a discovered environment, enter a custom interpreter/environment path on the project host, or create a new environment with `python -m venv`. Existing destinations are never overwritten. Disable **Include pip** on hosts without `ensurepip`.

Interpreter choices are saved per host/project. New terminals activate the selected environment; selecting another interpreter keeps an existing terminal running. Use **Activate in current shell** with an idle shell, or **New** to start another terminal. Enable **Persist** in its header if it should continue after exit. Reattaching an existing terminal preserves its environment and never injects activation commands into running work. Run-file, REPL, pytest, and package-list controls use the selected executable. Ruff runs as `uvx ruff` by default on the project host. RODE searches PATH, then the standard `~/.local/bin/uvx` location so non-interactive SSH sessions work with a user-local uv installation. In Project settings, select `auto` to prefer Ruff beside the chosen interpreter, then its Python module, then PATH. An explicit `ruff` executable or absolute path is also supported; the setting is an executable path or one of these known modes, not a shell command.

See [Programming languages](languages.md) for Go, JS/TS/web, C-family, and shell tools and run/build commands. Python formatting/checking uses the project's [Ruff configuration](https://docs.astral.sh/ruff/configuration/); Rust formatting uses `rustfmt`. Install [uv](https://docs.astral.sh/uv/getting-started/installation/) on the project host to use the default `uvx ruff` runner. uv may download Ruff into its isolated cache on first use; warm that cache before working without network access. RODE keeps the default uv tool registry under `~/.cache/uv/tools` (or `$UV_CACHE_DIR/tools`) so read-only `~/.local` mounts work, and respects an explicit `UV_TOOL_DIR`. Python environments and other packages are created or installed only through explicit actions. Install `rustfmt` with your Rust toolchain for Rust formatting. **Code → Format document** / **Shift+Alt+F**, **Check document**, and **Ruff: apply safe fixes** are available manually. Safe fixes explicitly disable unsafe fixes even if the project's Ruff configuration enables them.

Formatting/checking on save and supported-language checks after a 900 ms typing pause are enabled by default, with switches in Project settings. Checks use the current in-memory buffer. Issues appear as colored underlines, gutter marks/tooltips, and clickable entries in **Problems** / **Ctrl+Shift+M**. Live checks cover the active supported buffer up to 512 KiB; manual/save checks use the normal 8 MiB editing limit. Formatting results are applied only if the buffer and settings still match the submitted version. If new edits arrive, they remain intact. Missing/failed tools leave the buffer editable and allow the submitted save without formatting; Python syntax errors still produce diagnostics when Ruff can check them.

**Save tool settings** flushes changes to local app preferences immediately. Python issues use Ruff; Go, Prettier-supported formats, and shell files have parser syntax diagnostics. A general LSP client and Rust compiler diagnostics are not implemented.

## Host monitor

Use the **System monitor** activity icon, **View → System monitor**, or **System: Monitor** in the command palette. No project is required: the welcome screen can show your local computer's metrics. With an SSH workspace, it monitors that remote host once connected, even before a project folder is ready; offline SSH sessions show a reconnect message rather than local metrics.

The themed terminal-style panel shows CPU/per-core activity, RAM/swap, load/uptime, disk capacity, network throughput, and a short process list, with rolling CPU/RAM/GPU history charts. Disk capacity describes the project's filesystem when a project is ready, or the host's root filesystem otherwise. The panel identifies the host, and changing hosts clears the previous samples.

NVIDIA metrics come from [nvidia-smi](https://docs.nvidia.com/deploy/nvidia-smi/); AMD metrics use the kernel's [DRM/sysfs counters](https://docs.kernel.org/gpu/amdgpu/index.html) when exposed. Available readings include usage, VRAM, temperature, and power. Other DRM GPUs are listed with the readings the driver exposes. Missing/unsupported readings are shown as unavailable. No GPU telemetry was exposed by the development host, so GPU parsing is covered with representative samples rather than a claimed hardware validation.

Monitoring is on demand, samples in background workers every two seconds by default (configurable 1–30 s), bounds history to 120 samples, and pauses when hidden. CPU/network rates need two samples. Process CPU percentages are lifetime averages from `ps`. Telemetry intentionally reads the host outside the project sandbox to show host counters and devices; project tools remain inside the selected sandbox.

## Slurm allocations

**View → Terminals & Slurm** lists your scheduler jobs and allocated GPU counts, with confirmed whole-allocation release. Use [native persistent terminals](terminals.md) for interactive jobs that should survive closing RODE; see the [Slurm workflow and host requirements](slurm.md). The host monitor measures physical utilization separately.
