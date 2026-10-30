# Security review

Review date: **2026-10-29**. This review covers the current local/SSH file services, sandbox construction, preference transactions, subprocess/terminal ownership, encrypted-mount coordination, document loading and extension/agent boundaries. It includes targeted source review, reproductions and dependency advisory checks; it is not a guarantee that every vulnerability has been found.

## Findings fixed

### Named pipes could block file workers

**Availability issue.** Native and Python file readers opened a path before checking that its descriptor represented a regular file. Opening a FIFO for reading can wait indefinitely for a writer, so the later type check never runs. Save revision checks and preference loading/transactions had the same opening pattern. A special file could therefore retain a worker or prevent the corresponding operation from finishing.

The shared native regular-file opener and Python peer now open with `O_NONBLOCK` and check the opened descriptor before reading or locking. Regular files and ordinary file symlinks retain their behavior. Checking only the pathname before opening would leave a replacement race, so descriptor validation remains necessary. This addresses special-file opening; it does not promise a timeout for every filesystem or failing network mount.

Regression tests invoke both backends in deadline-bounded subprocesses. They cover text/automatic/hex/image/PDF reads, text/binary saves, symlinks to FIFOs, and preference reads/transactions without a FIFO writer. Existing file lifecycle, symlink/permission and concurrent-save tests check ordinary behavior.

Implementation: `src/persistence.rs`, `src/files.rs`, `src/remote_files.py`. Regression coverage: `tests/security.rs`.

### A symlinked home could bypass broad-grant rejection

**Sandbox policy issue.** Project and additional grant paths were canonicalized, but the whole-home guard compared them only against the spelling of `HOME`. When `HOME` was a symlink, a path naming its real target or an ancestor could pass that comparison and expose the home tree under a sandbox grant that the guard was intended to reject.

Both backends now check the original home path and its resolved location before accepting project or additional mounts. Failure to resolve the home location is an error during sandbox-policy validation. Normal subprojects remain permitted, and the user-facing home spelling is retained in the launch environment.

Regression tests cover whole-home and parent roots, read-only/read-write additional grants, native/Python agreement, and an allowed subproject through the same home alias. The tests build mount arguments without needing to mount or expose an actual user's home.

Implementation: `src/sandbox.rs`, `src/remote_sandbox.py`. Regression coverage: `tests/security.rs`.

### Background tools retained the launcher's controlling terminal

**Terminal isolation issue.** Native background tools, agents and kernels started in a separate process group but retained the controlling terminal used to launch RODE. A child could still open `/dev/tty`, including inside an active project sandbox. That provided unintended access to the launching terminal; terminal-input injection additionally depends on the host's kernel policy.

Native background workers now create a new session before execution. This removes the inherited controlling terminal and retains the owned process group used for cancellation. The Python project peer already starts its background tools in new sessions. Interactive terminals continue to create their own PTYs with working job control.

A regression test starts the test child under a real controlling PTY, verifies that the fixture can open `/dev/tty`, then checks that both bounded project tools and the agent/kernel transport cannot open it. It covers normal execution and bubblewrap when available. Existing lifecycle tests check cancellation, agent/debugger cleanup and independent interactive terminals.

Implementation: `src/process.rs`, `src/project.rs`. Regression coverage: `tests/security.rs`.

## Dependency audit

`cargo-audit` **0.22.2** checked **361 locked packages** against the RustSec database at commit `ee1a49a2769a24a596afc1c20cb33748d2769586`, updated on the review date. It reported **zero known vulnerabilities** and these informational maintenance warnings:

| Dependency | Dependency path | Advisory |
| --- | --- | --- |
| `bincode 1.3.3` | `syntect`, used by syntax/highlighting libraries | [RUSTSEC-2025-0141](https://rustsec.org/advisories/RUSTSEC-2025-0141.html) |
| `ttf-parser 0.25.1` | `owned_ttf_parser` → `ab_glyph` → egui rendering | [RUSTSEC-2026-0192](https://rustsec.org/advisories/RUSTSEC-2026-0192.html) |
| `yaml-rust 0.4.5` | `syntect`, used by syntax/highlighting libraries | [RUSTSEC-2024-0320](https://rustsec.org/advisories/RUSTSEC-2024-0320.html) |

These advisories describe unmaintained crates and list no patched versions. They remain visible maintenance items; none was suppressed or presented as an exploitable vulnerability. Replacing these transitive components requires compatible upstream dependency changes or a separately assessed integration change. RODE's syntax definitions and native fonts are embedded; custom theme packages use the documented bounded JSON formats.

With an installed [cargo-audit](https://github.com/RustSec/rustsec/tree/main/cargo-audit), repeat the advisory check with `cargo audit --file Cargo.lock`. Results depend on the database at that time. The lockfile audit does not cover installed OpenSSH, bubblewrap, Poppler, gocryptfs, model servers or other external project tools.

## Boundaries and limits

Subsequent feature addition (2026-10-30): the [optional Codex runtime](agents.md#codex) uses committed release/archive/executable/notice pins in a separate package. Its protected installation manifest selects the pinned version; package installation or an explicit bundle override is the trust boundary. Checksums detect damaged assets and are not a publisher signature. Runtime discovery and streamed SSH deployment check sizes/hashes, reject unsafe metadata, symlinked cache trees and unexpected files, and publish under a shared lock without replacing a running cached version. Selector manifests are opened with `O_NOFOLLOW` and `O_NONBLOCK`, then checked for regular-file type, ownership and permissions through the open descriptor. This rejects a symlink even when its target has safe permissions, without a separate check/open race. Agent execution remains inside the project policy; its additional tool mount is read-only. The application does not fetch binaries from the internet or transfer desktop credentials. Regression coverage is in `tests/bundled.rs`; this addition is not covered by the earlier dependency-audit snapshot above.

The reviewed SSH channels use the authenticated master and quoted remote command boundaries. Installed adapters require explicit installation/activation; agent approvals do not change an active sandbox's outer grants. Native terminal control verifies the caller's UID and mount namespace, while encrypted-project locking verifies leases and normal unmount rather than treating a hidden mountpoint as locked.

Normal mode retains the user's host permissions. Project tools and their configuration can execute project code. The optional development sandbox has explicit shared resources, including configurable caches and networking; it is not a complete isolation boundary for arbitrary hostile code. See [sandbox access](sandbox.md), [agents](agents.md), [terminal lifetime](terminals.md) and [encrypted projects](encrypted-projects.md) for the specific contracts. Dependency scanning and bounded inputs complement these boundaries without proving that external parsers/tools are vulnerability-free.
