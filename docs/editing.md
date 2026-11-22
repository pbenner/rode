# Editing and shortcuts

## Word wrap

Choose **View → Word wrap**, press **Alt+Z**, or run **Editor: Toggle word wrap**
from the command palette. Long source lines reflow to the editor width, including
when a window is resized. Wrapping is off by default; the choice is shared across
editor windows and saved immediately for the next launch.

This is a display setting: it never inserts line breaks, saves a file, or resets
the cursor, selection or undo history. Line numbers, Git markers, breakpoints and
diagnostics continue to refer to file lines, with continuation rows sharing their
original line number. Terminal output and rendered previews keep their own layout.

## Git line changes

When the Git extension is enabled and Git is available on the project host, the
source editor shows changes beside the line numbers: green bars for added lines,
blue bars for modified lines, and red triangles where lines were deleted. Hover
a marker for its meaning. Breakpoints and diagnostics keep their own gutter space.

Click a marker to reveal its source location and open a read-only change preview
inside the editor. Removed
lines appear in red and added lines in green, with the old and current line
ranges above them. **Previous** and **Next** move between changes and reveal the
corresponding source location. Click the same marker again, select **Close**, or
press **Escape** to dismiss the preview. The preview uses the same comparison as
the markers, including unsaved text; opening it does not save, stage or discard
anything. It closes when the buffer, file or comparison changes, so an old hunk
cannot be mistaken for current code.
Previews above 1 MiB display a truncation notice.

Choose **Revert change** in the preview to restore just that block from the last
commit. This removes added lines, restores deleted lines, or replaces modified
lines while preserving changes elsewhere in the buffer. This edits only the
buffer. Use **Save** to write the result if it differs from the saved file.
Git's staging area is unchanged.
Use ordinary **Undo** / **Redo** to undo or repeat the revert. For a new file,
reverting its added block clears the buffer without deleting the file.

Markers compare the current buffer with the file in `HEAD`, including staged and
unsaved edits. New, non-ignored files show added lines; ignored files and files
outside a repository have no markers. Committing changes updates the baseline
within a few seconds. Staging or saving alone does not clear the markers.
CRLF and LF line endings compare equally, so Git-normalized checkouts do not
produce whole-file markers.

Reads run asynchronously on the local or SSH project host under the project's
current access policy, independently of the Git panel's execution mode. Missing
Git, disconnected hosts, denied reads and binary/non-regular Git entries omit
markers. Source or baseline text above 1 MiB or 50,000 lines, and comparisons that
exceed the background diff time budget, also omit markers to keep editing responsive.
No Git hooks, external diff drivers or temporary index files are used.

## Explorer context menu

Right-click a file, folder, or the project name in the explorer to open its menu. Press Escape to close the menu. Mutations run on supported Linux project hosts.

- **Open as project** opens the normal project dialog with the selected folder prefilled on the current local or SSH host. It loads any saved sandbox settings for that folder, otherwise starting with the current project's policy. Choose the sandbox and access rules before confirming Open. Cancelling leaves the current project intact; confirming uses the normal unsaved-edit and encrypted-project guards.
- **Open as encrypted project…** opens the [encrypted-project form](encrypted-projects.md) with the selected folder and host prefilled. Browse or edit the storage path before unlocking; opening the form alone does not mount or create storage.
- **New file…** and **New folder…** create a named entry inside the selected folder, or beside a selected file.
- **Rename…** updates open editor paths, including files inside a renamed folder, while preserving unsaved edits.
- **Duplicate…** copies saved file or folder contents to a new name. Existing destinations are never replaced. Executable permissions are retained and symbolic links are copied as links.
- **Delete…** asks for confirmation and removes the selected entry. Folders are removed recursively; symbolic link targets are preserved. Open editors remain available, and **Save As** can retain their contents separately.
- **Copy path**, **Copy relative path**, and **Refresh** work for both files and folders. Files also retain **Open as hex / inspect binary**.

