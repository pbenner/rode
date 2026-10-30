# RODE

A lightweight native Rust code editor for local and SSH projects, with on-demand tools, persistent terminals and configurable sandboxing.

- **[Terminals that survive editor exit](docs/terminals.md).** Enable **Persist**, keep shells running through editor exit or SSH loss, and reattach later—even from multiple RODE windows. Native Rust implementation; no tmux/screen dependency.
- **[Remote projects through your SSH setup](docs/remote.md).** Use existing OpenSSH aliases, keys and jump hosts. Browse and edit remote folders while Git, tools and debuggers run where the files live.
- **[One sandbox policy for the project](docs/sandbox.md).** Configure filesystem grants locally or over SSH; editor I/O, shells, agents and tools share the active policy. Unavailable sandboxing is clearly labelled normal mode.
- **[Background agents inside the sandbox](docs/agents.md).** Use Codex, local models through Ollama, or ACP agents locally and over SSH. Enable **Persist** to keep tasks running after exit and reattach through **Running**. Browse saved chats, control model/reasoning/permissions, and install Codex without root, VS Code or Node.js. Approvals cannot widen the active sandbox.
- **[Interactive GPU allocations](docs/slurm.md).** Request Slurm resources beside the terminal's Persist button, watch pending/active status and work in an allocated shell. Persist and reattach it for ML work; release allocations after confirmation. Host CPU/GPU telemetry is available separately.
- **[Encrypted local and remote projects](docs/encrypted-projects.md).** Open gocryptfs storage as a normal project; locking requires active project jobs to stop.

Also included:

- [Standard, Emacs and Vim editing / pane shortcuts](docs/editing.md#keyboard-pane-navigation), plus [independent interface and syntax themes](docs/themes.md).
- [Multi-language highlighting and tools](docs/languages.md), [Git branches/worktrees and Python environments](docs/project-tools.md), [Python/native debugging](docs/debugger.md), and [extensions](docs/extensions.md).
- [Jupyter notebooks](docs/notebooks.md), [Markdown](docs/markdown.md), [Marp slides](docs/marp.md), [LaTeX](docs/latex.md), [images](docs/images.md), [PDFs](docs/pdf.md), and [hex editing / binary analysis](docs/binary.md).

![RODE](assets/carbon.png)

```sh
cargo install --locked --path .
rode /path/to/project
# Or connect to a remote computer:
rode --ssh user@host --remote-path '~/project'
```

Linux is the validated platform. Build with Rust 1.88+; Cargo installs both the editor and its headless terminal helper. Remote access requires OpenSSH, a POSIX shell, and Python 3. For hosts needing a static helper, see [terminal setup](docs/terminals.md#remote-helper-and-packaging); no tmux/screen is required. Python checks use `uvx ruff` by default. PDF previews require desktop Poppler tools (`poppler-utils`).

Debian/Ubuntu packages are built for release version tags or manually through **Actions → Package Debian → Run workflow**. Build locally with `scripts/build-deb.sh`; see [packaging and installation](docs/packaging.md).

[Documentation](https://pbenner.github.io/rode/) · [Setup](docs/getting-started.md) · [Agent setup](docs/agents.md) · [Development and hooks](docs/development.md) · [Source architecture](docs/architecture.md)

MIT licensed; bundled dependencies retain their [own licenses](docs/licensing.md). Text/hex editing up to 8 MiB and [image previews](docs/images.md) up to 32 MiB; no LSP.
