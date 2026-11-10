# Security review

Latest review: **2026-11-10**. This review covers local/SSH file services, executable-cache publication, sandbox construction, preference transactions, subprocess/terminal ownership, encrypted-mount coordination, document loading and extension/agent boundaries. It includes targeted source review, reproductions and dependency advisory checks; it is not a guarantee that every vulnerability has been found.

The general Python host backend described in the historical findings below has been removed. Current local/headless regressions exercise the shared Rust services. Findings from the earlier **2026-10-29** review remain recorded below with current implementation references.

## Findings fixed

### Transient commands could outlive their capture operation

**Process ownership/availability issue (additional pass, 2026-11-10).** The
bounded binary helper runner killed only its immediate child on timeout.
Descendants could remain alive after the reported failure, including retaining
pipes or continuing project work. Both this runner and the project command
runner could also return success while a descendant that had closed its output
continued running. The binary runner additionally inherited the launcher's
controlling terminal and silently accepted stderr beyond its documented budget.

Host helpers and project commands now share `command/capture.rs`. It starts a
new session, enforces independent stdout/stderr budgets while reading, and owns
a scoped child guard that stops the process group on completion and every error
path before collecting output. It preserves binary transfers, streamed input,
stdin cancellation leases and the renderer's existing SIGINT cleanup grace.
Project commands retain their per-stream budgets; binary helpers retain the
caller-selected stdout budget and a separate 64 KiB stderr budget. Exceeding
either budget fails the operation instead of silently clipping diagnostics.