Names retained by an open editor cannot be reused by creating, renaming or duplicating an entry. Save that buffer under another name or close its tab first.

Operations use the prepared project's actual filesystem policy. Mutation controls are disabled for active read-only projects, during project or sandbox-policy preparation, while files are being opened or saved, and while another explorer mutation is pending. Duplication streams file contents without a byte-size ceiling; folder traversal remains limited to 10,000 entries and 64 directory levels; unsupported special files cause a visible error, and incomplete copies are not published.

## Editing features

- Shared local/SSH file and project-folder browser with left-aligned names, optional hidden files, and parent-folder navigation
- Aligned single-row tabs with bounded widths, duplicate-filename context, active-tab reveal, overflow arrows/editor list, drag reordering, and middle-click closing
- Line numbers, UTF-8 text, [multi-language highlighting and tools](languages.md), undo/redo, clipboard
- Optional soft word wrapping, shared across editor windows and saved for the next launch
- Read-only [PDF previews](pdf.md) with page navigation, zoom/pan, and reload
- Read-only [image previews](images.md) in normal tabs, with zoom, fit, transparency, and reload
- Switch Markdown tabs between source editing and [rendered previews](markdown.md), including unsaved changes
- Browse to open text, images, PDFs, notebooks, or any other supported file; the same browser serves Open as hex and Save As
- New files, Save, Save As, Save All, unsaved-change confirmation
- Literal find/replace, case-sensitive or Unicode-aware case-insensitive matching
- [Multiple persistent PTY terminals](terminals.md) with reattachment, shared views, ANSI colors, resizing, scrollback, bracketed paste, shell control keys, and a copy-output button
- Command palette, separate editor font size and interface scale, and persistent configurable themes
- Optional local/remote bubblewrap or Landlock project isolation with visible normal-mode fallback
- Configurable sandbox access table, Git source control/worktrees, project environments and tasks, Ruff diagnostics/formatting, and on-demand system/GPU monitoring

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
| Alt+Z | Toggle word wrap |
| Ctrl+Shift+R | Toggle Markdown preview, or Marp slides for detected decks |
| Ctrl+Shift+G | Git source control |
| Ctrl+Shift+M | Problems |
| Ctrl+Shift+A | Agent chat |
| Ctrl+Shift+X | Manage extensions |
| Enter in agent chat | Send message, or queue a follow-up while the agent is working |
| Shift+Enter in agent chat | Insert a newline |
| Up / Down at the start of agent input | Recall older / newer messages in the current conversation; Down past the newest restores your unsent draft |
| Shift+Alt+F | Format document |
| F5 / Shift+F5 | [Start/continue debugging / stop or detach](debugger.md) |
| F6 / F9 | Pause debugger / toggle source breakpoint |
| F10 / F11 / Shift+F11 | Step over / into / out |
| F11 with a PDF/PPTX/DOCX editor pane focused / Escape in fullscreen | Toggle document fullscreen / exit fullscreen |
| Ctrl+~ / Ctrl+` | Toggle tools sidebar, restoring its last selected tab |
| Ctrl+Shift+T | Toggle / focus terminal |
| Ctrl+Plus / Minus | Editor/terminal font size, or image zoom when the image canvas has focus |
| Ctrl+0 | Reset interface scale to 100% (outside the terminal) |

The **Standard** preset also provides these shortcuts in focused source tabs:

| Shortcut | Action |
| --- | --- |
| Ctrl+] / Ctrl+[ | Indent / outdent the current line or selected lines |
| Tab / Shift+Tab | Indent selected lines / outdent; Tab without a selection inserts indentation to the next stop |
| Ctrl+/ | Toggle line comments for the current line or selected lines |
| Ctrl+L | Select the whole line; repeat to extend to following lines |
| Ctrl+Shift+D | Duplicate the current line or selected lines |
| Ctrl+Shift+K | Delete the current line or selected lines |
| Ctrl+Enter / Ctrl+Shift+Enter | Insert an indented line below / above without splitting the current line |
| Ctrl+Z / Ctrl+Shift+Z or Ctrl+Y | Undo / redo |

Indentation follows existing tabs or spaces and stays consistent within the open tab, with a four-space fallback (tabs for new Go files and Makefiles). A selection ending at the start of a line excludes that line. Block edits preserve Unicode text, selection direction and line endings, and each operation is one undo step. Standard copy/paste, word movement and Shift-selection remain available.

Line commenting uses the detected language's `//`, `#`, `--`, `!` or `%` prefix and preserves leading indentation. Languages without a supported line-comment form, including JSON, HTML/XML and CSS, show a notice without changing the buffer. Terminal input, form fields and structured document views keep their existing keys. Emacs retains Ctrl+/ for undo, and Vim retains Ctrl+[ for Normal mode.

