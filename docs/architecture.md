# Source architecture

RODE has seven Rust library crates. `rode` owns the egui/eframe desktop; `rode-host-protocol` owns bounded host wire contracts; `rode-host-core` owns shared headless filesystem, command, environment and sandbox services; `rode-agent-core` and `rode-runtime-core` own agent and notebook/DAP controllers; `rode-session` composes the native host gateway and independent persistent service; `rode-bwrap` builds the pinned native sandbox fallback. The native SSH backend links no GUI libraries and needs no Python for general host operations. Only Python-specific interpreter and Jupyter adapters use Python at runtime.

The [native host architecture](#native-host-architecture) below records the host services, deployment contract and supported limits.

Build browsable API documentation with:

```sh
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --document-private-items --open
```

The entry points are `target/doc/rode/index.html`, `target/doc/rode_agent_core/index.html`, `target/doc/rode_runtime_core/index.html`, `target/doc/rode_session/index.html`, `target/doc/rode_host_core/index.html`, `target/doc/rode_host_protocol/index.html` and `target/doc/rode_bwrap/index.html`. Public items document input/output contracts; module comments explain private state and ownership. [Development](development.md) lists validation commands and test fixtures; the [release checklist](release.md) distinguishes ordinary checks from platform and package validation.

## Source map

| Source | Responsibility |
| --- | --- |
| `src/main.rs`, `src/lib.rs` | CLI/helper dispatch, native startup, library entry points |
| `src/app.rs`, `src/app/lifecycle.rs` | Documents, selected workspace, guarded actions and native window lifecycle |
| `src/app/frame.rs` | Application menus, connection banners, activity controls and pane entry views; routes through the existing lifecycle guards |
| `src/app/workers.rs`, `src/app/dialogs.rs`, `src/app/appearance.rs` | Captured file/decode jobs and result routing, guarded dialogs/notices, interface scale and theme preferences |
| `src/app/chrome.rs`, `commands.rs`, `icons.rs`, `file_browser.rs` | Tabs/layout, typed palette actions, native icons and shared asynchronous pickers |
| `src/app/layout.rs` | Shared bounded tool pages, responsive headings, themed cards/sections and empty states; reuses theme control/frame builders |
| `src/app/navigation/` | Pane focus, validated shortcut presets, explorer keys and native source-editing modes |
| `src/app/explorer.rs`, `crates/rode-host-core/src/files/mutations.rs` | Explorer menus/forms, acknowledged path updates and bounded native filesystem mutations on the captured project host |
| `src/app/workbench.rs`, `src/app/workbench/` | Captured project preferences and domain tool panels; `jobs.rs` owns dispatch, scheduling and version-checked result routing, while `tests.rs` holds coordinator regressions |
| `crates/rode-host-protocol/src/files.rs`, `crates/rode-host-core/src/files.rs`, `src/files.rs` | Wire-only file results, shared revision-checked saves and desktop decode attachments |
| `src/host/`, `crates/rode-session/src/host.rs` | Verified native deployment, connection-owned RPC, captured contexts and scoped workers |
| `crates/rode-host-core/src/` | Shared project/Git/tools, sandbox launchers, process supervision, runtime cache and compute launch |
| `crates/rode-bwrap/` | Pinned bubblewrap/libcap sources, Cargo C builds and isolated native fallback entry point; no runtime installation |
| `src/ssh.rs`, `src/sandbox.rs`, `crates/rode-host-core/src/landlock.rs` | Authenticated transport identity, actual host-policy preparation and sandbox execution |
| `src/persistence.rs` | Locked preference deltas, atomic configuration writes and private framework snapshots |
| `src/project.rs`, `src/languages.rs`, `crates/rode-host-core/src/project.rs` | Installed tools, embedded language catalogue, normalized diagnostics and host counters |
| `crates/rode-host-core/src/project/stash.rs`, `crates/rode-host-core/src/project/stash/transaction.rs` | Captured stash identities, Git-compatible writer locks and protected files-backend reflog removal |
| `src/extensions.rs`, `src/app/extensions.rs`, `crates/rode-host-core/src/extensions.rs` | Native feature activation, opt-in installed adapters and their UI/preferences |
| `src/mise.rs`, `crates/rode-host-core/src/mise/native.rs`, `crates/rode-host-core/src/environment.rs` | Optional mise feature, bounded tool/task discovery, host-side environment resolution before exec and shared literal environment/command helpers |
| `src/process.rs`, `crates/rode-host-core/src/supervisor.rs` | Shared bounded JSON-line subprocess transport and owned process-group cleanup |
| `src/bundled.rs`, `src/bundled/install.rs`, `src/app/agents/runtime.rs`, `crates/rode-host-core/src/runtime_cache.rs`, `scripts/bundle-codex.py` | Pinned native Codex discovery, explicit native per-user setup, verified private SSH-cache deployment and release-bundle preparation |
| `crates/rode-agent-core/` | Shared Codex/ACP state machines, bounded chat/history, capabilities, personalities and host-service wire views |
| `crates/rode-runtime-core/`, `crates/rode-session/src/runtimes.rs`, `src/runtime.rs` | Shared notebook/DAP controllers, bounded output/state replay, exclusive leases, mutation acknowledgements and native runtime discovery |
| `src/agent/`, `src/agent.rs`, `src/app/agents.rs`, `src/app/agents/background.rs` | Project-host preparation, disposable agent attachment, live-session discovery, history and permission/question UI |
| `crates/rode-session/src/agents.rs`, `agents/transport.rs` | Headless agent ownership, fixed sandbox/executable/environment, controller leases, bounded subprocess I/O and lifetime cleanup |
| `src/agent/personality.rs` | Bounded response-style hints, explicit default resets and display-only history cleanup; independent of tools and execution policies |
| `crates/rode-agent-core/src/controls.rs`, `controls/catalog.rs`, `src/agent/controls.rs`, `src/app/agents/controls.rs` | Shared bounded model paging, metadata-only discovery and native model/access selectors |
| `src/debugger/`, `src/debugger.rs`, `src/app/debugging/`, `crates/rode-host-core/src/debugger/bridge.rs` | DAP framing/client, shared adapter/debug-PTY coordinator and interactive debugger UI |
| `src/images.rs`, `src/pdf.rs`, `src/binary.rs`, corresponding `src/app/*_view.rs` | Bounded desktop decoding/analysis, memory leases, retained snapshots and specialized tabs |
| `src/app/markdown.rs` | Unsaved-source Markdown rendering and sandbox-aware linked assets |
| `src/notebook/`, `src/notebook.rs`, `crates/rode-host-core/src/notebook_kernel.py`, `src/app/notebook_view.rs`, `notebook_view/ui.rs`, `notebook_view/source.rs` | Metadata-preserving notebook model, kernel ownership/versioned execution, cell/output rendering and stateful source editing |
| `src/marp.rs`, `src/latex.rs`, corresponding `src/app/` and `crates/rode-host-core/src/` modules | Explicit installed-tool builds/renders, compiler issues, preview/export lifecycles |
| `src/terminal.rs`, `src/terminal/colors.rs`, `src/app/terminal_colors.rs`, `src/sessions.rs`, `src/app/sessions.rs`, `src/bin/rode-sessiond.rs`, `crates/rode-session/` | Native terminal rendering, workspace policy, shared host PTYs, the Cargo-installed headless frontend and view lifetimes |
| `crates/rode-session/src/tunnels.rs`, `src/app/tunnels.rs`, `src/ssh.rs` | Daemon-owned reverse SSH forwarding, reconnect/trust state, explicit host controls and return profiles preserving compute identity |
| `src/slurm.rs`, `crates/rode-host-core/src/slurm/`, `src/app/sessions/allocation.rs`, `crates/rode-session/src/workspace.rs` | Interactive allocations, persistent notebook reservations, captured execution targets, stdio compute steps, compute-node sandbox policy, scheduler snapshots and ownership-checked cancellation |
| `src/vault.rs`, `crates/rode-host-core/src/vault.rs`, `src/app/vaults.rs` | Verified gocryptfs mounts, cross-window leases and guarded locking |
| `src/theme.rs`, `src/theme/syntax.rs`, `themes/` | Independent interface/syntax choices, bounded custom data and cached highlighting |
| `tests/`, `tests/fixtures/`, `scripts/test-*.py` | Service integration tests, deterministic peers and isolated native UI checks |

## Current service boundaries

The native gateway in `src/host/native.rs` selects, verifies and deploys one helper per SSH connection. Terminal, agent and runtime requests reuse that connection; they do not rediscover binaries, track upload-ready flags or send replacement executables. Session requests carry only the operation and optional runtime directory.

RODE requires the current session protocol. Each control connection authenticates and checks that version before executing an operation or attaching a view. One protocol-specific runtime directory owns discovery, creation and mutations; historical service directories, feature probes and unauthenticated control fallbacks are not supported. Updating the editor and native helper together is required. Earlier services are neither discovered nor stopped by the new client. ACP selectors use `configOptions` and `session/set_config_option`; there is no legacy mode-selector fallback. Agent access uses the `access` preference directly.

`environment_probe.py` and `notebook_kernel.py` remain active language-specific adapters. They inspect Python installations and integrate with Jupyter respectively. They are not remnants of the removed general Python host backend. Direct agent/notebook/DAP library clients also remain supported alongside GUI-managed controllers and are exercised by integration tests.

Runtime command validation belongs to `rode_runtime_core::service::Spec::validate_command` and runs before host executable resolution as well as controller creation. The per-execution notebook source limit lives in `rode_runtime_core::notebook::MAX_CODE`, re-exported by the desktop model, so moving execution into the service cannot leave a stale desktop-only limit.

## Design decisions and tradeoffs

The choices below explain the current implementation. Module-level rustdoc records ownership and local constraints; the main service modules also include **Design decisions** sections beside the code they constrain. The sections below describe consequences as well as benefits; they are not a list of proposed features.

Keep the desktop coordinator focused on application state and shared lifecycle guards. Application menus and connection banners belong in `app/frame.rs`, file/decode dispatch in `app/workers.rs`, modal presentation in `app/dialogs.rs`, and appearance controls in `app/appearance.rs`. Workbench background work lives in `workbench/jobs.rs`, independently of tool-panel rendering and saved settings. Domain views own their rendering and controller state; notebook rendering lives in `notebook_view/ui.rs` so it can be changed without navigating kernel ownership and background-result code. Large regression suites live in adjacent `tests.rs` modules and retain their existing Rust module/test paths.

### Theme composition

`src/theme.rs` owns theme selection, startup loading and highlighting invalidation. `theme/palette.rs` defines interface/terminal color roles; `theme/definition.rs` parses each JSON definition once per resolution and resolves built-in inheritance by ID, with cycle detection independent of menu order. `theme/interface.rs` validates component tokens and installs shared egui styling. `theme/components.rs` supplies control metrics, secondary labels, primary actions and card/dialog/panel frames; the app’s layout helpers reuse them. `theme/surfaces.rs` draws optional native decoration, while `theme/syntax.rs` handles independently selected code palettes and cached highlighting.

A preset is data, not a separate renderer. Variants inherit a base and override the relevant roles: Carbon Edge adds decoration to Carbon; Studio opts into shared rounded-component defaults with `"ui": {}`; Studio Neutral and Studio Hacker change their colors while inheriting the same components. Keep the default tokens in `InterfaceStyle::default` and avoid duplicating them in presets. UI, syntax, terminal colors and frame treatment remain independently composable, and views must not branch on preset IDs.

### Native UI and work on demand

Tool pages and dialogs share presentation helpers in `src/app/layout.rs`, following the chat-history, running-agent and monitor layouts. Use `page` around a tool's scrolling region, `heading` (or `header` with responsive toolbar actions), and `section` for related controls. Form grids use `field_label` to reserve a readable label column or stack labels above inputs in compact views. Lists use `card` or customize `card_frame` for selection; `empty` explains an unpopulated list. Pages cap forms/lists at 1000 logical points; the monitor keeps a wider responsive grid, while document canvases, source editors and terminals retain their own space requirements. Dialogs share `dialog_frame` and a viewport-bounded scrolling budget. These helpers own presentation only: retain stable scroll/widget IDs and controller state in each domain so docking/resizing never changes task ownership or unsaved drafts.

The Rust/egui application keeps the editor independent of an embedded browser application runtime. Cached layouts, virtualized rows and explicit worker jobs keep slow I/O and tool execution out of ordinary input/rendering. On-demand activation limits idle work, at the cost of waiting for a tool or preview's first result. This choice does not by itself establish a performance claim: measure relevant release workloads when changing those boundaries. Rationale lives in `src/lib.rs` and `src/app.rs`.

One UI-thread owner avoids concurrent mutation of source buffers. Cancellation alone cannot revoke a result already queued, so generation/source checks remain necessary even for cancellable jobs. Stable tab/cell IDs preserve ownership across reordering, while positional indices are presentation state. See [worker results](#worker-results-and-version-checks).

### SSH clients and remote service peers

The default embedded russh client owns a connection process with interactive authentication in a real PTY. Common OpenSSH configuration, keys, agents and native jump-host channels are supported; an explicitly selected OpenSSH backend preserves advanced configuration compatibility. File/tool/project-shell subprocesses attach to the existing connection through bounded local frames. Linux control endpoints reuse the sealed process capability and reject foreign mount namespaces. Neither backend starts a fresh login when a channel loses its master; disconnect preserves recoverable buffers. Native client implementation and compatibility limits live in `src/ssh/native/` and [remote connections](remote.md#client-compatibility).

Native SSH deploys a small verified helper and reuses the same Rust services as local execution. Its distribution and protocol checks replace the general interpreter prerequisite. Persistent sessions have a separate process owner because their jobs must survive a gateway or editor exit. Rationale lives in `src/ssh.rs` and `crates/rode-session/src/lib.rs`; [remote setup](remote.md) records the prerequisites.

### Sandbox preparation and enforcement

Project file helpers and process trees run inside the selected host's sandbox; display/authentication resources stay outside it. This gives editor reads, shells and tools a consistent project policy without sandboxing the whole desktop application. Shared mount-policy data reduces drift between implementations.

Probing distinguishes requested isolation from what the host can actually enforce, allowing visibly ordinary operation when the selected bubblewrap or Landlock backend is unavailable. Local and SSH workspaces use a single-threaded Rust Landlock launcher. It enforces kernel access rules rather than namespaces. That fallback is decided during preparation. Retrying a denied operation outside an already-active sandbox would change the granted access, so it remains a failure. Rationale lives in `src/sandbox.rs`; the concrete access model and exceptions are in [sandbox access](sandbox.md).

### Explicit settings merges and protected source saves

Two windows may hold different stale preference snapshots. Field-level changes merge independent edits into the latest file without a database or a general distributed merge engine. Arrays are replaced as units because concurrent insertion makes array positions unreliable identities. Conflicting explicit edits to the same scalar/array follow transaction order. Unknown fields and baseline-checked record deletion let different versions/windows coexist without erasing data they did not edit.

A stable configuration lock survives atomic replacement; locking only the old inode would stop coordinating new writers after rename. Atomic publication prevents partial contents, while the lock protects the read/merge/replace transaction. Neither replaces the other. Source saves instead coordinate through current/new inodes and atomic creation so repositories need no permanent editor lock files. Content digests detect edits that size/timestamp checks could miss; acknowledgements retain the submitted baseline so newer typing stays dirty.

Persistence uses a worker because lock waits and filesystem sync can stall. Failed writes pause until explicit retry, and eframe receives a private snapshot because its whole-file persistence cannot perform the shared merge. This makes `Storage::flush` asynchronous and final shutdown flushing best-effort when storage fails. Rationale lives in `src/persistence.rs` and `src/files.rs`; [configuration](configuration.md) records supported behavior and limits.

### Separate process and terminal ownership

Bounded stdio protocols reuse authenticated transport without a new application listener or embedded agent runtime. Queue bounds make slow-consumer pressure visible instead of allowing unlimited memory or UI-thread pipe waits. Protocol clients must handle queue-full results; JSON/base64 encoding has overhead in return for simple bounded framing. Background workers start in new sessions to remove the launcher's controlling terminal while retaining owned process groups for cancellation. Interactive shells instead establish their own PTYs. Rationale lives in `src/process.rs`.

A GUI/SSH attachment cannot own a shell that must outlive it. The headless native terminal service owns PTYs, removing a tmux/screen prerequisite and supporting shared views. The cost is native-helper deployment and an exact session-protocol contract. Persistence belongs to the session, so closing one window differs from explicitly ending the shared shell. The client uses only its current service generation and rejects protocol mismatches. See [process ownership](#process-and-resource-ownership) and [terminal lifetime](terminals.md).

Agent history uses a separately supervised connection so failed or cancelled loading cannot replace the current conversation. Only explicit History starts it; opening the panel remains inert. Validated host/project metadata and current sandbox/permission settings precede replay. On success ownership transfers to the chat, with bounded recent messages and full context retained by the backend. RODE stores no duplicate transcript database. See `crates/rode-agent-core/src/history.rs`, `src/agent/history.rs`, `src/app/agents/history.rs` and [chat history](agents.md#chat-history).

### Host-owned agents

Persistent agents need their complete protocol controller on the host: leaving only the executable alive would strand initialization, permissions and cancellation timers in the departed GUI. `rode-agent-core` shares those controllers between the existing direct-transport API and `rode-session` without pulling desktop libraries into the remote helper. Agents keep the exact captured sandbox and host environment; the attachment broker runs outside that namespace but supplies no tool execution or policy escalation.

One controlling lease serializes prompts and answers, while other windows may list/end jobs. New agent chats default to Auto: after attachment loss, accepted tasks and queued prompts finish before the controller exits. Pending questions and paused or edited prompts retain the session for reattachment. On also retains idle detached chats, while Off stops on detachment. Attached chats stay available for follow-ups in every mode. The host owns grace periods, cancellation deadlines and process-tree reaping. The GUI waits for persistence/start/end acknowledgements during clean exit and retains retiring views until their pending lifetime work finishes. Read-only discovery never starts a service or agent. Capability negotiation selects a compatible service generation without terminating older jobs.

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

The SSH master owns interactive authentication and its private control socket. Rust channels use authenticated bounded IPC; the optional OpenSSH backend uses ControlMaster. Neither can fall back to a new login. Losing a connection invalidates its workspace; retained documents become unbound recovery buffers. Only an explicit same-host/project reconnect restores their bindings. Local Save As creates a separate copy.

Native terminal attachments can receive shell diagnostics before the first binary frame because the SSH client's stderr shares the view's PTY. `src/terminal/framing.rs` publishes complete diagnostic lines while waiting, with a total prelude limit of 64 KiB, and then switches to strict frame decoding. Startup failure text remains visible when no frame arrives. This tolerance applies to terminal attachments only: the native host RPC channel still requires its exact handshake and silent stdout.

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

Source and binary reads produce a content digest. A save captures the submitted bytes and expected revision, follows the destination's symlink target, locks the current inode and verifies that it still represents the path after waiting. The writer compares the digest, writes a neighboring temporary inode, preserves existing permissions and publishes atomically. New destinations use publication that refuses replacement. These locks coordinate RODE's local and SSH writers; unrelated programs still have best-effort conflict detection.

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

### Daemon-owned reverse SSH tunnels

`crates/rode-session/src/tunnels.rs` owns bounded native SSH reverse forwards;
`src/app/tunnels.rs` manages them over the existing authenticated host-admin path.
Local projects and the welcome/file-only views use the matching local session
service directly, without requiring a project or initial SSH connection. SSH
workspaces retain their remote host and reject requests while disconnected;
they never fall back locally. Workers capture their host before dispatch and
workspace generation changes discard stale replies. Discovery does not start
the service; only an explicit tunnel start can do so.
The compute node already runs sshd. Each outbound connection requests a fixed
loopback relay listener and accepts only forwarding channels naming that exact
address/port, then connects to a fixed compute loopback SSH port. There is no
public session-service endpoint or general forwarding proxy.

Workers retain explicit compute-host credentials and a pinned relay public key,
not desktop attachments. Network failures retry with capped exponential delay;
unknown/changed host keys and credential failures require an explicit action.
Stop cancels the worker and its streams. Retained tunnels participate in idle,
last-terminal and shutdown checks, and creation shares the existing service-stop
lock. State is memory-only and bounded to eight tunnels and 32 channels each.

Return profiles retain the original compute alias, authentication and host-key
identity while overriding only the final transport destination through the
relay. Locally started tunnels instead provide connection details for a profile
on another computer, where credentials and compute host trust are chosen explicitly.
Normal compute SSH verification, host operations and sandbox preparation
remain in force. Specifications use bounded existing control frames; neither
JSON settings nor credentials enter argv. See [setup and limits](remote.md#compute-node-return-tunnels).

### Compact process arguments

RODE-owned launchers must keep OS-visible command lines short and useful for process inspection. This applies to local and remote execution, intermediate shells, sandbox wrappers, helpers, agents, tools and terminals. Increasing a profile, policy, script or document payload must not increase the launch command line by embedding that payload in `argv`.

- Keep the executable, operation, ordinary target arguments and short identifiers, paths or integrity digests in `argv`. Do not put generated script bodies, serialized profiles/requests, document contents, credentials or full sandbox mount lists there. An ordinary user-supplied command retains its intended arguments; do not truncate or rewrite it to meet this rule.
- Carry payloads through existing framed stdin, authenticated private sockets or explicitly passed file descriptors. Use an owner-only file when those transports conflict with interactive stdin or PTY descriptor handling. Do not introduce a network listener merely to shorten arguments. Base64 encoding, moving a large payload into an environment variable or changing the displayed process name is not a substitute for a payload transport.
- Preserve protocol bounds, deadlines and binary framing. A script prelude must not consume the following upload or interactive input. Validate file ownership/type/mode and bind sensitive launch data to the intended snapshot, such as with a digest supplied separately from the file. Clean up only the owning operation's files and descriptors, including on failed startup or cancellation.
- Compact arguments at the launch boundary. Keep the complete captured policy for enforcement, session fingerprints and mount translation. Preserve authentication, host verification, sandbox grants and process ownership; transport failure must not trigger an unsandboxed retry. Descriptor handoffs must handle PTY descriptor closure and concurrent spawns without leaking descriptors to unrelated children.
- Prefer the shared launch helpers over another shell wrapper. Current implementations are `crates/rode-host-core/src/command/arguments.rs` for bubblewrap argument files, `src/ssh.rs` / `src/ssh/native.rs` for the private profile handoff, and `src/host/native/helper.rs` for streamed bootstrap scripts. A third-party tool that requires inline data needs a documented, bounded exception at its adapter; it does not establish a default for other launchers.

For new or changed launch paths, inspect the actual process arguments, including intermediate processes on the execution host. Regressions should establish that payloads are absent from `argv`, literal arguments and binary/interactive stdin survive, and cleanup and the original access policy still hold. Cover the affected pipe, PTY and SSH paths. See [launch validation](development.md#process-launch-changes).

## Protocols and feature adapters

The native gateway protocol is described [below](#protocol-and-channel-contract). File/project operations use tagged JSON request/reply shapes shared by the desktop and headless helper. Long-lived agent/kernel/coordinator transports use bounded JSON lines. DAP messages use Content-Length measured in bytes. Binary/image/PDF/PTY payloads use explicit base64 where required by their protocol. Stdout is protocol-only; diagnostics belong on stderr or in explicit result fields.

`environment_probe.py` inspects the selected Python interpreter. `notebook_kernel.py` integrates with installed Jupyter packages. These are language-specific adapters, not file transport, process supervision or sandbox launchers. Development/test and packaging scripts may also use Python.

Input, output, frames, directory rows, subprocess duration and retained data have explicit limits. Reject over-limit input; never bypass an active policy or retry on the desktop. When changing a schema, update the shared Rust types and the [contract, sandbox and SSH tests](development.md).

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

## Documentation maintenance

Document a public item's behavior, units, indexing, side effects and lifetime where those affect its use. State error/absence semantics when an empty result differs from failure, and explain whether a function blocks or merely queues work. Use module comments for shared invariants instead of repeating them on every private getter.

Apply the same contract to internal APIs shared between modules: state types, UI entry points, worker dispatch/completion methods and lifetime operations. Small private formatting helpers and trait implementations can rely on the owning module/type contract when an additional comment would only repeat the code. A private item's visibility is not a reason to omit its cancellation, indexing or ownership requirements.

Private comments should explain ordering and ownership that the code alone cannot show: lock publication, stale-result rejection, preserved source snapshots and signal scope. Keep comments close to that decision and update them when behavior changes. Rustdoc links and missing public docs are checked in CI. Python feature-adapter and test-script docstrings describe their contracts; Ruff checks their formatting and lint.

For a design choice, record the motivating constraint, the selected approach and its costs or limits. If a change revisits that choice, update both the owning module's rationale and the relevant guide section, including compatibility/lifetime effects and the checks that substantiate the new behavior. An API-description update alone does not explain a changed design tradeoff.

## Native host architecture

The selected SSH client owns authentication, host verification and multiplexed channels. `rode-sessiond` owns native host commands independently of the persistent session service. There is one Rust host implementation and no alternative interpreter transport.

### Process and dependency boundaries

```text
Desktop worker -> SSH connection process -> connection-owned host-rpc gateway
                                       |                   |
                          captured project context    narrow administration
                                       |                   |
                          sandbox -> --host-worker    existing session service
                          files / Git / tools         sandboxed persistent jobs

Separate multiplexed channels -> bounded process/build/debugger streams
```

| Component | Owner and contract |
| --- | --- |
| `rode-host-protocol` | Versioned framing and generic file/host envelopes; Serde-only, no OS or GUI services |
| `rode-host-core` | Shared safe files, locks, Git, installed tools, environment/mise, sandbox, supervisor, runtime cache, vault and Slurm services; no desktop/session dependency |
| `rode-session::host` | Gateway, request composition, opaque project contexts and fresh scoped worker processes |
| `rode-session::workspace` | Session administration DTOs and launch conversion, shared with the desktop |
| Existing session controllers | Persistent PTYs, agents, notebook/debugger state, leases, protected IPC and service generations |
| Desktop `src/host/native.rs` | Installed-helper discovery and compatibility, remote-architecture selection, verified bootstrap, bounded requests and connection failure reporting |
| Desktop facades | `files`, `project`, `sandbox`, `sessions` compose host services with UI attachments and local helper discovery |

The dependency direction is `protocol <- host-core <- session`. Agent/runtime controllers remain headless. Image/PDF decoding, notebook documents, textures, source buffers and preference merging remain in the desktop. Wire file results contain only encoded data, entries, paths and revisions.

Local services use host core directly or a retained native helper. Landlock and mise launchers use Rust on both local and SSH hosts. Python remains a feature dependency for interpreter inspection and Jupyter, and may be used by development tooling.

### Contexts and authority

The gateway opens no listener and stores at most 128 opaque contexts per connection. Preparation captures the canonical root, requested grants, actual enforcement result and helper generation. Environment changes derive a new context without changing grants. Context IDs from another connection are rejected. Reconnect when the context limit is reached; old contexts are not silently evicted while work may reference them.

Ordinary file/tool RPC runs in a fresh helper process under the captured plan. Caller-supplied prefixes cannot replace that plan. Unscoped RPC permits only pre-project directory browsing, read-only host monitoring and session administration. Scoped workers reject preparation, monitoring and session administration. Specialist process/build channels retain their captured launch plan and existing cancellation ownership.

Requested but unavailable sandboxing remains visibly normal during preparation. An active sandbox failure stays failed. Native errors never retry through Python, another host, another backend or unrestricted commands. The Rust Landlock launcher requires ABI 4 and selects filesystem masks/ruleset layouts by the probed host ABI: ABI 5 adds device-ioctl mediation and ABI 6 adds signal/abstract-socket scoping. Network-off seccomp policy, protected runtime dependencies and inherited-descriptor closure apply to every supported ABI. Capability limitations appear in the workspace mode’s tooltip. Bubblewrap retains its distinct namespace semantics.

Session service IPC remains protected independently of RPC: same-UID and namespace checks, authenticated Linux abstract control sockets and rejecting filesystem discovery endpoints. `control_auth.rs` keeps a random capability in a sealed, non-executable, anonymous service-owned memory file and publishes only its proc-descriptor reference. Clients validate the reference against the socket’s peer PID before reading it. Landlock’s ptrace restrictions protect the capability even when the runtime directory is shared; ABI 6 adds kernel socket scoping. A restricted worker cannot use an outer service as an unrestricted command broker. Gateway/stdin descriptors are not inherited by project children.

### Protocol and channel contract

Host wire version 1 is separate from session protocol 17 and helper API revision 1. A fixed preamble and handshake verify the selected helper digest, OS/architecture, metadata format, API revision, host/session protocols and required capabilities before operations. Package versions are diagnostic; matching release labels alone are insufficient. Installed discovery and the live handshake must report identical helper metadata. Stdout is exclusively protocol data; stderr supplies bounded diagnostics. Startup banners, wrong versions or wrong executable identity fail explicitly.

Maintain `rode_host_protocol::helper::API` for incompatible changes to helper CLI arguments, shared request/response DTOs, or auxiliary streams not already versioned by the host/session protocols. Additive capabilities may be advertised without breaking old clients; update the required set when the desktop depends on them. Bump the metadata format for incompatible discovery-schema changes. Keep metadata side-effect-free and repeat it in the handshake; do not substitute package-version comparisons for these contracts.

- Each physical frame is a big-endian 32-bit length plus at most 64 KiB of data. A zero length ends the message. Lengths are checked before allocation.
- The bounded `framing::read/write` entry points retain a 64 MiB control-envelope limit. The authenticated host RPC channel uses `read_document/write_document` with the same wire format, 64 KiB physical frames, streaming JSON serialization/deserialization and no whole-file byte ceiling. The `large_document_transfer` helper capability prevents reuse of an older capped helper. Tool/attachment/decoded-raster contracts retain their independent limits; document snapshots are accounted outside the raster pixel budget. Version 1 still uses JSON/base64 file payloads rather than a separate binary payload protocol.
- The desktop allows four pending calls; the gateway has four workers and four queued requests. Replies carry request IDs and may arrive out of order. A worker handles one operation, then exits.
- RPC writes have a 45-second deadline and responses a 160-second deadline. Gateway input EOF cancels owned transient commands. Process/build streams keep their existing narrower operation limits and cancellation paths.
- Terminal and long-lived process streams use separate channels on the same master. They retain their own bounded queues and cannot be blocked behind a bulk RPC response. Host SSH server channel limits still apply.
- Lost acknowledgements are uncertain outcomes. RODE does not replay a save, Git mutation, deployment or session creation after channel loss. Revision checks and explicit reconnect/reattach provide recovery; there is no exactly-once guarantee across gateway restarts.

Host services use threads and bounded channels. The desktop SSH client uses Tokio in its separate connection/channel processes. Explicit daemon-owned reverse tunnels each use a current-thread Tokio worker in the session service; no tunnel runtime starts during ordinary service discovery. Physical transport buffers and pending-call counts remain bounded. Document fields and editor snapshots are loaded into RAM, so their sizes and concurrent copies determine peak memory use; document messages have no whole-file byte ceiling.

A text save passes one submitted snapshot between its worker request and acknowledgement instead of retaining a duplicate in each. The editable document stays independent so typing during I/O remains unsaved. Native RPC envelopes borrow their file payload during synchronous serialization; they do not clone the whole document to submit an operation.

### Deployment and lifetime

`src/host/installed.sh` performs bounded discovery and checks canonical path ownership/modes before executing `--host-info`. `src/host/native/helper.rs` validates the installed contract, honours explicit remote paths without fallback, and otherwise falls back to a matching desktop Linux x86_64/aarch64 ELF. `src/host/bootstrap.sh` checks ownership/modes, size and SHA-256 before publishing either uploaded bytes or an on-host copy of the selected installation. No helper is downloaded or built implicitly. See [host prerequisites and overrides](remote.md#host-service-backends).

The private cache is `~/.rode-host/host-<digest>`. A unique temporary directory and non-replacing hardlink publication make simultaneous installs safe; interrupted uploads clean only their own staging directory. Existing generations are reverified and preserved on verification failure. Installed packages remain untouched; the private copy prevents package upgrades from changing live launch paths. Discovery runs only during connection setup. There is no automatic garbage collection. Keep referenced helpers while persistent jobs run. This cache is separate from the older session-helper cache, whose live service generations remain untouched.

The gateway lasts for one SSH connection and owns transient work only. The existing service owns persistent jobs, retains their original policy/environment and can coexist with older live service generations. Session control from the gateway runs through a short-lived administration process with its captured runtime directory; it does not mutate the gateway's shared environment. Generic supervisors remain alive for their child lifetime, including bubblewrap's parent-death relationship.

Codex cache deployment streams explicitly supplied pinned bytes, verifies private paths, sizes, digests, licenses and executable artifacts, then publishes under a stable lock. It performs no download or authentication. Vault coordination exposes the existing narrow mount/lease protocol, with passwords on stdin and unchanged project-lock guards.

### Slurm boundary

Native Slurm revalidates the exact owned running allocation on the scheduler host. The compute bootstrap verifies scheduler-provided job identity, architecture, ownership/mode and SHA-256 of a **shared helper path before execution**. The compute helper reapplies the captured sandbox and environment before resolving the runtime. Scheduler GPU visibility is preserved; stale login-host allocation/GPU variables are removed before `srun`.

The helper cache, project, tools and granted paths must therefore be available on compute nodes. Native per-node staging is not implemented. A missing/mismatched shared helper or failed compute sandbox is an error, never a login-node runtime fallback. Python remains required for the Jupyter adapter, not for scheduler supervision or native debugger coordination.

### Validation and supported limits

Shared contract tests exercise desktop and headless entry points. Direct/ProxyJump SSH tests and a fixture without Python on PATH cover the remote transport. `tests/native_host.rs` exercises concurrent responses, context identity, handshake rejection, verified deployment and cancellation. Sandbox suites exercise actual kernel enforcement when available and fail when their required-enforcement flags are set.

Linux x86_64 is the locally validated architecture. Artifact selection supports aarch64 but does not establish execution coverage there. Slurm integration requires a shared helper path; per-node staging is not implemented. Real Jupyter, debugger, FUSE and site Slurm validation requires the corresponding installed tools and host capabilities. Fixture coverage does not establish performance or compatibility on every cluster.
