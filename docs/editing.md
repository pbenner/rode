# Editing and shortcuts


- Shared local/SSH file and project-folder browser with left-aligned names, optional hidden files, and parent-folder navigation
- Aligned single-row tabs with bounded widths, duplicate-filename context, active-tab reveal, overflow arrows/editor list, drag reordering, and middle-click closing
- Line numbers, UTF-8 text, [multi-language highlighting and tools](languages.md), undo/redo, clipboard
- Read-only [PDF previews](pdf.md) with page navigation, zoom/pan, and reload
- Read-only [image previews](images.md) in normal tabs, with zoom, fit, transparency, and reload
- Switch Markdown tabs between source editing and [rendered previews](markdown.md), including unsaved changes
- Browse to open text, images, PDFs, notebooks, or any other supported file; the same browser serves Open as hex and Save As
- New files, Save, Save As, Save All, unsaved-change confirmation
- Literal find/replace, case-sensitive or Unicode-aware case-insensitive matching
- [Multiple persistent PTY terminals](terminals.md) with reattachment, shared views, ANSI colors, resizing, scrollback, bracketed paste, shell control keys, and a copy-output button
- Command palette, separate editor font size and interface scale, and persistent configurable themes
- Optional local/remote bubblewrap project mode with explicit normal-mode fallback
- Configurable sandbox access table, Git source control/worktrees, Python environments, Ruff diagnostics/formatting, and on-demand system/GPU monitoring

Open File, Open as hex, and Save As share the project-folder browser's directory rows. Double-click a folder to enter it, or select it and press Right/Enter; the **Up** button returns to its parent. Select a file and choose Open, or double-click it. Arrow keys navigate results, and Tab moves between controls. The filename field also filters the current listing and known files in expanded folders/open tabs; exact paths still work. Save As lets you enter a new name. File browsing runs on the displayed local/SSH host under the active project's sandbox rules.

| Shortcut | Action |
| --- | --- |
| Ctrl+Shift+P | Command palette |
| Ctrl+P | Browse/open a file |
| Ctrl+Shift+O | Open project folder on the current host |
| Ctrl+N | New file |
| Ctrl+S | Save |
| Ctrl+Shift+S | Save As |
| Ctrl+W | Close tab |
| Ctrl+PageUp / PageDown | Previous / next tab |
| Ctrl+Shift+Tab / Ctrl+Tab | Previous / next tab |
| Ctrl+B | Toggle explorer |
| Ctrl+F | Find / replace |
| Ctrl+Shift+R | Toggle Markdown preview, or Marp slides for detected decks |
| Ctrl+Shift+G | Git source control |
| Ctrl+Shift+M | Problems |
| Ctrl+Shift+A | Agent chat |
| Ctrl+Shift+X | Manage extensions |
| Enter in agent chat | Send message |
| Shift+Enter in agent chat | Insert a newline |
| Shift+Alt+F | Format document |
| F5 / Shift+F5 | [Start/continue debugging / stop or detach](debugger.md) |
| F6 / F9 | Pause debugger / toggle source breakpoint |
| F10 / F11 / Shift+F11 | Step over / into / out |
| Ctrl+` | Toggle / focus terminal |
| Ctrl+Plus / Minus | Editor/terminal font size, or image zoom when the image canvas has focus |
| Ctrl+0 | Reset interface scale to 100% (outside the terminal) |

Use Command for editor shortcuts on macOS; tab cycling with Control+Tab remains available. When the terminal has focus, Tab/Shift+Tab, arrows, Escape, and normal control keys go to the shell; the command palette, project/tools shortcuts, tab switching, and terminal toggle remain editor shortcuts. Escape closes find and returns to the editor; Escape in the terminal belongs to the shell.

Choose a percentage in **View → Interface scale** to resize the entire GUI, including menu text, icons, buttons, side panels and editor/terminal text. **Smaller (5%)** and **Larger (5%)** provide fine adjustments. Each action applies after a completed click and closes the menu; there is no slider to chase as the interface resizes. The 50–200% scale is relative to the operating system's monitor scaling: **100%** follows the system, **75%** makes the interface smaller, and **50%** compensates for a 2x display scale when you want a more compact layout. Changes apply immediately and are saved on the desktop computer, independently of the project or SSH host.

**View → Editor font size** adjusts source/terminal text without resizing the menus and icons. **Ctrl+0** or **View → Reset interface scale (100%)** resets the whole-interface scale. The command palette offers **Interface: Zoom in**, **Interface: Zoom out**, and **Interface: Reset scale**. Interface scaling leaves buffers, terminal sessions, source font preferences and theme choices intact; it continues to follow monitor DPI when moving the window between displays.

Side panels narrow to preserve a usable editor at the minimum 850×560 window size. Paths and status text stay on one line with full hover text, find/replace uses two rows when space is tight, and terminal/header controls stay accessible. Opening or switching a file focuses its editor, with cursor/undo state retained across tab switches and reordering. Unsaved tabs still require a discard decision when closed.

Drag the terminal's top edge to resize it up to the full available workspace height. The application menu and status bar remain accessible. Drag it down again, or use **Ctrl+`** to hide it and return to the editor; the shell session keeps running.