Use Command for editor shortcuts on macOS; tab cycling with Control+Tab remains available. When the terminal has focus, Tab/Shift+Tab, arrows, Escape, and normal control keys go to the shell; the command palette, project/tools shortcuts, tab switching, and sidebar/terminal toggles remain editor shortcuts. Escape closes find and returns to the editor; Escape in the terminal belongs to the shell.

Choose a percentage in **View → Interface scale** to resize the entire GUI, including menu text, icons, buttons, side panels and editor/terminal text. **Smaller (5%)** and **Larger (5%)** provide fine adjustments. Each action applies after a completed click and closes the menu; there is no slider to chase as the interface resizes. The 50–200% scale is relative to the operating system's monitor scaling: **100%** follows the system, **75%** makes the interface smaller, and **50%** compensates for a 2x display scale when you want a more compact layout. Changes apply immediately and are saved on the desktop computer, independently of the project or SSH host.

**View → Editor font size** adjusts source/terminal text without resizing the menus and icons. **Ctrl+0** or **View → Interface scale → Reset to system default** resets the whole-interface scale. The command palette offers **Interface: Zoom in**, **Interface: Zoom out**, and **Interface: Reset scale**. Interface scaling leaves buffers, terminal sessions, source font preferences and theme choices intact; it continues to follow monitor DPI when moving the window between displays.

Source text defaults to **15 pt**. Saved text sizes are retained; use the existing
**Editor font size** control or **Ctrl+Plus / Ctrl+Minus** to adjust them.

## Workspace layout

The top-right header action is **Open project…** in the implicit CWD context,
**Close project** for an explicitly opened folder, or **Close & lock** for an
encrypted project. Open another folder through **File → Open project folder…**,
**Ctrl+Shift+O**, or Connections. Closing checks edits across all editor windows
and returns to Welcome, restoring the local CWD in Normal mode or retaining SSH
without the closed project's grants. Encrypted close requires verified unmount.
The bottom status bar has separate **LOCAL** or **SSH · user@host**,
project-path and access-mode controls. Open the connection control to disconnect
or reconnect; open the path control for the full folder path, **Copy path** and
**Open project…**. Long paths truncate in the footer and scroll inside the popup.
Encrypted projects add a compact lock-status control to this same footer; click
it for encryption and lock controls, or hover for the storage host and paths.
A **Disconnect** button also appears beside the project action while SSH is
connected. It closes the project first and releases SSH after any required lock
succeeds; a failed lock keeps the connection available.

The left rail contains **Projects**, **Connections**, **System monitor**, **File transfers**, and **Stocks**. **App settings**, **Extensions**, and keyboard help sit at the bottom. Disabled extensions hide their entries. Connections opens a resizable sidebar beside the current content; the other application views use the main content space without duplicating project tools. Returning to **Projects** restores its selected view, unsaved files, chat draft and running shell.

The left activity bar groups **Explorer** and **Project tools** directly below **Projects**, followed by a separator before application views. Their highlights show whether Explorer and tools are open. Project tools provides **Chat**, **Terminal**, **Git**, **Problems**, **Tasks**, **Shared folders**, **Debugger** and **Settings**, in that order; disabled extensions hide their tabs. The tab bar wraps on narrow windows and disappears from the main window when tools are closed or detached. **Settings → Environment** contains runtime paths, project variables and Python environments. Right-click a tool tab to open it in a separate window.

