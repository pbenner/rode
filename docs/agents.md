# Agent chat

Open **View → Agent chat**, use the speech-bubble activity icon, or press **Ctrl+Shift+A**. Choose a provider and open its settings button. Send a message with **Send** or **Enter**; **Shift+Enter** inserts a newline. Replies stream into the chat, with selectable text, copy buttons, code blocks, and expandable tool results. Permission requests and questions appear above the conversation.

**Copy response** copies the displayed message text, including code contents but excluding Markdown code fences and their language labels.

Drag the divider above **Model** and **Access** to resize the message input; double-click it to reset the height. Tab to the divider and use **Up/Down** to resize or **Home** to reset with the keyboard. Long drafts scroll independently using the mouse wheel, trackpad or scrollbar. Typing and cursor movement keep the caret visible, while Send/Stop stay outside the scrolling text. The preferred height is kept for the current window and adapts when the window gets smaller; history temporarily hides the input without clearing it.

At the start of the message box (**Ctrl+Home**), press **Up** repeatedly to browse older sent prompts and **Down** for newer ones. Down past the newest restores your unsent draft. RODE keeps all prompts submitted in the current conversation independently of the rolling display transcript, so long replies and tool output do not remove them from input history. This history stays in memory until you change conversations; restoring or reattaching a chat seeds it with the prompts included in the loaded transcript.

The agent starts in the opened project folder, on the computer hosting that project. The panel identifies **LOCAL** or **SSH**, the folder, and the actual sandbox mode. Without an opened folder, it uses RODE's startup working directory. Opening the panel starts no process. Sending a message or explicitly opening **History** starts the selected agent; history browsing sends no model prompt.

## Queued follow-ups

While the agent is working, **Enter** or **Queue** adds your message to the queue above the composer. Follow-ups run one at a time, in order, after a successful reply. They do not interrupt the current task. **Shift+Enter** still inserts a newline.

Use **↑ / ↓** to reorder prompts, **Edit** to change one, or **Delete** to remove it. Editing waits for **Save prompt** or **Cancel edit** before dispatch continues, without changing the draft in your main message box. **Pause queue / Resume queue** controls automatic sending. **Stop**, a failed turn or a lost agent connection pauses remaining prompts; resume them explicitly when ready. Queued prompts use the selected model and permissions when they run.

The queue holds up to 16 messages for the current conversation. It continues when you switch to code or detach the tools window. New chat, switching projects/conversations/providers, or disconnecting the chat clears it. Pending prompts stay in this RODE window and are not saved or sent by background agents after the window closes; **Persist** retains the active agent task, not this queue.

## Background agents

Use **Persist: off/on** at the right of the chat's Send row. New chats default to **off**. Enable it before sending or during a task; the filled button appears after the host acknowledges the change. Turning it off again keeps the current task running until its controlling view closes. Choosing persistence alone starts no agent.

With persistence on, the native host service keeps the agent and its tool processes running when RODE exits, the project changes, or SSH disconnects. Open the original project with the same sandbox policy, click **Running** beside History (or **Agent: Running sessions** in the command palette), then **Attach**. Reattachment restores the live conversation, pending requests and actual model/access settings without restarting the backend. Each agent has one controlling window at a time; detach it before attaching elsewhere. The list can be inspected from other windows on the same host. **End → Confirm end** stops a session and its owned tools.

Permission requests wait while detached; persistence never approves them or widens the captured filesystem/network policy. **Stop** interrupts a turn. **Detach** leaves an idle persistent agent running. **New chat** releases the current view and returns persistence to off: an old persistent session remains in Running. Closing only the chat panel hides the view and continues work with either setting.

