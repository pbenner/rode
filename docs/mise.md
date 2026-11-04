# mise toolchains, environments and tasks

RODE includes an optional **mise** bundled extension (`rode.mise`), disabled by default. Enable it in **View → Extensions**, then open **Environment**. Install [mise](https://mise.jdx.dev/) on the project host first; an SSH project uses the remote installation. RODE searches that host's PATH and `~/.local/bin/mise`, or uses the executable override you enter.

The host also needs Python 3 for the bounded environment launcher and metadata bridge.

The bundled extension owns its native Environment/Tasks UI and launch integration. API v1 installed adapters currently provide formatting, diagnostics and palette commands; they cannot supply a project environment, so this integration uses the existing bundled-feature mechanism. Mise remains an external, optional executable.

With an active sandbox, grant access to the mise config/state/cache paths you use through the existing access settings. Tool installations under `~/.local` are read-only by default; trusting configurations or installing tools needs the corresponding writable paths. RODE never adds these grants automatically or retries a denied command outside the sandbox.

## Inspect and enable a project

**Refresh mise tools and tasks** reads the project's mise configuration hierarchy through the installed CLI. It lists selected tools and project tasks without installing anything. Configuration discovery, executable lookup and commands run on the captured project host inside its active sandbox. A missing executable, inaccessible cache, untrusted configuration or unavailable tool produces a visible error.

Review the project's `mise.toml` and tasks before choosing **Trust project configuration**. This explicitly runs `mise trust` on that host using mise's paranoid mode. If mise then requests trust for another configuration file, such as `rust-toolchain.toml`, repeat the action for that file. **Install declared tools** explicitly runs `mise install`; sandbox grants and network restrictions still apply. RODE neither installs mise itself nor automatically trusts repositories or downloads missing tools during background work. Install/trust commands have bounded output and time; large installations can be performed in a suitable host terminal instead.

Select **Use mise for new project processes**, then **Apply environment**. This saves only the opt-in, executable override and your literal Rode settings. The provider resolves values in the child process immediately before starting work; resolved secrets never enter desktop preferences or metadata snapshots. The launcher rejects malformed environments and missing declared tools rather than silently selecting another version. Mise's trust checks are enforced even if the editor inherited a CI environment.

The configured environment applies to new project terminals, tasks, formatters/checkers, executable adapters, agent backends, notebook kernels, debugger coordinators and build tools. Ordinary editor file access and session discovery remain available if the provider needs repair. Explicit Rode variables/runtime overrides and selected Python interpreters or conventional project venvs retain precedence. Standard interpreter and environment features remain usable when this extension is disabled.

Existing and reattached terminals, agents, kernels and debuggers keep the environment with which they started. Changing configuration or disabling the extension does not rewrite those processes. Start new work to use changed settings. Task commands use a child environment and leave the parent terminal's environment intact.

## Run project tasks

Open **Tasks**, refresh mise metadata and click a task name. Tasks are discovered using `mise tasks ls --local --json` and run as literal task names through `mise run` in the existing project terminal. Their dependencies, arguments and execution remain owned by mise. Only names and bounded descriptions are shown; RODE does not save resolved environment values.

For Slurm work, the existing allocation lifecycle and compute-node sandbox checks remain in force. Mise resolution is also applied inside the compute-node launch boundary. Mise, its configuration and selected tool installations must be accessible there; site modules, GPU drivers and allocator-provided device visibility remain managed by the cluster.

## Developing RODE with mise

The repository's `mise.toml` and `mise.lock` pin Go, Python, Node.js, uv and Ruff. `rust-toolchain.toml` is the Rust version authority and mise reads it directly. The lockfile includes Linux x86-64 and ARM64 artifacts. Keep `Cargo.lock` for application dependencies and `scripts/ci-deps.sh` for OS packages such as Poppler, Mesa, bubblewrap and Xvfb. CI installs the pinned Go toolchain only for the tools/runtime jobs; it never downloads Go through APT. For a local run of either suite's dependency setup, use `mise exec -- bash scripts/ci-deps.sh tools` (or `runtimes`) after installing the tools.

```sh
mise trust
mise install --locked
mise run check       # lint, docs and all ordinary Rust integration/unit tests
mise run build
mise run test-ui     # workspace UI suite; system UI tools must already be installed
```

`mise run lint`, `mise run test`, and individual `lint:*` tasks use the same commands as GitHub Actions. Existing scripts retain suite selection, deadlines, logs, SSH fixtures and artifact publication. Extended installed-tool tests still require the dependencies and explicit overrides documented in [Development](development.md).

To update a tool, edit its version in `mise.toml` (or Rust's toolchain file), regenerate with `mise lock --platform linux-x64,linux-arm64`, install with `mise install --locked`, and run the checks. The workflow also pins mise itself; update that version separately when changing the supported CLI baseline.