Welcome is the initial view, with Explorer hidden. With no local or SSH target, the current working directory opens as the project in **Normal** mode, including when it is your home directory. Saved sandbox settings are preserved but do not apply to this implicit startup; use `--sandbox` to request isolation explicitly. **Projects**, **Explorer** and **Project tools** are always available in the left rail, including while viewing application tools. Clicking the selected **Projects** icon deselects it and returns to Welcome; clicking it again restores the project view. An empty project shows **Start editing**, while Welcome remains a separate screen. Welcome leaves open files, unsaved edits, chats, terminals and detached windows intact. Opening a file or selecting Explorer or Project tools enters the project directly.

**Connections** and **Explorer** use the same left sidebar space. Connections
keeps **Open project…**, **Connect SSH…** and **New tunnel…** above the saved
projects, SSH profiles and tunnel setups. All three saved-list sections can be
collapsed and use full-width rows without vertical guide lines. Switching between
the two retains the current editor/tool view and each sidebar’s layout. The
compact bookmark icon sits at the right of **Saved projects**. SSH and tunnel
setup buttons live in Connections; **Active Tunnels** opens the running-tunnel list.

The explorer remains beside the project content and can be hidden independently. Opening a project tool replaces the code view instead of shrinking it. **Project tools** initially opens **Chat**, then remembers the tab you select. Automatic project-shell startup and SSH login do not change that choice. Close **Project tools** with the same button, or open a file in the explorer, to return to editing; buffers, cursors, undo history, chats and terminal sessions are retained.

Choose **Detach** at the far right of the project tab bar to move the project tools into a separate native window while the editor stays visible in the main window. **Dock back**, or closing the tools window, returns it without stopping its work. **Ctrl+~** hides or restores that window. Detached tool windows contain tool tabs and window controls. The window controls are separated from the selectable tabs by a vertical rule; on very narrow windows they move to their own right-aligned row.

Choose **Detach tab** beside **Detach**, or right-click a tool and choose **Open in separate window**, to detach just that tool. Several tools can detach independently; each keeps its existing controller. Use **Reattach** in its window, or close it, to return it. Tool commands raise its existing window. Conversations, drafts, running shells and tool state stay intact when moving between windows.

The **Connections**, **System monitor**, **File transfers**, **Stocks** and **App settings** icons in the left rail also offer **Open in separate window** on right-click (or the keyboard context-menu shortcut). Detaching the current application page returns the main window to **Projects**; detaching Connections leaves the central view unchanged. Clicking a detached icon raises its existing window without changing the main window's page. Right-click the icon and choose **Reattach to main window**, use **Reattach** in the separate window, or close that window to bring the page back. These windows share one application process: monitor sampling, transfers, host selections and drafts continue without being restarted or duplicated.

