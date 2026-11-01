# Source architecture

RODE has four Rust library crates: `rode` supplies desktop/editor services, `rode-agent-core` supplies headless agent controllers, `rode-runtime-core` supplies headless notebook/DAP controllers, and `rode-session` supplies their native host service and PTYs. The desktop uses egui/eframe. Local services are Rust; SSH peers use the remote host's Python standard library to deploy/control the native helper without requiring a remote editor or GUI libraries.

Build browsable API documentation with:

```sh
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --open
```

The entry points are `target/doc/rode/index.html`, `target/doc/rode_agent_core/index.html`, `target/doc/rode_runtime_core/index.html` and `target/doc/rode_session/index.html`. Public items document input/output contracts; module comments explain private state and ownership. [Development](development.md) lists validation commands and test fixtures.

## Source map

| Source | Responsibility |
| --- | --- |
| `src/main.rs`, `src/lib.rs` | CLI/helper dispatch, native startup, library entry points |
| `src/app.rs`, `src/app/lifecycle.rs` | Documents, selected workspace, guarded actions and native window lifecycle |
| `src/app/workers.rs`, `src/app/dialogs.rs`, `src/app/appearance.rs` | Captured file/decode jobs and result routing, guarded dialogs/notices, interface scale and theme preferences |
| `src/app/chrome.rs`, `commands.rs`, `icons.rs`, `file_browser.rs` | Tabs/layout, typed palette actions, native icons and shared asynchronous pickers |
| `src/app/navigation/` | Pane focus, validated shortcut presets, explorer keys and native source-editing modes |
| `src/app/workbench.rs`, `src/app/workbench/` | Captured project preferences, Git, code tools, environments, monitoring and settings panels |
| `src/files.rs`, `src/remote_files.py` | Bounded file protocol, content revisions and protected saves |
| `src/ssh.rs`, `src/sandbox.rs`, `src/remote_sandbox.py` | Authenticated transport identity, actual host-policy preparation and sandbox execution |
| `src/persistence.rs` | Locked preference deltas, atomic configuration writes and private framework snapshots |
| `src/project.rs`, `src/languages.rs`, `src/remote_project.py` | Installed tools, embedded language catalogue, normalized diagnostics and host counters |
| `src/project/stash.rs`, `src/remote_git_stash.py` | Captured stash identities, Git-compatible writer locks and protected files-backend reflog removal |
| `src/extensions.rs`, `src/app/extensions.rs`, `src/remote_extensions.py` | Native feature activation, opt-in installed adapters and their UI/preferences |
| `src/process.rs`, `src/remote_agent.py` | Shared bounded JSON-line subprocess transport and owned process-group cleanup |
| `src/bundled.rs`, `src/bundled/install.rs`, `src/app/agents/runtime.rs`, `src/remote_codex.py`, `scripts/bundle-codex.py` | Pinned native Codex discovery, explicit native per-user setup, verified private SSH-cache deployment and release-bundle preparation |
| `crates/rode-agent-core/` | Shared Codex/ACP state machines, bounded chat/history, capabilities, personalities and host-service wire views |
| `crates/rode-runtime-core/`, `crates/rode-session/src/runtimes.rs`, `src/runtime.rs` | Shared notebook/DAP controllers, bounded output/state replay, exclusive leases, mutation acknowledgements and native runtime discovery |
| `src/agent/`, `src/agent.rs`, `src/app/agents.rs`, `src/app/agents/background.rs` | Project-host preparation, disposable agent attachment, live-session discovery, history and permission/question UI |
| `crates/rode-session/src/agents.rs`, `agents/transport.rs` | Headless agent ownership, fixed sandbox/executable/environment, controller leases, bounded subprocess I/O and lifetime cleanup |
| `src/agent/personality.rs` | Bounded response-style hints, explicit default resets and display-only history cleanup; independent of tools and execution policies |
| `crates/rode-agent-core/src/controls.rs`, `controls/catalog.rs`, `src/agent/controls.rs`, `src/app/agents/controls.rs` | Shared bounded model paging, metadata-only discovery and native model/access selectors |
| `src/debugger/`, `src/debugger.rs`, `src/app/debugging/`, `src/remote_debugger.py` | DAP framing/client, shared adapter/debug-PTY coordinator and interactive debugger UI |
| `src/images.rs`, `src/pdf.rs`, `src/binary.rs`, corresponding `src/app/*_view.rs` | Bounded desktop decoding/analysis, memory leases, retained snapshots and specialized tabs |
| `src/app/markdown.rs` | Unsaved-source Markdown rendering and sandbox-aware linked assets |
| `src/notebook/`, `src/notebook.rs`, `src/notebook_kernel.py`, `src/app/notebook_view.rs`, `notebook_view/ui.rs`, `notebook_view/source.rs` | Metadata-preserving notebook model, kernel ownership/versioned execution, cell/output rendering and stateful source editing |
| `src/marp.rs`, `src/latex.rs`, corresponding `src/app/` modules and Python peers | Explicit installed-tool builds/renders, compiler issues, preview/export lifecycles |
| `src/terminal.rs`, `src/terminal/colors.rs`, `src/app/terminal_colors.rs`, `src/sessions.rs`, `src/app/sessions.rs`, `src/bin/rode-sessiond.rs`, `crates/rode-session/` | Native terminal rendering, workspace policy, shared host PTYs, the Cargo-installed headless frontend and view lifetimes |
| `src/slurm.rs`, `src/slurm/`, `src/app/sessions/allocation.rs`, `src/remote_sessions.py` | Interactive allocations, persistent notebook reservations, captured execution targets, stdio compute steps, compute-node sandbox policy, scheduler snapshots and ownership-checked cancellation |
| `src/vault.rs`, `src/remote_vault.py`, `src/app/vaults.rs` | Verified gocryptfs mounts, cross-window leases and guarded locking |
| `src/theme.rs`, `src/theme/syntax.rs`, `themes/` | Independent interface/syntax choices, bounded custom data and cached highlighting |
| `tests/`, `tests/fixtures/`, `scripts/test-*.py` | Service integration tests, deterministic peers and isolated native UI checks |

