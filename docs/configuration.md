# Configuration and multiple windows

Run multiple RODE processes normally. Each window has its own workspace, buffers, terminal views, agent process, SSH master/socket, sandbox helper, and temporary GUI snapshot. Closing one window releases its private resources without disconnecting another window's SSH master. [Native persistent terminals](terminals.md) belong to the per-user service on their host and may be shared between windows. Detaching/keeping sessions leaves all views and work intact; explicitly ending a shared session affects every attached view.

Shared configuration stays in the existing locations:

| File on Linux | Contents |
| --- | --- |
| `${XDG_DATA_HOME:-~/.local/share}/rode/app.ron` | Theme/font, project and agent preferences, sandbox defaults, and the last saved window/layout state |
| `${XDG_CONFIG_HOME:-~/.config}/rode/connections.json` | Named SSH connection profiles |
| `${XDG_CONFIG_HOME:-~/.config}/rode/themes/` | Custom theme JSON files, read at startup |

Settings saves lock the configuration, read its latest contents, apply only the window's changed fields, and replace the file atomically. Changes to different projects, profiles, or fields merge. When two windows change the same setting, the last completed save of that setting wins; a list such as additional sandbox grants is one setting. Closing an older window does not restore its old theme, font, or project preferences over a newer edit. Unknown configuration fields are retained.

Existing windows keep their current in-memory settings. Other windows' changes are loaded on a new launch; the SSH connection picker reloads saved profiles when opened. RODE does not watch configuration files continuously or change an active sandbox/agent behind your back. Window size/layout uses the last saved GUI state as the default for future launches.

Configuration writes run in the background. New files use owner-only permissions. Stable adjacent `.lock` files coordinate writers and remain in place; they do not mean a process is still running. The OS releases locks on process exit, including a crash. Do not remove lock files while RODE is running. GUI state is written through private temporary snapshots so the GUI framework cannot overwrite shared configuration using an old snapshot.

Malformed, oversized, or unwritable configuration is preserved and a save error is reported. After correcting the file or permissions, edit the setting again to retry retained changes. Preferences are limited to 8 MiB, agent settings to 128 entries / 256 KiB, and SSH profiles to 128 entries / 1 MiB. All concurrently running instances need the updated writer; older builds do not participate in this coordination.

## Editing the same document

Local and SSH file writers coordinate revision checking and atomic replacement using advisory file locks. If two RODE windows save conflicting changes based on the same revision, one succeeds and the other reports a save conflict, preserving its buffer. Simultaneous Save As operations cannot overwrite a file another process just created. Document saves create no permanent per-file lock files in source folders.

Saves preserve existing permissions and symlinks. Configuration updates also coordinate with RODE's text-file writer when editing those configuration files directly. Changes from other programs are checked using content revisions; those checks remain best effort when the other program does not use RODE's locking protocol. New Python environments reserve their destination before starting Python, so concurrent creation cannot overwrite the same environment. Git operations, agent commands, and external tools retain their own concurrency rules.

The validated platform is Linux. The filesystem must support POSIX advisory file locks, atomic rename, and atomic hard-link creation for new files. If a safe operation is unavailable or a lock remains busy for three seconds, RODE reports an error and retains edits instead of retrying an unsafe overwrite.

Interface color selection (`color_theme`) and syntax selection (`syntax_theme`) are independent preferences. Each UI action saves only its own changed choice, so different windows can adjust them without reverting one another. See [syntax themes](syntax-themes.md).

[Keyboard navigation](editing.md#keyboard-pane-navigation) stores the leader in `keyboard.leader` and each action in its own `keyboard.<action>` scalar setting, such as `keyboard.left` or `keyboard.terminal`. Applying bindings writes only changed fields; unrelated edits in other windows remain intact. Bindings reload on startup and invalid saved combinations activate the documented defaults with an explanation in the bindings dialog. Layout, pane focus and pending leader sequences are private to each window.

Shortcut presets use `keyboard.style` (`standard`, `emacs`, `vim`). Standard bindings retain the original `keyboard.leader` and `keyboard.<action>` keys. Emacs and Vim use `keyboard.emacs.leader` / `keyboard.emacs.<action>` and `keyboard.vim.leader` / `keyboard.vim.<action>`. A style change writes only the style key; applying edits writes only changed fields in that profile. Mode, mark, operator counts, last-kill text and queued keystrokes stay in memory and are never saved to shared configuration.
