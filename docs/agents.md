# Agent chat

Open **View → Agent chat**, use the speech-bubble activity icon, or press **Ctrl+Shift+A**. Choose a provider and open its settings button. Send a message with **Send** or **Ctrl+Enter**; Enter inserts a newline. Replies stream into the chat, with selectable text, copy buttons, code blocks, and expandable tool results. Permission requests and questions appear above the conversation.

The agent starts in the opened project folder, on the computer hosting that project. The panel identifies **LOCAL** or **SSH**, the folder, and the actual sandbox mode. Without an opened folder, it uses RODE's startup working directory. Opening the panel starts no process. Sending a message or explicitly opening **History** starts the selected agent; history browsing sends no model prompt.

## Model and access controls

Use **Model** and **Access** / **Permissions** above the message field. Choices are saved per host/project through the shared preference locks; opening a menu starts no process. Controls are disabled during active work. Stop the current turn before changing them.

For **Codex**, Model accepts an explicit model ID or the agent default. After the first message or restoring a conversation, the picker loads the running agent's model catalog and advertised reasoning efforts. Catalog entries describe available choices, not an account-entitlement guarantee. Custom model/effort IDs remain available when discovery is unsupported. For **Codex · local model**, enter the installed server model ID and an optional supported reasoning effort. Codex's cloud-oriented catalog is not used as an inventory of installed Ollama models.

Codex tool access offers **Read only**, **Project access** (default), and **Full access**. Read only prevents approval-based tool escalation. Project access allows workspace writes and can request additional access. Full access uses the host permissions left inside the active project sandbox; in normal mode it uses the user's host permissions. The project's actual read-only restriction always takes precedence. These are agent tool policies, separate from the enclosing filesystem sandbox.

The common permission-request choices are:

- **Ask when needed** (default): display the requests the agent sends. This does not force a prompt for every command or edit already permitted by its mode.
- **Never ask**: decline permission requests automatically. Already permitted tools can run; this setting does not grant additional access. Agent questions still require an answer.

For **Other agent · ACP**, RODE renders the select/boolean options advertised by that session, including model, reasoning and operating modes. Modern session configuration is preferred, with a legacy mode fallback. Unsupported option types are omitted. Model-related choices appear under Model; modes and other options appear under Permissions. Idle changes wait for acknowledgement and refresh dependent choices without sending a message. Unavailable or rejected saved settings stop submission with an explanation; **Agent settings → Reset saved agent controls** restores a fresh session's defaults. RODE does not invent a universal read-only/full-access switch for ACP agents that expose no such mode.

Codex changes apply to the next turn while retaining the conversation. Returning an explicit model/effort override to its default starts a fresh conversation so old overrides cannot remain active. Executable/provider changes also start a fresh conversation. Transcripts stay in memory; they are never written to preferences. No selector or approval can widen the outer project sandbox's filesystem or network grants.

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

The editor and Codex are separate packages. Optional **`rode-codex`** supplies a pinned native [Codex CLI](https://developers.openai.com/codex/cli/) executable, independent of VS Code, Node.js and npm. Leave **Codex executable** set to `auto`. Older saved settings containing `codex` also select this mode. An explicit custom executable overrides discovery; without the optional runtime, an installed Codex on PATH or in `~/.local/bin` remains usable. RODE uses the [app-server protocol](https://developers.openai.com/codex/app-server/) for conversations, command/file tools, approvals, questions and interruption.

For a local project, authenticate in a terminal with `rode --codex login`. Use `rode --codex --version` to check the bundled version. These commands forward to Codex without starting the editor. Existing Codex authentication/configuration in `~/.codex` remains usable after uninstalling VS Code. Local-model mode does not require OpenAI authentication.

For SSH projects, the first message or explicit History action deploys the pinned runtime to a private cache on the project host, using the existing authenticated connection. Transfers are streamed, size/SHA-256 checked, serialized across windows and atomically published. Subsequent starts reuse the verified cache. Credentials are never copied from the desktop. Authenticate on that host using the cached executable at `~/.cache/rode/codex/VERSION/ARCH/SHA256/codex`, for example with `login --device-auth`. `RODE_CODEX_CACHE` on the host can select another absolute cache location with an owned, non-writable-by-others parent.

The optional package includes its own CPU architecture. If the remote computer needs another architecture, prepare it with `python3 scripts/bundle-codex.py --architecture aarch64` (or `x86_64`) in a source build, include it in a portable bundle, or configure an installed executable on that host. When no upstream binary exists, provide a compatible source-built executable if supported, or choose an ACP agent. Missing Codex affects only that integration. RODE does not silently download another runtime or execute the agent on the desktop instead.

Source builds optionally prepare the runtime with `python3 scripts/bundle-codex.py`. Portable distributions put version/architecture folders under `rode-codex/` beside `rode`; Debian's optional package installs them under `/usr/lib/rode/codex/`. `RODE_CODEX_BUNDLE` selects an explicit prepared runtime directory. The optional package has its own version and protected checksum manifest, allowing compatible runtime upgrades without rebuilding the editor. Runtime releases still require protocol compatibility checks. The application does not fetch updates during startup or chat. Discovery and verification happen on workers, and no agent starts before a message or explicit History action. The binary adds disk/download size only when installed and is not loaded into the editor's idle process.

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

**Stop** requests interruption, then closes an unresponsive process after three seconds; a second click closes it immediately. **New chat** stops the process and clears the displayed conversation. **Disconnect** closes an idle agent. Closing the chat panel hides it while a running task continues. Switching projects or applying a new sandbox stops the old agent. Agent shutdown never interrupts the project terminal or shared SSH master.

Provider settings are saved immediately per host/project. Changing the executable/provider starts a new conversation on the next send; changing Python environments requires New chat for the process to inherit the new environment. RODE keeps a bounded conversation in memory, without saving transcripts. After exit, use **History** to reopen conversations saved by the agent. URLs and command arguments are saved preferences, so use the CLI's login/environment for secrets.

Messages are limited to 32 KiB, normal chat protocol lines to 256 KiB (2 MiB for history connections), and displayed history to 128 messages / 512 KiB. Large tool output is truncated explicitly. Images, file attachments and interactive login in chat are not implemented. Transport tests use deterministic peers; a real Codex compatibility test is available in the [development guide](development.md).