## Design decisions and tradeoffs

The choices below explain the current implementation. Module-level rustdoc records ownership and local constraints; the main service modules also include **Design decisions** sections beside the code they constrain. The sections below describe consequences as well as benefits; they are not a list of proposed features.

Keep the desktop coordinator focused on application state and shared lifecycle guards. File/decode dispatch belongs in `app/workers.rs`, modal presentation in `app/dialogs.rs`, and appearance controls in `app/appearance.rs`. Domain views own their rendering and controller state; notebook rendering lives in `notebook_view/ui.rs` so it can be changed without navigating kernel ownership and background-result code. Large regression suites live in adjacent `tests.rs` modules and retain their existing Rust module/test paths.

### Native UI and work on demand

The Rust/egui application keeps the editor independent of an embedded browser application runtime. Cached layouts, virtualized rows and explicit worker jobs keep slow I/O and tool execution out of ordinary input/rendering. On-demand activation limits idle work, at the cost of waiting for a tool or preview's first result. This choice does not by itself establish a performance claim: measure relevant release workloads when changing those boundaries. Rationale lives in `src/lib.rs` and `src/app.rs`.

One UI-thread owner avoids concurrent mutation of source buffers. Cancellation alone cannot revoke a result already queued, so generation/source checks remain necessary even for cancellable jobs. Stable tab/cell IDs preserve ownership across reordering, while positional indices are presentation state. See [worker results](#worker-results-and-version-checks).

### OpenSSH and remote service peers

System OpenSSH preserves the user's aliases, keys, jump hosts, agents and authentication workflow. A separate authentication master lets independent file/tool/project-shell channels reuse the same connection. Multiplex-only slave arguments prevent a lost socket from starting an unexpected fresh login; disconnect is handled as an explicit state transition with recoverable buffers.

Standard-library Python peers avoid deploying the desktop editor and its dependencies to every file host. The tradeoff is maintaining equivalent native/Python service behavior and testing both. Persistent terminals use a separate native helper because their owner must survive a file channel or editor process. Rationale lives in `src/ssh.rs` and `crates/rode-session/src/lib.rs`; [remote setup](remote.md) records the prerequisites.

### Sandbox preparation and enforcement

Project file helpers and process trees run inside the selected host's sandbox; display/authentication resources stay outside it. This gives editor reads, shells and tools a consistent project policy without sandboxing the whole desktop application. Shared mount-policy data reduces drift between implementations.

Probing distinguishes requested isolation from what the host can actually enforce, allowing visibly ordinary operation on hosts without usable bubblewrap. That fallback is decided during preparation. Retrying a denied operation outside an already-active sandbox would change the granted access, so it remains a failure. Rationale lives in `src/sandbox.rs`; the concrete access model and exceptions are in [sandbox access](sandbox.md).

### Explicit settings merges and protected source saves

Two windows may hold different stale preference snapshots. Field-level changes merge independent edits into the latest file without a database or a general distributed merge engine. Arrays are replaced as units because concurrent insertion makes array positions unreliable identities. Conflicting explicit edits to the same scalar/array follow transaction order. Unknown fields and baseline-checked record deletion let different versions/windows coexist without erasing data they did not edit.

A stable configuration lock survives atomic replacement; locking only the old inode would stop coordinating new writers after rename. Atomic publication prevents partial contents, while the lock protects the read/merge/replace transaction. Neither replaces the other. Source saves instead coordinate through current/new inodes and atomic creation so repositories need no permanent editor lock files. Content digests detect edits that size/timestamp checks could miss; acknowledgements retain the submitted baseline so newer typing stays dirty.

Persistence uses a worker because lock waits and filesystem sync can stall. Failed writes pause until explicit retry, and eframe receives a private snapshot because its whole-file persistence cannot perform the shared merge. This makes `Storage::flush` asynchronous and final shutdown flushing best-effort when storage fails. Rationale lives in `src/persistence.rs` and `src/files.rs`; [configuration](configuration.md) records supported behavior and limits.

### Separate process and terminal ownership

Bounded stdio protocols reuse authenticated transport without a new application listener or embedded agent runtime. Queue bounds make slow-consumer pressure visible instead of allowing unlimited memory or UI-thread pipe waits. Protocol clients must handle queue-full results; JSON/base64 encoding has overhead in return for simple bounded framing. Background workers start in new sessions to remove the launcher's controlling terminal while retaining owned process groups for cancellation. Interactive shells instead establish their own PTYs. Rationale lives in `src/process.rs`.

A GUI/SSH attachment cannot own a shell that must outlive it. The headless native terminal service owns PTYs, removing a tmux/screen prerequisite and supporting shared views. The cost is native-helper deployment and protocol compatibility across upgrades. Persistence belongs to the session, so closing one window differs from explicitly ending the shared shell. Capability probing preserves existing jobs instead of restarting their service for a newer client. New controller creation probes runtime support, so a service that only supports history deletion is kept alongside a compatible helper rather than receiving unsupported runtime requests. See [process ownership](#process-and-resource-ownership) and [terminal lifetime](terminals.md).

Agent history uses a separately supervised connection so failed or cancelled loading cannot replace the current conversation. Only explicit History starts it; opening the panel remains inert. Validated host/project metadata and current sandbox/permission settings precede replay. On success ownership transfers to the chat, with bounded recent messages and full context retained by the backend. RODE stores no duplicate transcript database. See `crates/rode-agent-core/src/history.rs`, `src/agent/history.rs`, `src/app/agents/history.rs` and [chat history](agents.md#chat-history).

### Host-owned agents

Persistent agents need their complete protocol controller on the host: leaving only the executable alive would strand initialization, permissions and cancellation timers in the departed GUI. `rode-agent-core` shares those controllers between the existing direct-transport API and `rode-session` without pulling desktop libraries into the remote helper. Agents keep the exact captured sandbox and host environment; the attachment broker runs outside that namespace but supplies no tool execution or policy escalation.

One controlling lease serializes prompts and answers, while other windows may list/end jobs. Attachment loss preserves only explicitly persistent sessions. The host owns grace periods, cancellation deadlines and process-tree reaping. The GUI waits for persistence/start/end acknowledgements during clean exit and retains retiring views until their pending lifetime work finishes. Read-only discovery never starts a service or agent. Capability negotiation selects a compatible service generation without terminating older jobs.

Bounded state remains in host memory; backend History is the durable store. Chat updates contain changed messages plus the retained message order, avoiding a full transcript transfer for each streamed token. Detached workers retain bounded output without continuously serializing UI snapshots. Approvals remain pending offline, and reattachment requires the original project policy. These choices keep the background path native and independent of tmux, screen or model servers managed by RODE.

### Native features and installed tools

Bundled features remain Rust modules with native UI and common lifecycle guards. Disabling suppresses activation/work rather than unloading code. Installed adapters use explicit, versioned executable messages instead of a dynamic Rust ABI or a general scripting runtime. This limits extension surfaces and adds process startup overhead, while keeping the execution boundary and source-result guards consistent with other tools.

Desktop-installed, initially disabled manifests keep repository opening separate from executable activation. Tools still run on the project host within its grants; an adapter or agent approval cannot widen that boundary. Installed compilers, formatters, model servers and debugger adapters remain user-managed, with the documented default `uvx ruff` exception. Rationale lives in `src/extensions.rs` and `src/process.rs`; [extensions](extensions.md) defines the supported API.

### Optional native Codex runtime

Codex is a separately versioned native executable in the optional `rode-codex` package, keeping the core editor independent of that payload, VS Code, Node.js and npm. Core packaging never prepares or downloads Codex. SHA-256/size pins and upstream notices are explicit runtime-package build inputs. Source builds optionally prepare the same runtime with a standard-library Python script; the application itself never downloads binaries from the internet or starts inference servers. The optional package's protected manifest selects its version independently of the editor; package installation is the trust boundary, while bounded manifests and checksum validation detect damaged assets. Runtime updates require protocol compatibility checks.

An `auto` executable is resolved only in the process worker after a message. A custom executable bypasses bundle preparation. SSH deployment streams the matching binary over authenticated multiplexed channels, verifies and atomically publishes a private immutable cache, and shares a lock across windows. Credentials remain on their host. An active project sandbox adds only this verified tool directory as a read-only mount; it does not acquire new user-folder or network grants. Versioned directories avoid replacing another process's running runtime. See [agent setup](agents.md) and [packaging](packaging.md).

Runtime completeness is part of Codex verification: main and helper executables have independent archive/file pins, a bounded streaming order, and one read-only sandbox tool-directory mount. The SSH cache key covers all executable pins so a previously verified main-only cache cannot satisfy the complete runtime. Explicit setup can add missing helpers to a verified old local installation without replacing a running main executable; publication updates its manifest/selector only after verification, and rejects unrelated or altered files. No extra payload is downloaded during ordinary startup.

### Document models and editing history

Native image/PDF views avoid a viewer/browser runtime and remote rendering dependencies. Static/page-at-a-time rendering trades general viewer features for bounded resources. Shared leases account for old previews and worker snapshots during reload, while serialized image transfer/decode reduces temporary peaks at the cost of parallel loading. Separate text/byte/read-only paths prevent source formatting or saving from corrupting binary previews. See `src/images.rs`, [images](images.md), [PDFs](pdf.md) and [binary editing](binary.md).

Notebook cells retain unknown JSON/metadata/output bundles so native editing does not silently destroy another tool's data. This duplicates some state within explicit size limits. Kernels start separately from parsing/saving, keeping execution dependencies and lifetime out of ordinary notebook viewing. Rationale lives in `src/notebook.rs`.

Shortcut styles translate commands through the existing TextEdit edit/undo path rather than introducing another buffer engine. The result shares native undo/save/version behavior but implements a documented subset of Emacs/Vim, not full emulation. Syntax themes are independent bounded color data: customization does not require executable theme code, and startup-only loading avoids idle file polling. Rationale lives in `src/app/navigation/editing.rs` and `src/theme.rs`.

### Debugger coordination

DAP separates the native inspection UI from language-specific debuggers. An adapter and its interactive `runInTerminal` targets share one project-host coordinator so separate launches do not put them in unrelated private namespaces or cleanup scopes. Attach is normal-mode only because an already-running target cannot retroactively acquire the project's sandbox policy. These choices require installed adapters and capability-aware behavior. Rationale lives in `src/debugger.rs`; [debugger setup](debugger.md) describes supported backends and limits.

### Verified filesystem encryption

Gocryptfs lets ordinary editor/tools use decrypted project paths while an installed filesystem implementation handles encryption. RODE owns mount verification and leases, not a cryptographic format. This depends on usable host FUSE/tools and protects data at rest rather than arbitrary running-process memory or external caches.

Normal unmount must succeed after jobs/other references stop. Lazy unmount could hide the visible path while a persistent shell or sandbox still retains decrypted access, so it would not justify a locked indicator. Verification and shared leases make closure more involved, but busy/unknown results remain truthful. Rationale lives in `src/vault.rs`; [encrypted projects](encrypted-projects.md) records the workflow.

## Thread and host boundaries

`EditorApp` owns mutable UI state on the egui thread. Directory/file access, SSH channels, external tools, decode/render work and preference-file locking run on workers or helper processes. Workers publish bounded events and wake the UI; they do not mutate documents directly. Rendering uses cached state and virtualized rows rather than launching tools or scanning the filesystem every frame.

Paths are interpreted on the host captured for an operation. For an SSH workspace, `localhost` in agent settings also means the remote host. Avoid using a desktop filesystem lookup to resolve a remote source or executable.

The SSH master owns interactive authentication and its private ControlMaster socket. Slave channels use that socket with fresh-login fallback disabled. Losing a connection invalidates its workspace; retained documents become unbound recovery buffers. Only an explicit same-host/project reconnect restores their bindings. Local Save As creates a separate copy.

Sandbox options express requested access; `Plan::active` records whether the complete host probe succeeded. Active plans prefix project file/tool/agent/debugger/kernel execution. A failed active operation stays failed. Project-folder discovery intentionally reads the selected host so another project can be chosen; ordinary file/hex/Save As browsing uses the current project plan. Monitoring intentionally samples read-only host counters outside the project sandbox. Encrypted-mount management is a separate host-filesystem operation before a decrypted project can be prepared.

## Worker results and version checks

Different identities protect different lifetimes:

| Identity | What it protects |
| --- | --- |
| Workspace generation | Rejects results from an old host, project or disconnected context |
| Request token | Rejects superseded picker, preview, reload or build requests within a context |
| Stable document/cell ID | Follows the same tab/cell across reordering; avoids applying results by position |
| Buffer/source version | Rejects formatting, diagnostics, parsed notebooks and outputs for obsolete source |
| Tool/extension epoch | Invalidates work after interpreter, settings or activation changes |
| Debugger inspection epoch | Prevents paused variable/frame/watch handles from surviving resume or selection changes |

Capture these identifiers and the source snapshot before dispatch. Check them before publishing results. A canceled worker may still deliver a result, so cancellation does not replace validation. Failed/stale preview reloads retain the previous usable view, and closing/replacing a view releases its texture and memory leases.

## File saves and shared preferences

Source and binary reads produce a content digest. A save captures the submitted bytes and expected revision, follows the destination's symlink target, locks the current inode and verifies that it still represents the path after waiting. The writer compares the digest, writes a neighboring temporary inode, preserves existing permissions and publishes atomically. New destinations use publication that refuses replacement. These locks coordinate RODE's Rust/Python writers; unrelated programs still have best-effort conflict detection.

An acknowledgement updates the saved baseline to the submitted snapshot. Edits typed while the save ran remain dirty. Save As must not authorize silently replacing an unrelated existing file. Preview tabs use dedicated read-only paths and never enter text formatting or source saving.

Configuration has a different lock scope: a stable adjacent lock serializes read/merge/replace transactions. Never unlink that lock during cleanup, because another process may be waiting on its inode. `Preferences` computes explicit scalar/nested-field edits against its local baseline, reloads the latest file under the lock and merges only those edits. Unknown fields survive. Structured defaults are comparison data, not implicit user edits.

`Storage::flush` queues preference work; it is not a synchronous durability barrier. Errors remain visible and queued writes pause until an explicit retry. Drop attempts a final flush and joins the worker. Eframe receives a private per-process GUI snapshot so its whole-file writer cannot replace shared application preferences. See [configuration and multiple windows](configuration.md).

## Process and resource ownership

| Owner | Resources and cleanup |
| --- | --- |
| Workspace/window | Private SSH master, background generations, active project tools and UI views |
| `process::Session` | One supervised tool transport and owned process group; stop/drop requests scoped termination |
| Debug coordinator | Adapter and debug-launched PTYs/descendants; detach externally attached targets |
| Native host service | Shell PTYs, parser history, agent protocol controllers and per-session persistence independent of GUI processes |
| Agent view | One controlling attachment; closing it stops only nonpersistent controllers |
| Terminal view | Attachment transport only; last-view closure follows the session's explicit persistence setting |
| Native runtime service | Shared headless notebook queues and DAP controllers, retained outputs/inspection state, exclusive controlling leases and acknowledged persistence |
| Notebook/debugger view | Disposable local/SSH attachment; close follows acknowledged persistence, explicit shutdown ends owned computation |
| Preview lease | Shared accounting retained by the last worker/tab/snapshot owner |
| Encrypted-project lease | Verified mount identity and view ownership; normal locking waits for jobs and other users of the mount |

The native agent worker owns prompt queue scheduling as well as payloads. It drains pending controls before dispatching an eligible head, uses acknowledged settings within the original launch policy, and publishes progress without an attached view. Pause, edits, failed turns and unanswered permissions hold dispatch; the desktop only renders and edits the queue. Capability probing routes new agents to a compatible service while existing controllers retain their original lifetime and dispatch behavior.

Runtime attachments use explicit preparing, connecting, attached, offline and ending states. Shutdown takes ownership of a pending creation reply, including one already queued for the UI, and waits for creation before ending the captured host identity. This prevents Stop or foreground-view disposal from losing a runtime between the worker and the first UI poll. A window-owned notebook shutdown registry retains stopped kernels until the host acknowledges cleanup, even when their views are reparsed, reloaded or removed. Keeping that ownership above document views lets normal exit wait for all shutdowns and report failures after the originating tab disappears.

Confirmed runtime terminations use a bounded worker separate from read-only discovery. Requests capture their original host and policy, survive project-view resets and drain even when the browser drops. Normal application exit waits for their results; a failed termination cancels that exit and remains visible across discovery refreshes. This costs a separate queue but prevents a slow listing from silently discarding an approved mutation.

Use idempotent cleanup and identify owned groups/handles before sending signals. Never clean up unrelated SSH masters, terminal services or processes by executable name. Hidden monitors stop scheduling samples; hidden terminals retain their shell state without forcing repaints. Per-window runtime state is separate from field-merged preferences.

## Protocols and Python peers

`files::REMOTE_HELPER` concatenates the sandbox/project/session/extension/Marp/LaTeX peers, shared JSON policy/catalogue data and the final file dispatcher. Some fragments intentionally use globals/imports supplied by earlier fragments. They are not independent Python packages. Keep their names, concatenation order and Rust wire types aligned. `remote_agent.py`, `remote_debugger.py`, `remote_vault.py` and `notebook_kernel.py` have their own process entry points.

File/project operations use tagged JSON request/reply shapes. Long-lived agent/kernel/coordinator transports use bounded JSON lines. SSH Python peers are sent in a bounded length-prefixed stdin prelude, then executed from a private temporary script with a short role-specific filename. The script removes its file and directory before running the original helper source; exact unbuffered prelude reads leave the following JSON/binary protocol untouched. This keeps helper source out of process arguments without adding a remote installation or persistent cache, at the cost of staging one temporary file per helper. exec retains the supervised PID and process group. DAP messages use Content-Length measured in bytes, including non-ASCII JSON. Binary/image/PDF/PTY payloads use explicit base64 where required by their protocol. Stdout is protocol-only; diagnostic text belongs on stderr or in an explicit result field.

Input, output, frames, directory rows, subprocess duration and retained document data have explicit limits near their implementation. Reject over-limit input with a useful error; do not bypass an active policy or retry on the desktop. When changing a schema, update both native and Python peers and use the [relevant parity, sandbox and SSH tests](development.md).

## Following an operation through the code

The internal entry points below are included by `--document-private-items`. Their comments describe when work is merely queued, which identity must match a reply, and which owner must remain alive. Start at these entry points before changing a feature's rendering or transport.

| Operation | Entry points and completion path | Contract to preserve |
| --- | --- | --- |
| Open a document | `app/workers.rs`: `open_file` → `dispatch` → `poll` | Capture the selected host and actual plan before reading; a late reply cannot bind a tab to another workspace. |
| Save or format | `app.rs`: `save_document`; `workbench/code_tools.rs`: `save_with_tools`, `analyze_document`; `workbench.rs`: `handle_tool` | `false` from `save_with_tools` delegates to ordinary saving. Analysis accepts only its submitted source/version/epoch. Save acknowledgement advances the saved baseline, preserving newer typing. |
| Run a notebook | `notebook_view.rs`: `run_notebook`, `poll_notebooks`, `View::sync`; `notebook/kernel.rs` and `runtime.rs` | Parsing/viewing does not start execution. Outputs bind to captured cell/source versions. Accepted structured edits serialize through the document's normal save path. |
| Stop a notebook | `notebook_view/shutdowns.rs`: `Shutdowns::stop`, `poll`; `sessions/runtimes/ending.rs`: `Queue` | Stopping or removing a tab does not discard cleanup ownership. Confirmed termination retains its original host even if discovery or the project view changes. |
| Submit queued chat | `app/agents/queue.rs`: `Queue`; `app/agents.rs`: `flush_queue`, `poll_agents`; `rode-agent-core::queue`; `rode-session/src/agents.rs` | UI edits transfer once accepted by the bounded transport. The native controller owns scheduling and attachment payloads after detachment. A failed command must remain visible and retryable. |
| Attach a running job | `app/agents/background.rs`, `app/sessions/runtimes.rs`; the host stores' `find`/`attach` methods | Discovery does not create computation. ID and original policy must match; only one controlling agent/runtime attachment is accepted. Terminal views have their separate shared-PTY contract. |
| Change interpreter/settings | `workbench/python.rs`: `python_environment_picker`; `workbench/settings.rs`; `Workbench::invalidate_tools` | Applied settings and form drafts are distinct. New jobs capture accepted settings; existing processes retain their environment unless an explicit supported operation changes it. |
| Refresh telemetry | `Workbench::workbench_tick` → `dispatch_tool` → `monitoring::State::record` → `monitoring/dashboard.rs` | Rendering uses cached successful samples. Rate baselines use successful sample times, not failed-request scheduling times. Missing measurements remain gaps. |
| Navigate debugger source | `app/debugging/navigation.rs`: `Target`, `resolve_debug_source`; `debugging::State::source_version` | A source jump belongs to a stopped inspection epoch and the original host. Resume, changed source or a failed read cannot trigger a stale jump or local fallback. |
| Change appearance | `app/appearance.rs`: `select_theme`, `select_syntax_theme`, `persist_theme_choice`, `set_ui_scale` | Applying appearance is immediate; saving is a separate queued preference edit. Interface scale, editor font size and syntax selection are independent. |
| Close or lock | `app/lifecycle.rs`: `handle_close_request`; `app.rs`: `guarded`; `sessions.rs`: `quit_terminals`; `vaults.rs`: `lock_encrypted`, `poll_vaults` | Native close and menu Quit share guards. A queued lifetime mutation or unmount request is not completed cleanup. Failed cleanup keeps the window and truthful mount state available. |

### Units, identities and absence

These conventions prevent otherwise plausible changes from silently retargeting work:

| Value | Meaning |
| --- | --- |
| `Document::id`, notebook cell `uid`, native runtime ID | Stable identity within its owning lifetime. Vector indices, visible row indices and display names are not substitutes. |
| Document `edit_version`, cell `version`, cell `outputs_version` | Independent source/output revisions used by captured work and render caches. A save acknowledgement is not itself a source edit. |
| Native source selections and editing motions | Zero-based character offsets. UTF-8 byte offsets are converted at string slicing or diagnostic/match boundaries. |
| Source diagnostics, displayed cursor locations and PDF page numbers | One-based positions. Binary offsets are zero-based bytes; UI dimensions are logical points. |
| Monitoring values | Utilization is a percentage in 0–100; memory sizes are bytes and network rates are bytes/second. `None` means unavailable, rather than a measured zero. |
| `Instant` timestamps versus creation timestamps | `Instant` measures local monotonic elapsed time and never crosses the wire. Service metadata records creation in Unix seconds. |
| Requested sandbox options versus `Plan::active` | Options describe desired grants. Only the prepared plan describes effective enforcement, including an explicit host-capability fallback. |
| Start/stop/persistence request acceptance | Ownership transferred to a worker/controller. Callers still poll for startup errors or lifetime acknowledgement before reporting completion or exiting. |
| No cached result versus an empty successful result | A probe may not have run, may be pending or may have failed. Preserve errors and pending flags separately from a valid empty list. |

### Python entry points and composition

`files::REMOTE_HELPER` is the authoritative concatenation order. It embeds sandbox policy, then `remote_sandbox.py`, `environment_probe.py`, `remote_project.py`, `remote_git_stash.py`, `remote_sessions.py`, `remote_extensions.py`, `remote_marp.py` and `remote_latex.py`, followed by the language catalogue and `remote_files.py` dispatcher. Function globals are resolved in that assembled interpreter; some fragments intentionally use services defined elsewhere in the composition. Preserve both the ordering and the final protocol-only stdout.

| Helper | Invocation and ownership |
| --- | --- |
| Assembled file helper | One selected-host file/project request and JSON reply. Adapter/build entry points can use a retained stdin lifeline for cancellation. |
| `remote_agent.py` | Captured subprocess launch followed by bounded forwarded input. EOF/signals stop only the owned child group. |
| `remote_debugger.py` | Coordinator for DAP and its integrated PTYs. Reader/writer threads isolate pipe backpressure; terminal requests use literal arguments. |
| `notebook_kernel.py` | One Jupyter bridge with bounded execution-tagged output. Interrupt/control input remains independent of queued execution; EOF owns kernel cleanup. |
| `remote_codex.py` | Explicit verified deployment/setup of the optional native runtime, separate from conversation execution. |
| `remote_vault.py` | Mount verification and lease broker. Retained references and normal unmount determine the truthful lock result. |
| `slurm/step.py` | Revalidates the captured allocation on the login host, then replaces itself with a stdio compute step. The compute bootstrap resolves Python under the compute host's policy. |
| `ssh/python_stage.py` | Writes and execs the staged helper, removing its temporary source before execution while preserving subsequent protocol bytes. |

Worker callbacks document their EOF, byte-bound and cleanup behavior beside the implementation. Do not turn a read-only discovery helper into an execution path, print diagnostics into a protocol stream, or resolve remote executable paths using the desktop filesystem.

## Documentation maintenance

Document a public item's behavior, units, indexing, side effects and lifetime where those affect its use. State error/absence semantics when an empty result differs from failure, and explain whether a function blocks or merely queues work. Use module comments for shared invariants instead of repeating them on every private getter.

Apply the same contract to internal APIs shared between modules: state types, UI entry points, worker dispatch/completion methods and lifetime operations. Small private formatting helpers and trait implementations can rely on the owning module/type contract when an additional comment would only repeat the code. A private item's visibility is not a reason to omit its cancellation, indexing or ownership requirements.

Private comments should explain ordering and ownership that the code alone cannot show: lock publication, stale-result rejection, preserved source snapshots and signal scope. Keep comments close to that decision and update them when behavior changes. Rustdoc links and missing public docs are checked in CI. Python helper docstrings describe their host/protocol contracts; the existing Ruff checks remain the formatting/lint workflow.

For a design choice, record the motivating constraint, the selected approach and its costs or limits. If a change revisits that choice, update both the owning module's rationale and the relevant guide section, including compatibility/lifetime effects and the checks that substantiate the new behavior. An API-description update alone does not explain a changed design tradeoff.