Live sessions are held by `rode-sessiond`, alongside persistent terminals, without tmux/screen or a remote GUI. Cargo installation includes this helper; SSH uses the verified [remote helper](terminals.md#remote-helper-and-packaging). The service exits after its last agent/PTY process has stopped. It retains at most 16 agent controllers per service generation and the same bounded recent transcript as the UI, in memory only. These live sessions do not survive host shutdown; **History** separately browses conversations saved by the backend. User-managed model servers and authentication must remain available. Encrypted projects must have their jobs stopped before they can lock; use Running → End for persistent agents.

## Model and access controls

Use **Model** and **Access** / **Permissions** above the message field. Choices are saved per host/project through the shared preference locks; opening a menu starts no process. Controls are disabled during active work. Stop the current turn before changing them.

For **Codex**, Model accepts an explicit model ID or the agent default. After the first message or restoring a conversation, the picker loads the running agent's model catalog and advertised reasoning efforts. Open **Reasoning effort** and choose **Default** or one of the model's advertised levels from its submenu. The choice is saved immediately and applies to the next message. Catalog entries describe available choices, not an account-entitlement guarantee. Custom model/effort IDs remain available when discovery is unsupported. For **Codex · local model**, enter the installed server model ID and an optional supported reasoning effort. Codex's cloud-oriented catalog is not used as an inventory of installed Ollama models.

Codex tool access offers **Read only**, **Project access** (default), and **Full access**. Read only prevents approval-based tool escalation. Project access allows workspace writes and can request additional access. Full access uses the host permissions left inside the active project sandbox; in normal mode it uses the user's host permissions. The project's actual read-only restriction always takes precedence. These are agent tool policies, separate from the enclosing filesystem sandbox.

The common permission-request choices are:

- **Ask when needed** (default): display the requests the agent sends. This does not force a prompt for every command or edit already permitted by its mode.
- **Never ask**: decline permission requests automatically. Already permitted tools can run; this setting does not grant additional access. Agent questions still require an answer.

For **Other agent · ACP**, RODE renders the select/boolean options advertised by that session, including model, reasoning and operating modes. Modern session configuration is preferred, with a legacy mode fallback. Unsupported option types are omitted. Model-related choices appear under Model; modes and other options appear under Permissions. Idle changes wait for acknowledgement and refresh dependent choices without sending a message. Unavailable or rejected saved settings stop submission with an explanation; **Agent settings → Reset saved agent controls** restores a fresh session's defaults. RODE does not invent a universal read-only/full-access switch for ACP agents that expose no such mode.

Codex changes apply to the next turn while retaining the conversation. Returning an explicit model/effort override to its default starts a fresh conversation so old overrides cannot remain active. Executable/provider changes also start a fresh conversation. Transcripts stay in memory; they are never written to preferences. No selector or approval can widen the outer project sandbox's filesystem or network grants.

## Response personalities

Use **Tone**, beside **New chat**, to choose **Default** or **Jackson Lamb**. The latter is inspired by the [Slow Horses boss](https://www.apple.com/tv-pr/originals/slow-horses/): dry British wit, blunt understatement and sardonic commentary, using original wording. The selected custom style appears as **Lamb** in the header. Default uses the agent's own configured tone.

The choice is saved per host/project and applies to the next message, including in an existing or restored chat. It works through Codex, Codex local-model mode and ACP. Choosing a style starts no process, sends no message and does not discard conversation context. Stop an active turn before changing it.

RODE sends a small response-style reminder with **every message**, including follow-ups and resumed chats, alongside the unchanged user task. It explicitly requests the selected voice throughout progress updates, explanations and the final answer, rather than only in the first reply or opening line. It does not change the executable, model, reasoning effort, tools, approval settings, sandbox or project environment. The hint asks for normal task-appropriate code, comments, documentation and other artifacts; only explanatory chat replies should adopt the voice unless explicitly requested otherwise. Explicit output formats and user instructions take precedence. There is no second model call or response-rewriting service.

This is model guidance, so delivery varies by agent/model; prompting cannot guarantee identical model decisions apart from wording. Execution restrictions continue to be enforced independently. Returning to Default explicitly clears earlier RODE tone preferences, without creating a new chat. Recognized style hints are hidden in RODE's displayed history, although the backend may retain them in its own saved conversation. No transcript or custom system-prompt file is added to the project.

## Chat history

Click **History** beside the chat status. RODE asks the selected agent on the current project host for saved conversations in this project folder. Filter the loaded titles/previews, use **Load more conversations** for older entries, and select a conversation to restore its recent messages and continue chatting. **Back** or **Cancel** returns to the current chat; **Refresh** reloads the list. Stop an active turn before restoring another conversation.

Codex and Codex local-model history use the agent's saved threads, including conversations created by its CLI or VS Code integration in the same folder. They use separate provider filters. ACP history requires the agent to advertise session listing and loading; unsupported capabilities are explained in the browser. Entries come from the agent's storage visible inside the active sandbox, on the project host—not from the desktop when using SSH. No RODE transcript database or preference-file copy is created. **New chat** does not delete saved conversations.

Listing/loading starts a separate, short-lived agent process without requesting model inference. A successful restore replaces the current chat process; a failure or cancellation leaves it intact. Closing history/the agent panel or changing project, host, sandbox, interpreter or agent settings stops the browser. The current Codex access/approval policy overrides the saved thread's policy; current explicit model/effort choices apply when you continue. ACP controls are applied before the next prompt, using the session's advertised capabilities. The outer project sandbox always remains in force.

RODE displays up to 32 recent Codex turns, 128 messages and 512 KiB of replay; the agent retains the full conversation context. Metadata browsing is limited to 256 conversations and 32 pages. History connections accept protocol lines up to 2 MiB, including after resuming; oversized replies fail visibly. ACP replay shares the display limits. Agents must already have readable saved history; RODE cannot recover conversations that were never persisted by the agent or were deleted. Archive/delete management stays in the agent's own tools.

## Providers

| Provider | Required software on the project host | Configuration |
| --- | --- | --- |
| Codex | Optional `rode-codex` runtime or an installed Codex; authenticate on the project host | Executable defaults to `auto`; optional model; blank model uses the Codex default |
| Codex · local model | Optional runtime or installed Codex and a running Ollama or compatible Responses server | Executable defaults to `auto`, API base URL, and an already installed model name |
| Other agent · ACP | An agent implementing ACP v1 over stdio, with its own file/command tools | Executable and arguments as a JSON array |

Custom executables can be absolute paths, project-relative paths such as `./tools/my-agent`, or names on PATH. RODE also checks `~/.local/bin`. Arguments are passed directly, without shell expansion. To customize environment variables or launch a special CLI profile, select your own small wrapper script. Other agents and model servers remain user-installed; RODE does not download models, manage accounts, or store API keys.

### Codex

The editor and Codex are separate components. Click **Install Codex** in a new chat or **Agent settings** to install the pinned native [Codex CLI](https://developers.openai.com/codex/cli/) for your user. The optional **`rode-codex`** Debian package and existing Codex executables are also supported. No VS Code, Node.js or npm is required. Leave **Codex executable** set to `auto`. Older saved settings containing `codex` also select this mode. An explicit custom executable overrides discovery; without the optional runtime, an installed Codex on PATH or in `~/.local/bin` remains usable. RODE uses the [app-server protocol](https://developers.openai.com/codex/app-server/) for conversations, command/file tools, approvals, questions and interruption.

The managed runtime includes the main `codex` executable plus its pinned `codex-code-mode-host` and `codex-exec-server` helpers. All three are verified, packaged, deployed over SSH and exposed together inside the project sandbox. No separate Node.js installation is required. If an older RODE installation reports a missing helper, rebuild/update RODE, click **Install Codex**, then choose **New chat** (end an old persistent agent through Running if needed). Explicit installation adds missing verified helpers while retaining an unchanged main executable; changed or unsafe existing files are rejected. An older main-only portable/package bundle is skipped in favor of a verified complete user installation. Discovery never downloads or repairs files automatically. Custom executables remain user managed.

For a local project, authenticate in a terminal with `rode --codex login`. Use `rode --codex --version` to check the bundled version. These commands forward to Codex without starting the editor. Existing Codex authentication/configuration in `~/.codex` remains usable after uninstalling VS Code. Local-model mode does not require OpenAI authentication.

For SSH projects, the first message or explicit History action deploys the pinned runtime to a private cache on the project host, using the existing authenticated connection. Transfers are streamed, size/SHA-256 checked, serialized across windows and atomically published. Subsequent starts reuse the verified cache. Credentials are never copied from the desktop. Authenticate on that host using the cached executable at `~/.cache/rode/codex/VERSION/ARCH/RUNTIME_SHA256/codex`, for example with `login --device-auth`. `RODE_CODEX_CACHE` on the host can select another absolute cache location with an owned, non-writable-by-others parent.

The optional package includes its own CPU architecture. **Install Codex** detects an SSH project's host architecture and downloads that runtime to your desktop's user data directory; the normal chat launcher transfers it to the remote cache when needed. Alternatively, prepare it with `python3 scripts/bundle-codex.py --install --architecture aarch64` (or `x86_64`), include it in a portable bundle, or configure an installed executable on that host. When no upstream binary exists, provide a compatible source-built executable if supported, or choose an ACP agent. Missing Codex affects only that integration. RODE does not silently download another runtime or execute the agent on the desktop instead.

Source builds optionally prepare the runtime with `python3 scripts/bundle-codex.py`; this default writes into the checkout's `target/codex/`, discovered by editors in its usual `target/debug/` or `target/release/` directories. Portable distributions put version/architecture folders under `rode-codex/` beside `rode`. Debian's optional package installs them under `/usr/lib/rode/codex/`, also discoverable by Cargo-installed or copied editors. `RODE_CODEX_BUNDLE` selects an explicit prepared **version/architecture directory** containing `codex`, `manifest.json` and the notices.

The optional runtime has its own version and protected checksum manifest, allowing compatible upgrades without rebuilding the editor. Runtime releases still require protocol compatibility checks. The application downloads only after an explicit **Install Codex** click, never during startup, message submission or history browsing. Discovery and verification happen on workers, and no agent starts before a message or explicit History action. The binary adds disk/download size only when installed and is not loaded into the editor's idle process.

### Installing Codex for a Cargo-installed editor

Install RODE from the source checkout:

```sh
cargo install --locked --path .
rode /path/to/project
```

Open **View → Agent chat** (or **Ctrl+Shift+A**) and click **Install Codex**. It also appears in **Agent settings** for an existing conversation, for both Codex providers. The native Rust installer needs no Python script or administrator access. It shows download/extraction progress and **Cancel**, verifies the pinned archive, executable and embedded license notices, and installs under `$XDG_DATA_HOME/rode/codex/VERSION/ARCH` (default `~/.local/share/rode/codex/VERSION/ARCH`). Empty or relative `XDG_DATA_HOME` values use the default. This keeps managed executables separate from `~/.codex` / `CODEX_HOME`, authentication and configuration.

The protected, atomically updated `manifest.json` selects the complete verified runtime. Concurrent RODE windows and the optional preparation script serialize publication. Cancellation, failed downloads and failed verification leave the previous selector intact; existing verified files are reused without a download. Damaged or different files in the destination are reported rather than overwritten. The installer removes group/world write permission from the owned RODE application-data parent when needed, preserving existing preferences and locks; runtime roots and files retain their strict safety checks. Changing the project/host or executable/provider cancels that window's setup. Closing RODE cancels setup and waits for temporary-file/lock cleanup before exiting. Installation only prepares assets: it never starts a chat, signs in, downloads a model or changes project sandbox grants. SSH architecture discovery requires the existing authenticated connection, but runtime downloads need internet only on the desktop.

Keep **Codex executable** set to `auto`, and use `rode --codex login` if authentication is needed; `rode --codex --version` checks discovery without opening the editor. Reinstalling/updating the editor does not remove the user runtime. An existing portable or installation-relative runtime takes precedence over the user installation; `RODE_CODEX_BUNDLE` takes precedence over all automatic locations. Custom executables and an explicit bundle override disable the UI installer until cleared. The UI installs the version pinned by this editor, not an automatically discovered latest release.

For scripted/offline preparation, `python3 scripts/bundle-codex.py --install` remains available and uses the same user directory and install lock. It can reuse a previously cached archive. Add `--architecture aarch64` / `x86_64` for another host. `--install` cannot be combined with `--output`; without either option, preparation uses the checkout's cache. To use an already prepared directory without installing it, set `RODE_CODEX_BUNDLE` to the absolute version/architecture directory printed by the command when launching RODE.

Codex defaults to `on-request` approval and workspace-write access; the visible Model/Access controls can override these per project. Existing CLI instructions, managed restrictions, configuration, and authentication still apply. You may need to grant access to a custom `CODEX_HOME`, executable, or runtime through the sandbox settings. RODE does not edit the CLI's configuration files.

### Local models with Ollama

Use Codex as the coding agent and Ollama as the model server. This keeps planning, file editing, and command execution in the agent, with inference served locally. Ollama documents a [Codex integration using its Responses API](https://docs.ollama.com/integrations/codex).

1. Use the optional Codex runtime or an installed Codex, and install Ollama on the appropriate host. Start Ollama, for example with `ollama serve`, if it is not already running.
2. Install a tool-capable model of your choice explicitly. Use `ollama list` to see installed model names. The model must fit your hardware; Ollama recommends at least a 64k context window for Codex.
3. In RODE, choose **Codex · local model**. Set **Local model API base URL** to `http://127.0.0.1:11434/v1` and enter the exact installed model name.
4. Send a message. RODE selects a dedicated `rode_local` Responses provider without requiring OpenAI authentication or falling back to an OpenAI model. Web search is disabled for this provider.

**`localhost` means the project host.** For an SSH project, that endpoint is on the remote computer. If your server is on your desktop, use an explicitly configured SSH reverse tunnel or another address reachable from the remote host. RODE does not create model tunnels. A server offering only `/v1/chat/completions` is insufficient; it must implement Codex-compatible `/v1/responses` streaming and tools. Other local servers can use the same settings when they meet that interface. Select an actual local model if you want local inference; a server may also expose cloud-backed model names.

The model server is a separately managed service. The coding agent and its command children enter the project sandbox; RODE does not start the model server inside it. A loopback server also requires **Allow network access** in the sandbox policy.

### Other agents

Choose **Other agent · ACP** and supply the installed agent's ACP command. For example, arguments `['acp', '--verbose']` must be entered as valid JSON: `["acp", "--verbose"]`. Use the agent's documented ACP launch arguments, rather than its ordinary interactive terminal command. See the [ACP agent directory](https://agentclientprotocol.com/get-started/agents) for implementations and adapters.

RODE implements ACP **v1** initialization, project sessions, streamed text/tool updates, permission choices, and cancellation. It advertises no client filesystem or terminal capabilities: the agent must execute its own tools inside its process tree. Unsupported client filesystem, terminal, authentication, custom-tool, and elicitation requests fail explicitly. Authenticate the agent beforehand. Arbitrary interactive CLIs and the ACP v2 draft are not supported by this chat client; ordinary agent CLIs remain usable in the integrated terminal.

## Sandbox, edits, and lifecycle

An active [project sandbox](sandbox.md) encloses the whole agent process and its tool children, locally or over SSH. It uses the same folder grants, writable caches, network setting, and selected Python environment as other project commands. Approving an agent request cannot widen those outer mounts or network restrictions. Codex is told when a writable external sandbox is enforcing access, avoiding nested isolation. Read-only projects retain Codex read-only mode as well as the outer filesystem restrictions.

Bundled Codex adds only its verified runtime directory as a read-only tool mount at `/rode-codex`. Other cached files and the rest of home are not exposed by this mount. Preparing/deploying the runtime happens on the host; execution always uses the captured project sandbox. Custom executables retain the existing explicit grant requirements.

When bwrap is unavailable, the panel reports normal mode with the same visible fallback as the rest of the project. No later agent failure triggers a retry outside an active sandbox. `~/.codex` is a shared writable path by default; other agents' settings, credentials, runtimes, and caches may need explicit grants. Their child processes inherit the project host environment.

Agents read files from disk. The composer shows an unsaved-buffer notice with **Save all**; sending a message does not save or upload unsaved buffers. After a turn, clean editor files refresh using version checks while dirty buffers stay intact. If both you and the agent change the same file, the normal save conflict protections apply. Review agent changes through the existing Git panel.

**Stop** requests interruption, then closes an unresponsive process after three seconds; a second click closes it immediately. **Disconnect** closes an idle nonpersistent agent. **New chat**, project/sandbox/interpreter changes and editor exit release the current view: nonpersistent agents stop, while persistent agents retain their original environment until explicitly ended. Closing the chat panel only hides it. Agent shutdown never interrupts the project terminal or shared SSH master.

Provider settings are saved immediately per host/project. Changing the executable/provider starts a new conversation on the next send; an existing persistent session keeps its original executable and environment. RODE keeps a bounded conversation in memory, without saving transcripts. Use **Running** for a live detached session or **History** for conversations saved by the backend. URLs and command arguments are saved preferences, so use the CLI's login/environment for secrets.

Messages are limited to 32 KiB, normal chat protocol lines to 256 KiB (2 MiB for history connections), and displayed history to 128 messages / 512 KiB. Large tool output is truncated explicitly. Images, file attachments and interactive login in chat are not implemented. Transport tests use deterministic peers; a real Codex compatibility test is available in the [development guide](development.md).
