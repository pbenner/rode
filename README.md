# RODE

A lightweight native Rust code editor for local and SSH projects, with on-demand tools, persistent terminals and configurable sandboxing.

- **[Terminals that survive editor exit](docs/terminals.md).** Enable **Persist**, keep shells running through editor exit or SSH loss, and reattach later—even from multiple RODE windows. Native Rust implementation; no tmux/screen dependency.
- **[Remote projects through your SSH setup](docs/remote.md).** Use existing OpenSSH aliases, keys and jump hosts. Browse and edit remote folders while Git, tools and debuggers run where the files live.
- **[One sandbox policy for the project](docs/sandbox.md).** Configure filesystem grants locally or over SSH; editor I/O, shells, agents and tools share the active policy. Unavailable sandboxing is clearly labelled normal mode.
- **[Agent chat inside that sandbox](docs/agents.md).** Run Codex or ACP agents on the project host, including Codex with local models served by Ollama. Agent approvals cannot widen the active sandbox.
- **[GPU and Slurm controls](docs/slurm.md).** View host CPU/GPU telemetry and Slurm GPU allocation counts; release selected allocations after confirmation. Pair with persistent terminals for interactive ML work.
- **[Encrypted local and remote projects](docs/encrypted-projects.md).** Open gocryptfs storage as a normal project; locking requires active project jobs to stop.

Also included:

- [Standard, Emacs and Vim editing / pane shortcuts](docs/editing.md#keyboard-pane-navigation), plus [independent interface and syntax themes](docs/themes.md).
- [Multi-language highlighting and tools](docs/languages.md), [Git branches/worktrees and Python environments](docs/project-tools.md), [Python/native debugging](docs/debugger.md), and [extensions](docs/extensions.md).
- [Jupyter notebooks](docs/notebooks.md), [Markdown](docs/markdown.md), [Marp slides](docs/marp.md), [LaTeX](docs/latex.md), [images](docs/images.md), [PDFs](docs/pdf.md), and [hex editing / binary analysis](docs/binary.md).

![RODE](assets/carbon.png)

```sh
cargo run --release -- /path/to/project
# Build the headless helper for remote persistent terminals:
scripts/build-sessiond.sh
# Or connect to a remote computer:
./target/release/rode --ssh user@host --remote-path '~/project'
```

Linux is the validated platform. Build with Rust 1.88+; remote access requires OpenSSH, a POSIX shell, and Python 3. Package `rode-sessiond-static` beside `rode` for remote persistent terminals; no tmux/screen is required. Python checks use `uvx ruff` by default. PDF previews require desktop Poppler tools (`poppler-utils`).

Build a Debian/Ubuntu package with `scripts/build-deb.sh`, or use **Actions → Package Debian → Run workflow** on GitHub; see [packaging and installation](docs/packaging.md).

[Documentation](https://pbenner.github.io/rode/) · [Setup](docs/getting-started.md) · [Agent setup](docs/agents.md) · [Development and hooks](docs/development.md) · [Source architecture](docs/architecture.md)

MIT licensed; bundled dependencies retain their [own licenses](docs/licensing.md). Text/hex editing up to 8 MiB and [image previews](docs/images.md) up to 32 MiB; no LSP.
