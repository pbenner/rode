# Development

Start with the [source architecture and maintenance guide](architecture.md) for module responsibilities, host boundaries and lifecycle contracts. Its [design decisions and tradeoffs](architecture.md#design-decisions-and-tradeoffs) explain why the boundaries exist and point to the owning modules' in-source rationale. Public Rust types, fields and methods have API documentation; both library crates warn on missing public documentation. When revisiting a design choice, update its rationale and consequences alongside the code.

Build the public documentation with `scripts/uvx.sh --from mkdocs==1.6.1 mkdocs build --strict`. Repository-relative links to files outside `docs/` must also be exported or rewritten in `scripts/docs_hook.py`; file existence alone does not make them part of the published site.

```sh
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
cargo clippy --workspace --all-targets -- -D warnings
RODE_TEST_RUFF=/path/to/ruff cargo test --workspace --all-targets
```

Open `target/doc/rode/index.html` for editor services and `target/doc/rode_session/index.html` for the native terminal service. Add `--open` to the documentation command to open it in a browser. CI uses the same warning-free documentation check, including intra-doc links. Comment-only changes need formatting, API/documentation builds and a consistency review; application rebuilds and native UI tests are required only when executable behavior changes.

## CI tiers and timings

The **Check** workflow runs on pull requests and pushes to `main`, avoiding duplicate branch-push and PR runs. New commits cancel superseded PR/main checks. Runners are pinned to Ubuntu 24.04, and routine Rust tests and the editor share the `ci` Cargo profile (development optimisation without debug information or release LTO). The editor, session daemon, static SSH helper and selected integration-test executables are built once and transferred together, preserving executable permissions.

| Tier | Coverage | When |
| --- | --- | --- |
| Fast | Formatting, linting, API docs, packaging-script checks, Rust tests, editor/workspace, keyboard input, multiwindow preferences, agents/attachments/queues/persistence, terminal sessions/Slurm, installer, direct/ProxyJump SSH and failed-login diagnostics | Every PR and main push |
| Extended | Environment/language tools/layout/themes, local and remote document viewers, real LLDB/Jupyter/formatters, and required sandbox enforcement | Main, nightly, full manual runs, or relevant PR changes |
| Release | Optimised Debian package and extracted-package GUI validation | Nightly/full manual checks, plus the existing version-tag/manual package workflow |

Fast UI checks run in four isolated jobs: `workspace`, `agents`, `terminals` and `ssh`. Extended jobs are `tools`, `documents`, `remote`, `runtimes` and `sandbox`. Each job has its own X display, clipboard and session directories; tests inside a job run sequentially. Specialist dependencies are installed only in their owning jobs. The `runtimes` job explicitly enables installed-tool integration tests that the ordinary Rust run may skip. SSH transport tests run once in the fast SSH job and once with those additional real tools in the runtime job; remote viewer cases share a private sshd without rerunning transport tests per viewer.

`scripts/ci.py` owns the suite manifest and PR selection. Changes under `src/`, `crates/`, `tests/` or `.github/`, Cargo changes, and shared harness changes select every extended job. Individual UI-script changes select their associated extended jobs; documentation-only PRs keep the fast checks. Add the **full-ci** PR label to force the complete extended tier. **Run workflow → full** also includes release packaging; disabling full runs only the fast tier. The nightly check runs at 03:23 UTC. The existing required **linux** check aggregates the fast jobs and reports independently of extended results. Repositories that require extended coverage before merging should also require the corresponding extended checks in branch protection.

The sandbox job adjusts the unprivileged-user-namespace AppArmor setting only on its disposable GitHub runner. `RODE_TEST_REQUIRE_SANDBOX=1` requires both Rust and Python project preparation to activate the real sandbox; unavailable sandboxing fails that job. Ordinary jobs retain explicit fallback coverage and print unavailable-host diagnostics.

Every test command has a deadline and writes its log and elapsed time under `target/ci-results/<suite>/`. Timings also appear in the Actions job summary. Long console output retains both the beginning and end, so SSH daemon chatter cannot hide an earlier test assertion; the artifact retains the complete log. Job limits bound setup/build hangs; failure logs and screenshots are uploaded even after test failures, with separate artifact names per job. Tests are not automatically retried. Use the recorded timings to rebalance jobs before tightening budgets; a sub-ten-minute fast tier is a target, not an assumed runner speed.

Inspect or run the same suites locally:

```sh
python3 scripts/ci.py list
mkdir -p target
cargo test --locked --profile ci --workspace --all-targets --no-run --message-format=json > target/ci-test-binaries.json
cargo build --locked --profile ci --bins
bash scripts/build-sessiond.sh
mkdir -p target/test-tools/bin
scripts/uvx.sh --from ruff==0.17.3 python -c 'import shutil; shutil.copy2(shutil.which("ruff"), "target/test-tools/bin/ruff")'
python3 scripts/ci.py bundle
LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -a -s '-screen 0 1600x1100x24' python3 scripts/ci.py run workspace
```

The bundle includes the pinned Ruff executable in `target/ci/tools/ruff`; document and runtime suites validate it before starting. Document jobs also install uv for the default `uvx ruff` path. The private SSH fixture exposes the invoking process's `PATH` so both sides use the same installed test tools.

Install each suite's prerequisites first (listed in `scripts/ci-deps.sh`); that script uses sudo and is intended for disposable CI runners. The failed-login clipboard regression also requires `xclip`. `RODE_TEST_BINARY`, `RODE_TEST_SESSION_HELPER` and `RODE_SESSION_HELPER` can override the binaries for local runs. The CI runner defaults `RODE_SESSION_HELPER` to the selected test helper as well: transferred test executables live in a separate directory, so service-backed history/agents cannot rely on sibling discovery. Explicit overrides remain respected. `RODE_SSH_TEST_BINARY` selects a prebuilt transport-test executable; standalone `scripts/test-ssh.sh` still builds/runs its Rust tests by default. To batch UI-only commands under one SSH fixture, use `RODE_SSH_RUN_TRANSPORT=0 bash scripts/test-ssh.sh <command> [arguments...]`; omitting the command in this mode is an error. `python3 scripts/test-ci.py` checks suite selection, helper discovery, artifact permissions, bounded failure output, timeout cleanup and both SSH-wrapper execution modes without contacting an SSH host.

The root package declares both `rode` and `rode-sessiond`, so plain `cargo install --locked --path .` installs both. `tests/sessions.rs` exercises the headless binary through `CARGO_BIN_EXE_rode-sessiond`, including protocol negotiation without a display and reattachment through the ordinary editor after the helper's clients exit. The library package's opt-in `standalone` feature exposes a separate `rode-sessiond-standalone` target for the static builder; distinct names avoid workspace output collisions even with all features enabled. `scripts/build-sessiond.sh` and Debian packaging build that target without desktop dependencies; the published helper names remain unchanged.

An additional test starts a temporary localhost-only SSH daemon using fresh keys, a private known-hosts file, and an isolated config. It never edits `~/.ssh`:

```sh
scripts/build-sessiond.sh
bash scripts/test-ssh.sh
```

This requires `openssh-server`, `ssh-keygen`, and Python 3. The tests use a local PTY and remote non-TTY project channels because an unprivileged test daemon cannot write system login records for a remote TTY. Production project terminals request a remote TTY. Direct and ProxyJump sessions check real authentication/multiplexing, switching project folders without disconnecting, terminal working directories, quoted Unicode paths, multi-megabyte reads/writes, save conflicts, sandbox file operations, and disconnection.

For native UI checks on Debian/Ubuntu, install `xvfb`, `xauth`, `x11-utils`, `imagemagick`, `libxtst6`, `libxkbcommon-x11-0`, `libegl1`, `libegl-mesa0`, `libglx-mesa0` and `libgl1-mesa-dri`. The scripts use `xwininfo` from `x11-utils` to find and inspect windows, ImageMagick to capture/inspect screenshots, and libXtst to send input. The editor dynamically loads the X11 keyboard library, so installing `libxkbcommon-dev` alone does not provide its X11 runtime. CI explicitly installs the runtime libraries, selects Mesa software rendering and uses a 1600×1100 virtual screen so the fixtures’ 1380×900 windows fit without being clamped by the window system. Set `LIBGL_ALWAYS_SOFTWARE=1`, `__GLX_VENDOR_LIBRARY_NAME=mesa` and `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json` when reproducing CI on a host with vendor GPU drivers. Early debugger-window failures print the exit code and the last 16 KiB of the captured startup log directly in the Actions output; the full log remains in the `native-ui-smoke` artifact.

```sh
cargo build
xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-ui.py
# Or test the release binary:
RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-ui.py
# Shell Tab completion, focus handoff, history arrows, and terminal/editor shortcuts:
RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-terminal-keys-ui.py
# Multiple tabs, drag ordering, dirty-close protection, and minimum-window layouts:
RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-layout-ui.py
# Agent chat, saved settings, approvals, Stop, sandbox scope, and dirty buffers:
RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-agent-ui.py
# Two processes sharing preferences/profiles and closing independently:
RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-multiprocess-ui.py
# Image tabs, zoom, reload, themes, and text preservation, locally and over SSH:
RODE_TEST_BINARY="$PWD/target/release/rode" RODE_IMAGES_UI=1 xvfb-run -a -s "-screen 0 1600x1100x24" bash scripts/test-ssh.sh
# Markdown previews, inline images/sandbox scope, save, cursor, and undo/redo:
RODE_TEST_BINARY="$PWD/target/release/rode" RODE_MARKDOWN_UI=1 xvfb-run -a -s "-screen 0 1600x1100x24" bash scripts/test-ssh.sh
# Project-folder and sandbox UI checks, including the isolated SSH host:
RODE_TEST_BINARY="$PWD/target/release/rode" RODE_SSH_UI=1 xvfb-run -a -s "-screen 0 1600x1100x24" bash scripts/test-ssh.sh
# Shared file/hex/Save As browser, text edits, image/PDF opens and keyboard navigation:
RODE_TEST_BINARY="$PWD/target/release/rode" RODE_FILE_BROWSER_UI=1 xvfb-run -a -s "-screen 0 1600x1100x24" bash scripts/test-ssh.sh
# Ruff/venv, monitor, Git, settings, and policy-restoration UI checks:
RODE_TEST_RUFF=/path/to/ruff RODE_TEST_BINARY="$PWD/target/release/rode" RODE_WORKBENCH_UI=1 xvfb-run -a -s "-screen 0 1600x1100x24" bash scripts/test-ssh.sh
```

Screenshots and logs go to `target/ui-smoke/`, `target/layout-ui/`, `target/project-ui/`, and `target/workbench-ui/`. Set `RODE_TEST_RUFF` for the Rust and SSH tests too, to exercise real Ruff formatting/checking. The smoke tests cover editing, saves, terminal commands, themes/preferences, local/remote folder browsing, sandbox/fallback, Git staging/commits, Python environments, and saved project policies. Namespace-isolation tests report when the host cannot run bubblewrap. These are basic regression checks, not a benchmark suite. `RODE_SMOKE_TEST=1 ./target/release/rode` provides a separate five-second launch/exit check.

`cargo test --lib workbench::git::rows` renders and scrolls mixed Git file states at narrow and wide pane widths, checking stable list/action columns, single-row alignment with long Unicode paths, and compact-menu actions bound to the correct file. The native workbench check above exercises staging and committing through the surrounding panel.

`scripts/test-file-browser-ui.py` also runs locally on its own; screenshots/logs go to `target/file-browser-ui/`. Library tests cover stale picker results, relative destinations, preserved Save As names and project-environment guards.

## README feature tour

The README embeds `assets/rode-tour.gif` and links to an H.264 MP4 and a still PNG. The 30-second tour holds captioned native screenshots rather than recording every idle frame. The GIF uses variable frame durations and a shared palette; the MP4 uses H.264, `yuv420p` and fast-start metadata. Keep the GIF below 1 MiB when refreshing it and inspect the encoded text at README size.

`scripts/capture-readme-tour.py` captures a fresh demonstration project at 1380×900 with isolated preferences and session storage. It uses the native UI harness and the existing loopback SSH fixture. Install the UI/SSH prerequisites above and provide a Python environment containing `jupyter_client`, `ipykernel` and Matplotlib. Build the current editor and matching helper before capturing:

```sh
cargo build --locked --release --bins
RODE_TEST_BINARY="$PWD/target/release/rode" \
RODE_SESSION_HELPER="$PWD/target/release/rode-sessiond" \
RODE_TEST_JUPYTER=/absolute/path/to/python \
RODE_SSH_RUN_TRANSPORT=0 LIBGL_ALWAYS_SOFTWARE=1 \
xvfb-run -a -s '-screen 0 1600x1100x24' \
  bash scripts/test-ssh.sh python3 scripts/capture-readme-tour.py
```

The capture script checks real notebook output and the terminal persistence acknowledgements, records the source revision and binary/capture hashes in `target/readme-demo/captures/capture.json`, and publishes the new capture set only after all scenes complete. `--output` selects another capture directory. It removes its temporary project and closes its owned sessions. Review the captured screens after UI changes, since interaction coordinates may need updating.

`scripts/build-readme-tour.py` lists the captions and timings. With FFmpeg (including libx264 and drawtext) and DejaVu Sans installed, encode the captures into the three assets:

```sh
python3 scripts/build-readme-tour.py target/readme-demo/captures
```

The captures use the native editor, a real Python notebook kernel, a real loopback SSH connection, active bubblewrap isolation and a real Git diff. The terminal frames show persistence being enabled. The agent frame shows provider selection without model inference. The Slurm form uses the existing deterministic scheduler fixture with an empty queue; it requests no real allocation. Keep those distinctions in the captions. Saved captures stay under ignored `target/`; do not capture personal chats, credentials or private projects. A clean checkout needs fresh captures before encoding.

After regeneration, check all three files visually, use `ffprobe` to check duration/dimensions/codec, and review README links. Re-encoding existing captures needs no application rebuild. Repository-relative GIF embedding works without uploading a GitHub attachment; the MP4 is a linked file, not an assumed inline README video player.

## Concurrent processes

`cargo test --test persistence --test concurrency` starts real subprocesses to exercise field-level preference/profile merging, external configuration edits, malformed-file preservation, lock release after a killed writer, private GUI snapshots, parallel sandboxed helpers with identical namespace PIDs, concurrent environment creation, and simultaneous local/Python-peer document saves. Existing-file checks force writers to wait on the same inode, then verify that only one buffer based on the old revision can win. Save As tests verify atomic creation without overwriting another writer. Native two-window screenshots/logs go to `target/multiprocess-ui/`.

The SSH fixture also opens two independent masters for the same alias and checks that closing one preserves the other. The project UI check (`RODE_SSH_UI=1`) exercises banner/palette disconnect, compact connection controls, dirty buffers across reconnect, local file/folder access after explicit and unexpected disconnect, and persistent versus nonpersistent terminal lifetime. Library tests also cover failed logins, stale replies, disconnected Save As and prevention of remote/local path rebinding. See [configuration and multiple windows](configuration.md) for supported filesystem behavior and limitations.

`RODE_PROJECT_UI_CASE=failure RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-project-ui.py` reproduces a real refused loopback SSH login without an SSH server. It checks retained read-only error output, Copy/paste into a saved local file, local folder browsing and a neutral host title. The default project UI run includes it. Library connection/terminal tests also check a real fast SSH exit and late PTY output after cleanup, while ensuring diagnostic views cannot send input or own/signal the old process.

## Security checks

`cargo test --test security --test files --test sandbox --test persistence --test concurrency` checks special-file rejection with subprocess deadlines, canonical home grant restrictions, controlling-terminal isolation under a real PTY, local/Python parity, protected saves and shared configuration. Run `cargo test --test sessions --test agents --test debugger` for process lifecycle checks, and `scripts/test-ssh.sh` for direct/ProxyJump transport and sandbox integration. With an installed cargo-audit, `cargo audit --file Cargo.lock` checks the current advisory database. The [security review](security.md) records reproduced findings, fixes, maintenance warnings and the scope of the audit.

## Image checks

`cargo test --test images` checks all supported codecs, alpha and EXIF orientation, GPU downsampling, corrupt/oversized input rejection, local/Python binary-protocol parity, and actual sandbox access. Library tests cover shared pixel-budget release, stale reloads, and read-only save commands. The direct/ProxyJump SSH fixture also transfers and decodes images under the active project policy. `scripts/test-images-ui.py` checks rendered PNG pixels, zoom, JPEG tabs, reload, both themes, compact layout, read-only saves, and preservation of dirty text. It can run locally on its own, or with `RODE_IMAGES_UI=1` in the SSH fixture. Screenshots/logs go to `target/images-ui/`.

## Markdown checks

Markdown checks use `cargo test --lib markdown` for unsaved-buffer rendering, source saves, source-path resolution, input limits, image URI isolation, stale-result rejection, and texture-budget release. Native local/SSH screenshots and logs go to `target/markdown-ui/`; these checks also exercise actual sandboxed image reads, view switching, undo/redo, cursor preservation, themes, and find returning to source.

## Terminal color checks

`cargo test --lib terminal` and `cargo test --test themes --test persistence` cover actual egui glyph colors for ANSI/256-color/RGB/inverse text, Bash PS1 bold/bright colors and resets across every built-in theme, exact-color mode, unchanged parser output, decorative glyphs, bounded contrast caches, theme defaults/validation, per-theme overrides, independent concurrent edits and stale explicit resets. `scripts/test-terminal-colors-ui.py` uses a real native PTY and isolated preferences to check contrast changes, ANSI swatches, theme switching, restart persistence and continued shell input. It also runs a real Bash prompt with status/Git expansions, checks bright-black host/marker and bright-blue path colors, and compares success/failure colors with xterm when installed; xterm is an optional test reference, not an application dependency. Run it on an isolated X display; screenshots/logs go to `target/terminal-colors-ui/`.

## Agent checks

`cargo test --lib composer_sends` checks Enter submission, Shift+Enter/newline paste, focus/modal guards, key repeat and IME confirmation against the native text widget. The native agent UI test sends a multiline message using Shift+Enter followed by Enter, verifies the exact backend prompt, and checks that inserting a newline alone never starts an agent.

`cargo test --lib composer` also exercises the actual bounded input widget with long multiline/Unicode/wrapped drafts, cursor following after typing and resize, independent manual wheel scrolling, a stable visible Send row, and real divider dragging across small-window/history changes. Native agent UI checks paste a long UTF-8 document through the application's clipboard, scroll and enlarge the composer, keep typing after releasing the divider, and copy/save the draft to verify exact retention without starting an agent; screenshots include the default and enlarged input.

`cargo test --test agents --test history` uses deterministic Codex app-server and ACP v1 peers without calling a model. It exercises multi-turn streaming, approvals/questions, cancellation, process-tree cleanup, protocol/output limits, actual bwrap scope/read-only access, and explicit normal-mode fallback. The isolated direct/ProxyJump SSH tests also run both agent protocols, checking working directory, outside-file visibility, approvals, cancellation, and continued use of the authenticated master after closing chat.

Native agent UI checks also open nested reasoning selectors, switch advertised levels repeatedly, verify immediate persistence, check the effort sent on the next turn and after restarting, and exercise dependent ACP model/effort submenus through acknowledged configuration requests. Agent-control checks cover paginated model catalogs, hidden models, next-turn model/effort/access changes, never-ask denial, read-only precedence, full access inside a network-restricted outer sandbox, modern ACP model/mode/boolean settings and legacy modes. Dependent settings wait for model acknowledgement; missing/rejected settings and cancellation cannot submit a queued prompt. Idle changes refresh capabilities without creating a model turn. Library parser tests bound metadata and omit invalid/unsupported selectors; persistence tests check resetting ACP overrides while another window edits models and approval preferences.

History checks cover project/provider filtering, metadata pagination without inference, bounded/ordered Codex and ACP replay, unsupported capabilities, cancelled loading, current access-policy overrides, and continuation using the resumed session. Deletion checks cover individual removal, bulk discovery beyond the 256-row browser limit, archived chats, protected open/active conversations and their Codex ancestors, scope revalidation, unsupported backends, invalid cursors, idempotent ACP removal and partial/refresh errors. Native-service checks protect a separate live controller while deleting history. Sandbox policy assertions use the prepared plan's actual `active` state rather than the requested flag; an explicit unavailable-sandbox fixture covers normal-mode fallback even on hosts where bubblewrap works. Direct/ProxyJump fixtures exercise the same flow without closing the shared SSH master; use `RODE_SSH_TEST_FILTER=agent_history_deletion bash scripts/test-ssh.sh` for the focused agent/history case. `scripts/test-agent-history-ui.py` checks per-chat confirmation/cancel, unloaded and archived bulk deletion despite a text filter, and absence of inference. Native chat UI checks also exercise the History list and restore action.

Personality checks (`cargo test --lib personality` and `cargo test --test agents --test history --test persistence`) cover unchanged task blocks, repeated hints on consecutive styled turns, process reuse, default resets after styled/restored chats, byte-for-byte execution-policy fields, approval requests, all three providers, active sandbox scope, safe preference merges and hidden replay hints. The optional real Codex loopback test checks the latest model-bound user input on three consecutive styled turns and after history restore, so a hint present only in earlier context cannot make the check pass. Current and legacy hints stay out of history titles/messages; no real inference or account is used. Native agent UI checks exercise selection/persistence before any process starts and changing tone in an existing chat. Qualitative delivery requires a real user-managed model; deterministic fixtures do not judge the voice.

To check an installed real Codex CLI without a model or account access, use:

```sh
RODE_TEST_CODEX=/absolute/path/to/codex cargo test --test codex -- --nocapture
```

This uses an isolated temporary `CODEX_HOME` and local model provider. It checks initialization, a complete streamed chat turn and replay from the agent's saved history through a loopback Responses fixture, in normal mode and with bwrap when available, without calling a real model. Both passed with the pinned native Codex `0.166.0`. Prepare it with `python3 scripts/bundle-codex.py`, then use the printed directory's `codex` executable as `RODE_TEST_CODEX`. Native screenshots/logs go to `target/agent-ui/`. Real Ollama model inference and third-party ACP executables require their own installations and are not covered by the deterministic peers.

`cargo test --test bundled` checks automatic selection, independent optional-package versions, relocated Cargo-style editor installs, XDG/default user runtime paths, unavailable architectures and installed fallback, custom overrides, changed executable rejection, streamed deployment bounds/checksums, failed-transfer cleanup, private cache/symlink checks and concurrent publication. `cargo test --lib bundled` checks search-path precedence, the extra read-only tool mount and rejection of writable, symlinked or nonregular runtime selector manifests. Selector tests explicitly set the linked target's permissions so a group-writable default cannot mask a symlink-following bug; run the selector test under both `umask 002` and `umask 022` when changing these checks. `python3 scripts/test-packaging.py` checks that core packaging never consults or downloads Codex and that a missing runtime target affects only its explicit component. It also exercises explicit per-user runtime setup with tiny cached archives: concurrent installation, verification failure preserving the old selector, XDG fallback and unsafe-path rejection. Normal commit checks use small deterministic executables, without downloading Codex. Separate optional-package validation exercises `rode --codex --version`, app-server support, static architecture and complete notices after overlaying it on an extracted editor package.

Native installer tests in `src/bundled/install.rs` use tiny in-memory archives without internet or agent execution. They cover concurrent reuse, cancellation during transfer/extraction/lock waits, archive and executable hash/size failures, extra or linked members, missing notices, unsafe root/lock/destination paths, and unchanged previous selectors on failure. Run them under both `umask 002` and `umask 022`; staging permissions must not depend on the caller's umask. The UI installer starts only on an explicit click and keeps setup separate from chat processes and credentials.

`RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-codex-install-ui.py` checks the native install/cancel/retry controls, an existing preferences directory created with umask 002, untouched Codex configuration/project files, compact layout and close-during-download cleanup. A loopback proxy rejects/holds CONNECT so this test never reaches GitHub. Set `RODE_TEST_CODEX_BUNDLE` to the actual prepared pin to additionally check successful verified reuse without downloading. To explicitly validate the native HTTPS downloader against the official asset, run `cargo test --lib official_native_download_installs_a_working_runtime_without_a_source_installer -- --ignored`; this downloads the pinned archive into a temporary test directory and checks its executable's version without an account or model.

Set `RODE_TEST_CODEX_BUNDLE` to the prepared directory when running `cargo test --test codex` to check actual automatic discovery in normal mode and a sandbox. With the same variable, `scripts/test-ssh.sh` checks deployment of the actual runtime, initialization and thread creation through both direct SSH and ProxyJump. The fixture provides private authentication/cache directories and does not use a real model or account.

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

With Go on PATH and `RODE_TEST_PRETTIER` set, run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-languages-ui.py` for actual Go format/run/build, TypeScript formatting/Problems, and saves after syntax errors. Screenshots/logs and `toolchain.json` go to `target/languages-ui/`. The standard-library-only Go fixture declares its actual minimum (Go 1.16), disables toolchain/module downloads and uses private caches; shell startup and user Go configuration cannot redirect its tools. CI installs Go, clang-format, shfmt and a pinned test Prettier; the application itself never installs those tools.

## Independent syntax themes

`cargo test --test themes --test persistence` covers full preset colors, independence from interface/terminal colors, custom scope rules, italic/underline separation, UTF-8 layout ranges, invalid/oversized definitions, cache invalidation and concurrent preference merges. Library tests check immediate independent saves, startup/CLI restore, unavailable-palette fallback and dirty-buffer preservation.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-syntax-themes-ui.py` to check all syntax choices against a fixed Carbon interface, a custom theme, unsaved source and live preference writes, menu layout and restart persistence. Native captures/logs go to `target/syntax-themes-ui/`.

## PDF checks

Install desktop Poppler tools (`pdfinfo` / `pdftoppm`). `cargo test --test pdf --lib pdf` checks real two-page rendering, portrait/landscape content, page limits and cancellation, malformed input, read-only commands, stale results, shared-budget release, native/Python binary parity and actual sandbox reads/symlink escapes. The isolated direct/ProxyJump tests also transfer PDFs under the project policy.

For native local and SSH viewing: `RODE_TEST_BINARY="$PWD/target/release/rode" RODE_PDF_UI=1 xvfb-run -a -s "-screen 0 1600x1100x24" bash scripts/test-ssh.sh`. This checks pages, wheel scrolling within/across pages, zoom, source reload preserving the selected page, failed-reload preservation, themes, read-only saves and dirty code buffers. `scripts/test-pdf-ui.py` can run locally on its own. Screenshots/logs go to `target/pdf-ui/`. The two-page vector fixture at `tests/fixtures/pages.pdf` is generated test data with colored shapes and built-in Helvetica text, requiring no PDF-generation library.

## Native terminals and Slurm

`cargo test --test sessions --test slurm` exercises native PTY persistence after views exit, in-place persistence changes shared across views, last-view cleanup for nonpersistent sessions, two simultaneous views sharing input/output, per-session cleanup, concurrent service creation, fresh caller environments, login-startup activation without reactivation on attach, and real bwrap lifetime/access checks. The native supervisor keeps bwrap's spawning thread alive; closing the request channel must not trigger `--die-with-parent`. Slurm tests use fake queue/cancel executables to check TRES counts, pending/unknown resources, local/Python-peer parity, exact allocation IDs, UID filtering, and removal of inherited cancellation overrides. They never allocate or cancel real jobs.

The interactive Slurm fixture additionally waits for an explicit grant, starts a real PTY shell with scheduler/GPU environment variables, preserves allocation metadata across rename/shared reattachment, and cancels the exact owned fixture job. Command tests check literal quoting, H/H:MM and GB conversion, bounds, and refusal to fall back when the compute sandbox fails. `RODE_SSH_TEST_FILTER=interactive_slurm scripts/test-ssh.sh` narrows the isolated SSH run to allocated shells with and without a sandbox, directly and through ProxyJump. The complete SSH run includes these cases too.

Notebook reservation tests detach the persistent progress view without ending its allocation. With `RODE_TEST_JUPYTER` set to an installed environment containing `jupyter_client` and `ipykernel`, `cargo test --test sessions --test notebooks --test slurm` additionally runs real kernels in fixture steps, checks GPU/job environment, interruption, kernel shutdown preserving the reservation, and allocation release ending a live kernel. `RODE_TEST_JUPYTER=/path/to/python RODE_SSH_TEST_FILTER=slurm_notebooks scripts/test-ssh.sh` runs kernels through direct SSH and ProxyJump and verifies expiry cleanup preserves the SSH master. The runtimes CI suite includes these checks. Mandatory stdio tests also exercise exact UID-filtered ID/name/state revalidation, stale inherited scheduler overrides, literal arguments, missing jobs and compute-policy refusal through native and Python-peer transports. No real scheduler job is submitted.

`RODE_TEST_JUPYTER=/path/to/python cargo test --test runtimes` exercises real native-service queues surviving loss of their controlling view, source-bound output retention, kernel-variable continuity, exclusive attachments, policy rejection, acknowledged persistence changes and last-view cleanup. The deterministic DAP case retains pause/stack/scope/variable state, rejects an obsolete inspection epoch and steps after reattachment. `RODE_TEST_JUPYTER=/path/to/python RODE_SSH_TEST_FILTER=persistent_slurm_runtimes scripts/test-ssh.sh` repeats kernel and debugger reattachment after dropping complete SSH connections, directly and through ProxyJump, with scheduler fixtures. Ended debugger cleanup must preserve the independent kernel. Use a current release/stripped native helper for remote tests; debug helpers containing symbols can exceed the existing 24 MiB upload limit.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-slurm-ui.py` for the allocation form, pending/active fill beside Persist, actual shell variables, compact layout and persistence/reattachment. Artifacts go to `target/slurm-ui/`. It uses installed executable fixtures and never submits a real scheduler job.

With `RODE_TEST_JUPYTER` set, `xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-slurm-notebooks-ui.py` exercises **Reserve for notebooks**, its persistent owner, selection of the running job, real cell execution with scheduler/GPU variables, enabling persistence on the existing kernel, complete desktop restart, reattachment preserving Python state, switching back to a fresh host kernel, and retention of the allocation after kernel changes. Screenshots and logs go to `target/slurm-notebook-ui/`; it uses the same scheduler fixtures. The debugger UI smoke test additionally enables persistence, restarts the desktop, reattaches the paused controller and exercises stepping and terminal input.

A legacy-protocol fixture owns a real shell and rejects newer requests. Local and direct/ProxyJump checks keep its shell state across detach/reattach while starting new terminals in the compatible service, verify opt-in persistence and combined discovery, prevent unsupported toggles from reaching the old daemon, and preserve the new terminal after ending the old one. They also check stale helper rejection, stale socket recovery, and that dropping a native bridge cannot inject an EOF keystroke into the shell. Each fixture has a private runtime; no running user service is restarted to test an upgrade.

Idle-lifetime checks start a service without a terminal and naturally exit its last shell, then verify socket cleanup and automatic restart. They keep a detached persistent shell alive beyond the five-second idle timeout, inspect the raw service protocol to verify completed records are released while that shell runs, preserve final output in open views, and check that polling or finished views cannot keep an otherwise empty service alive. The native UI check also leaves the session manager open while a terminal exits to check automatic list refresh.

Build the remote helper with `scripts/build-sessiond.sh` before running `scripts/test-ssh.sh`. For a compatible local fixture host, `RODE_TEST_SESSION_HELPER=/absolute/path/to/rode-sessiond` can select an already built helper instead. Both direct and ProxyJump tests deploy a private checksum-verified helper, start a sandboxed native terminal with a real project venv, disconnect its view/master, establish a fresh independent connection, and reattach to the same running shell. Fixtures use isolated runtime/helper paths and end only their own sessions.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-sessions-ui.py` for multiple terminals, the rightmost Persist button with theme-colored fill, on/off changes preserving the shell, compact layout, prompt-free mixed-session exit, reattachment, shared last-view lifetime across two native windows, temporary shells, and confirmed Slurm release against deterministic mocks. Screenshots/logs go to `target/sessions-ui/`. Test harnesses use private `RODE_SESSION_DIR` directories and explicitly end their own sessions on cleanup. A failed test may leave its native fixture service running: use that directory's service API to list/end its sessions; do not kill other users' services or unrelated jobs.

Linux x86_64 static helper and isolated SSH are validated. No real Slurm controller or GPU allocation was available; site-specific Slurm/authentication/logout policies require on-site validation. See [terminal limits](terminals.md#lifetime-and-limits) and [Slurm requirements](slurm.md).

## Debian package checks

See [Debian packaging](packaging.md) for prerequisites and build options. After the native release and static helper builds, run `scripts/build-deb.sh --skip-build`, then `python3 scripts/test-deb.py target/debian/rode_0.1.0-1_amd64.deb`. Build the optional runtime with `scripts/build-deb.sh --component codex` and add `--codex-package target/debian/rode-codex_0.166.0-1_amd64.deb` to test the pair. The test inspects and extracts archives, validates allowlists/licenses/checksums, checks the editor without Codex, and exercises optional discovery plus the packaged native helper in isolated temporary directories without installing into the host. **Package Debian** publishes the core artifact before starting the separate optional-runtime job; manual runs can disable the latter. Regular commit/pull-request checks retain Rust, Python, SSH and native UI validation without assembling a Debian archive.

## Extensions and binary tools

`cargo test --test binary --test extensions --test persistence` checks byte patch/undo/search, protected binary saves and permissions on both backends, ELF/PE/Mach-O fixtures, native x86 decoding, sandbox reads/writes, data-budget release, extension manifests/activation, versioned stdio adapters, lifetime cancellation and concurrent namespaced preferences. Library tests cover disabled-module activation, stale-result invalidation, avoiding eager Python probes and byte edits arriving during saves. The isolated SSH/ProxyJump fixture includes binary transport/saves and sandbox-aware adapters.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-binary-ui.py` for native grid/patch/keyboard/undo, conflicts, Save As, object views, the Extensions activity icon/shortcut, toggles/persistence, installed formatters and palette contributions. For local plus actual SSH sandbox UI: `RODE_BINARY_UI=1 RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" bash scripts/test-ssh.sh`. Screenshots/logs go to `target/binary-ui/`. Fixtures inspect generated/simple binaries; they do not execute inspected programs.

## Notebook checks

Set `RODE_TEST_JUPYTER` to an installed interpreter with `jupyter_client`, `ipykernel` and Matplotlib, then run `cargo test --test notebooks` and `scripts/test-ssh.sh`. CI uses an isolated `target/notebook-env`; it never installs these packages into a user's project. The tests cover metadata/attachments/unknown-field round trips, versioned outputs, real stateful Python execution, PNG plots, errors/Run All, interrupts, owned-process cleanup, and real bwrap operation without shared networking. Direct/ProxyJump tests also execute a notebook kernel inside its SSH sandbox while preserving the authenticated connection.

Run `RODE_TEST_JUPYTER=/absolute/interpreter RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-notebooks-ui.py` for native cells, plots, cell search/edit/Shift+Enter, metadata preservation, saves/conflicts/Save As, JSON switching and dirty-close protection. Add `RODE_NOTEBOOK_UI=1` when running the SSH fixture for remote UI coverage; `RODE_NOTEBOOK_UI_REMOTE_ONLY=1` skips the repeated local UI pass. Captures/logs go to `target/notebook-ui/`. The shared process transport retains the existing Codex/ACP regression tests.

## Marp checks

`cargo test --test marp` exercises snapshots, preserved source, export conflicts/atomic replacement and symlink protection, conversion limits, native/Python coordinator cancellation (including a detached browser-shaped child), and read-only bwrap preview without project writes. `cargo test --lib marp_view` checks source saves, dirty guards, cached previews and independent export errors; native UI tests cover focus and interaction. Full Rust checks include the existing Markdown/PDF tests after their shared-view refactoring.

For real conversion, install the test CLI into an ignored development directory and point the tests at an installed browser:

```sh
npm install --prefix target/marp-tools --ignore-scripts --no-audit --no-fund @marp-team/marp-cli@4.5.1
RODE_TEST_MARP="$PWD/target/marp-tools/node_modules/.bin/marp" RODE_TEST_MARP_BROWSER=/usr/bin/firefox cargo test --test marp
```

This checks real themed HTML, PDF, math and PowerPoint output. The editor never installs these tools for users. Without the test environment variable, deterministic CLI fixtures exercise the same protocol and rendering UI without a browser dependency.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-marp-ui.py` for native Slides/navigation, current-buffer export, source saves and focus, explicit export replacement, new-deck dirty guards and compact layouts. Add `RODE_MARP_UI=1` to the SSH fixture for local/SSH sandbox UI, or `RODE_MARP_UI_REMOTE_ONLY=1` to skip repeating the local UI pass. `RODE_TEST_MARP` and `RODE_TEST_MARP_BROWSER` also enable the real renderer for its local UI pass. Captures/logs go to `target/marp-ui/`. Direct/ProxyJump integration exercises remote rendering/export and cancellation without closing the authenticated connection.

## LaTeX checks

`cargo test --test latex --test languages --test persistence` covers TeX/BibTeX detection and highlighting, generic literal compiler arguments, main-file/root redirects, dotted/quoted/Unicode filenames, source preservation, compiler/ChkTeX diagnostics, stdin formatting, cancellation of owned children, shared output-directory locks and actual bwrap access. Raw native and Python-peer requests exercise the same behavior. Library tests guard dirty sources, stale build results and context cancellation. Concurrent preference tests merge LaTeX command/argument edits across processes.

The real compilation test runs when `latexmk`, pdfLaTeX and a usable `pdflatex.fmt` are installed. `RODE_TEST_TEXMF` and `RODE_TEST_TEXFORMATS` can select an isolated development TeX tree/format directory; tests isolate TeX's writable home/config/cache paths. These overrides are test-only. No application toolchain or packages are installed automatically. Development validation also used an isolated format generated from the host's installed TeX sources.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-latex-ui.py` for dirty guards, configured format/check tools, native red/blue PDF rendering and reload, compiler errors and compact layout, normally and with bwrap. Set `RODE_LATEX_UI_REAL=1` plus a usable TeX environment to exercise real local compilation; the sandbox UI pass uses deterministic installed-tool fixtures. Add `RODE_LATEX_UI=1` to `scripts/test-ssh.sh` for SSH UI, or also set `RODE_LATEX_UI_REMOTE_ONLY=1` to skip repeating local checks. Direct/ProxyJump integration checks build/PDF transfer, lint/format, compiler errors and cancellation without losing authentication. Captures/logs go to `target/latex-ui/`.

## Encrypted-project checks

`cargo test --lib --test vaults` checks mount identity, leases, underlying mountpoint protection, initialization refusal for existing plaintext, buffer/exit protection, deferred cleanup, operation draining and password handling on native/Python peers. A Python component test uses real host file locks with a mock unmount to check atomic shared-close release without FUSE. With installed gocryptfs it also initializes a real encrypted configuration and rejects an incorrect password. If working FUSE is available, the same test performs real encrypted writes, shared-window refusal, release and locking; `/dev/fuse` is absent in the current development environment, so that mount round trip could not be validated here. CI installs gocryptfs/FUSE tools; actual kernel mount availability still depends on the runner.

FUSE access does not guarantee permission to inspect other same-account mount namespaces. In that specific case, the real-mount test requires an explicitly unlocked refusal and readable mounted content, then stops its own lease peers and normally unmounts only its fixture. Other close failures still fail the test, with both protocol replies included in the assertion. The FUSE-independent component check verifies that namespace-verification refusal retains the final view lease and that locking succeeds after the restriction is removed; the application's namespace checks remain required.

Direct/ProxyJump SSH tests check that a successful tool cannot substitute an ordinary directory for a verified encrypted mount, keep passwords out of diagnostics and preserve the authentication master after cleanup. Run `RODE_TEST_BINARY="$PWD/target/release/rode" RODE_VAULT_UI=1 xvfb-run -a -s "-screen 0 1600x1100x24" bash scripts/test-ssh.sh` for local/SSH UI, failed unlock preserving a dirty source, preference files free of the submitted password and compact layouts. Captures/logs go to `target/vaults-ui/`. The native library tests use a deterministic protocol peer for successful/failed lock UI state transitions without claiming a FUSE mount.

The vault UI fixture records the native/SSH helper's cleanup reply in `<host>-cleanup.json`. It requires normal window exit after verified cleanup. If the host specifically refuses mount-namespace inspection, it instead requires the window to remain open after Close; other cleanup errors still fail the test. This exercises the documented exit guard on restricted CI hosts without disabling namespace checks or claiming an unverified lock. A deterministic UI-state test also checks that a failed unlock and cleanup refusal received together preserve both diagnostics.

## Debugger checks

`cargo test --test debugger` checks DAP byte framing/Unicode, launch/configuration ordering with deferred replies, verified/unverified/conditional breakpoints, stepping, scopes/children/watches, stale evaluations, real interactive PTYs, unread-input responsiveness and owned detached-child cleanup on the native and Python coordinators, normally and under bwrap. Attach requests detach; sandboxed attach is rejected. Library checks cover source guards and extension disable. The shared preference tests merge independent debugger and other tool edits across processes.

Set `RODE_TEST_DEBUGPY` to an installed interpreter with debugpy (or reuse `RODE_TEST_JUPYTER`) for real Python breakpoint/step/evaluate/stdin checks. Set `RODE_TEST_LLDB` to an installed `lldb-dap` executable for real C/C++/Rust compilation and debugging across both coordinators and sandbox modes. Test compilers and adapter libraries must already be available to the test host. RODE installs none of them. Development validation used debugpy 1.8.22 and LLDB 19.1.7, including native interactive launch in the shared sandbox namespace.

`RODE_TEST_DEBUGPY=/path/to/python RODE_TEST_LLDB=/path/to/lldb-dap scripts/test-ssh.sh` also exercises the deterministic peer, real debugpy and native C stepping inside direct and ProxyJump SSH projects, then verifies the authenticated master still works. `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-debugger-ui.py` checks native local/sandbox controls, dirty-source launch protection, breakpoints, step shortcuts, terminal input after a multi-megabyte output burst, themes and compact layout. For remote UI use `RODE_DEBUGGER_UI=1 RODE_DEBUGGER_UI_REMOTE_ONLY=1` with the SSH fixture. Captures/logs go to `target/debugger-ui/`.

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

`cargo test --lib app::lifecycle` also checks closing while a save acknowledgement is still in flight. A successful save completes the pending quit only when every buffer is clean; failed saves, newer edits, cancelled quits and encrypted-project cleanup retain their guards. The native editor smoke test waits for the formatted write instead of assuming a fixed formatter latency. SSH agent and debugger cleanup checks identify their marked child in the runner's PID namespace before shutdown, so sandbox-local PIDs cannot be mistaken for unrelated host processes. The installer UI fixture runs a private copy of the editor to avoid discovering a development bundle beside the original executable.

A display-free render sample is available for manual comparison:

```sh
cargo test --release --lib hex_frame_profile -- --ignored --nocapture
```

On the development machine, a dirty 8 MiB binary differing at its final byte took **0.798 ms median / 0.990 ms p95** per unchanged editor frame before the review, and **0.625 ms median / 0.642 ms p95** after moving full-buffer dirty comparisons out of unchanged frames. Each sample used 100 warmed egui frames with a virtualized hex grid. This is a synthetic render-only measurement; it does not measure total application, GPU rendering, SSH or tool execution time and is not a general speed guarantee.

## Keyboard navigation checks

`cargo test --lib navigation` checks default bindings, exact modifiers, leader input consumption, list selection, invalid-edit rollback, remapping/reload and per-binding concurrent preference merges. On an isolated X display, `RODE_TEST_BINARY=target/release/rode python3 scripts/test-navigation-ui.py` exercises the workflow entirely with keyboard events: explorer expansion/opening, file suggestions, pane focus, terminal creation/cycling, shell Tab/Ctrl+K, dialog return, resizing/maximization and UI binding persistence. Screenshots/logs are in `target/navigation-ui/`. Set `RODE_NAVIGATION_UI=1` when running `scripts/test-ssh.sh` to repeat the keyboard workflow over real SSH. Run the terminal-key, layout and project UI regressions after changes to focus routing. Window-manager grabs cannot be reproduced by the isolated X test; defaults deliberately contain no Alt.

Style regressions use `cargo test --lib navigation`: preset validation/isolation, native Emacs mark/kill/yank, Unicode motions, Vim operators/counts/visual selection, shared native undo, batched insert/Escape ordering and cancellation of queued input after context changes. `scripts/test-shortcut-styles-ui.py` checks actual native Ctrl+X clipboard events, both source keymaps, safe file saves, shell Ctrl+W, pane switching and immediate style persistence. Use `RODE_SHORTCUT_STYLES_UI=1` in the isolated SSH fixture for the same workflow over SSH. Captures/logs go to `target/shortcut-styles-ui/` and `target/shortcut-styles-ssh-ui/`.

## Interface scaling checks

`cargo test --lib interface_scale` simulates 1x/2x monitor DPI, validates saved scale independently of stale framework state, rejects malformed settings and preserves dirty buffers, editor font size and themes. `cargo test --test persistence` checks explicit resets from stale windows without reverting another window's font choice. Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 3000x2000x24" python3 scripts/test-scale-ui.py` for real native font/GUI separation, measured menu resizing, percentage presets applied only after pointer release, 5% Smaller/Larger actions, Ctrl+0, dirty-buffer preservation and restart at forced X11 1x/2x DPI. The isolated X display should provide at least 3000×2000 physical pixels for the 2x window; screenshots/logs go to `target/scale-ui/`. Actual display-server DPI detection on other computers still depends on their OS/monitor configuration.

### Background agent checks

`cargo test --test background_agents --test sessions --test agents --test history` uses real native services with deterministic Codex/local-model/ACP peers. It checks work completed with no view, original PID/conversation reattachment, in-place persistence on/off, last-view cleanup, pending permissions offline, stale answers, policy mismatches and automatic service shutdown. No account or inference is used. `scripts/test-agent-persistence-ui.py` toggles Persist through the native button, closes/reopens RODE, attaches through Running, continues the same backend and switches persistence off again. Screenshots/logs are in `target/agent-persistence-ui/`.

The ignored `background_agents_survive_ssh_disconnect_and_keep_their_sandbox` check in `tests/ssh_roundtrip.rs` runs through `scripts/test-ssh.sh`. It covers direct/ProxyJump SSH, normal/actual bwrap operation, work while disconnected, fresh-master reattachment and explicit End. Use `RODE_SSH_TEST_FILTER=background_agents_survive_ssh scripts/test-ssh.sh` for that check alone; build the standalone helper first as described above.

### Complete Codex runtimes

The pinned Codex release needs `codex-code-mode-host` and `codex-exec-server` alongside `codex`. Runtime tests check multi-executable transfers, executable permissions, incomplete/corrupt transfers, distinct cache identities for main-only versus complete deployments, and explicit repair that preserves the original main binary and previous selector on download failure. `python3 scripts/test-packaging.py` exercises the same missing-helper repair through the preparation script.

With `RODE_TEST_CODEX` pointing to a complete prepared runtime, the existing loopback Responses test also enables code mode in its isolated configuration and returns a deterministic tool call. It must create a file with a real shell command and return the output to the next model request, in normal mode and an available bwrap sandbox. This uses no account or model and catches runtime layouts that pass initialization/chat-only checks but cannot execute tools. The SSH checks verify complete runtime deployment over direct and ProxyJump connections.
