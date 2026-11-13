# Development

For a shared local/CI toolchain, install [mise](mise.md#developing-rode-with-mise), then run `mise trust`, `mise install --locked` and `mise run check`. The repository pins the tooling and exposes the same check tasks used by GitHub Actions. Direct Cargo/Python commands below remain available; install their OS dependencies separately.

Start with the [source architecture and maintenance guide](architecture.md) for module responsibilities, host boundaries and lifecycle contracts. Its [design decisions and tradeoffs](architecture.md#design-decisions-and-tradeoffs) explain why the boundaries exist and point to the owning modules' in-source rationale. Public Rust types, fields and methods have API documentation; all seven library crates warn on missing public documentation. When revisiting a design choice, update its rationale and consequences alongside the code.

Build the public documentation with `scripts/uvx.sh --from mkdocs==1.6.1 mkdocs build --strict`. Repository-relative links to files outside `docs/` must also be exported or rewritten in `scripts/docs_hook.py`; file existence alone does not make them part of the published site.

```sh
cargo fmt --all --check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items
cargo clippy --workspace --all-targets -- -D warnings
RODE_TEST_RUFF=/path/to/ruff python3 scripts/rust_tests.py
```

`mise run test` and `scripts/rust_tests.py` run all ordinary workspace targets
with a private home, XDG directories, temporary directory, session directory and
vault coordination directory for each test binary. They preserve explicit Cargo
and Rust toolchain locations and remove the development virtual environment
from fixture command lookup. Tests therefore exercise sandbox enforcement
without writing into the developer's home or relying on its Python activation.
Explicit project interpreters and `RODE_TEST_*` tool paths remain available. Inherited
native UI probe destinations are removed: Rust layout tests install their own
per-context outputs, so parallel tests cannot replace each other's snapshots.
The runner cleans up each private home after the test executable exits and keeps
its exit status; it does not filter failing tests. Extra command-line arguments
replace the default Cargo test arguments, for example
`python3 scripts/rust_tests.py --locked --profile ci -p rode --test sessions`.
Run `python3 scripts/test-rust-tests.py` to verify isolation, cleanup, argument
forwarding and exit codes. Set `RODE_HOST_HELPER` to the freshly built
`target/ci/rode-sessiond` when exercising the native-host integration cases.
The SSH fixture uses the same private-home runner for both Cargo and prebuilt
transport tests. Its remote shell shares private session/vault coordination
directories within that fixture, so parallel fixtures do not borrow a developer's
service or contend over a global vault directory.

Open `target/doc/rode/index.html` for desktop/editor services, `target/doc/rode_host_core/index.html` for shared headless services, `target/doc/rode_host_protocol/index.html` for wire contracts, `target/doc/rode_agent_core/index.html` for agent protocols, `target/doc/rode_runtime_core/index.html` for notebook/debugger controllers, and `target/doc/rode_session/index.html` for the native gateway and persistent service. Add `--open` to the documentation command to open it in a browser. CI uses the same warning-free documentation check, including intra-doc links. Comment-only changes need formatting, API/documentation builds and a consistency review; application rebuilds and native UI tests are required only when executable behavior changes.

The architecture guide's [operation reference](architecture.md#following-an-operation-through-the-code) traces the UI, worker and host-controller entry points for common changes. Internal Rust contracts appear in the private-item API build; the remaining interpreter/Jupyter adapter docstrings cover their protocol callbacks, byte limits and EOF/cleanup ownership. The [units and identity reference](architecture.md#units-identities-and-absence) explains coordinate conversions, version checks and the difference between queued work and acknowledged completion. Build/CI/publication helpers also document their command entry points; test functions describe behavior through their names and adjacent assertions.

Large desktop regression suites are adjacent to their implementations: `src/app/tests.rs`, `src/app/agents/tests.rs`, `src/app/workbench/tests.rs`, `src/app/sessions/tests.rs`, and `src/app/notebook_view/tests.rs`. Moving tests into these files preserves their module paths, so existing `cargo test` filters continue to work. Add behavior regressions to the owning domain instead of growing the application coordinator.

Collection invariants live in `src/app/documents/tests.rs`; bounded polling and
cross-generation routing regressions live in `src/app/workers/tests.rs`. Run the
root library tests after changing these owners, then exercise native tab/dirty-save
workflows and SSH disconnect/reconnect with rebuilt editor/helper artifacts.

The **2026-11-10 structure/documentation review** consolidated desktop/helper
runtime pin contracts in `runtime_cache::pins`, shared nonreaping child-exit
observation in `process::wait`, and moved active document rendering/find controls
to `app/editor_view.rs`. The view methods were preserved byte for byte apart from
module visibility. The internal documentation pass added API contracts to 74
shared items, including acknowledgement/lifetime rules, worker entry points,
host authentication state and character/byte/cell coordinates.

Validation passed **948 ordinary Rust tests** across 49 executables, with 40
declared harness/opt-in cases ignored, zero failures and none filtered out. The
run used rebuilt editor/helper binaries, explicit real-tool fixtures and required
Landlock enforcement. The native keyboard/navigation fixture also passed on an
owned authenticated X display. The fixed cache-key and retained-child-status
regressions exercise compatibility and process ownership. These checks do not
establish opt-in SSH/Bubblewrap/FUSE platform coverage; see the existing
[validation limits](security.md#validation-snapshot).

Session-control schema changes belong in
`crates/rode-session/src/protocol.rs`; keep its crate-root re-exports stable and
advance `PROTOCOL` when compatibility or required authentication changes. Frame
changes belong in `framing.rs`, which owns both socket budgets. Check exact
length boundaries, invalid/truncated prefixes, JSON kinds and the larger runtime
budget with `cargo test -p rode-session framing::tests`. Run
`cargo test -p rode-session --lib` and
`cargo test --test sessions --test background_agents --test runtimes` when
changing service ownership, directory setup or attachment routing. The existing
control tests exercise inode substitution and authentication; service integration
tests cover retained PTYs, permissions, handshake admission and runtime leases.

Process lifetime changes start at `rode_runtime_core::process::ProcessGroup`,
shared by desktop, agent and runtime transports and the transient supervisor.
Its regressions run real owned
process groups and check stop-before-spawn escalation, concurrent publication
and cancellation, isolation from independent groups, lost-ownership refusal and
retention of the leader's PID through final signalling:

```sh
cargo test -p rode-runtime-core process::group::tests
cargo test --test agents --test background_agents --test notebooks --test debugger --test sessions --test native_host
```

Use `ProcessGroup::wait` and its nonreaping `exited` poll instead of calling
`Child::wait` or `try_wait` directly for a published leader. Reaping before final
signalling releases the numeric ID and permits signals to address a reused group.
Retain each adapter's protocol
limits and captured host/sandbox when changing this shared lifetime code.
The installed `rode-sessiond` and standalone/static helper also share
`rode_session::helper_main`; early bootstrap dispatch must
remain before thread/runtime initialization. The service integration tests cover
the installed helper, and the standalone helper can be checked without GUI
dependencies using the build command below.

Synchronous binary helpers and UTF-8 project commands share
`rode_host_core::command::capture`. Its owner must keep the leader unreaped until
its final group signal and clean up on normal exit as well as errors. Run
`cargo test -p rode-host-core command::` for real-process cleanup, independent
work, exact/uncapped binary limits and oversized diagnostics. Run
`cargo test --lib pdf::tests` for the PDF adapter's capture cleanup and
`cargo test --test pdf` for actual desktop Poppler tools. Run
`cargo test --test project command_` for the project runner checks and
`cargo test --test security` for the real-PTY harness and file-security checks.
LaTeX, Marp and extension suites cover cancellation/renderer cleanup; terminal,
agent and runtime integration suites check that retained service jobs keep their
separate lifetime. Native-host cases require an explicit `RODE_HOST_HELPER`
pointing to the freshly built helper; a passing invocation without that fixture
can leave gateway cases unexercised.

The argument-packing unit tests share a checked-in executable shell fixture for
stdio and PTY launches. Generating that executable during parallel tests lets a
concurrent fork retain a writable descriptor until exec, causing `ETXTBSY` even
after the writing thread closes it. Keep the executable static and each test's
argument log private; literal policy, stdin and independent-offset assertions
still apply to both launch paths.

Validate the ordinary lane through `mise run test` as well as focused Cargo
commands. The shared deterministic agent peers use `/usr/bin/python3 -I`, the
system runtime covered by fixture sandbox policy; a Python selected through
mise/venv `PATH` may live outside those grants. This choice applies to scripted
peers, while real interpreter/Jupyter checks retain their explicit tool paths.
The mise task preserves an explicit `RODE_TEST_RUFF`, including CI's staged copy,
and discovers Ruff through `PATH` only when no override is set.

The real-mise trust fixture removes inherited `MISE_TRUSTED_CONFIG_PATHS` from
its child commands. [mise-action's defaults](https://github.com/jdx/mise-action/blob/v5/src/index.ts)
trust the checkout by prefix, which otherwise also trusts temporary projects
under `target/`. Isolating mise's data/config directories alone does not establish
an untrusted starting state. Keep the initial refusal, explicit Trust and
successful later inspection/task assertions together.

The transport owner and synchronous capture guard share nonreaping kernel
observation in `rode_runtime_core::process::wait`. Keep blocking observation,
polling, interruption retries and retained wait status there; each owner still
invalidates its own signals. Run `cargo test -p rode-runtime-core process::wait`
and the process/capture checks above when changing it. A cached `Child::wait`
result cannot substitute for ownership verified by the kernel.

Runtime pins belong in `rode_host_core::runtime_cache::pins`. Desktop release
selection and host upload/cache handling must use the shared target validation,
artifact ordering and `cache_key`; adding a local copy can make the desktop
reject a cache path produced by its own helper. Run
`cargo test -p rode-host-core runtime_cache::pins`,
`cargo test --lib bundled::` and `cargo test --test bundled` as separate
invocations to exercise the shared contract, installer units and host deployment.
Single- and multi-executable targets use the same ordered artifact-metadata
hash. The fixed cache-key fixture checks agreement between desktop and helper.

Background Git status requests use `--no-optional-locks` in the shared project
service. Git's optional stat-cache refresh can otherwise race an external commit
for `index.lock`; see [Git's background-refresh contract](https://git-scm.com/docs/git-status#_background_refresh).
`cargo test --test project background_git_status` checks unchanged index bytes,
status during an external lock, and continued refusal of a conflicting writer
on both desktop and headless paths. The HTTPS Git UI fixture also waits for an
enabled post-submit control before creating its push commit: publication of a
remote-tracking ref alone does not acknowledge the desktop's Fetch completion
and subsequent status refresh. Keep that acknowledgement separate from the
repository/server-state checks and credential-lifetime assertions.

Active document rendering and find/replace live in `src/app/editor_view.rs`.
Application/project lifecycles remain in `app.rs` and its lifecycle modules;
native source-edit commands remain under `app/navigation`. Moving a view must
preserve document IDs, UTF-8 byte/character conversions, undo state and buffer
version updates. Existing application/navigation regressions exercise these
contracts; `scripts/test-navigation-ui.py` checks actual keyboard/pane behavior
on an isolated authenticated display.

`cargo test --lib saved_connection` checks Explorer profile context menus,
dragging in both directions, Escape cancellation, ordinary clicks and failed
write acknowledgements without opening windows or connecting to a host.
`cargo test --lib saved_profile` checks persisted ordering, removal, stale
deletion/save protection, unknown fields and malformed storage.
`cargo test --test persistence` includes real concurrent profile edits,
additions, removals and reordering from separate processes.

Use the [release checklist](release.md) before tagging a version. `mise run check` runs lint and ordinary Rust tests; it does not establish native UI, real-tool, sandbox or package coverage on its own.

The project settings regressions exercise category navigation, retained drafts,
isolated text-field undo, validation, category-scoped saves/discards and direct LaTeX/Marp links. The tool-page
layout matrix renders every settings category across compact/detached widths,
themes and interface scales. The native workbench fixture uses reported control
geometry for settings navigation and verifies discarded/saved edits and persistence.

Headless form regressions compare painted label and input centres across all
built-in themes and interface scales, including wrapped labels, numeric inputs,
the SSH dialog's compact layout and runtime version columns. Theme tests compare
the actual painted colours of empty hints and identical entered values; changing
themes must keep hints muted without changing field contents. These checks run
in `cargo test -p rode --lib` without opening a window.

Directory-only locks, permission handles and durability syncs must use
`rode_host_core::locking::open_directory`. A prior `is_dir`, `canonicalize` or
`create_dir_all` does not protect a subsequent plain open from replacement by a
FIFO. The shared guard preserves directory symlinks; private runtime paths still
need their own ownership/permission and no-follow rules. Run
`cargo test -p rode-host-core locking::tests` for directory substitution,
private-file metadata and private-key checks, plus the affected LaTeX,
persistence, bundle or encrypted-project integration tests.

## Process launch changes

New and changed launchers must follow the [compact process argument contract](architecture.md#compact-process-arguments). Review the complete local or remote process chain, including intermediate shell and sandbox arguments. Keep generated payloads off `argv`; reuse the shared handoff mechanisms and retain the full policy internally. Add regressions for the affected transport, input framing, descriptor/file lifetime and enforcement, including actual process-argument inspection. Document any external-tool constraint that requires an inline payload at the adapter that owns it.

## Reverse tunnel validation

`cargo test -p rode-session tunnels` runs a private Rust relay and compute-side
TCP fixture: explicit host-key trust, changed-key refusal before credentials,
real byte forwarding, unexpected-channel denial, idempotent starts, relay-port
conflicts, forced-disconnect recovery, credential attention, nonblocking identity
validation and scoped shutdown.
Credential regressions require the resolved identity/agent path, the corrective
UI field and the original failure in errors, without private-key contents.
Listener regressions occupy the requested port and refuse forwarding by policy,
require an honest refusal diagnostic with the exact relay-side check, and verify
automatic recovery with stale errors cleared. Transport failures must not be
labelled as listener refusals; diagnostics retain their cause within the existing
size limit. Duplicate retained listeners identify the owning tunnel and endpoint.
It needs no installed SSH tools or display server.

`cargo test --test ssh_client tunnel_return_route` checks a real native jump
connection to an otherwise unreachable compute alias, retaining its original
known-hosts identity and rejecting a changed compute key before authentication.
`cargo test --test sessions retained_tunnels` checks that list-only discovery does
not start a service, tunnel state survives both terminal End and last-view
cleanup, idle timeout does not kill it, and shutdown requires explicit Stop.
`cargo test --test sessions local_tunnels` exercises local discovery/start/list/stop
from an empty directory with no SSH executable on `PATH`, using a private service
runtime. Headless tunnel UI tests cover welcome-screen access, local host scope,
copied return-connection details and refusal to fall back from a disconnected SSH
workspace. Local controls do not depend on project readiness or sandbox settings.
Endpoint rendering checks show the tunnel listener port and named compute
destination, without confusing the listener with the relay's SSH login port.
They include IPv6 and rejection of service replies without hostname metadata. The service
fixture checks that new replies report the owning hostname.
The local-service fixture also checks that a missing helper override names the
selected executable and `RODE_SESSION_HELPER` while preserving the OS cause.
`cargo test --lib tunnel` covers saved profile arguments, UI state/layout and
late results after a workspace change or dialog dismissal. The tunnel-dialog
layout matrix checks both tabs across compact/short viewports, interface scales
and themes, with long relay names and retained tunnels. Pointer/keyboard tests
check draft retention and editing during a pending status response; scroll checks
keep navigation and Start/Close actions visible. These tests open no native window.
They also cover long multiline diagnostics and copying request/tunnel errors
without dispatching host work.

The service embeds russh and needs Rust 1.89. Rebuild the matching helper
alongside the editor; `rode_session::PROTOCOL` defines the required wire
contract, and mismatched services are rejected. Validate
the static musl helper against the existing 24 MiB deployment limit. Native UI
checks must use the isolated headless display workflow below.

The patched SSH sources in `vendor/russh` and `vendor/ssh-key` use `rsa >=0.10.0`
and `pkcs1 >=0.8.0`, with the RSA `pkcs1v15` feature enabled for SSH signatures.
Edit the normalized manifests for the local patch; retain `Cargo.toml.orig`
unchanged as upstream provenance and record every adaptation in `PATCH.md`.
The adapter unwraps the
validated prime wrappers when exporting SSH key components, and reuses the
RSA library's PKCS#1 decoder for PKCS#8 payloads. Do not restore the earlier
RSA prerelease compatibility pins: RustSec marks those versions affected by
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html).
`cargo test --test ssh_client rsa_private_key_formats` checks encrypted/plain
key round trips, signatures and rejection of changed messages/wrong passwords.
Vendored SSH sources omit upstream serialized private-key fixtures, their
dependent tests and embedded-key documentation examples. The normalized
`ssh-key` manifest omits the corresponding integration-test targets; the
original manifest remains an unchanged provenance record. Run
`cargo test --test ssh_client generated_openssh_private_keys` for generated
Ed25519, ECDSA and RSA OpenSSH key round trips, password rejection and signatures
without committed key files.

`cargo test -p rode-host-core coordinator::tests` covers shared tool-helper
request limits and preservation of the following cancellation stream.
`cargo test -p rode-session attachment::tests` covers shared attachment command
limits, typed decoding, EOF and write failure. The LaTeX, Marp, extensions,
background-agent and runtime integration suites cover their separate execution
policies and attachment lifetimes.

`cargo test --test sessions unauthenticated_connections` fills the service's
pending-authentication budget, checks immediate rejection of excess peers and
verifies that retained work and authenticated control survive once a slot frees.

Socket fixtures need a short, writable temporary directory. For example, create
`target/test-tmp` and set `TMPDIR="$PWD/target/test-tmp"` for Cargo tests. A deeply
nested inherited temporary path can exceed Unix socket pathname limits before
the service can start.

## Native SSH validation

Native client regressions stream concurrent uploads and echo traffic through
small SSH windows, checking complete digests and payloads after repeated
flow-control pauses. Session-control regressions distinguish a disconnected
daemon's disappearing capability from a live daemon's authentication denial.

The default transport is embedded russh. `cargo test --test ssh_client` uses an
in-process Rust server and real login PTYs to check trust confirmation, changed
host refusal, password/encrypted-key/MFA input, early command exit, and denial of
control access from local Landlock/bubblewrap sandboxes. It needs no OpenSSH tools.
It also checks supported authentication with GSSAPI defaults enabled, preservation
of disabled methods, and a clear configuration diagnostic for Kerberos-only servers.
Multi-key servers exercise known-host algorithm preference, RSA SHA-2 signatures
and rejection of a changed preferred key without accepting another key type.

Launch regressions check private, consumed SSH profile files and streamed bootstrap
scripts with binary stdin, literal arguments and truncated input. Shared host tests
exercise compact bubblewrap arguments through ordinary pipes and real PTYs,
including repeated spawns with independent file offsets. Sandbox, session and agent
integration tests retain their actual isolation and persistence checks; use
`RODE_TEST_REQUIRE_SANDBOX=1` when namespace support is required.

`python3 scripts/test-kerberos.py` creates a private MIT Kerberos realm and two
loopback SSH servers for `tests/ssh_kerberos.rs`. It requires development/test
installations of `krb5-kdc`, `krb5-admin-server`, `krb5-user` and `openssh-server`;
it does not install tools or modify the user's SSH/Kerberos files. Tests exercise
Kerberos-only direct/ProxyJump login with an empty client `PATH`, channel reuse,
explicit credential delegation, expired/missing tickets with and without key
fallback, disabled Kerberos, host verification and Kerberos-plus-public-key MFA.
The fast SSH CI job runs this fixture using its prebuilt test binary.
The normal SSH unit suite covers absent-library errors and malformed/bounded
GSSAPI exchanges without needing a KDC. The runtime library is loaded dynamically,
so neither building RODE nor ordinary key/password SSH requires Kerberos headers
or libraries. Only Linux MIT Kerberos is covered by the real-realm fixture.

The `rust_client_works_without_client_path_and_preserves_channel_status` test in
the isolated SSH suite runs a child with an empty `PATH`, exercising direct and
ProxyJump connections, file revisions, concurrent channels and large stdin.
The suite also checks the explicit OpenSSH backend, agent and X11 forwarding.
`RODE_SSH_TEST_FILTER=x11_forwarding_protocol bash scripts/test-ssh.sh` checks
authenticated X11 protocol traffic through private TCP display endpoints without
Xvfb or a desktop. It covers both SSH backends, direct and ProxyJump connections,
explicit trust overrides, zero-timeout forwarding after a deliberate delay and
native terminal reattachment. The separate real-X-server test still checks
`xdpyinfo` and untrusted authorization generation under Xvfb.
Agent forwarding checks destination-restricted keys against a real private
OpenSSH agent: a key restricted to another host must be absent from forwarded
channels, Git and native terminal listings, on direct and ProxyJump connections.

The private sshd fixture probes remote PTY availability with OpenSSH before testing
RODE. Unprivileged/container sshd instances may lack login-record permissions;
set `RODE_TEST_REQUIRE_SSH_PTY=1` to make that limitation fatal. Native session
PTYs and binary attachment streams remain exercised regardless. The fixture
uses a private umask so caller defaults cannot create unsafe runtime ancestors.
`RODE_TEST_SSH_CLIENT` identifies the actual editor executable for relocated
Cargo test harnesses; it is a test override, not automatic executable discovery.

Build the headless helper independently of desktop dependencies, then test it explicitly:

```sh
cargo build -p rode-session --features standalone --bin rode-sessiond-standalone --release
export RODE_HOST_HELPER="$PWD/target/release/rode-sessiond-standalone"
export RODE_TEST_SESSION_HELPER="$RODE_HOST_HELPER"
RODE_TEST_REQUIRE_SANDBOX=1 RODE_TEST_REQUIRE_LANDLOCK=1 cargo test --test native_host
bash scripts/test-ssh.sh
RODE_SSH_TEST_NO_PYTHON=1 RODE_SSH_TEST_FILTER=native_generic_operations bash scripts/test-ssh.sh
```

Use the static helper from `cargo xtask build-helper` for the release/platform matrix. Set the real-tool fixture variables documented below for Jupyter/debugger integration; an optional-tool skip is not coverage of that feature. The no-Python fixture removes Python from its remote command path and exercises files, Git, native Landlock, mise process launches and persistent terminal creation through direct SSH and ProxyJump. `native_host` additionally checks installed-helper path safety, on-host snapshot publication, metadata/live-handshake agreement, protocol rejection, concurrent replies, revisions, connection identity, sandbox denial, EOF cancellation, runtime-cache integrity and the compute-launch boundary with a fake scheduler. The SSH test `preinstalled_helpers_are_discovered_pinned_and_version_checked_over_ssh` covers explicit paths, automatic discovery, direct/ProxyJump transport, version/capability rejection, metadata size limits and deployment fallback. It removes desktop artifact overrides only in a child process with its own packaged helper, checking that explicit incompatible paths still fail when a fallback artifact is available. CI supplies the native helper for all remote suites.

Shared embedded Python feature adapters and JSON policy/catalogue data live in `crates/rode-host-core/src/`. Ruff checks the whole repository, including these files. Keep a single canonical source for assets shared by desktop and helper.

`crates/rode-bwrap` builds pinned bubblewrap and libcap C sources into the native helper, using a dedicated `--bwrap-launch` process when the host lacks system `bwrap`. Linux builds need a C compiler and Linux UAPI headers; native static builds need `musl-tools` (or a configured target C compiler for cross builds). No system libcap development package or runtime download is required. Source provenance, update rules and licenses live with the crate. The missing-PATH sandbox regression requires actual bundled enforcement in the sandbox CI job; ordinary hosts also cover visible namespace-denial fallback and rejection of a broken system executable without retrying. Fingerprint tests cover bundled-helper relocation.

## CI tiers and timings

The **Check** workflow runs on pull requests and pushes to `main`, avoiding duplicate branch-push and PR runs. New commits cancel superseded PR/main checks. Runners are pinned to Ubuntu 24.04, and routine Rust tests and the editor share the `ci` Cargo profile (development optimisation without debug information or release LTO). The editor, session daemon, static SSH helper and selected integration-test executables are built once and transferred together, preserving executable permissions.

| Tier | Coverage | When |
| --- | --- | --- |
| Fast | Formatting, linting, API docs, packaging-script checks, Rust tests, editor/workspace, keyboard input, multiwindow preferences, agents/attachments/queues/persistence, terminal sessions/Slurm, installer, direct/ProxyJump SSH and failed-login diagnostics | Every PR and main push |
| Extended | Environment/language tools/layout/themes, local and remote document viewers, real LLDB/Jupyter/formatters, and required sandbox enforcement | Main, nightly, full manual runs, or relevant PR changes |
| Release | Optimised Debian package and extracted-package GUI validation | Nightly/full manual checks, plus the existing version-tag/manual package workflow |

Fast UI checks run in four isolated jobs: `workspace`, `agents`, `terminals` and `ssh`. Extended jobs are `tools`, `documents`, `remote`, `runtimes` and `sandbox`. Each job has its own X display, clipboard and session directories; tests inside a job run sequentially. Specialist dependencies are installed only in their owning jobs. The `runtimes` job explicitly enables installed-tool integration tests that the ordinary Rust run may skip. SSH transport tests run once in the fast SSH job and once with those additional real tools in the runtime job; remote viewer cases share a private sshd without rerunning transport tests per viewer.

`scripts/ci.py` owns the suite manifest and PR selection. Changes under `src/`, `crates/`, `tests/` or `.github/`, Cargo changes, and shared harness changes select every extended job. Individual UI-script changes select their associated extended jobs; documentation-only PRs keep the fast checks. Add the **full-ci** PR label to force the complete extended tier. **Run workflow → full** also includes release packaging; disabling full runs only the fast tier. The nightly check runs at 03:23 UTC. The existing required **linux** check aggregates the fast jobs and reports independently of extended results. Repositories that require extended coverage before merging should also require the corresponding extended checks in branch protection.

The sandbox job adjusts the unprivileged-user-namespace AppArmor setting only on its disposable GitHub runner. `RODE_TEST_REQUIRE_SANDBOX=1` requires both desktop and headless project preparation to activate the real sandbox; unavailable sandboxing fails that job. Ordinary jobs retain explicit fallback coverage and print unavailable-host diagnostics.

Terminal attachment regressions include shell startup diagnostics before the first frame, bounded startup output and visible early-EOF failures. The no-Python SSH fixture uses a private POSIX shell without login profiles; its sandbox grants that exact script read-only, so the host's personal or system profiles cannot add interpreter dependencies or consume test input. Runtime command regressions exercise malformed argv before host resolution, rather than only testing an already running service.

The same suite includes `landlock`. On a host with Landlock ABI 4 or newer, run `RODE_TEST_REQUIRE_LANDLOCK=1 cargo test --test landlock --test agents --test sessions` to require actual enforcement. Ordinary CI reports availability skips when Landlock is disabled or the host ABI is older than 4. Ruleset unit tests cover ABI 4/5/6 and forward-compatible masks/layouts; session tests also reject unauthenticated control and protect the host capability even with a runtime-directory grant. `RODE_TEST_REQUIRE_LANDLOCK=1 RODE_SSH_TEST_FILTER=landlock_file bash scripts/test-ssh.sh` checks the real direct/ProxyJump path. Direct helper tests use `tests/support/native.rs` to exercise the same services through a separate headless process.

Every test command has a deadline and writes its log and elapsed time under `target/ci-results/<suite>/`. Timings also appear in the Actions job summary. Long console output retains both the beginning and end. Failed Rust runs also include a bounded excerpt of the libtest failure sections, so assertions in the omitted middle remain visible; the artifact retains the complete log. Job limits bound setup/build hangs; failure logs and screenshots are uploaded even after test failures, with separate artifact names per job. Tests are not automatically retried. Nested suite output keeps its full raw log, while child group markers are removed from the console replay so failure tracebacks remain visible in the parent Actions group. Use the recorded timings to rebalance jobs before tightening budgets; a sub-ten-minute fast tier is a target, not an assumed runner speed.

Each command also writes a neighbouring JSON context file containing its literal argv, shell-quoted invocation, working directory, exit code, deadline, Python/platform identity and an explicit allowlist of runner/fixture settings. Arbitrary environment variables and project values are excluded. Failure summaries include the command to reproduce from the repository root; use the same pinned toolchain, required external tools and fixture setup. For a remote subgroup, start its SSH fixture through the parent suite rather than reusing a vanished temporary host configuration. Read the failing command's log and context, not only the aggregate `linux`/`fast-ui` result.

Real LLDB regressions report language, desktop/headless transport, requested sandbox and the operation being awaited. The tiny fixtures disable eager symbol preloading and external system-symbol lookup so installed shared-library debug-info packages do not dominate startup. Local C/C++/Rust symbols, breakpoints, stepping and expression evaluation remain required; the 25-second operation deadline is unchanged. Ordinary debugger defaults are unchanged. See LLDB's [symbol settings](https://lldb.llvm.org/use/settings.html#target-preload-symbols-boolean).

UI fixtures can opt into `RODE_UI_TEST_STATE` to locate named controls in the current rendered layout, including menus and SSH profile fields. They still click with native X11 input and verify persisted settings, process activity and real file effects; snapshots contain control geometry, availability, final focus, composer emptiness and the last native edit's frame number, without chat or document text. Focus/edit barriers keep dependent actions such as New file → Paste → Save As in separate processed frames on busy runners. The chat draft regression waits for an empty composer after cleanup, copies each draft through the real clipboard before sending, and requires an enabled Send control before pressing Enter. A missed focus or incomplete edit therefore fails before submitting an unintended prompt; exact expected/actual fixture text is retained on failure. Prompt-recall checks wait for an unselected caret at the beginning of the composer and for the accepted turn to finish, then verify each recalled prompt through Copy before editing or navigating again. Headless composer tests deliberately batch arrows and edits into one frame to catch lost input independently of runner speed. On failure, the native agent fixture also retains its backend request log and submitted prompts. Use a separate snapshot path for each process in multi-window tests. Prefer these controls over fixed menu coordinates or counts of Tab presses, which change when forms gain fields or sandbox options. Test log names include the script, for example `02-test-agent-file-only-ui.log`, so the failing check is identifiable in the job summary.

The shared `NativeUI` driver acknowledges hover before either mouse button,
checks complete edits through native Copy, and keeps keyboard actions tied to a
focused named control. Callers then require the actual effect: modal closure,
host file contents or the corresponding service reply. Explorer menus and delete
confirmations include their target path in the read-only probe, so a directory
refresh cannot silently redirect the test to a different row. Input is sent once;
effect waits tolerate a missing file while it is being created, and still fail
on permission errors or malformed data. `scripts/test-ui-harness.py` exercises
row movement, focus, incomplete snapshots and literal paths.

The SSH smoke fixtures use `fixture_directory` and `fixture_environment` for
canonical project paths and private HOME/XDG/session/vault state. Explicit tools
and SSH settings remain inherited. Other standard UI fixtures reuse the same
canonical directory helper. Resolve temporary-directory aliases before
constructing path-based control names; the host canonicalizes the same paths.
These data helpers live in `scripts/fixture_support.py` and are re-exported by
the UI harness. `python3 scripts/test-fixture-support.py` runs their filesystem,
launcher and RON regressions without a display in `lint:ci`.

Native UI, Rust-runner and SSH fixtures share temporary-root selection:
`RODE_TEST_TMPDIR` takes precedence, followed by Python's standard temporary
directory selection (`TMPDIR`, `TEMP`, `TMP`, then platform defaults). The
chosen parent must exist and be writable; an invalid explicit override fails
without silently moving fixture state elsewhere. Paths are canonicalized before
host requests and control names are constructed. Keep a configured parent short
enough for nested Unix sockets. The Rust runner retains its compact `rt-` prefix;
stress tests add awkward names to individual project fixtures instead.

Smoke, keyboard navigation, workspace, attachment/reattachment and environment
flows acknowledge active documents, complete queries, focused input, finished
file operations and resized layouts before dependent actions. The shared theme
helper waits for the complete matching command; theme, prompt-color and tab-marker
checks poll the painted result. Screenshot capture supplies no readiness delay.
Physical key, drag and held-button pacing and explicit idle measurement intervals remain;
they must not substitute for a readiness acknowledgement. Keyboard-only tests
observe focus without clicking a control to repair it.

`write_python_launcher` quotes both interpreter and script paths, including
spaces and apostrophes. Whole preference-map checks use `read_preferences`;
single-field checks decode the outer RON string with `decode_ron_string` before
parsing embedded JSON. RON's apostrophe and Unicode escapes are not JSON string
escapes. Missing preference storage is pending; malformed storage fails explicitly.
Project readiness checks reject both pending preparation and offline SSH candidate
titles before sending dependent input.

The owned headless launcher and CI display use Xvfb's `-noreset`: transient
readiness probes and gaps between cases must not reset authentication/display
state when the final client disconnects. The launcher still stops its owned
display process group at the end of the invocation.

`scripts/ssh_fixture.py` owns shell and OpenSSH configuration quoting. The SSH
fixture retains a directory descriptor and passes its `/proc` alias to OpenSSH,
whose generated ProxyJump command otherwise mishandles metacharacters in its
config filename. The descriptor belongs to that fixture and expires with it.
`python3 scripts/test-ssh-fixture.py` checks literal exports, Xauth arguments and
config boundaries without an account, daemon or display; it runs in `lint:ci`.
The project disconnect check uses the current window's reported SSH master
endpoint and verifies its peer credentials before stopping that fixture master.
Connection runtime directories use compact unique names to leave room for Unix
sockets beneath private-home temporary roots.

The ignored
`native_proxyjump_uploads_cross_channel_windows_without_relying_on_a_warm_helper_cache`
regression uploads and echoes 8 MiB without deploying a helper, so another test
warming the remote helper cache cannot hide a cold-upload deadlock. Run it with
`RODE_SSH_TEST_FILTER=native_proxyjump_uploads bash scripts/test-ssh.sh`.

The SSH lifetime regressions cancel a master during stalled authentication and
after connection, while requiring an independent master's channel to remain
usable. Their owner subprocess starts the master on a short-lived worker thread
so thread exit cannot masquerade as process exit. Linux pidfds pin only fixture
processes for failure cleanup; unsupported hosts record a `pidfd` capability skip.
Run the connected case with
`RODE_SSH_TEST_FILTER=owner_exit_closes_only_its_authenticated_master bash scripts/test-ssh.sh`.

Agent Markdown regressions cover column alignment with proportional and code
fonts, empty cells, escaped pipes, Unicode, formulas, multiple tables and streamed
updates. They assert viewport containment, horizontal scrolling and native
selection/copy across blocks. `python3 scripts/test-agent-markdown-ui.py` streams
a deterministic mixed Markdown reply, then checks the real clipboard against a
fixture at wide and narrow native window sizes; it runs in the `agents` CI group.
The table layout reuses the message parser and selectable buffer, with measured
column spacing and cached paragraph layouts so prose still wraps independently.

The main agent fixture uses named controls for provider/settings changes, permission replies, Stop and history navigation. It verifies complete setting values, finished host work and the rendered window size before dependent input. Its explicit sleeps only poll bounded conditions or pace physical drag/double-click gestures; they do not acknowledge asynchronous work. Fixture RPC readers accept only newline-terminated JSON records, so an in-progress write cannot appear as a reply; malformed completed records still fail. Clipboard comparisons and backend request checks retain the exact prompt assertions.

The workspace fixture resolves tab, detach, dock and reattach controls by name,
so tab ordering and wrapped toolbars cannot redirect a click. Each native
viewport keeps separate controls and edit revisions: the root uses the supplied
snapshot path, while children use `<path>.<viewport-id>.json`. Child snapshots
include their builder title so the harness can distinguish equal-sized Chat,
Terminal and whole-sidebar windows even when the platform cannot read titles
back. The fixture checks disabled placeholders explicitly and retains all window
snapshots and backend requests on failure. Native input and real shell PID,
agent-session, draft and unsaved-file checks remain required.

The history deletion fixture also uses named controls for setup, filtering,
refresh, confirmation, diagnostics and restore. It checks the saved executable
before opening History and waits for completed host operations and resized
layouts. Row actions identify the conversation, so Delete cannot accidentally
hit Restore after a layout change. Failures retain the UI snapshot, backend RPC
log and fixture deletion state. Headless history row checks cover compact widths,
display scaling and protected chats. Native input checks still require Xvfb;
CI deadlines and one-click activation remain unchanged.

Project fixtures use named controls and completed directory listings for folder selection, confirm complete paths and editor buffers through the real clipboard, and wait for native save acknowledgements after checking file effects. Project switches and SSH reconnects verify the displayed terminal's native session ID, project root, live attachment and input focus before continuing. Resize checks require the new rendered size; refused SSH login checks require an ended terminal before testing read-only diagnostics. They click Copy directly, using its mouse-hover acknowledgement to process pending rejected input, and require the current connection's port in the copied error. Waiting for an unsolicited extra frame can stall an idle, read-only terminal. The fixture retains its final layout snapshot on failure. Its explicit sleeps only poll bounded conditions; CI command deadlines are unchanged and activating input is not retried.

Inspect or run the same suites locally:

```sh
python3 scripts/ci.py list
mkdir -p target/test-tools/bin
scripts/uvx.sh --from ruff==0.17.3 python -c 'import shutil; shutil.copy2(shutil.which("ruff"), "target/test-tools/bin/ruff")'
cargo test --locked --profile ci --workspace --all-targets --no-run --message-format=json > target/ci-test-binaries.json
RODE_TEST_RUFF="$PWD/target/test-tools/bin/ruff" cargo test --locked --profile ci --workspace --all-targets
cargo build --locked --profile ci --bins
cargo xtask build-helper
python3 scripts/ci.py bundle
LIBGL_ALWAYS_SOFTWARE=1 xvfb-run -a -s '-screen 0 1600x1100x24' python3 scripts/ci.py run workspace
```

The bundle includes the pinned Ruff executable in `target/ci/tools/ruff`; document and runtime suites validate it before starting. Document jobs also install uv for the default `uvx ruff` path. The private SSH fixture preserves tool order on the invoking process's `PATH`, but filters ambient `rode-sessiond` executables so a developer's installation cannot outrank the private-home helper fixture. Other tools in those directories remain available through symlinks. It copies installed `uv`/`uvx` executables into its temporary home so sandboxed tests can use them without accessing the real home. A supplied `RODE_TEST_SESSION_HELPER` also supplies standalone transport tests' service helper unless an explicit helper override is already set. Its loopback daemon permits up to 64 concurrent unauthenticated connections to accommodate parallel direct and ProxyJump tests without sshd's default random throttling.

For the ordinary Rust build job, run `RODE_TEST_RUFF="$PWD/target/test-tools/bin/ruff" cargo test --locked --profile ci --workspace --all-targets`. Building with `--no-run` does not execute tests, and filters such as `--lib app::` omit integration tests under `tests/`. Targeted checks help during development but do not establish that the full build job passes. Optional installed-tool tests still need the overrides and dependencies of their owning extended suite.

Check ordinary-suite CI parity with `env -u RODE_SESSION_HELPER -u RODE_TEST_SESSION_HELPER -u RODE_HOST_HELPER mise run test` to exclude developer helper overrides. Service-backed integration fixtures must select Cargo's matching `CARGO_BIN_EXE_rode-sessiond` in their child environment: a Rust test harness cannot serve the editor's `--session-service` entry point. An inherited helper override can hide missing fixture setup. Check successful agent completion before reading expected output files so startup failures retain their actual diagnostics.

The Rust runner creates its private homes below the configured temporary root, independently of the checkout path, to leave room for nested Unix-domain sockets. Set `RODE_TEST_TMPDIR` to a short, executable temporary directory when needed; the default follows Python's platform temporary-directory selection. `scripts/test-rust-tests.py` checks this with a real nested socket, and `scripts/test-ssh-fixture.py` checks host-helper isolation without changing tool precedence.

For a complete development validation, use:

```sh
python3 scripts/validate.py --preflight
python3 scripts/validate.py
# Equivalent mise task: mise run test-full
```

The full command runs formatting/lint and harness regressions, builds the current
checkout, publishes integration binaries from Cargo's reported paths, runs the
ordinary Rust suite and doctests, then runs every fast and extended CI suite.
Each UI suite gets its own display through `scripts/ui_display.py`. Every CI case
also gets a fresh private HOME, XDG directories, session/vault roots and temporary
directory, including older fixtures which replace only some environment fields.
Explicit tool paths, Cargo/Rust toolchain locations and the enclosing SSH fixture
remain available. Nested groups retain the same short `RODE_TEST_TMPDIR` parent
while owning separate directories, so nesting does not grow Unix socket paths.
Case homes are cleaned after execution. It installs
nothing; provide the system prerequisites from `scripts/ci-deps.sh` and the
explicit installed-tool overrides documented below. `--preflight` reports
availability, not proof of exercised coverage. `--phases ui --suite documents`
runs a focused prepared-build check; omitting `build` deliberately uses existing
binaries. Ordinary full runs always build before using those binaries.

Results live under a fresh `target/ci-results/validation-*/` directory. Its
`summary.json` distinguishes command success from capability availability,
lists optional omissions and retains the selected phases/suites. Logs, native
screenshots and state snapshots stay within the corresponding invocation.
Nested fixture paths stay distinct, including tests with the same `local` or
`ssh` directory name. Interrupted commands remain recorded in the summary.
Independent cases continue after a failure, but the command still exits nonzero;
interruptions stop further launches. A failed build prevents UI checks against
stale binaries. Release packaging, opt-in upstream downloads and manual
benchmarks remain separate workflows, not part of this development command.

Schema version 2 adds `provenance` and `repeat_results`. Provenance records the
Git revision, a digest of tracked and non-ignored untracked source (including
dirty contents and executable bits), hashes of supplied application/test/tool
binaries, and bounded tool-version probes. It stores no source diff or arbitrary
environment. The source, artifacts and tool versions must remain unchanged
during validation; a detected change fails the run and suppresses comparison
rates. Existing/prebuilt binaries are identified by hash; omitting `build` does
not establish that they were built from the recorded source.

Use `mise run test-repeat`, or
`python3 scripts/validate.py --phases build ui --suite reliability --repeat 3`,
for three baseline repetitions. Each case/condition records all attempts, status
counts, executed-command count and command failure rate. A mixture of pass and
failure is an intermittent-failure candidate for investigation, not proof of a
false positive. Failed attempts continue to fail the aggregate run. Blocked
preflight and unexecuted cases have explicit records and never count as passes
or enter the failure-rate denominator. Successful commands with optional skips
remain labelled `passed-with-skips`; their success does not prove the omitted
feature's reliability. A rate is absent if source/artifact/tool comparability
could not be established. A few successful repetitions do not establish a
long-term flake rate.
Unreadable or missing case records are retained as unexecuted evidence and fail
validation even if the enclosing command exited successfully. The parent suite's
plan must match the expected cases; child reports cannot substitute for a missing
parent. Malformed capability-skip records also fail validation, with their paths
and diagnostics retained. Truncated metadata from an interrupted child cannot
erase the other attempts or prevent independent later commands from running.

Use `python3 scripts/validate.py --phases build ui --suite reliability --repeat 3 --stress`
(`mise run test-stress`) to exercise delayed GUI input, paths containing spaces
and quotes, and child umasks `0002`, `0077`, `0022`. Each repetition has distinct
logs and fresh fixtures; any failed repetition fails the run. This is stress
coverage, not a retry policy. Different umasks are reported as separate
conditions, so the three default stress repetitions provide one sample per
condition. Scheduled CI and manually requested full CI run both baseline and
stress repetitions; failure of either remains a failed job. The suite retains
one complete workbench journey alongside
independent picker cases; the document lanes run separate tools, Git and settings
cases so an early failure cannot suppress the others.
The tools suite also runs the 1x/2x DPI regression on its own larger private
display; a standard-size display cannot contain the doubled window.

Runtime CI requires real Jupyter, debugpy and LLDB. Sandbox CI requires actual
enforcement. Requirement flags are assigned by their owning suite, not inherited
from another lane; the deliberate unavailable-sandbox fixture must remain able
to test normal-mode fallback. Optional runtime tests use
`tests/support/capabilities.rs`, and Python fixtures use `test_capabilities.skip`:
missing required capabilities fail, while optional omissions emit individual JSON
records even when libtest captures successful output. The command record then
says `passed-with-skips`. Libtest's raw pass count still includes early-returning
tests; use the coverage reports as well as its totals. Declared `#[ignore]` tests
remain distinct, including subprocess fixtures and separately invoked SSH tests.

Fixtures that replace `HOME` must create that directory with explicit `0700`
permissions. The desktop and helper reject group-writable homes, so relying on
the caller's umask makes tests fail under `0002`. Keep that rejection intact;
do not change the process-wide umask while Rust tests run concurrently. The
tunnel fixture must also decode quoted SSH configuration tokens before using
the relay username; surrounding quotes are syntax, not part of the account.
Use `fixture_support.fixture_environment` for native Python fixtures. It creates
private homes/config/cache/runtime directories, clears ambient Codex home state
and checks explicit directory overrides. It rejects unsafe existing directories
and symlinks rather than changing permissions on someone else's files. The Rust
runner uses the same directory primitive, and prebuilt integration tests go
through that runner too. Preserve explicit tool overrides and private remote
fixtures. A test must not depend on tools or preferences in the developer's home.

Notebook IPC sockets remain inside the host's private temporary directory.
On Linux the adapter addresses them through a held `/proc/<pid>/fd/<fd>`
directory handle, keeping socket names within Unix pathname limits even with
long home paths and project-specific temporary directories. Keep the handle
alive until the kernel and channels have stopped. The real sandbox notebook
regression verifies IPC transport and the resolved private directory with
network sharing disabled.

The document, binary, language, debugger, layout, queued-prompt and attachment UI fixtures use `NativeUI` in `scripts/ui_harness.py` for complete command/path input, focused controls, document identity and resize acknowledgements. Opt-in snapshots expose value fingerprints instead of input text; they only observe state. Queue edits wait for the acknowledged row contents, and attachment snapshots must finish reading before tests mutate their source files. Fixture protocol logs are read as completed newline-terminated records, including when the last UTF-8 character arrives in pieces. Activating input is sent once through XTest, never retried. Window discovery checks the owning PID, and focus retries mapping races without letting Xlib terminate the test process. Polling intervals and deliberate gesture/idle measurement intervals remain; arbitrary startup and dialog sleeps do not establish readiness.

Use `scripts/ui_fixture.py` for shared project-window readiness and owned native
sessions. Notebook, LaTeX, Marp and DPI fixtures share this lifecycle, including
normal shutdown checks and failure snapshots. Keep feature setup and assertions
in named scenario functions: execution, protected saves, editing, preview/export
and shutdown should be readable independently. `project_ui` also serves fixtures
that deliberately restart their process or retain host sessions. Keyboard-only
navigation uses the palette's keyboard activation option; it waits for the full
query and focus before pressing Enter, without substituting a mouse click.
Prompt-recall checks wait for the composer value and cursor-at-start
acknowledgement before sending the next history key; polling never resends
selection or clipboard commands.

Notebook tests observe the kernel's busy state and changed execution counts
before saving once. Cell source fingerprints and visible controls acknowledge
edits, insertion, target selection and persistence. The probe records no source
or output text and clears notebook state when the view closes. LaTeX and Marp
checks wait for formatted buffers, notices, rendered previews and export results;
the DPI test retains its deliberate held-pointer interval. Slurm indicator checks
poll the rendered result across the scheduler's normal refresh interval instead
of sleeping for an assumed refresh time. Legacy gesture tests still contain
timed steps; those are not evidence that all UI workflows are timing-independent.

The SSH integration entry point delegates to modules under
`tests/ssh_roundtrip/`: transport, policy, agents, documents and runtimes, alongside
the existing focused tunnel, X11 and transfer fixtures. Test names gain their
module prefix; leaf names and assertions are retained. Run them through
`scripts/test-ssh.sh` so all modules use the same isolated sshd and credentials.

Failed UI fixtures terminate and reap their owned process without attempting another UI interaction, so cleanup cannot replace the original assertion with a close-window timeout. Queue and attachment fixtures also end their own project's agents in a private service directory; other projects' agents are left alone. Natural terminal exit drains queued output before publishing exit; the reader owns the quiet interval, with byte and time limits for descendants that continue writing. Regressions cover a slow reader with an open descendant and a continuously writing descendant.

The workspace lane also runs `scripts/test-readiness-ui.py`: it pauses only its own GUI process through a pidfd while entering multiline source and opening successive file dialogs. The test requires complete source fingerprints before opening the next modal, so a queued Enter cannot choose a command from the new palette. Small source acknowledgements are limited to 64 KiB; larger files keep their existing streaming/size-specific checks. The private SSH shell has a fixed readiness prompt, bypasses host login files and lives under the fixture home’s read-only `.local` grant. PTY checks wait for that prompt before sending commands.

Process fixtures must publish readiness markers only after their contents are complete, for example by writing a sibling temporary file and renaming it into place. File creation can precede its first write; waiting for existence and then parsing immediately can fail under another scheduler. Exercise that interleaving directly in a regression rather than relying on longer sleeps or automatic retries.

The terminal session UI fixture waits for palette focus and a complete command match, then for the exact host session to have an attached, focused terminal view. Shell commands acknowledge execution through complete file contents before another modal opens. Natural exit is gated until the open manager lists the session; the fixture then requires its removal from both the host service and the manager's cached list without clicking Refresh. The opt-in UI snapshot includes opaque session IDs for the whole manager list, including scrolled-out rows, and distinguishes an empty list from a closed manager. Multi-window snapshots have separate paths, and failures retain those snapshots, shell markers and the host session list alongside screenshots. Deadlines are unchanged and input is never retried.

Observe widgets through their original `egui::Response`. `Context::read_response` replays interaction handling and can surrender focus after an outside click, even when the caller only wants geometry. The terminal tab regression checks real input delivery with the probe enabled and disabled, with mouse press/release in separate frames and batched together. The mise fixture retains the click on the already selected Terminal tab, waits for that same session's focus, and compares shell PID and environment before and after task execution. Metadata waits require the task button to render, not just the host request to be logged; failure artifacts retain the UI snapshot, fixture calls and shell markers.

Remote startup foregrounds chat and collapses the explorer. UI fixtures must
select the surfaces they exercise explicitly. Theme, Git staging/commit and image
Fit controls use reported response geometry; Markdown image checks inspect the
full document area. The coordinate-based notebook fixture deliberately expands
the explorer before using its shared local/remote cell layout.

Agent selector UI tests use named menu/option geometry and wait for a fresh rendered idle state between configuration changes. A saved preference or logged `session/set_config_option` request does not establish that the host and provider have finished applying it. The chat fixture creates `slow-controls` to delay provider replies and exercise that gap; the test still requires the new typed backend request, the saved selection and the subsequent idle acknowledgement. Clicks are not retried and timeout limits are unchanged.

Git authentication tests use a private TLS server and the real Git HTTP backend. `scripts/test-git-auth-ui.py` covers cancellation, rejected credentials, successful fetch/push and a fresh prompt on the next operation; the workspace lane runs it locally and SSH smoke repeats it on the project host. OpenSSL creates only the fixture certificate. The fixture instruments configured credential writers and inspects Git argv, inherited environment, preferences and traces for supplied secrets. `tests/git_auth.rs` checks the headless in-memory helper's origin/port isolation and ignored store/erase operations. The `git_memory_auth` capability prevents older helpers from accepting credentialed requests without this contract.

Negative IPC probes must validate the peer's explicit rejection even if it closes before a write completes. A write-side `EPIPE` alone is not proof of policy enforcement. Use socket pairs to exercise that ordering deterministically; EOF, incomplete frames, successful replies and unrelated errors must still fail the security probe.

Large-file regressions cover reads and revision-checked saves above the old 8 MiB limit, binary RPC payloads above the control-message ceiling, image/PDF sources above 32 MiB, metadata-heavy notebooks, and streaming duplication above 256 MiB. `scripts/test-large-files-ui.py` (documents suite) edits and saves the tail of a 49 MiB binary through the real UI, including undo and display-page navigation. The SSH smoke subgroup repeats the workflow with `RODE_LARGE_FILES_UI_REMOTE=1`; `RODE_SSH_TEST_FILTER=large_document bash scripts/test-ssh.sh` checks payloads above 64 MiB on direct SSH and ProxyJump. Raster allocation, control-message, attachment and tool-output bounds are separate contracts, not file-size ceilings.

Explorer mutations run through the shared file channel and require the `explorer_mutations` helper capability. `cargo test --test files` checks both in-process and native-helper dispatch, including symlinks, collision races, bounded copy cleanup and real sandbox restrictions. The sandbox CI suite additionally requires actual bwrap enforcement for these file tests. `scripts/test-explorer-ui.py` uses named control geometry and native clicks to exercise creation, rename with unsaved buffers, duplication, confirmed deletion, path copying and guarded project opening. The workspace CI suite runs it locally; the SSH smoke subgroup repeats it with `RODE_EXPLORER_UI_REMOTE=1`. `RODE_SSH_TEST_FILTER=explorer_mutations bash scripts/test-ssh.sh` additionally covers file requests over direct SSH and ProxyJump.

Install each suite's prerequisites first (listed in `scripts/ci-deps.sh`); that script uses sudo and is intended for disposable CI runners. Clipboard regressions for failed login and long chat drafts also require `xclip`. `RODE_TEST_BINARY`, `RODE_TEST_SESSION_HELPER` and `RODE_SESSION_HELPER` can override the binaries for local runs. The CI runner defaults `RODE_SESSION_HELPER` to the selected test helper as well: transferred test executables live in a separate directory, so service-backed history/agents cannot rely on sibling discovery. Explicit overrides remain respected. `RODE_SSH_TEST_BINARY` selects a prebuilt transport-test executable; standalone `scripts/test-ssh.sh` still builds/runs its Rust tests by default. To batch UI-only commands under one SSH fixture, use `RODE_SSH_RUN_TRANSPORT=0 bash scripts/test-ssh.sh <command> [arguments...]`; omitting the command in this mode is an error. `python3 scripts/test-ci.py` checks suite selection, helper discovery, artifact permissions, bounded failure output, timeout cleanup and both SSH-wrapper execution modes without contacting an SSH host.

Dependency setup keeps the outer 600-second CI deadline. APT uses 30-second HTTP/HTTPS connection and data timeouts, three retries per failed download and a 60-second dpkg lock wait. Index download failures are errors, and package setup runs noninteractively. Only downloads are retried; dpkg installation and tests retain their normal failure behavior. Tools/runtime jobs install the Go toolchain pinned in `mise.toml`/`mise.lock` through mise-action before OS dependency setup; other jobs omit Go. The dependency script requires `go` and `gofmt` on `PATH` for these suites and fails immediately if either is missing, rather than downloading Ubuntu's SDK/source packages. Dependency regression tests use isolated command fixtures without sudo privileges, package changes or network access.

The root package declares both `rode` and `rode-sessiond`, so plain `cargo install --locked --path .` installs both. For source installations intended for SSH, use `cargo xtask install`: it builds/verifies a static helper before installing the editor, then atomically publishes `rode-sessiond-static` beside it. Add `--root PREFIX` to test without updating a real installation. `cargo test -p xtask` covers failed installations, static-helper ELF checks, atomic replacement and custom Cargo target directories. The static builder reads Cargo's reported executable path, validates the ELF architecture and absence of a dynamic loader/shared libraries, and prints the absolute artifact path on stdout. It honors Cargo's configured target directory; build diagnostics remain on stderr.

`tests/sessions.rs` exercises the headless binary through `CARGO_BIN_EXE_rode-sessiond`, including protocol negotiation without a display and reattachment through the ordinary editor after the helper's clients exit. The library package's opt-in `standalone` feature exposes a separate `rode-sessiond-standalone` target for the static builder; distinct names avoid workspace output collisions even with all features enabled. `cargo xtask build-helper` builds that target without desktop dependencies, and Debian packaging calls the same task; the published helper names remain unchanged.

An additional test starts a temporary localhost-only SSH daemon using fresh keys, a private known-hosts file, an isolated config and a temporary remote `HOME` for helper deployment. It never edits `~/.ssh`:

```sh
cargo xtask build-helper
bash scripts/test-ssh.sh
```

This requires `openssh-server`, `ssh-keygen`, and Python 3. The tests use a local login PTY and binary channels to the remote helper, which owns real project PTYs. Raw SSH PTY channels are also checked when the fixture daemon has permission to allocate them. Direct and ProxyJump sessions check real authentication/multiplexing, switching project folders without disconnecting, terminal working directories, quoted Unicode paths, multi-megabyte reads/writes, save conflicts, sandbox file operations, and disconnection.

For native UI checks on Debian/Ubuntu, install `xvfb`, `xauth`, `x11-utils`, `imagemagick`, `libxtst6`, `libxkbcommon-x11-0`, `libegl1`, `libegl-mesa0`, `libglx-mesa0` and `libgl1-mesa-dri`. The scripts use `xwininfo` from `x11-utils` to find and inspect windows, ImageMagick to capture/inspect screenshots, and libXtst to send input. The editor dynamically loads the X11 keyboard library, so installing `libxkbcommon-dev` alone does not provide its X11 runtime. CI explicitly installs the runtime libraries, selects Mesa software rendering and uses a 1600×1100 virtual screen so the fixtures’ 1380×900 windows fit without being clamped by the window system. Set `LIBGL_ALWAYS_SOFTWARE=1`, `__GLX_VENDOR_LIBRARY_NAME=mesa` and `__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json` when reproducing CI on a host with vendor GPU drivers. Early debugger-window failures print the exit code and the last 16 KiB of the captured startup log directly in the Actions output; the full log remains in the `native-ui-smoke` artifact.

Use `python3 scripts/ui_display.py -- python3 scripts/test-workbench-ui.py` for
local UI validation. The launcher requires Xvfb, removes inherited X11/Wayland
desktop routes, creates a headless display and cleans up its own process group.
`mise run test-ui` uses the same launcher. Use `--screen 3000x2000x24` before `--`
for the scale fixture. `python3 scripts/test-ui-display.py` checks launch refusal,
desktop-environment removal and interruption cleanup without opening a window.

Never run automated UI input on the user's desktop or start a visible Xephyr
window as a fallback. If headless dependencies are unavailable, report that the
native UI check could not run; do not install tools automatically or substitute a
visible test session. The direct `xvfb-run` commands below are also headless; unset
inherited `WAYLAND_DISPLAY` and `WAYLAND_SOCKET` when using them outside CI.

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

Shared tool presentation lives in `src/app/layout.rs`; see the [UI layout conventions](architecture.md#native-ui-and-work-on-demand) before adding a panel. `cargo test --lib tool_pages_fit_compact_and_detached_widths` checks every non-chat tool panel and Git subtab at compact and detached widths. `cargo test --lib forms_expand_after_a_short_command_palette_and_keep_actions_visible` checks modal sizing, form-label readability and action visibility when a short palette is replaced by a larger form.

`scripts/test-file-browser-ui.py` also runs locally on its own; screenshots/logs go to `target/file-browser-ui/`. Library tests cover stale picker results, relative destinations, preserved Save As names and project-environment guards.

## README feature tour

The README embeds `assets/rode-tour.gif` and links to an H.264 MP4 and a still PNG. The 30-second tour holds captioned native screenshots rather than recording every idle frame. The GIF uses variable frame durations and a shared palette; the MP4 uses H.264, `yuv420p` and fast-start metadata. Keep the GIF below 1 MiB when refreshing it and inspect the encoded text at README size.

`scripts/capture-readme-tour.py` captures a fresh demonstration project at 1380×900 with isolated preferences and session storage. It uses named native controls and readiness acknowledgements from the UI harness, including the inline Git change preview, and the existing loopback SSH fixture. Install the UI/SSH prerequisites above and provide a Python environment containing `jupyter_client`, `ipykernel` and Matplotlib. Build the current editor and matching helper before capturing:

```sh
cargo build --locked --release --bins
RODE_TEST_BINARY="$PWD/target/release/rode" \
RODE_SESSION_HELPER="$PWD/target/release/rode-sessiond" \
RODE_TEST_JUPYTER=/absolute/path/to/python \
RODE_SSH_RUN_TRANSPORT=0 LIBGL_ALWAYS_SOFTWARE=1 \
python3 scripts/ui_display.py -- \
  bash scripts/test-ssh.sh python3 scripts/capture-readme-tour.py
```

The capture script checks real notebook output and the terminal persistence acknowledgements, records the source revision, selected sandbox backend and binary/capture hashes in `target/readme-demo/captures/capture.json`, and publishes the new capture set only after all scenes complete. `--output` selects another capture directory. It removes its temporary project and closes its owned sessions. Review every captured screen after UI changes. The SSH scenes use an active Landlock sandbox by default; pass `--sandbox-backend bwrap` on a host that supports bubblewrap. Capture fails if the requested sandbox is unavailable rather than labelling normal mode as sandboxed.

`scripts/build-readme-tour.py` lists the captions and timings. With FFmpeg (including libx264 and drawtext) and DejaVu Sans installed, encode the captures into the three assets:

```sh
python3 scripts/build-readme-tour.py target/readme-demo/captures
```

The captures use the native editor, a real Python notebook kernel, a real loopback SSH connection, an active project sandbox and a real Git change preview. Local scenes use normal project mode; the remote scenes verify active sandboxing. The terminal frames show persistence being enabled. The agent frame shows provider selection without model inference. The Slurm form uses the existing deterministic scheduler fixture with an empty queue; it requests no real allocation. Keep those distinctions in the captions. Saved captures stay under ignored `target/`; do not capture personal chats, credentials or private projects. A clean checkout needs fresh captures before encoding.

After regeneration, check all three files visually, use `ffprobe` to check duration/dimensions/codec, and review README links. Re-encoding existing captures needs no application rebuild. Repository-relative GIF embedding works without uploading a GitHub attachment; the MP4 is a linked file, not an assumed inline README video player.

## Concurrent processes

`cargo test --test persistence --test concurrency` starts real subprocesses to exercise field-level preference/profile merging, external configuration edits, malformed-file preservation, lock release after a killed writer, private GUI snapshots, parallel sandboxed helpers with identical namespace PIDs, concurrent environment creation, and simultaneous desktop/headless document saves. Existing-file checks force writers to wait on the same inode, then verify that only one buffer based on the old revision can win. Save As tests verify atomic creation without overwriting another writer. Native two-window screenshots/logs go to `target/multiprocess-ui/`.

The SSH fixture also opens two independent masters for the same alias and checks that closing one preserves the other. The project UI check (`RODE_SSH_UI=1`) exercises banner/palette disconnect, compact connection controls, dirty buffers across reconnect, local file/folder access after explicit and unexpected disconnect, and persistent versus nonpersistent terminal lifetime. Library tests also cover failed logins, stale replies, disconnected Save As and prevention of remote/local path rebinding. See [configuration and multiple windows](configuration.md) for supported filesystem behavior and limitations.

`RODE_PROJECT_UI_CASE=failure RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-project-ui.py` reproduces a real refused loopback SSH login without an SSH server. It checks retained read-only error output, Copy/paste into a saved local file, local folder browsing and a neutral host title. The default project UI run includes it. Library connection/terminal tests also check a real fast SSH exit and late PTY output after cleanup, while ensuring diagnostic views cannot send input or own/signal the old process.

## Security checks

`cargo test --test security --test files --test sandbox --test persistence --test concurrency` checks special-file rejection with subprocess deadlines, canonical home grant restrictions, controlling-terminal isolation under a real PTY, desktop/headless contract parity, protected saves and shared configuration. Run `cargo test --test sessions --test agents --test debugger` for process lifecycle checks, and `scripts/test-ssh.sh` for direct/ProxyJump transport and sandbox integration. With an installed cargo-audit, `cargo audit --file Cargo.lock` checks the current advisory database. The [security review](security.md) records reproduced findings, fixes, maintenance warnings and the scope of the audit.

## Image checks

`cargo test --test images` checks all supported codecs, alpha and EXIF orientation, GPU downsampling, corrupt/oversized input rejection, desktop/headless binary-protocol parity, and actual sandbox access. Library tests cover shared pixel-budget release, stale reloads, and read-only save commands. The direct/ProxyJump SSH fixture also transfers and decodes images under the active project policy. `scripts/test-images-ui.py` checks rendered PNG pixels, zoom, JPEG tabs, reload, both themes, compact layout, read-only saves, and preservation of dirty text. It can run locally on its own, or with `RODE_IMAGES_UI=1` in the SSH fixture. Screenshots/logs go to `target/images-ui/`.

## Markdown checks

Markdown checks use `cargo test --lib markdown` for unsaved-buffer rendering, source saves, source-path resolution, input limits, image URI isolation, stale-result rejection, and texture-budget release. Native local/SSH screenshots and logs go to `target/markdown-ui/`; these checks also exercise actual sandboxed image reads, view switching, undo/redo, cursor preservation, themes, and find returning to source.

## Terminal color checks

`cargo test --lib terminal` and `cargo test --test themes --test persistence` cover actual egui glyph colors for ANSI/256-color/RGB/inverse text, Bash PS1 bold/bright colors and resets across every built-in theme, exact-color mode, unchanged parser output, decorative glyphs, bounded contrast caches, theme defaults/validation, per-theme overrides, independent concurrent edits and stale explicit resets. `scripts/test-terminal-colors-ui.py` uses a real native PTY and isolated preferences to check contrast changes, ANSI swatches, theme switching, restart persistence and continued shell input. It also runs a real Bash prompt with status/Git expansions, checks bright-black host/marker and bright-blue path colors, and compares success/failure colors with xterm when installed; xterm is an optional test reference, not an application dependency. Run it on an isolated X display; screenshots/logs go to `target/terminal-colors-ui/`.

## Agent checks

`cargo test --lib composer_sends` checks Enter submission, Shift+Enter/newline paste, focus/modal guards, key repeat and IME confirmation against the native text widget. The native agent UI test sends a multiline message using Shift+Enter followed by Enter, verifies the exact backend prompt, and checks that inserting a newline alone never starts an agent.

`cargo test --lib composer` also exercises the actual bounded input widget with long multiline/Unicode/wrapped drafts, cursor following after typing and resize, independent manual wheel scrolling, a stable visible Send row, and real divider dragging across small-window/history changes. Native agent UI checks paste a long UTF-8 document through the application's clipboard, scroll and enlarge the composer, keep typing after releasing the divider, and copy/save the draft to verify exact retention without starting an agent; screenshots include the default and enlarged input.

`cargo test --test agents --test history --test background_agents` uses deterministic Codex app-server and Claude/custom ACP v1 peers without calling a model. It exercises multi-turn streaming, approvals/questions, cancellation, process-tree cleanup, protocol/output limits, actual bwrap scope/read-only access, and explicit normal-mode fallback. The isolated direct/ProxyJump SSH tests also run both agent protocols, checking working directory, outside-file visibility, approvals, cancellation, and continued use of the authenticated master after closing chat.

Claude Code uses the shared ACP controller with separate provider settings. Agent/history/background-agent regressions exercise Claude alongside Codex and custom ACP fixtures, including advertised model/effort changes, explicit approvals, attachments, replay and persistence without inference. `cargo test -p rode-agent-core` covers default settings and independent Claude/custom-ACP arguments and options. The Claude ACP 0.100.0 adapter has also been checked with an isolated, credential-free ACP v1 initialization; a real authenticated coding turn requires a user-installed/authenticated adapter as described in [Agent chat](agents.md#claude-code-anthropic).

Native agent UI checks also open nested reasoning selectors, switch advertised levels repeatedly, verify immediate persistence, check the effort sent on the next turn and after restarting, and exercise dependent ACP model/effort submenus through acknowledged configuration requests. Agent-control checks cover paginated model catalogs, hidden models, next-turn model/effort/access changes, never-ask denial, read-only precedence, full access inside a network-restricted outer sandbox, ACP model/mode/boolean configuration settings. Dependent settings wait for model acknowledgement; missing/rejected settings and cancellation cannot submit a queued prompt. Idle changes refresh capabilities without creating a model turn. Library parser tests bound metadata and omit invalid/unsupported selectors; persistence tests check resetting ACP overrides while another window edits models and approval preferences.

Native reasoning and ACP submenus open by hovering over their named control and
waiting for the advertised choice to accept hover before clicking that choice.
The fixture uses the shared geometry/hover helper without an extra opener click;
boolean ACP options still use a single checkbox click. Library checks cover both
hover and click submenu opening at desktop and compact widths.

History checks cover project/provider filtering, metadata pagination without inference, bounded/ordered Codex and ACP replay, unsupported capabilities, cancelled loading, current access-policy overrides, and continuation using the resumed session. Deletion checks cover individual removal, bulk discovery beyond the 256-row browser limit, archived chats, protected open/active conversations and their Codex ancestors, scope revalidation, unsupported backends, invalid cursors, idempotent ACP removal and partial/refresh errors. Native-service checks protect a separate live controller while deleting history. Sandbox policy assertions use the prepared plan's actual `active` state rather than the requested flag; an explicit unavailable-sandbox fixture covers normal-mode fallback even on hosts where bubblewrap works. Direct/ProxyJump fixtures exercise the same flow without closing the shared SSH master; use `RODE_SSH_TEST_FILTER=agent_history_deletion bash scripts/test-ssh.sh` for the focused agent/history case. `scripts/test-agent-history-ui.py` checks per-chat confirmation/cancel, unloaded and archived bulk deletion despite a text filter, and absence of inference. Native chat UI checks also exercise the History list and restore action.

Personality checks (`cargo test --lib personality` and `cargo test --test agents --test history --test persistence`) cover unchanged task blocks, repeated hints on consecutive styled turns, process reuse, default resets after styled/restored chats, byte-for-byte execution-policy fields, approval requests, all four providers, active sandbox scope, safe preference merges and hidden replay hints. The optional real Codex loopback test checks the latest model-bound user input on three consecutive styled turns and after history restore, so a hint present only in earlier context cannot make the check pass. Generated style hints stay out of history titles/messages; no real inference or account is used. Native agent UI checks exercise selection/persistence before any process starts and changing tone in an existing chat. Qualitative delivery requires a real user-managed model; deterministic fixtures do not judge the voice.

To check an installed real Codex CLI without a model or account access, use:

```sh
RODE_TEST_CODEX=/absolute/path/to/codex cargo test --test codex -- --nocapture
```

This uses an isolated temporary `CODEX_HOME` and local model provider. It checks initialization, a complete streamed chat turn and replay from the agent's saved history through a loopback Responses fixture, in normal mode and with each sandbox backend when available, without calling a real model. Set `RODE_TEST_REQUIRE_LANDLOCK=1` to require actual Landlock enforcement for that case. Prepare the pinned native Codex `0.166.0` with `python3 scripts/bundle-codex.py`, then use the printed directory's `codex` executable as `RODE_TEST_CODEX`. Native screenshots/logs go to `target/agent-ui/`. Real Ollama model inference and third-party ACP executables require their own installations and are not covered by the deterministic peers.

`cargo test --test prompt_history` checks read-only pagination beyond 128 prompts, duplicate submissions, failed local submissions and rejection of wrong-project/repeated cursors. The real Codex check also reads earlier prompts through a separate metadata connection while the original process owns the chat, then continues that chat and verifies no extra model request occurred. `scripts/test-agent-persistence-ui.py` closes/reopens RODE, reattaches a live agent, and browses backend-owned older prompts with real Up/Down input while preserving its PID, completed turns and unsent draft.

`cargo test --test bundled` checks automatic selection, independent optional-package versions, relocated Cargo-style editor installs, XDG/default user runtime paths, unavailable architectures and installed fallback, custom overrides, changed executable rejection, streamed deployment bounds/checksums, failed-transfer cleanup, private cache/symlink checks and concurrent publication. `cargo test --lib bundled` checks search-path precedence, the extra read-only tool mount and rejection of writable, symlinked or nonregular runtime selector manifests. Selector tests explicitly set the linked target's permissions so a group-writable default cannot mask a symlink-following bug; run the selector test under both `umask 002` and `umask 022` when changing these checks. `python3 scripts/test-packaging.py` checks that core packaging never consults or downloads Codex and that a missing runtime target affects only its explicit component. It also exercises explicit per-user runtime setup with tiny cached archives: concurrent installation, verification failure preserving the old selector, XDG fallback and unsafe-path rejection. Normal commit checks use small deterministic executables, without downloading Codex. Separate optional-package validation exercises `rode --codex --version`, app-server support, static architecture and complete notices after overlaying it on an extracted editor package.

Native installer tests in `src/bundled/install.rs` use tiny in-memory archives without internet or agent execution. They cover concurrent reuse, cancellation during transfer/extraction/lock waits, archive and executable hash/size failures, extra or linked members, missing notices, unsafe root/lock/destination paths, and unchanged previous selectors on failure. Run them under both `umask 002` and `umask 022`; staging permissions must not depend on the caller's umask. The UI installer starts only on an explicit click and keeps setup separate from chat processes and credentials.

`RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-codex-install-ui.py` checks the native install/cancel/retry controls, an existing preferences directory created with umask 002, untouched Codex configuration/project files, compact layout and close-during-download cleanup. A loopback proxy rejects/holds CONNECT so this test never reaches GitHub. Set `RODE_TEST_CODEX_BUNDLE` to the actual prepared pin to additionally check successful verified reuse without downloading. To explicitly validate the native HTTPS downloader against the official asset, run `cargo test --lib official_native_download_installs_a_working_runtime_without_a_source_installer -- --ignored`; this downloads the pinned archive into a temporary test directory and checks its executable's version without an account or model.

The installer fixture locates controls through `RODE_UI_TEST_STATE`, an opt-in path for atomic JSON layout snapshots. Only explicitly registered control names, interaction states, edit revisions, clipped bounds, window size and scale are reported; document/chat contents and credentials are excluded. Ordinary startup installs no snapshot hooks. The harness waits for a prepared project, raises and focuses its window, checks the snapshot's process and window size, then moves the real X11 pointer to the control. It clicks once only after a newer frame acknowledges hover on the enabled control; modal-blocked controls cannot satisfy this barrier. This keeps layout transitions and runner speed independent of fixed click coordinates, without retrying actions that could run twice. Fixtures also wait for each action's result: session attachment must close its dialog and focus the selected terminal; authentication retries must present a fresh challenge. Credential typing waits for hover on the next control without changing focus, so trailing input is processed before submission without publishing secret values or lengths. `widgets.json`, screenshots and logs are retained in `target/codex-install-ui/` on CI failures. `scripts/test-ui-harness.py` checks delayed/disabled controls, stale processes and hover snapshots, moving controls, resizing, DPI conversion and timeout diagnostics. Rust tests check final-pass publication, modal blocking, pointer input and credential-entry ordering.

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

Only interpreter inspection and the Jupyter bridge use Python adapters. The Jupyter bridge requires the installed packages documented in the notebook guide. Ruff covers these adapters, the test harness and documentation hook.

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

`cargo test --test languages --test project` checks filename/grammar coverage, colored Go/Python/TS/TSX/TOML/Dockerfile tokens across themes, Unicode issue columns, stdin formatting and source preservation, and active sandbox enforcement. Real Go/clang-format checks run when those tools are on PATH. Set `RODE_TEST_PRETTIER` to an installed Prettier executable and `RODE_TEST_RUFF` for their real desktop/headless parity checks. The SSH fixture also checks installed project-local formatter selection and syntax diagnostics in both direct and ProxyJump sandboxed sessions.

With Go on PATH and `RODE_TEST_PRETTIER` set, run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-languages-ui.py` for actual Go format/run/build, TypeScript formatting/Problems, and saves after syntax errors. Screenshots/logs and `toolchain.json` go to `target/languages-ui/`. The standard-library-only Go fixture declares its actual minimum (Go 1.16), disables toolchain/module downloads and uses private caches; shell startup and user Go configuration cannot redirect its tools. CI installs Go, clang-format, shfmt and a pinned test Prettier; the application itself never installs those tools.

## Independent syntax themes

`cargo test --test themes --test persistence` covers full preset colors, independence from interface/terminal colors, custom scope rules, italic/underline separation, UTF-8 layout ranges, invalid/oversized definitions, cache invalidation and concurrent preference merges. Library tests check immediate independent saves, startup/CLI restore, unavailable-palette fallback and dirty-buffer preservation.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-syntax-themes-ui.py` to check all syntax choices against a fixed Carbon interface, a custom theme, unsaved source and live preference writes, menu layout and restart persistence. Native captures/logs go to `target/syntax-themes-ui/`.

## PDF checks

Install desktop Poppler tools (`pdfinfo` / `pdftoppm`). `cargo test --test pdf --lib pdf` checks real two-page rendering, portrait/landscape content, page limits and cancellation, malformed input, read-only commands, stale results, shared-budget release, native/Python binary parity and actual sandbox reads/symlink escapes. The isolated direct/ProxyJump tests also transfer PDFs under the project policy.

For native local and SSH viewing: `RODE_TEST_BINARY="$PWD/target/release/rode" RODE_PDF_UI=1 xvfb-run -a -s "-screen 0 1600x1100x24" bash scripts/test-ssh.sh`. This checks pages, wheel scrolling within/across pages, zoom, source reload preserving the selected page, failed-reload preservation, themes, read-only saves and dirty code buffers. `scripts/test-pdf-ui.py` can run locally on its own. Screenshots/logs go to `target/pdf-ui/`. The two-page vector fixture at `tests/fixtures/pages.pdf` is generated test data with colored shapes and built-in Helvetica text, requiring no PDF-generation library.

## Native terminals and Slurm

`cargo test --test sessions --test slurm` exercises native PTY persistence after views exit, in-place persistence changes shared across views, last-view cleanup for nonpersistent sessions, two simultaneous views sharing input/output, per-session cleanup, concurrent service creation, fresh caller environments, login-startup activation without reactivation on attach, and real bwrap lifetime/access checks. The native supervisor keeps bwrap's spawning thread alive; closing the request channel must not trigger `--die-with-parent`. Slurm tests use fake queue/cancel executables to check TRES counts, pending/unknown resources, desktop/headless parity, exact allocation IDs, UID filtering, and removal of inherited cancellation overrides. They never allocate or cancel real jobs.

The interactive Slurm fixture additionally waits for an explicit grant, starts a real PTY shell with scheduler/GPU environment variables, preserves allocation metadata across rename/shared reattachment, and cancels the exact owned fixture job. Command tests check literal quoting, H/H:MM and GB conversion, bounds, and refusal to fall back when the compute sandbox fails. `RODE_SSH_TEST_FILTER=interactive_slurm scripts/test-ssh.sh` narrows the isolated SSH run to allocated shells with and without a sandbox, directly and through ProxyJump. The complete SSH run includes these cases too.

Notebook reservation tests detach the persistent progress view without ending its allocation. With `RODE_TEST_JUPYTER` set to an installed environment containing `jupyter_client` and `ipykernel`, `cargo test --test sessions --test notebooks --test slurm` additionally runs real kernels in fixture steps, checks GPU/job environment, interruption, kernel shutdown preserving the reservation, and allocation release ending a live kernel. `RODE_TEST_JUPYTER=/path/to/python RODE_SSH_TEST_FILTER=slurm_notebooks scripts/test-ssh.sh` runs kernels through direct SSH and ProxyJump and verifies expiry cleanup preserves the SSH master. The runtimes CI suite includes these checks. Mandatory stdio tests also exercise exact UID-filtered ID/name/state revalidation, stale inherited scheduler overrides, literal arguments, missing jobs and compute-policy refusal through native and Python-peer transports. No real scheduler job is submitted.

`RODE_TEST_JUPYTER=/path/to/python cargo test --test runtimes` exercises real native-service queues surviving loss of their controlling view, source-bound output retention, kernel-variable continuity, exclusive attachments, policy rejection, acknowledged persistence changes and last-view cleanup. The deterministic DAP case retains pause/stack/scope/variable state, rejects an obsolete inspection epoch and steps after reattachment. `RODE_TEST_JUPYTER=/path/to/python RODE_SSH_TEST_FILTER=persistent_slurm_runtimes scripts/test-ssh.sh` repeats kernel and debugger reattachment after dropping complete SSH connections, directly and through ProxyJump, with scheduler fixtures. Ended debugger cleanup must preserve the independent kernel. Use a current release/stripped native helper for remote tests; debug helpers containing symbols can exceed the existing 24 MiB upload limit.

Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-slurm-ui.py` for the allocation form, pending/active fill beside Persist, actual shell variables, compact layout and persistence/reattachment. Artifacts go to `target/slurm-ui/`. It uses installed executable fixtures and never submits a real scheduler job.

With `RODE_TEST_JUPYTER` set, `xvfb-run -a -s "-screen 0 1600x1100x24" python3 scripts/test-slurm-notebooks-ui.py` exercises **Reserve for notebooks**, its persistent owner, selection of the running job, real cell execution with scheduler/GPU variables, enabling persistence on the existing kernel, complete desktop restart, reattachment preserving Python state, switching back to a fresh host kernel, and retention of the allocation after kernel changes. Screenshots and logs go to `target/slurm-notebook-ui/`; it uses the same scheduler fixtures. The debugger UI smoke test additionally enables persistence, restarts the desktop, reattaches the paused controller and exercises stepping and terminal input.

Current-protocol fixtures own real shells and exercise shared attachment, opt-in persistence, rename/end operations, stale-socket recovery and idle/child cleanup in private runtimes. Control tests require the matching protocol and reject unauthenticated endpoints. Agent and runtime checks use the same current service; historical protocol fixtures and upgrade-routing tests have been removed. Shutdown regressions hold the current helper's readiness check to verify that view replacement cannot discard pending host cleanup.

Idle-lifetime checks start a service without a terminal and naturally exit its last shell, then verify socket cleanup and automatic restart. They keep a detached persistent shell alive beyond the five-second idle timeout, inspect the raw service protocol to verify completed records are released while that shell runs, preserve final output in open views, and check that polling or finished views cannot keep an otherwise empty service alive. The native UI check also leaves the session manager open while a terminal exits to check automatic list refresh.

Build the remote helper with `cargo xtask build-helper` before running `scripts/test-ssh.sh`. For a compatible local fixture host, `RODE_TEST_SESSION_HELPER=/absolute/path/to/rode-sessiond` can select an already built helper instead. Both direct and ProxyJump tests deploy a private checksum-verified helper, start a sandboxed native terminal with a real project venv, disconnect its view/master, establish a fresh independent connection, and reattach to the same running shell. Fixtures use isolated runtime/helper paths and end only their own sessions.

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

`cargo test --test marp` exercises snapshots, preserved source, export conflicts/atomic replacement and symlink protection, conversion limits, desktop/headless coordinator cancellation (including a detached browser-shaped child), and read-only bwrap preview without project writes. `cargo test --lib marp_view` checks source saves, dirty guards, cached previews and independent export errors; native UI tests cover focus and interaction. Full Rust checks include the existing Markdown/PDF tests after their shared-view refactoring.

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

`cargo test --lib --test vaults` checks mount identity, leases, underlying mountpoint protection, initialization refusal for existing plaintext, buffer/exit protection, deferred cleanup, operation draining and password handling through desktop and headless Rust helpers. Native component tests also check shared view locks without FUSE. With installed gocryptfs it also initializes a real encrypted configuration and rejects an incorrect password. If working FUSE is available, the same test performs real encrypted writes, shared-window refusal, release and locking; `/dev/fuse` is absent in the current development environment, so that mount round trip could not be validated here. CI installs gocryptfs/FUSE tools; actual kernel mount availability still depends on the runner.

FUSE access does not guarantee permission to inspect other same-account mount namespaces. In that specific case, the real-mount test requires an explicitly unlocked refusal and readable mounted content, then stops its own lease peers and normally unmounts only its fixture. Other close failures still fail the test, with both protocol replies included in the assertion. The FUSE-independent component check verifies that namespace-verification refusal retains the final view lease and that locking succeeds after the restriction is removed; the application's namespace checks remain required.

Direct/ProxyJump SSH tests check that a successful tool cannot substitute an ordinary directory for a verified encrypted mount, keep passwords out of diagnostics and preserve the authentication master after cleanup. Run `RODE_TEST_BINARY="$PWD/target/release/rode" RODE_VAULT_UI=1 xvfb-run -a -s "-screen 0 1600x1100x24" bash scripts/test-ssh.sh` for local/SSH UI, failed unlock preserving a dirty source, preference files free of the submitted password and compact layouts. Captures/logs go to `target/vaults-ui/`. The native library tests use a deterministic protocol peer for successful/failed lock UI state transitions without claiming a FUSE mount.

The vault UI fixture records the native/SSH helper's cleanup reply in `<host>-cleanup.json`. It requires normal window exit after verified cleanup. If the host specifically refuses mount-namespace inspection, it instead requires the window to remain open after Close; other cleanup errors still fail the test. This exercises the documented exit guard on restricted CI hosts without disabling namespace checks or claiming an unverified lock. A deterministic UI-state test also checks that a failed unlock and cleanup refusal received together preserve both diagnostics.

## Debugger checks

`cargo test --test debugger` checks DAP byte framing/Unicode, launch/configuration ordering with deferred replies, verified/unverified/conditional breakpoints, stepping, scopes/children/watches, stale evaluations, real interactive PTYs, unread-input responsiveness and owned detached-child cleanup on the native and Python coordinators, normally and under bwrap. Attach requests detach; sandboxed attach is rejected. Library checks cover source guards and extension disable. The shared preference tests merge independent debugger and other tool edits across processes.

The deterministic debugger adapter is shared by the Rust integration and native
UI tests. It writes the terminal-input result to a temporary file and renames it
after closing the completed UTF-8 write. Readers may therefore use the final
file's existence as acknowledgement; creating that file before writing would
allow an empty read even after terminal input was delivered successfully.

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

`cargo test --test runtimes slow_debugger_views` deliberately stops reading an attached debugger during a multi-megabyte PTY output burst. The host retains unread terminal events within its bounded replay queue and propagates backpressure to the target instead of disconnecting the view. The test requires every output line, stepping and subsequent terminal input on the same attachment. A second case detaches the stalled reader, verifies the persistent target continues without a view, then reattaches and checks controls and input again.

The runtime worker drains productive bounded batches immediately, checking
queued controls between batches. It sleeps only when idle or waiting for replay
space. A fixed sleep after every batch throttles small PTY reads even after
detachment; increasing the test deadline would hide that delay. The retained
screen, replay and transport limits still apply while output is draining.

## Lifecycle and hex rendering review

Use `python3 scripts/rust_tests.py --locked --profile ci -p rode --lib` for the
isolated Rust regressions after workspace, lifecycle or input ownership changes.
`app::workspace::control` checks stale/duplicate preparation replies, superseded
confirmations, cancellation, active-context preservation and connection-token
ownership. The application preparation test also submits an old confirmation
after a replacement request starts. `app::lifecycle` and `app::sessions` check
readiness/failure barriers and forced transport loss while a persistence reply
is outstanding. Late worker edits still require a new dirty-buffer decision, while an unchanged approved discard does not. The notebook shutdown probe uses the real host service to check
that ordinary leave/disable detaches persistent work and vault locking terminates
it; the registry must retain shutdown ownership until acknowledgement.

The regression tests cover native window close with clean encrypted-project buffers, waiting for active operations before verified lock, failed locks preventing exit, unbound buffers with plain-text extension checkers, remote tools exiting before reading their password input, asynchronous SSH terminal attachment, and starting a native service from an executable unlinked by an update. Project UI checks also start a new shell after replacing the running editor's file. Parallel SSH and sandbox helper directories use a per-process sequence in addition to PID/time, so equal clock samples cannot collide. Finished terminal views do not send another signal to their reaped child. Disassembly results retain their buffer version, offset and decoder mode; changing only decoding parameters reuses the full-file object report.

`cargo test --lib app::lifecycle` also checks closing while a save acknowledgement is still in flight. A successful save completes the pending quit only when every buffer is clean; failed saves, newer edits, cancelled quits and encrypted-project cleanup retain their guards. The native editor smoke test waits for the formatted write instead of assuming a fixed formatter latency. SSH agent and debugger cleanup checks identify their marked child in the runner's PID namespace before shutdown, so sandbox-local PIDs cannot be mistaken for unrelated host processes. The installer UI fixture runs a private copy of the editor to avoid discovering a development bundle beside the original executable.

A display-free render sample is available for manual comparison:

```sh
cargo test --release --lib hex_frame_profile -- --ignored --nocapture
```

On the development machine, a dirty 8 MiB binary differing at its final byte took **0.798 ms median / 0.990 ms p95** per unchanged editor frame before the review, and **0.625 ms median / 0.642 ms p95** after moving full-buffer dirty comparisons out of unchanged frames. Each sample used 100 warmed egui frames with a virtualized hex grid. This is a synthetic render-only measurement; it does not measure total application, GPU rendering, SSH or tool execution time and is not a general speed guarantee.

## Keyboard navigation checks

The navigation fixture explicitly starts interactive Bash in the terminal on
the project host and waits for its readiness marker before checking readline
Tab/Ctrl+K behavior. A desktop `SHELL` value does not select an SSH host's shell.
Terminal command assertions wait for complete expected file contents, including
the completion result, rather than treating redirection's file creation as
command completion. Run the fixture locally and through the private SSH host
after changing its shell setup; `RODE_TEST_SLOW_UI=1` and
`RODE_TEST_AWKWARD_PATHS=1` exercise delayed frames and quoted temporary paths.

`cargo test --lib navigation` checks default bindings, exact modifiers, leader input consumption, list selection, invalid-edit rollback, remapping/reload and per-binding concurrent preference merges. On an isolated X display, `RODE_TEST_BINARY=target/release/rode python3 scripts/test-navigation-ui.py` exercises the workflow entirely with keyboard events: explorer expansion/opening, file suggestions, pane focus, terminal creation/cycling, shell Tab/Ctrl+K, dialog return, resizing/maximization and UI binding persistence. Screenshots/logs are in `target/navigation-ui/`. Set `RODE_NAVIGATION_UI=1` when running `scripts/test-ssh.sh` to repeat the keyboard workflow over real SSH. Run the terminal-key, layout and project UI regressions after changes to focus routing. Window-manager grabs cannot be reproduced by the isolated X test; defaults deliberately contain no Alt.

Style regressions use `cargo test --lib navigation`: preset validation/isolation, Standard indentation/comment/line commands, native Emacs mark/kill/yank, Unicode motions, Vim operators/counts/visual selection, shared native undo, batched input ordering and cancellation of queued input after context changes. Standard regressions cover reversed selections, the excluded next-line boundary, CRLF, empty/final lines, stable indentation, AltGr, IME composition, buffer limits, focus/modal barriers and key-release handling. `scripts/test-shortcut-styles-ui.py`, included in the workspace suite, checks actual native indentation, comments and line commands, Ctrl+X clipboard events, all three source keymaps, safe file saves, shell Ctrl+W, pane switching and immediate style persistence. Use `RODE_SHORTCUT_STYLES_UI=1` in the isolated SSH fixture for the same workflow over SSH. Captures/logs go to `target/shortcut-styles-ui/` and `target/shortcut-styles-ssh-ui/`.

Source edits preceding a navigation prefix or global shortcut stay ahead of that command in the bounded replay queue. `app::input` verifies that batched source edits reach disk before Save completes, tab/new/close commands preserve following input under the destination keymap, and replay requires acknowledgement from the matching native source widget. A deferred native Ctrl+X clipboard event retains its recognized prefix modifiers even after Ctrl is released. The Rust regression batches Emacs mark/kill/yank, replacement and Save into one input frame; the native fixture also batches the Emacs edit sequence when `RODE_TEST_SLOW_UI=1`. Buffer fingerprints and save acknowledgements distinguish editing failures from disk-write failures, and failed native runs retain the fixture documents and UI snapshot.

Input/lifecycle interaction regressions run source routing, native TextEdit and the
close guard in desktop frame order. They inject a single native close alongside
Standard/Emacs/Vim typing, edits on both sides of a document handoff, and queued
Save followed by a newer edit. Assertions require accepted text to remain with its
destination, submitted saves to reach disk, and quit to wait or re-enter the dirty
guard as appropriate. Exit-controller tests also require acknowledgement of a clean
save in flight and forbid cleanup retries from approving newer discard versions.
File-reply regressions inject failed text and binary saves while a clean buffer's
quit is waiting; the error must remain visible and cancel automatic exit.
These checks use isolated temporary files and native events, without timing-based
success or fixed widget coordinates.

## Interface scaling checks

`cargo test --lib interface_scale` simulates 1x/2x monitor DPI, validates saved scale independently of stale framework state, rejects malformed settings and preserves dirty buffers, editor font size and themes. `cargo test --test persistence` checks explicit resets from stale windows without reverting another window's font choice. Run `RODE_TEST_BINARY="$PWD/target/release/rode" xvfb-run -a -s "-screen 0 3000x2000x24" python3 scripts/test-scale-ui.py` for real native font/GUI separation, measured menu resizing, percentage presets applied only after pointer release, 5% Smaller/Larger actions, Ctrl+0, dirty-buffer preservation and restart at forced X11 1x/2x DPI. The isolated X display should provide at least 3000×2000 physical pixels for the 2x window; screenshots/logs go to `target/scale-ui/`. Actual display-server DPI detection on other computers still depends on their OS/monitor configuration.

### User execution mode checks

`python3 scripts/test-execution-modes-ui.py` uses native input, real PTYs and a Git hook that reads a private file outside the project. It checks both terminal modes, retained shell PIDs after switching/reattachment, Run under the project policy, default host Git, denied sandbox Git without fallback, retained commit drafts and saved mode choices. The `tools` job uses Landlock when available; `sandbox` requires actual bubblewrap enforcement. Set `RODE_TEST_REQUIRE_LANDLOCK=1` to require Landlock locally. The `remote-smoke` group repeats the workflow through its private SSH host using `RODE_EXECUTION_UI_REMOTE=1`; for a standalone run use `RODE_SSH_RUN_TRANSPORT=0 RODE_EXECUTION_UI_REMOTE=1 bash scripts/test-ssh.sh python3 scripts/test-execution-modes-ui.py` on an isolated X display. The fast SSH job also runs with `RODE_SSH_TEST_NO_BWRAP=1`, which supplies a failing bubblewrap probe in the private host fixture and requires the UI to report unavailable sandboxing. This covers normal-mode fallback on developer machines that support sandboxing. The fixture checks effective preference defaults because unchanged default fields may be omitted from storage; the changed terminal choice must still be saved. Logs and screenshots are in `target/execution-modes-ui/`, with a separate `ssh-fallback` directory for that case.

### Background agent checks

`cargo test --test background_agents --test sessions --test agents --test history` uses real native services with deterministic Codex/local-model/ACP peers. It checks Auto completing detached work and queues before reaping its backend, original PID/conversation reattachment, in-place Auto/On/Off changes, last-view cleanup, pending permissions offline, stale answers, policy mismatches and automatic service shutdown. No account or inference is used. `scripts/test-agent-persistence-ui.py` selects lifetime modes through the native menu, closes/reopens RODE, attaches through Running, continues the same backend and switches persistence Off again. Screenshots/logs are in `target/agent-persistence-ui/`.

The ignored `background_agents_survive_ssh_disconnect_and_keep_their_sandbox` check in `tests/ssh_roundtrip.rs` runs through `scripts/test-ssh.sh`. It covers direct/ProxyJump SSH, normal/actual bwrap operation, work while disconnected, fresh-master reattachment and explicit End. Use `RODE_SSH_TEST_FILTER=background_agents_survive_ssh scripts/test-ssh.sh` for that check alone; build the standalone helper first as described above.

### Complete Codex runtimes

Installer, runtime-discovery and executable-path fixtures create protected
directories explicitly, including their ancestors. They must pass under a
group-writable caller umask without relaxing production verification or changing
the process-wide umask while Rust tests run concurrently.

Optional full-runtime SSH checks serialize their shared fixture cache's initial
upload; the remaining transport tests still run concurrently. Missing system
bubblewrap selects the bundled implementation, so normal-mode UI fixtures must
simulate an unavailable backend rather than merely remove `bwrap` from `PATH`.

The pinned Codex release needs `codex-code-mode-host` and `codex-exec-server` alongside `codex`. Runtime tests check multi-executable transfers, executable permissions, incomplete/corrupt transfers, distinct cache identities for main-only versus complete deployments, and explicit repair that preserves the original main binary and previous selector on download failure. `python3 scripts/test-packaging.py` exercises the same missing-helper repair through the preparation script.

With `RODE_TEST_CODEX` pointing to a complete prepared runtime, the existing loopback Responses test also enables code mode in its isolated configuration and returns a deterministic tool call. It must create a file with a real shell command and return the output to the next model request, in normal mode and an available bwrap sandbox. This uses no account or model and catches runtime layouts that pass initialization/chat-only checks but cannot execute tools. The SSH checks verify complete runtime deployment over direct and ProxyJump connections.