[Compact layout with explorer, find/replace, terminal, and monitor](../assets/layout-compact.png).


## Keyboard pane navigation

Choose **Standard**, **Emacs**, or **Vim** in **View → Keyboard shortcuts / navigation**, then **Apply bindings** (**Ctrl+Enter**). The command palette also offers **Keyboard style: Standard / Emacs / Vim**, which applies and saves immediately. Each style keeps its own configurable leader and action bindings. Switching styles preserves buffers, cursors and native undo history.

| Style | Source editing | Pane/file command prefix |
| --- | --- | --- |
| Standard (default) | Existing editor shortcuts | `Ctrl+Space` |
| Emacs | Control-key movement, mark, kill/yank and search | `Ctrl+X` |
| Vim | Normal, insert and visual modes | `Ctrl+W` |

In a **terminal**, all presets use **Ctrl+Space** for pane commands so the shell keeps `Ctrl+X` and `Ctrl+W`. Terminal input, search/form fields, notebook cells, hex views and previews retain their existing editing controls. These are core editing presets with the commands listed below; text changes use RODE's native undo grouping and save protections. They run identically for local and SSH source files and do not load external editor plugins or configuration.

The following table describes the **Standard** preset:

Use **Ctrl+Space**, release Ctrl, then press a command key. Defaults avoid Alt so they coexist with window managers such as Notion. A border marks the focused pane and a hint appears while the leader is active. **Escape** cancels; the sequence expires after five seconds.

| After the leader | Action |
| --- | --- |
| `h` / `j` / `k` / `l`, or arrows | Focus a visible pane left / below / above / right |
| `Tab` / `Shift+Tab` | Next / previous visible pane |
| `e` / `f` / `t` / `p` | Focus editor / explorer / terminal / project tools |
| `u` | Focus the application menu; use Enter and Tab to operate controls |
| `Shift+H` / `Shift+L` | Shrink / grow the explorer, or the tools pane when focused |
| `Shift+J` / `Shift+K` | Shrink / grow terminal height |
| `m` | Maximize / restore the focused pane |
| `q` | Hide the focused side pane; terminal jobs keep running |
| `n` / `[` / `]` | New / previous / next terminal view |
| `s` | Toggle the active native terminal's persistence |
| `o` / `r` / `c` | Open file / open project folder / command palette |
| `/` | Edit navigation bindings and view existing editor shortcuts |

Focusing a hidden pane reveals it. Moving to another pane restores the regular layout after maximization. Tools reuse their last focused control; Tab and Shift+Tab move between their controls. The explorer supports Up/Down, Home/End, PageUp/PageDown, Right to expand, Left to collapse or select the parent, Enter to open, and typing a filename prefix. Navigation visits loaded tree entries; it does not crawl the filesystem. Escape returns to the editor.

The command palette and open-file suggestions support Up/Down and Enter. In Open file, Enter opens the typed path until you use arrows to choose a suggestion. **Ctrl+Shift+O**, a folder path, then **Ctrl+Enter** opens that project on the selected host. The same pane commands work for local and SSH projects. Terminal Tab, arrows, Ctrl+C and Ctrl+K remain shell keys outside navigation sequences. After a dialog closes, focus returns to the active pane.

Open **Keyboard shortcuts** from the command palette or use leader `/` to change the leader and every pane binding. Accepts `Leader+h`, direct shortcuts such as `Ctrl+Shift+Left`, or up to four comma-separated alternatives. Empty fields disable an action. **Apply bindings** (**Ctrl+Enter**) validates and saves changes immediately; Escape closes the dialog. Restore preset bindings fills the fields for the selected style; Apply activates them. Invalid or duplicate bindings leave the active configuration intact. Direct bindings take precedence over existing editor shortcuts, so choose them carefully. Window-manager grabs still take priority. These settings merge per style and binding between concurrent windows; other windows load the changes on their next start. Direct bindings run first; preset editing keys then take priority over remaining fixed editor shortcuts.

