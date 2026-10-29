# Development

Start with the [source architecture and maintenance guide](architecture.md) for module responsibilities, host boundaries and lifecycle contracts. Its [design decisions and tradeoffs](architecture.md#design-decisions-and-tradeoffs) explain why the boundaries exist and point to the owning modules' in-source rationale. Public Rust types, fields and methods have API documentation; both library crates warn on missing public documentation. When revisiting a design choice, update its rationale and consequences alongside the code.

```sh
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
cargo clippy --workspace --all-targets -- -D warnings
RODE_TEST_RUFF=/path/to/ruff cargo test --workspace --all-targets
```

Open `target/doc/rode/index.html` for editor services and `target/doc/rode_session/index.html` for the native terminal service. Add `--open` to the documentation command to open it in a browser. CI uses the same warning-free documentation check, including intra-doc links. Comment-only changes need formatting, API/documentation builds and a consistency review; application rebuilds and native UI tests are required only when executable behavior changes.

An additional test starts a temporary localhost-only SSH daemon using fresh keys, a private known-hosts file, and an isolated config. It never edits `~/.ssh`:

```sh
scripts/build-sessiond.sh
bash scripts/test-ssh.sh
```

This requires `openssh-server`, `ssh-keygen`, and Python 3. The tests use a local PTY and remote non-TTY project channels because an unprivileged test daemon cannot write system login records for a remote TTY. Production project terminals request a remote TTY. Direct and ProxyJump sessions check real authentication/multiplexing, switching project folders without disconnecting, terminal working directories, quoted Unicode paths, multi-megabyte reads/writes, save conflicts, sandbox file operations, and disconnection.

For a native UI check on an isolated X server (also requires ImageMagick and libXtst):

```sh
cargo build
xvfb-run -a python3 scripts/test-ui.py
# Or test the release binary:
RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-ui.py
# Shell Tab completion, focus handoff, history arrows, and terminal/editor shortcuts:
RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-terminal-keys-ui.py
# Multiple tabs, drag ordering, dirty-close protection, and minimum-window layouts:
RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-layout-ui.py
# Agent chat, saved settings, approvals, Stop, sandbox scope, and dirty buffers:
RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-agent-ui.py
# Two processes sharing preferences/profiles and closing independently:
RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-multiprocess-ui.py
# Image tabs, zoom, reload, themes, and text preservation, locally and over SSH:
RODE_TEST_BINARY="$PWD/target/release/rode" RODE_IMAGES_UI=1 xvfb-run -a bash scripts/test-ssh.sh
# Markdown previews, inline images/sandbox scope, save, cursor, and undo/redo:
RODE_TEST_BINARY="$PWD/target/release/rode" RODE_MARKDOWN_UI=1 xvfb-run -a bash scripts/test-ssh.sh
# Project-folder and sandbox UI checks, including the isolated SSH host:
RODE_TEST_BINARY="$PWD/target/release/rode" RODE_SSH_UI=1 xvfb-run -a bash scripts/test-ssh.sh
# Shared file/hex/Save As browser, text edits, image/PDF opens and keyboard navigation:
RODE_TEST_BINARY="$PWD/target/release/rode" RODE_FILE_BROWSER_UI=1 xvfb-run -a bash scripts/test-ssh.sh
# Ruff/venv, monitor, Git, settings, and policy-restoration UI checks:
RODE_TEST_RUFF=/path/to/ruff RODE_TEST_BINARY="$PWD/target/release/rode" RODE_WORKBENCH_UI=1 xvfb-run -a bash scripts/test-ssh.sh
```

Screenshots and logs go to `target/ui-smoke/`, `target/layout-ui/`, `target/project-ui/`, and `target/workbench-ui/`. Set `RODE_TEST_RUFF` for the Rust and SSH tests too, to exercise real Ruff formatting/checking. The smoke tests cover editing, saves, terminal commands, themes/preferences, local/remote folder browsing, sandbox/fallback, Git staging/commits, Python environments, and saved project policies. Namespace-isolation tests report when the host cannot run bubblewrap. These are basic regression checks, not a benchmark suite. `RODE_SMOKE_TEST=1 ./target/release/rode` provides a separate five-second launch/exit check.

`scripts/test-file-browser-ui.py` also runs locally on its own; screenshots/logs go to `target/file-browser-ui/`. Library tests cover stale picker results, relative destinations, preserved Save As names and project-environment guards.

## Concurrent processes

`cargo test --test persistence --test concurrency` starts real subprocesses to exercise field-level preference/profile merging, external configuration edits, malformed-file preservation, lock release after a killed writer, private GUI snapshots, parallel sandboxed helpers with identical namespace PIDs, concurrent environment creation, and simultaneous local/Python-peer document saves. Existing-file checks force writers to wait on the same inode, then verify that only one buffer based on the old revision can win. Save As tests verify atomic creation without overwriting another writer. Native two-window screenshots/logs go to `target/multiprocess-ui/`.

The SSH fixture also opens two independent masters for the same alias and checks that closing one preserves the other. The project UI check (`RODE_SSH_UI=1`) exercises banner/palette disconnect, compact connection controls, dirty buffers across reconnect, local file/folder access after explicit and unexpected disconnect, and persistent versus nonpersistent terminal lifetime. Library tests also cover failed logins, stale replies, disconnected Save As and prevention of remote/local path rebinding. See [configuration and multiple windows](configuration.md) for supported filesystem behavior and limitations.

## Security checks

`cargo test --test security --test files --test sandbox --test persistence --test concurrency` checks special-file rejection with subprocess deadlines, canonical home grant restrictions, controlling-terminal isolation under a real PTY, local/Python parity, protected saves and shared configuration. Run `cargo test --test sessions --test agents --test debugger` for process lifecycle checks, and `scripts/test-ssh.sh` for direct/ProxyJump transport and sandbox integration. With an installed cargo-audit, `cargo audit --file Cargo.lock` checks the current advisory database. The [security review](security.md) records reproduced findings, fixes, maintenance warnings and the scope of the audit.

## Image checks

`cargo test --test images` checks all supported codecs, alpha and EXIF orientation, GPU downsampling, corrupt/oversized input rejection, local/Python binary-protocol parity, and actual sandbox access. Library tests cover shared pixel-budget release, stale reloads, and read-only save commands. The direct/ProxyJump SSH fixture also transfers and decodes images under the active project policy. `scripts/test-images-ui.py` checks rendered PNG pixels, zoom, JPEG tabs, reload, both themes, compact layout, read-only saves, and preservation of dirty text. It can run locally on its own, or with `RODE_IMAGES_UI=1` in the SSH fixture. Screenshots/logs go to `target/images-ui/`.

## Markdown checks

Markdown checks use `cargo test --lib markdown` for unsaved-buffer rendering, source saves, source-path resolution, input limits, image URI isolation, stale-result rejection, and texture-budget release. Native local/SSH screenshots and logs go to `target/markdown-ui/`; these checks also exercise actual sandboxed image reads, view switching, undo/redo, cursor preservation, themes, and find returning to source.

## Agent checks

`cargo test --test agents` uses deterministic Codex app-server and ACP v1 peers without calling a model. It exercises multi-turn streaming, approvals/questions, cancellation, process-tree cleanup, protocol/output limits, actual bwrap scope/read-only access, and explicit normal-mode fallback. The isolated direct/ProxyJump SSH tests also run both agent protocols, checking working directory, outside-file visibility, approvals, cancellation, and continued use of the authenticated master after closing chat.

To check an installed real Codex CLI without a model or account access, use:

```sh
RODE_TEST_CODEX=/absolute/path/to/codex cargo test --test codex -- --nocapture
```

This uses an isolated temporary `CODEX_HOME` and local model provider. It checks initialization and a complete streamed chat turn through a loopback Responses fixture, in normal mode and with bwrap when available, without calling a real model. Both passed with Codex `0.164.0-alpha.11.1`. Native screenshots/logs go to `target/agent-ui/`. Real Ollama model inference and third-party ACP executables require their own installations and are not covered by the deterministic peers.

## Ruff and pre-commit

Install [uv](https://docs.astral.sh/uv/getting-started/installation/) and enable the versioned hook configuration:

```sh
scripts/uvx.sh pre-commit@4.7.0 install
scripts/uvx.sh pre-commit@4.7.0 run --all-files
```

The hooks run Ruff lint fixes followed by formatting through `uvx`, pinned to Ruff 0.17.3. Review and stage any resulting edits before committing. New clones need the install command once. The wrapper keeps uv's tool registry in the ignored `target/uv-tools` directory unless `UV_TOOL_DIR` is explicitly set.

For a read-only check matching CI:

```sh
scripts/uvx.sh ruff@0.17.3 check .
scripts/uvx.sh ruff@0.17.3 format --check .
```

Python helpers require only the standard library. Their source files are concatenated by Rust into one SSH program; keep their wire format and local/remote behavior aligned. The repository Ruff configuration covers the helpers, test harness, and documentation hook.

## Documentation

The short README links into these guides. MkDocs 1.6.1 builds the static documentation; its native search and a small local stylesheet require no application runtime changes.

```sh
scripts/uvx.sh --from mkdocs==1.6.1 mkdocs serve
scripts/uvx.sh --from mkdocs==1.6.1 mkdocs build --strict
```

The preview is mounted at `http://127.0.0.1:8000/rode/`. Generated output goes to `target/site/`. The build hook publishes the theme format, palette JSON, and screenshots without maintaining duplicate source copies. The sandbox guide lives directly in `docs/sandbox.md`.

The **Documentation** GitHub Actions workflow validates documentation on pull requests, then builds and deploys pushes to `main` through the GitHub Pages artifact workflow. In repository **Settings → Pages**, select **GitHub Actions** as the publishing source. The expected site is `https://pbenner.github.io/rode/` after the first successful deployment. Preparing this repository does not publish a site by itself. See [GitHub's workflow documentation](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages).

## Before the first commit

Run the Ruff hooks, Rust checks, real Ruff tests, isolated SSH checks, native UI smoke tests, and strict documentation build. Keep `Cargo.lock` in version control. Generated binaries, Python bytecode, documentation output, credentials, and local preferences belong outside the commit.

Linux is validated; the automated SSH daemon fixture does not cover every authentication setup or operating system. GPU parsers have sample coverage; the development machine exposed no GPU telemetry for hardware validation.

## Language tools checks

`cargo test --test languages --test project` checks filename/grammar coverage, colored Go/Python/TS/TSX/TOML/Dockerfile tokens across themes, Unicode issue columns, stdin formatting and source preservation, and active sandbox enforcement. Real Go/clang-format checks run when those tools are on PATH. Set `RODE_TEST_PRETTIER` to an installed Prettier executable and `RODE_TEST_RUFF` for their real local/Python-peer parity checks. The SSH fixture also checks installed project-local formatter selection and syntax diagnostics in both direct and ProxyJump sandboxed sessions.

With Go on PATH and `RODE_TEST_PRETTIER` set, run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-languages-ui.py` for actual Go format/run/build, TypeScript formatting/Problems, and saves after syntax errors. Screenshots/logs go to `target/languages-ui/`. The fixture uses an isolated shell startup so user login configuration does not change its toolchain paths. CI installs Go, clang-format, shfmt and a pinned test Prettier; the application itself never installs those tools.

## Independent syntax themes

`cargo test --test themes --test persistence` covers full preset colors, independence from interface/terminal colors, custom scope rules, italic/underline separation, UTF-8 layout ranges, invalid/oversized definitions, cache invalidation and concurrent preference merges. Library tests check immediate independent saves, startup/CLI restore, unavailable-palette fallback and dirty-buffer preservation.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-syntax-themes-ui.py` to check all syntax choices against a fixed Carbon interface, a custom theme, unsaved source and live preference writes, menu layout and restart persistence. Native captures/logs go to `target/syntax-themes-ui/`.

## PDF checks

Install desktop Poppler tools (`pdfinfo` / `pdftoppm`). `cargo test --test pdf --lib pdf` checks real two-page rendering, portrait/landscape content, page limits and cancellation, malformed input, read-only commands, stale results, shared-budget release, native/Python binary parity and actual sandbox reads/symlink escapes. The isolated direct/ProxyJump tests also transfer PDFs under the project policy.

For native local and SSH viewing: `RODE_TEST_BINARY="$PWD/target/release/rode" RODE_PDF_UI=1 xvfb-run -a bash scripts/test-ssh.sh`. This checks pages, wheel scrolling within/across pages, zoom, source reload preserving the selected page, failed-reload preservation, themes, read-only saves and dirty code buffers. `scripts/test-pdf-ui.py` can run locally on its own. Screenshots/logs go to `target/pdf-ui/`. The two-page vector fixture at `tests/fixtures/pages.pdf` is generated test data with colored shapes and built-in Helvetica text, requiring no PDF-generation library.

## Native terminals and Slurm

`cargo test --test sessions --test slurm` exercises native PTY persistence after views exit, in-place persistence changes shared across views, last-view cleanup for nonpersistent sessions, two simultaneous views sharing input/output, per-session cleanup, concurrent service creation, fresh caller environments, login-startup activation without reactivation on attach, and real bwrap lifetime/access checks. The native supervisor keeps bwrap's spawning thread alive; closing the request channel must not trigger `--die-with-parent`. Slurm tests use fake queue/cancel executables to check TRES counts, pending/unknown resources, local/Python-peer parity, exact allocation IDs, UID filtering, and removal of inherited cancellation overrides. They never allocate or cancel real jobs.

A legacy-protocol fixture owns a real shell and rejects newer requests. Local and direct/ProxyJump checks keep its shell state across detach/reattach while starting new terminals in the compatible service, verify opt-in persistence and combined discovery, prevent unsupported toggles from reaching the old daemon, and preserve the new terminal after ending the old one. They also check stale helper rejection, stale socket recovery, and that dropping a native bridge cannot inject an EOF keystroke into the shell. Each fixture has a private runtime; no running user service is restarted to test an upgrade.

Idle-lifetime checks start a service without a terminal and naturally exit its last shell, then verify socket cleanup and automatic restart. They keep a detached persistent shell alive beyond the five-second idle timeout, inspect the raw service protocol to verify completed records are released while that shell runs, preserve final output in open views, and check that polling or finished views cannot keep an otherwise empty service alive. The native UI check also leaves the session manager open while a terminal exits to check automatic list refresh.

Build the remote helper with `scripts/build-sessiond.sh` before running `scripts/test-ssh.sh`. Both direct and ProxyJump tests deploy a private checksum-verified static helper, start a sandboxed native terminal with a real project venv, disconnect its view/master, establish a fresh independent connection, and reattach to the same running shell. Fixtures use isolated runtime/helper paths and end only their own sessions.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-sessions-ui.py` for multiple terminals, the rightmost Persist button with theme-colored fill, on/off changes preserving the shell, compact layout, prompt-free mixed-session exit, reattachment, shared last-view lifetime across two native windows, temporary shells, and confirmed Slurm release against deterministic mocks. Screenshots/logs go to `target/sessions-ui/`. Test harnesses use private `RODE_SESSION_DIR` directories and explicitly end their own sessions on cleanup. A failed test may leave its native fixture service running: use that directory's service API to list/end its sessions; do not kill other users' services or unrelated jobs.

Linux x86_64 static helper and isolated SSH are validated. No real Slurm controller or GPU allocation was available; site-specific Slurm/authentication/logout policies require on-site validation. See [terminal limits](terminals.md#lifetime-and-limits) and [Slurm requirements](slurm.md).

## Debian package checks

See [Debian packaging](packaging.md) for prerequisites and build options. After the native release and static helper builds, run `scripts/build-deb.sh --skip-build`, then `python3 scripts/test-deb.py target/debian/rode_0.1.0-1_amd64.deb`. The test inspects and extracts the archive, validates its launcher and checksums, runs the packaged CLI and static helper in isolated temporary directories, and never installs into the host. CI uploads the built archive.

## Extensions and binary tools

`cargo test --test binary --test extensions --test persistence` checks byte patch/undo/search, protected binary saves and permissions on both backends, ELF/PE/Mach-O fixtures, native x86 decoding, sandbox reads/writes, data-budget release, extension manifests/activation, versioned stdio adapters, lifetime cancellation and concurrent namespaced preferences. Library tests cover disabled-module activation, stale-result invalidation, avoiding eager Python probes and byte edits arriving during saves. The isolated SSH/ProxyJump fixture includes binary transport/saves and sandbox-aware adapters.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-binary-ui.py` for native grid/patch/keyboard/undo, conflicts, Save As, object views, the Extensions activity icon/shortcut, toggles/persistence, installed formatters and palette contributions. For local plus actual SSH sandbox UI: `RODE_BINARY_UI=1 RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a bash scripts/test-ssh.sh`. Screenshots/logs go to `target/binary-ui/`. Fixtures inspect generated/simple binaries; they do not execute inspected programs.

## Notebook checks

Set `RODE_TEST_JUPYTER` to an installed interpreter with `jupyter_client`, `ipykernel` and Matplotlib, then run `cargo test --test notebooks` and `scripts/test-ssh.sh`. CI uses an isolated `target/notebook-env`; it never installs these packages into a user's project. The tests cover metadata/attachments/unknown-field round trips, versioned outputs, real stateful Python execution, PNG plots, errors/Run All, interrupts, owned-process cleanup, and real bwrap operation without shared networking. Direct/ProxyJump tests also execute a notebook kernel inside its SSH sandbox while preserving the authenticated connection.

Run `RODE_TEST_JUPYTER=/absolute/interpreter RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-notebooks-ui.py` for native cells, plots, cell search/edit/Shift+Enter, metadata preservation, saves/conflicts/Save As, JSON switching and dirty-close protection. Add `RODE_NOTEBOOK_UI=1` when running the SSH fixture for remote UI coverage; `RODE_NOTEBOOK_UI_REMOTE_ONLY=1` skips the repeated local UI pass. Captures/logs go to `target/notebook-ui/`. The shared process transport retains the existing Codex/ACP regression tests.

## Marp checks

`cargo test --test marp` exercises snapshots, preserved source, export conflicts/atomic replacement and symlink protection, conversion limits, native/Python coordinator cancellation (including a detached browser-shaped child), and read-only bwrap preview without project writes. `cargo test --lib marp_view` checks source saves, dirty guards, cached previews and independent export errors; native UI tests cover focus and interaction. Full Rust checks include the existing Markdown/PDF tests after their shared-view refactoring.

For real conversion, install the test CLI into an ignored development directory and point the tests at an installed browser:

```sh
npm install --prefix target/marp-tools --ignore-scripts --no-audit --no-fund @marp-team/marp-cli@4.5.1
RODE_TEST_MARP="$PWD/target/marp-tools/node_modules/.bin/marp" RODE_TEST_MARP_BROWSER=/usr/bin/firefox cargo test --test marp
```

This checks real themed HTML, PDF, math and PowerPoint output. The editor never installs these tools for users. Without the test environment variable, deterministic CLI fixtures exercise the same protocol and rendering UI without a browser dependency.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-marp-ui.py` for native Slides/navigation, current-buffer export, source saves and focus, explicit export replacement, new-deck dirty guards and compact layouts. Add `RODE_MARP_UI=1` to the SSH fixture for local/SSH sandbox UI, or `RODE_MARP_UI_REMOTE_ONLY=1` to skip repeating the local UI pass. `RODE_TEST_MARP` and `RODE_TEST_MARP_BROWSER` also enable the real renderer for its local UI pass. Captures/logs go to `target/marp-ui/`. Direct/ProxyJump integration exercises remote rendering/export and cancellation without closing the authenticated connection.

## LaTeX checks

`cargo test --test latex --test languages --test persistence` covers TeX/BibTeX detection and highlighting, generic literal compiler arguments, main-file/root redirects, dotted/quoted/Unicode filenames, source preservation, compiler/ChkTeX diagnostics, stdin formatting, cancellation of owned children, shared output-directory locks and actual bwrap access. Raw native and Python-peer requests exercise the same behavior. Library tests guard dirty sources, stale build results and context cancellation. Concurrent preference tests merge LaTeX command/argument edits across processes.

The real compilation test runs when `latexmk`, pdfLaTeX and a usable `pdflatex.fmt` are installed. `RODE_TEST_TEXMF` and `RODE_TEST_TEXFORMATS` can select an isolated development TeX tree/format directory; tests isolate TeX's writable home/config/cache paths. These overrides are test-only. No application toolchain or packages are installed automatically. Development validation also used an isolated format generated from the host's installed TeX sources.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-latex-ui.py` for dirty guards, configured format/check tools, native red/blue PDF rendering and reload, compiler errors and compact layout, normally and with bwrap. Set `RODE_LATEX_UI_REAL=1` plus a usable TeX environment to exercise real local compilation; the sandbox UI pass uses deterministic installed-tool fixtures. Add `RODE_LATEX_UI=1` to `scripts/test-ssh.sh` for SSH UI, or also set `RODE_LATEX_UI_REMOTE_ONLY=1` to skip repeating local checks. Direct/ProxyJump integration checks build/PDF transfer, lint/format, compiler errors and cancellation without losing authentication. Captures/logs go to `target/latex-ui/`.

## Encrypted-project checks

`cargo test --lib --test vaults` checks mount identity, leases, underlying mountpoint protection, initialization refusal for existing plaintext, buffer/exit protection, deferred cleanup, operation draining and password handling on native/Python peers. A Python component test uses real host file locks with a mock unmount to check atomic shared-close release without FUSE. With installed gocryptfs it also initializes a real encrypted configuration and rejects an incorrect password. If working FUSE is available, the same test performs real encrypted writes, shared-window refusal, release and locking; `/dev/fuse` is absent in the current development environment, so that mount round trip could not be validated here. CI installs gocryptfs/FUSE tools; actual kernel mount availability still depends on the runner.

Direct/ProxyJump SSH tests check that a successful tool cannot substitute an ordinary directory for a verified encrypted mount, keep passwords out of diagnostics and preserve the authentication master after cleanup. Run `RODE_TEST_BINARY="$PWD/target/release/rode" RODE_VAULT_UI=1 xvfb-run -a bash scripts/test-ssh.sh` for local/SSH UI, failed unlock preserving a dirty source, preference files free of the submitted password and compact layouts. Captures/logs go to `target/vaults-ui/`. The native library tests use a deterministic protocol peer for successful/failed lock UI state transitions without claiming a FUSE mount.

## Debugger checks

`cargo test --test debugger` checks DAP byte framing/Unicode, launch/configuration ordering with deferred replies, verified/unverified/conditional breakpoints, stepping, scopes/children/watches, stale evaluations, real interactive PTYs, unread-input responsiveness and owned detached-child cleanup on the native and Python coordinators, normally and under bwrap. Attach requests detach; sandboxed attach is rejected. Library checks cover source guards and extension disable. The shared preference tests merge independent debugger and other tool edits across processes.

Set `RODE_TEST_DEBUGPY` to an installed interpreter with debugpy (or reuse `RODE_TEST_JUPYTER`) for real Python breakpoint/step/evaluate/stdin checks. Set `RODE_TEST_LLDB` to an installed `lldb-dap` executable for real C/C++/Rust compilation and debugging across both coordinators and sandbox modes. Test compilers and adapter libraries must already be available to the test host. RODE installs none of them. Development validation used debugpy 1.8.22 and LLDB 19.1.7, including native interactive launch in the shared sandbox namespace.

`RODE_TEST_DEBUGPY=/path/to/python RODE_TEST_LLDB=/path/to/lldb-dap scripts/test-ssh.sh` also exercises the deterministic peer, real debugpy and native C stepping inside direct and ProxyJump SSH projects, then verifies the authenticated master still works. `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a python3 scripts/test-debugger-ui.py` checks native local/sandbox controls, dirty-source launch protection, breakpoints, step shortcuts, terminal input after a multi-megabyte output burst, themes and compact layout. For remote UI use `RODE_DEBUGGER_UI=1 RODE_DEBUGGER_UI_REMOTE_ONLY=1` with the SSH fixture. Captures/logs go to `target/debugger-ui/`.

### Debugger performance profile

The debugger panel separates rendering/actions, cached configuration editing, source navigation and terminal transport. It borrows inspection data while drawing, serializes queued requests once, cancels superseded queries, and persists settings only after edits. Its queued requests and slow-adapter buffers are limited to 256 messages; paused-variable and terminal-output budgets remain bounded.

Run the display-independent, manual release profile with:

```sh
cargo test --release --lib debugger_frame_profile -- --ignored --nocapture
```

It lays out 100 warmed frames at 390×900 with 1,600 variables, 1,024 breakpoints, 32 watches and a populated console. In the local review, median redraw time fell from 2.05 ms to 1.40 ms per frame (about 32%). This measures panel layout, not GPU presentation, editor startup or all project workloads; timings depend on the machine. The native debugger UI test also reports paused idle CPU/RSS and verifies that idle frames issue no new protocol requests. Performance timings are observations, not machine-dependent pass/fail thresholds.

Regression coverage includes full-sized configurations with a slow adapter, request-buffer retries, old watch/frame/breakpoint replies, delayed source opens, invalid setting rollback, and terminal-view lifecycle. Shutdown drains output in batches so verbose targets do not consume the grace period one message at a time.

## Lifecycle and hex rendering review

The regression tests cover native window close with clean encrypted-project buffers, waiting for active operations before verified lock, failed locks preventing exit, unbound buffers with plain-text extension checkers, remote tools exiting before reading their password input, asynchronous SSH terminal attachment, and starting a native service from an executable unlinked by an update. Project UI checks also start a new shell after replacing the running editor's file. Parallel SSH and sandbox helper directories use a per-process sequence in addition to PID/time, so equal clock samples cannot collide. Finished terminal views do not send another signal to their reaped child. Disassembly results retain their buffer version, offset and decoder mode; changing only decoding parameters reuses the full-file object report.

A display-free render sample is available for manual comparison:

```sh
cargo test --release --lib hex_frame_profile -- --ignored --nocapture
```

On the development machine, a dirty 8 MiB binary differing at its final byte took **0.798 ms median / 0.990 ms p95** per unchanged editor frame before the review, and **0.625 ms median / 0.642 ms p95** after moving full-buffer dirty comparisons out of unchanged frames. Each sample used 100 warmed egui frames with a virtualized hex grid. This is a synthetic render-only measurement; it does not measure total application, GPU rendering, SSH or tool execution time and is not a general speed guarantee.

## Keyboard navigation checks

`cargo test --lib navigation` checks default bindings, exact modifiers, leader input consumption, list selection, invalid-edit rollback, remapping/reload and per-binding concurrent preference merges. On an isolated X display, `RODE_TEST_BINARY=target/release/rode python3 scripts/test-navigation-ui.py` exercises the workflow entirely with keyboard events: explorer expansion/opening, file suggestions, pane focus, terminal creation/cycling, shell Tab/Ctrl+K, dialog return, resizing/maximization and UI binding persistence. Screenshots/logs are in `target/navigation-ui/`. Set `RODE_NAVIGATION_UI=1` when running `scripts/test-ssh.sh` to repeat the keyboard workflow over real SSH. Run the terminal-key, layout and project UI regressions after changes to focus routing. Window-manager grabs cannot be reproduced by the isolated X test; defaults deliberately contain no Alt.

Style regressions use `cargo test --lib navigation`: preset validation/isolation, native Emacs mark/kill/yank, Unicode motions, Vim operators/counts/visual selection, shared native undo, batched insert/Escape ordering and cancellation of queued input after context changes. `scripts/test-shortcut-styles-ui.py` checks actual native Ctrl+X clipboard events, both source keymaps, safe file saves, shell Ctrl+W, pane switching and immediate style persistence. Use `RODE_SHORTCUT_STYLES_UI=1` in the isolated SSH fixture for the same workflow over SSH. Captures/logs go to `target/shortcut-styles-ui/` and `target/shortcut-styles-ssh-ui/`.