The owner observes leader exit using Linux `waitid(WNOWAIT)` and sends the final
group signal **before** reaping, keeping the leader's PID reserved until signals
are finished. It disarms signalling when `waitid` reports that child ownership
has already been lost (`ECHILD`). The behavior follows the
[`waitid` contract](https://man7.org/linux/man-pages/man2/wait.2.html).

Four deterministic regressions failed before the fix: helper timeout left a
child alive, helper success left a child alive, project success left a child
alive, and successful helper output above the stderr budget was accepted. They
pass after the fix. Additional checks cover descendants holding output pipes,
unchanged binary transfers (including the uncapped document-response setting),
exit diagnostics and isolation from an independent command. The existing
real-PTY regression now also requires bounded binary helpers to be unable to
open the launcher's `/dev/tty`.

Implementation: `crates/rode-host-core/src/command/capture.rs`, `command.rs` and
`project.rs`. Regressions: `command/capture_tests.rs`, existing command tests and
`tests/security.rs::background_terminal_child`, driven by its real-PTY harness.
Rebuild the editor and native helper and reconnect SSH hosts to deploy new
capture code. The wire protocol is unchanged. Persistent service jobs retain
their independent ownership; transient helpers must not launch retained work by
backgrounding a child and exiting. Descendants that deliberately create another
session escape process-group cleanup; this is supervision under the existing
sandbox policy, not a replacement for kernel isolation.

### Directory-only opens could wait on substituted FIFOs

**Conditional availability issue (follow-up pass, 2026-11-10).** LaTeX output
locking and several directory-only permission/durability operations used plain
`File::open`. Earlier pathname checks or successful directory creation do not
ensure that the entry is still a directory when it is opened. A process able to
replace a writable directory entry in that interval could substitute a FIFO,
making the open wait for a writer, or substitute a regular file that the open
would incorrectly accept as a directory handle.

The shared `locking::open_directory` now requires `O_DIRECTORY` at open time,
with `O_NONBLOCK`, `O_NOCTTY` and close-on-exec. LaTeX output locks, configuration
and runtime-cache directory syncs, native installation publication and encrypted
mount directory handles all use it. Ordinary symlinks to directories retain
their existing behavior. These flags follow the Linux
[`open(2)` contract](https://man7.org/linux/man-pages/man2/open.2.html).

A deterministic regression first creates and canonicalizes an output directory,
then renames it and substitutes a FIFO before invoking the directory opener.
The former plain-open primitive failed its one-second deadline; the guarded
open rejects it immediately. A second regression checks normal directories and
directory aliases while refusing regular files, their aliases and missing paths.
This reproduces the vulnerable open sequence, rather than claiming a full
application exploit or privilege escalation.

Implementation: `crates/rode-host-core/src/locking.rs`, `latex.rs`,
`persistence.rs`, `runtime_cache.rs`, `vault.rs` and `src/bundled/install.rs`.
Regressions: `locking::tests::checked_directory_replaced_by_a_fifo_is_rejected_without_waiting`
and `locking::tests::directory_handles_allow_directory_aliases_and_reject_other_entry_types`.
Rebuild the editor and native helper, and reconnect SSH hosts to deploy the new
helper. Existing one-shot helpers retain their old code until they exit. The
session wire protocol is unchanged. The guard validates the opened type; it
does not pin ancestor names, forbid directory symlinks or guarantee deadlines
for unavailable/network filesystems.

### Runtime artifacts could redirect host file writes

**Local integrity/availability issue (2026-11-10).** Service startup opened
`service.log` with an ordinary append operation. A process able to plant an entry
in a writable runtime directory could redirect host-side log writes to another
file through a symbolic or hard link, or make the open wait on a FIFO. A
controlled reproduction replaced the log with a symlink to a private fixture
file and demonstrated the helper appending its error message to that file.

Shared private-file opening now rejects leaf symlinks, nonregular files, multiple
hard links, foreign owners and group/other permissions before writing. Opens are
nonblocking and cannot acquire a controlling terminal. Runtime locks and
configuration locks/temporary files use the same guard.

On Linux, service startup captures and validates the runtime directory descriptor
and addresses logs, locks, capability publication, socket binding and cleanup
through that inode. Renaming the directory and substituting a symlink therefore
cannot redirect those operations. The filesystem socket is created with private
permissions, avoiding a pathname chmod race; the original process mask is
restored before workers or user processes start.

Regressions preserve unrelated files behind symlinks and hard links, reject
FIFOs without waiting, check append/nontruncating-lock behavior and substitute a
runtime-directory path after capture. The log redirection regression failed
before the fix and passes after it.

Implementation: `crates/rode-host-core/src/locking.rs`, `persistence.rs`,
`crates/rode-session/src/lib.rs` and `control_tests.rs`. These checks protect
runtime artifacts; they do not isolate unrestricted processes with the same UID.

### Unauthenticated control connections could create unbounded workers

**Local availability issue (2026-11-10).** The persistent service spawned a
blocking thread for every accepted control socket before authenticating it.
A reachable local peer could hold connections open without proving access to
the host capability, growing the number of threads and open descriptors.
Authentication deadlines alone did not bound simultaneous resource use.

The service now admits at most 32 pending authentication/protocol handshakes
before creating workers and closes excess connections immediately. Completed
handshakes release their slots so established terminal views retain their
separate limits. EOF and authentication failures release admission automatically;
failed thread creation or socket cloning no longer panics the daemon.

The regression opens the full pending-connection budget, requires immediate EOF
for an excess peer, releases a slot and verifies authenticated listing of the
same still-running terminal. It failed against the previous implementation and
passes with admission control. This bounds unauthenticated worker growth; it
does not promise availability against a sustained connection flood.

Implementation: `crates/rode-session/src/lib.rs`. Regression:
`tests/sessions.rs::unauthenticated_connections_are_bounded_without_ending_retained_work`.

### Vendored SSH adapters retained an affected RSA prerelease

**Dependency finding (2026-11-10).** The build-compatible
`rsa 0.10.0-rc.18` pin is flagged by
[RUSTSEC-2023-0071](https://rustsec.org/advisories/RUSTSEC-2023-0071.html), whose
2026-11-08 update lists stable `rsa >=0.10.0` as patched for known timing issues.
A fresh audit of the previous lockfile reproduced the finding. This review did
not attempt private-key recovery or establish an exploit against RODE's SSH use.

Both vendored SSH adapters now use stable `rsa 0.10.0` and `pkcs1 0.8.0` where
needed. The adapters explicitly enable PKCS#1 v1.5 signatures, export the new
validated-prime representation and share the RSA library's PKCS#1 decoder for
PKCS#8 payloads. Regression coverage checks plain/encrypted key round trips,
signatures, changed-message rejection and wrong passwords; native SSH tests also
exercise RSA host verification and modern RSA SHA-2 negotiation.

New work selects **service generation 23**. Rebuild and install both editor and
native helper. Earlier daemons keep their existing jobs and code; end and restart
affected work to acquire the fixes. Updated clients do not migrate those jobs.

Implementation: `vendor/russh`, `vendor/ssh-key`, `Cargo.lock` and the service
generation constant. Regression:
`tests/ssh_client.rs::rsa_private_key_formats_preserve_signatures_after_the_crypto_upgrade`.

### Custom Codex state directories could be read through file-only tools

**Configuration/credential disclosure (fourth pass, 2026-11-06).** File-only tools excluded directories literally named `.codex` and prevented modifications to the configured `CODEX_HOME`, but did not apply that protection to reads, listings or recursive searches. If the agent inherited a custom `CODEX_HOME` inside the editable project, native file tools could return its configuration or credentials as ordinary project text. The prior static helper returned a fake credential from such a directory in a controlled reproduction; no real credentials or model account were used.

The helper now derives its effective state directory independently and excludes it from every file operation. Controllers additionally capture sandbox environment overrides and host/mount aliases. Absolute, project-relative, symlinked and lexically normalized state paths are protected. Unset/empty `CODEX_HOME` uses the usual home-directory default; the helper also handles a HOME-less Unix environment through the account database and fails closed if it cannot determine the state root. These rules follow the backend's [Codex home-directory semantics](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/utils/home-dir/src/lib.rs).

Direct and host-owned protocol regressions verify denied reads/edits/deletes, omitted directory entries and excluded search results while ordinary source edits still succeed. A separate helper regression omits all optional controller hints, verifying that its actual environment remains authoritative. Session protocol **15** and required helper capability `agent_private_state` select updated controllers for new jobs while preserving older jobs. **End and restart existing chats to acquire the fix**; reattaching an old controller does not replace its code or captured policy. These checks apply to native file tools; full command access retains the selected project policy. A trusted custom wrapper that changes `CODEX_HOME` internally must keep private state outside the editable tree or expose the setting to RODE before launch.

Implementation: `crates/rode-host-core/src/agent_files.rs`, `crates/rode-host-core/src/command.rs`, `src/agent.rs`, `crates/rode-session/src/agents/transport.rs` and helper/service compatibility checks. Regression coverage: `tests/agent_files.rs`, shared file-tool/command unit tests and authenticated current-protocol service tests.

### File-only tools could protect the wrong executable path

**File-only policy bypass (third pass, 2026-11-06).** The direct agent API protected the configured command string instead of the executable selected during asynchronous launch. With a bare custom command name found through `PATH`, native file tools could therefore overwrite an agent script within the editable project. A deterministic protocol fixture reproduced a successful self-edit while file-only access was selected. The reproduction wrote harmless replacement text; it did not attempt a code-execution payload. Replacing a trusted agent script could affect later agent execution, so the executable guard must not depend on configuration spelling.

The launch worker now publishes immutable executable identities before spawning the local process. File-tool requests protect both the launch path and its host alias, and reject missing/unresolved identities. Weak references retain protection for live/retiring transports without retaining completed controllers. Host-side command discovery also resolves relative/empty `PATH` entries against the child's project directory, rather than the broker's working directory; the selected absolute path is then used for launch and protection. Bind aliases for packaged runtimes are translated through the captured sandbox policy, and native service tools protect both spellings too.

Regression tests cover direct and host-owned file-only agents, absolute/relative `PATH` entries and an editable script executable. They also check longest-prefix bind translation and that command/environment arguments are not interpreted as mounts. Existing real-Codex, offline-tool, recovery and persistent-agent tests continue to exercise the protocol and lifecycle restrictions.

Implementation: `src/process.rs`, `src/agent.rs`, `crates/rode-host-core/src/command.rs`, `crates/rode-session/src/agents/transport.rs`. Regression coverage: `tests/agent_files.rs` and shared command-unit tests. This finding concerns native file-tool executable protection; it does not demonstrate a kernel sandbox escape.

### Special files could stall render preparation or editor startup

**Availability issue (second pass, 2026-11-06).** LaTeX root-comment resolution used a blocking open before validating the source type. A saved source could point `% !TeX root` at a FIFO and retain the coordinator before the compiler and its deadline even started. Successful LaTeX/Marp tools could also leave FIFO output paths that blocked preview reads after the tool exited. A bounded reproduction confirmed the LaTeX root-comment hang without starting a compiler.

Those reads now use the shared nonblocking regular-file opener. Deadline-bounded desktop/headless regressions cover direct and symlinked FIFO root targets, unusable generated PDF paths and a converter that produces a FIFO. A successful LaTeX build with invalid preview output still returns its build result and a preview warning.

SSH alias discovery also ran synchronously during editor construction using an unbounded configuration read. A FIFO at `~/.ssh/config` could therefore stall startup; an oversized file could allocate without the documented configuration bound. It now uses the bounded regular-file configuration reader. Subprocess regressions exercise FIFOs, FIFO symlinks, oversized/invalid configuration and ordinary sorted/deduplicated aliases. Interface/syntax theme loaders now use nonblocking descriptor validation too, so replacing a file with a FIFO after the existing metadata preflight cannot block the subsequent open. Document-size behavior is unchanged.

Implementation: `crates/rode-host-core/src/latex.rs`, `crates/rode-host-core/src/marp.rs`, `src/ssh.rs`, `src/theme.rs`, `src/theme/syntax.rs`. Regression coverage: `tests/security.rs`, together with the existing LaTeX, Marp and theme suites. These are availability fixes; this pass did not demonstrate another code-execution or sandbox-escape vulnerability.

### Writable ancestors could redirect a verified executable cache

**Conditional local code-execution issue (2026-11-06).** The SSH helper bootstrap checked ownership and permissions of the home/cache directories and verified helper bytes, but did not check every parent directory. If another user could rename a non-sticky writable ancestor, that user could replace the private directory's pathname between verification and execution. The same missing ancestor check affected optional Codex runtime discovery, installation and host-side runtime-cache publication. Ordinary homes beneath protected parents were not affected by this condition.

A deterministic reproduction published harmless verified bytes beneath a world-writable ancestor, renamed the private home and substituted different bytes at the returned pathname. It did not execute the replacement. After the fix, the bootstrap rejects that layout before creating its cache. Runtime caches now require root/current-user ownership and no group/other writes throughout their ancestry. Root/current-user-owned sticky parents such as `/tmp` remain supported. Rust/Python checks inspect symlink targets as well as the final resolved path, so an alias that traverses an unsafe tree and returns to a protected tree cannot hide the unsafe component. The SSH bootstrap publishes through its validated canonical home path.

These checks complement existing leaf metadata, checksum, locking and atomic publication checks. They do not isolate unrestricted processes running as the same UID or root. Unsafe shared-parent configurations fail closed; use an owner-controlled cache location rather than weakening the parent checks.

Implementation: `crates/rode-host-core/src/paths.rs`, `crates/rode-host-core/src/runtime_cache.rs`, `src/host/bootstrap.sh`, `src/bundled.rs`, `src/bundled/install.rs`, `scripts/bundle-codex.py`. Regression coverage: native bootstrap and runtime-cache integration tests, user-runtime discovery, installer packaging tests and ancestor/symlink unit tests.

### Patched Markdown parser advisory

The fresh audit found [RUSTSEC-2026-0433](https://rustsec.org/advisories/RUSTSEC-2026-0433.html) in `pulldown-cmark 0.13.5`: nested image wikilinks can cause quadratic parsing when `ENABLE_WIKILINKS` is enabled. RODE and its current Markdown renderer do not enable that option, so this review did not identify an exposed application denial of service through that feature. The dependency minimum and lockfile were nevertheless updated to the patched **0.13.6**. A regression test keeps untrusted wikilinks literal and verifies that they do not request images or create links.

### Earlier findings (2026-10-29)

#### Named pipes could block file workers

**Availability issue.** Native and Python file readers opened a path before checking that its descriptor represented a regular file. Opening a FIFO for reading can wait indefinitely for a writer, so the later type check never runs. Save revision checks and preference loading/transactions had the same opening pattern. A special file could therefore retain a worker or prevent the corresponding operation from finishing.

The shared native regular-file opener opens with `O_NONBLOCK` and checks the opened descriptor before reading or locking. Regular files and ordinary file symlinks retain their behavior. Checking only the pathname before opening would leave a replacement race, so descriptor validation remains necessary. This addresses special-file opening; it does not promise a timeout for every filesystem or failing network mount.

Regression tests invoke both backends in deadline-bounded subprocesses. They cover text/automatic/hex/image/PDF reads, text/binary saves, symlinks to FIFOs, and preference reads/transactions without a FIFO writer. Existing file lifecycle, symlink/permission and concurrent-save tests check ordinary behavior.

Implementation: `src/persistence.rs`, `crates/rode-host-core/src/persistence.rs`, `crates/rode-host-core/src/files.rs`, `crates/rode-host-core/src/locking.rs`. Regression coverage: `tests/security.rs`.

#### A symlinked home could bypass broad-grant rejection

**Sandbox policy issue.** Project and additional grant paths were canonicalized, but the whole-home guard compared them only against the spelling of `HOME`. When `HOME` was a symlink, a path naming its real target or an ancestor could pass that comparison and expose the home tree under a sandbox grant that the guard was intended to reject.

Both backends now check the original home path and its resolved location before accepting project or additional mounts. Failure to resolve the home location is an error during sandbox-policy validation. Normal subprojects remain permitted, and the user-facing home spelling is retained in the launch environment.

Regression tests cover whole-home and parent roots, read-only/read-write additional grants, native/Python agreement, and an allowed subproject through the same home alias. The tests build mount arguments without needing to mount or expose an actual user's home.

Implementation: `crates/rode-host-core/src/sandbox.rs`, `crates/rode-host-core/src/landlock.rs`. Regression coverage: `tests/security.rs`.

#### Background tools retained the launcher's controlling terminal

**Terminal isolation issue.** Native background tools, agents and kernels started in a separate process group but retained the controlling terminal used to launch RODE. A child could still open `/dev/tty`, including inside an active project sandbox. That provided unintended access to the launching terminal; terminal-input injection additionally depends on the host's kernel policy.

Native background workers now create a new session before execution. This removes the inherited controlling terminal and retains the owned process group used for cancellation. The Python project peer already starts its background tools in new sessions. Interactive terminals continue to create their own PTYs with working job control.

A regression test starts the test child under a real controlling PTY, verifies that the fixture can open `/dev/tty`, then checks that both bounded project tools and the agent/kernel transport cannot open it. It covers normal execution and bubblewrap when available. Existing lifecycle tests check cancellation, agent/debugger cleanup and independent interactive terminals.

Implementation: `src/process.rs`, `crates/rode-host-core/src/command.rs`, `crates/rode-host-core/src/project.rs`. Regression coverage: `tests/security.rs`.

## Dependency audit

On **2026-11-10**, `cargo-audit` **0.22.2** checked **510 locked packages** against
RustSec commit `58a64e69d7a08a2b83a80efa6e4b018a9acb51fc`, last updated
**2026-11-10** (1,430 advisories). The previous RSA prerelease produced one known
vulnerability finding; after the migration the audit reports **zero known
vulnerabilities**. The three unmaintained-crate warnings below remain visible
and unsuppressed. Path dependencies, including locally patched SSH source, still
require source review; a lockfile scan does not certify them.

On **2026-11-06**, `cargo-audit` **0.22.2** checked **384 locked packages** against the RustSec database at commit `fbba727d074c77fe5f7aced91930ad33eede654c`, last updated **2026-11-06** (1,413 advisories). After the parser update it reported **zero known vulnerabilities** and these informational maintenance warnings:

| Dependency | Dependency path | Advisory |
| --- | --- | --- |
| `bincode 1.3.3` | `syntect`, used by syntax/highlighting libraries | [RUSTSEC-2025-0141](https://rustsec.org/advisories/RUSTSEC-2025-0141.html) |
| `ttf-parser 0.25.1` | `owned_ttf_parser` → `ab_glyph` → egui rendering | [RUSTSEC-2026-0192](https://rustsec.org/advisories/RUSTSEC-2026-0192.html) |
| `yaml-rust 0.4.5` | `syntect`, used by syntax/highlighting libraries | [RUSTSEC-2024-0320](https://rustsec.org/advisories/RUSTSEC-2024-0320.html) |

These advisories describe unmaintained crates and list no patched versions. They remain visible maintenance items; none was suppressed or presented as an exploitable vulnerability. Replacing these transitive components requires compatible upstream dependency changes or a separately assessed integration change. RODE's syntax definitions and native fonts are embedded; custom theme packages use the documented bounded JSON formats.

With an installed [cargo-audit](https://github.com/RustSec/rustsec/tree/main/cargo-audit), repeat the advisory check with `cargo audit --file Cargo.lock`. Results depend on the database at that time. The lockfile audit does not cover installed OpenSSH, bubblewrap, Poppler, gocryptfs, model servers or other external project tools.

The vendored sandbox manifest pins bubblewrap **0.13.1** and libcap **2.78**. These are beyond the fixes listed in the upstream [bubblewrap setup advisory](https://github.com/containers/bubblewrap/security/advisories/GHSA-pxhw-h44j-8pfx) (0.12.0), [bubblewrap setuid advisory](https://github.com/containers/bubblewrap/security/advisories/GHSA-xq78-7hw4-5jvp) (0.11.2) and [libcap file-capability advisory](https://github.com/AndrewGMorgan/libcap_mirror/security/advisories/GHSA-f78v-p5hx-m7hh) (2.78). Packaging tests verify the vendored file hashes. This check does not establish that every C dependency or installed system executable is free of vulnerabilities; system binaries still need distribution security updates.

## Validation snapshot

The **2026-11-10 full-suite remediation run** passed **938 ordinary Rust tests**
across 49 executables, with 40 declared harness/opt-in cases ignored and none
filtered out. The run supplied a current native helper and installed real-tool
fixtures, including Jupyter, debugpy, LLDB, formatters and the isolated Codex
loopback peer. The final desktop check passed all 418 tests (4 declared ignores).
The focused agent-network, agent and session suites also passed all 59 tests
with `RODE_TEST_REQUIRE_LANDLOCK=1`. Private per-executable homes and fixture
session/vault directories resolved the earlier real-home and global-directory
failures without relaxing permissions. Workspace-local Poppler and an owned,
authenticated X display supplied the preview and native UI prerequisites that
earlier invocations had missed.
Native UI cases for workspace, agents, terminals, tools, local/remote documents,
SSH smoke workflows and local/remote runtimes passed after fixture corrections.
Clippy, formatting, warning-free Rustdoc, strict MkDocs, doctests, standalone
helper compilation and the CI/packaging/private-home runner regressions passed.

This remains an incomplete platform-enforcement result. Four required
Bubblewrap suite entries fail because the host denies namespace/network setup.
The real SSH transport run passed 23 cases and failed agent forwarding; a
separate plain OpenSSH probe reproduced sshd's `mkdtemp(): Permission denied`.
Four Kerberos cases passed, while delegation reached sshd and failed its
hardcoded `/tmp/krb5cc_*` cache creation with `Permission denied`. These failures
remain visible. Real gocryptfs initialization and wrong-password checks passed,
but opening `/dev/fuse` is denied, so mounting was not exercised. A device node's
existence alone is not evidence that the fixture can use it. The ordinary pass
does not establish these unavailable enforcement capabilities.

The **2026-11-10 additional process-capture pass** passed 10 shared command
checks, 3 focused project-runner checks, 17 native-host tests with
`RODE_HOST_HELPER` explicitly set to the built helper, 6 file-security tests
(2 subprocess entry points ignored by the outer harness), 5 concurrency tests,
5 extension tests, 3 Git-credential tests, 6 stash tests, 5 LaTeX tests and all
58 terminal/agent/runtime lifetime tests. These include the new real-process
regressions and the extended real-controlling-PTY check. Native-host tests run
without an explicit helper can return early; the initial invocation without
that fixture was not counted as gateway validation.

The Marp suite passed four cases and failed its PDF-preview case because this
environment lacks Poppler's `pdfinfo`; the cancellation case passed. The ordinary
agent-network invocation reported four passes but can skip unavailable
enforcement. Requiring Landlock explicitly then failed two cases because scratch
creation beneath the real home was denied by this execution environment. These
failures do not establish actual offline-tool enforcement for this pass. Cases
requiring missing external tools or optional runtime fixtures can also return
early. This is not a fully green broad/platform test run. Workspace Clippy,
warning-free private-item Rustdoc, formatting, strict MkDocs and standalone-helper
compilation passed. The refreshed 510-package audit used RustSec commit
`58a64e69d7a08a2b83a80efa6e4b018a9acb51fc` (updated 2026-11-10) and reported
zero known vulnerabilities and the three unsuppressed maintenance warnings.

The **2026-11-10 follow-up** directory-open checks passed all 5 shared file-guard
tests (including the reproduced FIFO substitution), 7 native installer tests
(1 explicit-download case ignored), 12 bundle tests, 5 LaTeX tests, 17 native-host
tests, 26 persistence tests (1 subprocess harness ignored) and 6 file-security
tests (2 subprocess harnesses ignored). Tests for unavailable external tools or
sandbox backends can return without exercising them; their availability remains
a separate validation requirement. Two encrypted-project integration tests
failed because this execution environment denies creating their required
`/tmp/rode-vaults-<uid>` directory. A direct directory-creation probe reproduced
`PermissionDenied`, and both helper entry points failed before returning a vault
reply. The optional real-FUSE case returned without an available fixture; it is
not evidence that FUSE enforcement passed. This is not a fully green integration
or native-GUI run. Workspace Clippy, warning-free private-item Rustdoc, formatting,
strict MkDocs and standalone-helper compilation passed. At that earlier point, a freshly fetched RustSec audit used commit
`d9cb059f3217db87aeccd190b81eaffe8cc29cb9` and reported zero known vulnerabilities
with the same three unsuppressed maintenance warnings documented above.

The **2026-11-10** targeted checks passed 28 terminal-service integration tests,
14 native SSH/crypto tests, 6 file-security tests (2 harness cases ignored),
17 native-host tests, 5 runtime integration tests, 26 preference-persistence
tests (1 subprocess harness ignored), 3 shared private-file tests and 35
headless-service unit tests. The unauthenticated-overload and redirected-log
regressions failed before their fixes and passed afterward. A parallel
background-agent run passed 24 tests and failed one history-initialization case;
that case passed an isolated recheck. This is an observed intermittency, not a
fully clean broad test result. These checks do not establish exhaustive exploit
coverage or a fully passing workspace/native-GUI run.

The fourth 2026-11-06 pass attempted the full workspace suite with installed real adapters. The optional `real_lldb_native_breakpoints_and_steps` test timed out, and an isolated recheck timed out too. That was **not a fully green workspace run**. A subsequent run excluding only that test passed **695 Rust tests**, with 27 harness/opt-in tests ignored and the single LLDB test filtered out. The preceding third-pass full run had passed all 693 tests then present.

The later release-readiness review traced that LLDB timeout to loading unrelated shared-library debug information, including the adapter's terminal launcher before it executed the fixture. The deterministic fixture now disables eager symbol preloading and external system-symbol lookup; its local program symbols, all C/C++/Rust breakpoint/step/evaluation assertions and 25-second operation deadline remain. The focused eight-test debugger suite passed with real LLDB/debugpy. Ordinary debugger defaults and sandbox enforcement were not changed. See [CI diagnostics and fixture scope](development.md#ci-tiers-and-timings).

The follow-up full workspace run passed **699 Rust tests**, with 27 harness/opt-in tests ignored and **none filtered out**. The real LLDB check passed in that run. All **20 CI-runner regressions**, native Environment/agent UI checks, the explicit sandbox-fallback UI check and the active Landlock project UI check also passed, along with Clippy, Ruff, Rustdoc and strict MkDocs checks. Recovery and legacy-service upgrade regressions passed without requiring changes to their behaviour. The local kernel/enforcement limits below still apply.

The fourth pass also passed all **six agent UI checks**, **10 packaging tests**, **19 CI-runner regressions**, Clippy with warnings denied, Ruff checks/formatting, Rustdoc with warnings denied and the strict MkDocs build. The static musl helper was rebuilt and verified. Across the four passes, native installer, Markdown, syntax-theme, LaTeX, Marp and agent UI checks plus SSH tests exercised ordinary installation, cancellation, protected paths, remote project/file operations and direct/ProxyJump connections. The fourth-pass SSH fixture explicitly ran 16 opt-in transport tests that the ordinary workspace invocation ignores. These checks establish the tested behavior, not exhaustive exploit coverage.

## Boundaries and limits

The [optional Codex runtime](agents.md#codex) uses committed release/archive/executable/notice pins. Its protected installation manifest selects the pinned version; package installation, explicit per-user setup or a bundle override is the trust boundary. Checksums detect damaged assets and are not a publisher signature. Runtime discovery and streamed SSH deployment check sizes/hashes, reject unsafe metadata, symlinked cache trees and unexpected files, and publish under a shared lock without replacing a running cached version. Selector manifests are opened with `O_NOFOLLOW` and `O_NONBLOCK`, then checked for regular-file type, ownership and permissions through the open descriptor. This rejects a symlink even when its target has safe permissions, without a separate check/open race. Agent execution remains inside the project policy; its additional tool mount is read-only. Desktop credentials are never transferred. Regression coverage is in `tests/bundled.rs`.

The explicit **Install Codex** action downloads only from the editor's embedded official release pin, over HTTPS with verified certificates and HTTPS-only redirects. It bounds network reads, archive bytes and decompression, requires exactly the expected regular member, verifies hashes and complete notices, and publishes under a per-user lock shared with scripted setup. It never executes archive contents during installation, follows no archive links, and rejects changed existing destinations. Cancellation/errors preserve the old selector and clean this job's temporary files. Setup deliberately operates on the desktop's application data outside project sandbox grants; agent execution still enters the selected project-host sandbox. Startup, Send and History never initiate internet downloads. Deterministic installer tests cover corruption, size limits, link entries, concurrent publication, cancellation and unsafe installation paths; they do not download a real runtime.

The reviewed SSH channels use the authenticated master and quoted remote command boundaries. Installed adapters require explicit installation/activation; agent approvals do not change an active sandbox's outer grants. Native terminal control verifies the caller's UID and mount namespace, while encrypted-project locking verifies leases and normal unmount rather than treating a hidden mountpoint as locked.

Normal mode retains the user's host permissions. Project tools and their configuration can execute project code. The optional development sandbox has explicit shared resources, including configurable caches and networking; it is not a complete isolation boundary for arbitrary hostile code. See [sandbox access](sandbox.md), [agents](agents.md), [terminal lifetime](terminals.md) and [encrypted projects](encrypted-projects.md) for the specific contracts. Dependency scanning and bounded inputs complement these boundaries without proving that external parsers/tools are vulnerability-free.

Local enforcement checks exercised Landlock **ABI 4**. This host cannot create the namespaces required by bubblewrap, so real bubblewrap enforcement still requires the dedicated CI host; ABI 5/6 execution was not validated on this kernel. Tests that report an unavailable fixture are not evidence that its enforcement passed. Opening uncapped documents can exhaust available memory; removal of fixed document-size caps does not provide process-level resource isolation.
