# Configuration and multiple windows

Run multiple RODE processes normally. Each process has its own workspace, buffers, terminal views, agent connection, SSH master/socket, sandbox helper and temporary GUI snapshot. Closing one window releases its private resources without disconnecting another window's SSH master. [Native persistent terminals](terminals.md) belong to the per-user service on their host and may be shared between windows. Detaching/keeping sessions leaves all views and work intact; explicitly ending a shared session affects every attached view.

For additional editor windows within the same process, choose **File → New editor window**, or right-click a file tab to move it to a new or existing editor window. These windows share the project, document collection and connection. Each has its own tabs, find state and hideable Explorer; chat, terminals and project tools keep their existing owner. Opening an already open file raises its current window. Closing an editor window returns its tabs and unsaved changes to the main window. Project tools and application views have separate detach/reattach controls; see [workspace layout](editing.md#workspace-layout).

Shared configuration stays in the existing locations:

| File on Linux | Contents |
| --- | --- |
| `${XDG_DATA_HOME:-~/.local/share}/rode/app.ron` | Theme/font/interface scale, project/agent/extension preferences, bookmarks, sandbox defaults and saved window/layout state |
| `${XDG_DATA_HOME:-~/.local/share}/rode/recovery/` | Private crash checkpoints for unsaved source/notebooks and composer text; separate from preferences |
| `${XDG_DATA_HOME:-~/.local/share}/rode/workspace-sessions/` | Opt-in saved workspace metadata and ordinary unsaved drafts, with per-generation window ownership |
| `${XDG_CONFIG_HOME:-~/.config}/rode/connections.json` | Named SSH connection profiles |
| `${XDG_CONFIG_HOME:-~/.config}/rode/themes/` | Custom interface/terminal theme JSON files, read at startup |
| `${XDG_CONFIG_HOME:-~/.config}/rode/syntax-themes/` | Custom syntax theme JSON files, read at startup |
| `${XDG_CONFIG_HOME:-~/.config}/rode/extensions/` | Installed tool-adapter JSON manifests |

Settings saves lock the configuration, read its latest contents, apply only the window's changed fields, and replace the file atomically. Changes to different projects, profiles, or fields merge. When two windows change the same setting, the last completed save of that setting wins; a list such as additional sandbox grants is one setting. Closing an older window does not restore its old theme, font, or project preferences over a newer edit. Unknown configuration fields are retained.

Existing windows keep their current in-memory settings. Other windows' changes are loaded on a new launch; the SSH connection picker reloads saved profiles when opened. RODE does not watch configuration files continuously or change an active sandbox/agent behind your back. Window size/layout uses the last saved GUI state as the default for future launches.

Manage saved SSH profiles in the **SSH connections** section of the
**Connections** sidebar. Right-click a profile row to rename, edit, remove or
move it; drag rows to reorder them. An optional
**Connection name** labels it in the sidebar and picker without changing the SSH
address. **Rename…** selects that field; **Save profile** applies the change. The order is the
array order in `connections.json`. Reordering moves one current record relative
to another under the same writer lock, preserving other windows' additions,
settings and unknown fields. Removal refuses a profile whose settings changed
since it was displayed. A stale profile save cannot recreate a removed record;
reopen **Connect SSH** to refresh before choosing a new action. Failed writes
leave the displayed list intact and report an error. List changes do not alter
active workspaces, authentication state or unsaved connection drafts.
Renaming a loaded profile preserves its position and merges other field edits;
an existing destination name is rejected rather than overwritten.
If manually edited storage contains duplicate labels, saves and list edits refuse
the ambiguous records. Give those records distinct names in `connections.json`
before editing them through the picker.

Empty connection fields show muted hints, such as an example identity-file path.
Hints are not saved or passed to SSH. Entered values use normal text colour;
leaving an optional field blank retains its documented default behaviour.

Configuration writes run in the background. New files use owner-only permissions. Stable adjacent `.lock` files coordinate writers and remain in place; they do not mean a process is still running. The OS releases locks on process exit, including a crash. Do not remove lock files while RODE is running. GUI state is written through private temporary snapshots so the GUI framework cannot overwrite shared configuration using an old snapshot.

Malformed, oversized, or unwritable configuration is preserved and a save error is reported. After correcting the file or permissions, edit the setting again to retry retained changes. Preferences are limited to 8 MiB, agent settings to 128 entries / 256 KiB, and SSH profiles to 128 entries / 1 MiB. Use the same RODE version in concurrently running instances so they share the configuration contracts.

## Project settings

Open **Project: Settings and sandbox access** from the command palette. The tab
has five categories when their extensions are enabled: **Code tools**,
**Environment**, **Sandbox**, **LaTeX** and **Presentations**. Environment contains
runtime paths, project variables and Python environments; it also opens through
**Environment: Runtimes and project variables** in the command palette.
Category navigation and save controls stay visible while the
selected form scrolls, including in compact or detached tool panes. The host and
project path identify the scope; these settings are saved per project.

A dot marks categories with unsaved changes. Switching categories keeps their
drafts. **Save changes** and **Discard changes** affect only the selected category;
Environment uses **Apply environment** for its settings. Incomplete edits elsewhere
are retained. LaTeX and presentation Settings buttons
open their corresponding category directly.

**Apply sandbox settings** validates the requested policy and probes enforcement
before activating it. The displayed sandbox status reports the actual current
mode. Applying restarts the project shell and keeps editor buffers open; it retains
drafts from other categories without applying them. Applied rules also become the
defaults for projects without a saved policy. Host monitoring is configured separately in **App settings → Host monitor**.
Collection starts only through Start and continues while hidden until stopped.

## Editing the same document

Local and SSH file writers coordinate revision checking and atomic replacement using advisory file locks. If two RODE windows save conflicting changes based on the same revision, one succeeds and the other reports a save conflict, preserving its buffer. Simultaneous Save As operations cannot overwrite a file another process just created. Document saves create no permanent per-file lock files in source folders.

Saves preserve existing permissions and symlinks. Configuration updates also coordinate with RODE's text-file writer when editing those configuration files directly. Changes from other programs are checked using content revisions; those checks remain best effort when the other program does not use RODE's locking protocol. New Python environments reserve their destination before starting Python, so concurrent creation cannot overwrite the same environment. Git operations, agent commands, and external tools retain their own concurrency rules.

The validated platform is Linux. The filesystem must support POSIX advisory file locks, atomic rename, and atomic hard-link creation for new files. If a safe operation is unavailable or a lock remains busy for three seconds, RODE reports an error and retains edits instead of retrying an unsafe overwrite.

Interface color selection (`color_theme`) and syntax selection (`syntax_theme`) are independent preferences. Each UI action saves only its own changed choice, so different windows can adjust them without reverting one another. See [syntax themes](syntax-themes.md).

Terminal color overrides use independent `terminal_color.<theme-id>.foreground`, `.background`, `.ansi.0`–`.ansi.15` and `.minimum_contrast` scalar keys in the same preferences file. Colors are opaque `#RRGGBB`; contrast is 1–21. The `theme` sentinel restores the corresponding theme default. Only explicit edits are submitted; resetting a theme explicitly restores its 19 fields, while idle/closing windows leave newer edits alone. Overrides load at startup and stay local to the desktop; theme switches select the matching overrides without restarting or modifying a local/SSH shell. See [terminal colors](themes.md).

The host monitor preferences are desktop-wide: `host_monitor_interval` and
`host_monitor_chart`. Each writes independently so unrelated edits in other windows are preserved. The
monitor initializes from saved project monitoring preferences only when no desktop-wide choice has been configured. Selected
application areas, host connections and Start/Stop intent are transient; they
are not restored as running work on startup.

[Keyboard navigation](editing.md#keyboard-pane-navigation) stores the leader in `keyboard.leader` and each action in its own `keyboard.<action>` scalar setting, such as `keyboard.left` or `keyboard.terminal`. Applying bindings writes only changed fields; unrelated edits in other windows remain intact. Bindings reload on startup and invalid saved combinations activate the documented defaults with an explanation in the bindings dialog. Layout, pane focus and pending leader sequences are private to each window.

Shortcut presets use `keyboard.style` (`standard`, `emacs`, `vim`). Standard bindings retain the original `keyboard.leader` and `keyboard.<action>` keys. Emacs and Vim use `keyboard.emacs.leader` / `keyboard.emacs.<action>` and `keyboard.vim.leader` / `keyboard.vim.<action>`. A style change writes only the style key; applying edits writes only changed fields in that profile. Mode, mark, operator counts, last-kill text and queued keystrokes stay in memory and are never saved to shared configuration.

Interface size uses a separate `ui_scale` string preference (`"1"` for 100%, `"0.75"` for 75%, supported range 0.5–2). It multiplies the monitor's native DPI scale, while `font_size` remains the editor/terminal font size in points. The View control saves only `ui_scale`; closing an unchanged older window cannot revert it. An explicit reset to 100% is saved even if that window already displayed 100%. Invalid/nonfinite/out-of-range values fall back to system scaling. This explicit preference takes precedence over scale saved in the GUI framework's private layout snapshot. See [interface sizing](editing.md).

Agent reply personality uses `agent_preferences[workspace].personality` (`default`, `jackson_lamb` or `panne`). Changes merge as one project-scoped scalar, independently of model, permission and other windows' settings. Missing or unknown personality IDs use Default. This preference controls a chat presentation hint; it never selects a new executable or operating mode. See [response personalities](agents.md#response-personalities).

Completed agent activity uses the desktop-wide `agent_show_completed_tools`
string preference. `"true"` keeps completed tool rows visible; the default
`"false"` groups them behind disclosures. Missing or invalid values use the
compact default. Change it under **Agent settings → Keep completed tool activity
visible**. The choice applies across projects and survives restart; other running
RODE processes retain their in-memory choice until relaunched, like other GUI
preferences. It is stored separately from per-project `agent_preferences`, is
never sent to a local or SSH agent, and does not restart active work or change
transcript retention. See [Agent chat](agents.md).

Only explicit display choices are written. Closing an unchanged older window
cannot replace another window's choice with its implicit compact default. An
explicit reset to compact mode is saved even when that window already showed
the default, following the same last-explicit-save rule as other scalar settings.

## Crash recovery

RODE checkpoints unsaved source buffers, structured notebooks and agent composer text after edited frames. A background worker coalesces changes and atomically writes owner-only files in the recovery directory above. Checkpoints contain plaintext, including remote file contents; the directory is private to your account. They are limited to 8 MiB per window. A failed or oversized write reports an error and preserves the last durable checkpoint. Files that were never edited, submitted chat history, attachments and binary buffers are not checkpointed.

After an interrupted window, the next launch offers **Recover drafts** or **Discard drafts**. Recovery opens unsaved buffers, including notebook cells, with their original names for reference. It does not reconnect SSH, restart jobs or write the original files. Choose a destination with Save As when ready. A live window's checkpoints cannot be claimed by another window; advisory locks also prevent two windows from restoring the same checkpoint at once. A confirmed ordinary exit removes that window's checkpoint, while crashes and failed event-loop shutdowns retain it.

Encrypted-project buffers and composer drafts entered while an encrypted project is active are excluded from desktop checkpoints. Retained disconnected encrypted buffers remain excluded after returning to an ordinary project. RODE reports this restriction when such edits are present. Save encrypted and binary edits explicitly. Recovery covers the latest completed checkpoint; edits still being processed when a process is killed may not yet have reached storage.

## Workspace sessions

Choose **Welcome → Keep workspace** to retain this window's prepared project,
open files, cursor positions, editor groups, tool/sidebar arrangement and unsent
agent prompt. RODE checkpoints changes in private files and keeps the saved entry
when the window closes or its desktop disappears. Startup still opens Welcome;
**Resume** explicitly reopens the selected workspace. **Rename** changes its label.
Turning off **Keep workspace** stops further updates; **Remove** deletes an
inactive saved entry and its drafts, without deleting project files or ending jobs.

For ordinary text and notebooks, closing a kept workspace waits for the current
unsaved versions to be checkpointed. Resume restores those drafts over their
original file binding only if the host file still has the saved revision. If the
file changed or cannot be read, RODE opens the draft separately and requires
**Save As**, preserving the host's version. Edits made in a destination buffer while
restoration is pending are kept; any saved draft is recovered separately. Binary edits still use the usual
save/discard prompt. Checkpoints have an 8 MiB limit; write failures keep the last
successful record and ordinary unsaved-file checks remain available.

**Keep workspace** is independent of host-side **Persist**. Resume reattaches
referenced agent, terminal and supported notebook/debugger sessions that still
exist; it never resubmits a prompt, restarts a shell or launches a kernel. Use
**Persist: On** on the corresponding host session if idle work must survive
closing the GUI. **Auto** still finishes accepted agent work and exits, and
**Off** still stops on disconnect. Missing host sessions are not recreated.

An open entry offers **Switch to window** on the same desktop. **Take over**
requires confirmation and transfers the saved-workspace generation; the original
window keeps its buffers but cannot overwrite the resumed entry. A matching
native agent controller can move with it. Agents whose controller changed since
the snapshot, and notebook/debugger sessions still attached elsewhere, require
their existing session controls instead of being silently stolen.

Encrypted workspaces save only their encrypted location and presentation/file
references. Resume requires explicit unlocking. Decrypted file contents and
unsent encrypted-project prompts never enter ordinary workspace storage, even
if their buffers survive a disconnect. Save those edits before leaving. Ordinary
workspace drafts are private user-readable files, not encrypted archives; saved
SSH profiles contain configuration paths, never key contents, tickets or tokens.