### Emacs style

| Key | Action |
| --- | --- |
| `Ctrl+A` / `Ctrl+E` | Start / end of line |
| `Ctrl+B` / `Ctrl+F`, `Ctrl+P` / `Ctrl+N` | Previous / next character, previous / next line |
| `Ctrl+Space` | Toggle mark; subsequent movement extends the selection |
| `Ctrl+G` | Cancel mark and selection |
| `Ctrl+D` / `Ctrl+H` | Delete forward / backward |
| `Ctrl+K` / `Ctrl+W` | Kill to line end (or its newline), kill selected region |
| `Ctrl+Y` | Yank the last killed text, including across source tabs |
| `Ctrl+/` | Undo |
| `Ctrl+S` | Open RODE's find bar; Enter selects a match |
| `Ctrl+X`, `Ctrl+F` / `Ctrl+S` / `Ctrl+W` | Open file / save / Save As |
| `Ctrl+X`, `o` / `k` / `b` / `Shift+B` | Next pane / close document / next document / previous document |
| `Ctrl+X`, `e` / `f` / `t` / `p` / `x` / `/` | Editor / explorer / terminal / tools / command palette / bindings |

Release the prefix key before the next key; holding Ctrl for `Ctrl+X Ctrl+S` also works. A killed region uses the system clipboard and a bounded last-kill buffer. Navigation defaults avoid Alt to leave window-manager shortcuts available.

### Vim style

Source tabs start in **NORMAL** mode. The mode strip and block cursor show the current state. Use `i` to type and **Escape** or **Ctrl+[** to return to Normal mode.

| Key | Action |
| --- | --- |
| `h/j/k/l`, arrows | Character / line movement |
| `w/b/e` | Next word / previous word / end of word |
| `0`, `^`, `$`, `gg`, `G` | Line start, first non-space, line end, document start/end |
| Counts such as `3w`, `2dd`, `d2w` | Repeat a motion or operator, up to 999 |
| `i/a/I/A`, `o/O` | Insert before/after cursor, at line start/end, or on a new line below/above |
| `v` / `Shift+V` | Character / line visual selection; movement extends it |
| `d/c/y` plus a motion; `dd/cc/yy` | Delete / change / yank a range or line; also work on visual selections |
| `x/X/s`, `D/C/Y` | Delete forward/backward, substitute, delete/change to line end, yank line |
| `p/P` | Paste the last yanked/killed text after/before the cursor or line |
| `u` / `Ctrl+R` | Undo / redo |
| `/`, `n`, `Shift+N` | Open find, next match, previous match |
| `:w`, `:q`, `:e`, `:help`, then Enter | Save, guarded document close, open file, keyboard settings |
| `Ctrl+W`, `h/j/k/l` / `w` / `o` / `q` | Directional focus / next pane / maximize-restore / hide side pane |
| `Ctrl+W`, `e/f/t/p` | Editor / explorer / terminal / tools |

The settings table lists the remaining pane/file commands. By default, `Ctrl+W` is RODE's pane prefix throughout source editing; in a shell it remains a shell key. Visual selection, clipboard pastes, Unicode motions and native undo share the same buffer as ordinary editing. The presets cover core commands rather than full Emacs or Vim emulation: Vim text objects, macros, named registers, block selection and extended Ex commands are outside this keymap.

Saves check the original SHA-256 revision, write a temporary file beside the destination, preserve existing permissions, and rename it into place. Symlinks are followed so the link itself is preserved. A detected external change produces a conflict and keeps your edits open. Save As refuses to silently overwrite an unrelated existing file. To resolve a conflict, save a separate copy or close/reopen the tab to load the disk version. RODE writers serialize revision checks and replacement with advisory locks, including local/SSH writers sharing a file; only one save based on the same old revision can succeed. For other programs, revision checks remain best effort. See [configuration and multiple windows](configuration.md). Unsaved buffers remain in memory and do not survive a crash.

This is a working first version, with deliberate limits: UTF-8 text files up to 8 MiB, image previews up to 32 MiB with bounded decoded pixels, 10,000 entries per directory, and one workspace per window. It does not yet include an LSP client, recursive project search, remote filesystem watching, or a complete terminal emulator feature set. The editor buffer uses egui's text editor; it is not a streaming editor for huge files. No VS Code performance comparison is claimed.

For binary files and explicit byte editing, see [Hex and binary analysis](binary.md). Optional modules and tool adapters are managed through [Extensions](extensions.md).