The terminal fills this workspace rather than occupying a separate bottom panel. **Ctrl+Shift+T** selects the terminal or returns to code. **Ctrl+~** (also **Ctrl+`**) hides or restores the tools sidebar. Its first opening selects **Chat** when enabled; later openings restore the selected tab, whether that is Chat, Terminal, Git or another tool. If the remembered tab is detached or disabled, opening tools selects an available docked panel. When all panels are detached, it raises an enabled tool window. Hiding it keeps the shell and agent running, and preserves drafts. A detached tools window hides and reopens without docking itself. Resize a detached tools window using its native borders. Paths and status text retain their full hover text, and find/replace adapts to narrow windows. Unsaved files still require a discard decision when closed.

## Keyboard pane navigation

Choose **Standard**, **Emacs**, or **Vim** in **View → Keyboard shortcuts…**, then **Apply bindings** (**Ctrl+Enter**). The command palette also offers **Keyboard style: Standard / Emacs / Vim**, which applies and saves immediately. Each style keeps its own configurable leader and action bindings. Switching styles preserves buffers, cursors and native undo history.

| Style | Source editing | Pane/file command prefix |
| --- | --- | --- |
| Standard (default) | Existing editor shortcuts | `Ctrl+Space` |
| Emacs | Control-key movement, mark, kill/yank and search | `Ctrl+X` |
| Vim | Normal, insert and visual modes | `Ctrl+W` |

In a **terminal**, all presets use **Ctrl+Space** for pane commands so the shell keeps `Ctrl+X` and `Ctrl+W`. Terminal input, search/form fields, notebook cells, hex views and previews retain their existing editing controls. These are core editing presets with the commands listed below; text changes use RODE's native undo grouping and save protections. They run identically for local and SSH source files and do not load external editor plugins or configuration.

For direct access to visible controls, press **Ctrl+Space**, release Ctrl, then
**A**. Type the short label beside a button, field or menu to focus it. **Enter**
or **Space** then uses that control's normal behavior; choosing a hint does not
execute the action. **Backspace** edits a partial hint and **Escape** cancels.
Hint labels remain assigned to the same controls while the mode is open.

In **View → Keyboard shortcuts…**, **Show keyboard hints** and **Find visible
control** are the first two bindings. Their applied key sequences appear below
the action names, for example **Ctrl+Space → A** and **Ctrl+Space → G** with the
Standard defaults. These labels follow your configured leader and action keys;
**Unassigned** means the action has no binding. Edit the field beside an action
and choose **Apply bindings** to change it.

Use leader **G**, or **Find visible control** in the command palette, to search
the current window's controls by name. Up/Down chooses a result, Enter focuses
it, and Escape returns to the previous control. This searches rendered controls,
not unloaded explorer entries or hidden panels. Open their pane or navigate the
list first. Command search also accepts abbreviations through fuzzy matching.

The same discovery bindings use the selected source-style leader. Dialogs and
terminals use Ctrl+Space with Emacs/Vim styles to preserve native text editing.
Discovery is local to the focused window and the current modal or popup.
Existing custom bindings take precedence over newly added defaults; a conflicting
new binding is left unassigned and can be edited in Keyboard shortcuts.

Editor tab strips and the project toolbar each have one Tab stop. Left/Right moves focus among their tabs;
Enter/Space selects a tab. Shift+F10 opens a focused item's contextual menu.
Editor tab menus include Move tab left/right and Move to new editor window.
Text fields retain their native editing keys.

The following table describes the **Standard** preset. Use **Ctrl+Space**, release Ctrl, then press a command key. Defaults avoid Alt so they coexist with window managers such as Notion. A border marks the focused pane and a hint appears while the leader is active. **Escape** cancels; the sequence expires after five seconds.

| After the leader | Action |
| --- | --- |
| `a` / `g` | Show keyboard hints / find a visible control |
| `h` / `j` / `k` / `l`, or arrows | Focus a visible pane left / below / above / right |
| `Tab` / `Shift+Tab` | Next / previous visible pane |
| `e` / `f` / `t` / `p` | Focus editor / explorer / terminal / project tools |
| `u` | Focus the application menu; use Enter and Tab to operate controls |
| `Shift+H` / `Shift+L` | Shrink / grow the explorer or detached tools window |
| `Shift+J` / `Shift+K` | Shrink / grow the detached tools window height |
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

Ordinary text, hex, image/PDF source and notebook files have no fixed byte-size ceiling. They are loaded into memory, so available RAM/address space is the practical bound. Decoded raster pixels, tool payloads and directory listings (10,000 entries) retain their separate limits. Each RODE process owns one workspace; its additional editor and tool windows share that workspace. LSP, recursive project search and remote filesystem watching are not implemented. The editor buffer uses egui's text editor and loads the source into memory.

For binary files and explicit byte editing, see [Hex and binary analysis](binary.md). Optional modules and tool adapters are managed through [Extensions](extensions.md).
