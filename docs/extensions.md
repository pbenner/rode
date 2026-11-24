# Extensions

The optional [mise extension](mise.md) (`rode.mise`) adds project toolchain/environment integration and task discovery. It is bundled and disabled by default; enable it in this manager and opt in per project from **Settings → Environment**.

Click the **Extensions** icon (four blocks) in the left activity bar, press **Ctrl+Shift+X**, or choose **View → Extensions** to open the extension manager. It is also available as **Extensions: Manage installed extensions** in the command palette, and works without an open project.

Use the checkboxes to enable/disable native features and installed adapters. The manager also installs local manifests, reloads them and edits extension-specific JSON settings through each entry's **Settings** button, including desktop panels. Changes save immediately and merge safely with other RODE windows' edits. A running window keeps its own choices until changed or restarted.

Filter the list by extension name or ID. The filter receives keyboard focus when the manager opens; after filtering, press **Tab** to focus a checkbox and **Space** to toggle it.

## Included extensions

Project bookmarks, Git, the system/GPU monitor, Python environments, language tools, agent chat, image previews, Markdown previews, PDF previews, [PowerPoint previews](powerpoint.md), [Word previews](word.md), hex/binary analysis, Jupyter notebooks, Marp presentations, LaTeX and the [interactive debugger](debugger.md) are bundled native modules, enabled by default. Stocks is an included independent executable using the same panel API as installed providers. “Included” and “Installed” describe distribution, not different integration capabilities. Native service modules remain in process while their host interfaces are migrated incrementally. The editor, syntax renderer/grammars, Problems UI, files, SSH, sandbox, preferences and persistent terminals remain core services. Interface and syntax themes retain their existing bounded data formats; they execute no plugin code.

Disabling a feature preserves open documents and unsaved edits. Binary edits can still be saved while their view is disabled. Monitoring starts only through **Start monitoring** and continues while hidden until stopped. Disabling the monitor stops collection. Python environment discovery starts when a Python file or environment panel is used, rather than probing every opened project. New terminals still honor configured environments; disabling the Python module leaves existing terminals alone and omits its environment activation from new ones. Chat still starts a process only on Send; disabling it stops its process. Debug adapters start only on explicit launch/attach; disabling the debugger stops its session.

Disabled extensions hide their global-area buttons, project tools, menu and command-palette
actions, dedicated toolbar controls and settings categories. Re-enabling an
extension restores these controls and its saved preferences. Shared core controls,
including file/save actions, terminals, project settings and the extension manager,
remain available. Enabled tool adapters can still expose formatting/checking
actions independently of the bundled Language tools extension.

Changing extensions/settings or the workspace cancels project-host executable adapter jobs and rejects stale edits/diagnostics. Source saves already submitted by a user can complete with their original buffer if formatting was cancelled. Disabling a bundled module stops future activation; short built-in tool requests already running may finish, but their disabled/stale results are not applied. Runtime disabling does not remove compiled dependencies or promise a smaller executable.

## File transfers

The **File transfers** module adds local/remote browsing and a background copy
queue while SSH is connected. Its browsers do not change the open project. See
[File transfers](remote.md#file-transfers) for navigation, collision choices and
the captured filesystem policy. It also provides **Project tools → Shared folders**
for live folder sharing under the current SSH project's permissions. Shares have
their own controller and remain active when transfer-browser hosts or access
settings change. Disabling the extension first unshares folders, then cancels
queued/running copies and hides both views, their commands and Explorer entry points.

## Project bookmarks

The bundled **Project bookmarks** extension (`rode.project-bookmarks`) adds a
**Saved projects** section in the Connections sidebar. It is enabled by default.
Use the compact bookmark icon at the right of the **Saved projects** heading to
save the current local or SSH project. Its tooltip is **Bookmark current project**,
or **Remove project bookmark** when already saved. Open a saved project with one
click, and right-click an entry to rename or remove it. The list is alphabetical
and shares the Connections scrolling region with SSH profiles and saved tunnels.

An entry retains the project location, requested sandbox settings and, for SSH,
the connection configuration, including ProxyJump, configuration and identity-file
paths. Reopening uses current saved project policy when available and runs the
normal project preparation and unsaved-buffer guards. A matching live SSH
connection is reused; another host goes through ordinary SSH authentication and
host verification. Loading the list reads metadata only. Editor tabs and running
processes keep their existing independent lifecycle.

Bookmarks are desktop preferences under this extension's `projects` settings map.
Individual records merge through the existing private, atomic preference writer,
so separate editor processes can add different projects without replacing the
whole list. A running instance keeps its own catalogue until changed or restarted,
as with other extension preferences. Disable the extension to hide its controls
while retaining saved projects. The library supports up to 128 entries within
192 KiB; malformed records are preserved and reported rather than overwritten.

New SSH bookmarks include current [shared-folder settings](remote.md#shared-folders)
by default. Right-click a bookmark to toggle **Include shared-folder settings**;
use **Update saved shared folders** after changing the recipes. Reopening mounts
the saved folders automatically once the project and SSH connection are ready,
using the project's access policy. Loading the catalogue alone starts no mounts.
RODE automatically unshares folders before changing projects, hosts or policies;
a busy mount stops the change and keeps the current workspace open.

Encrypted projects continue to use **Open encrypted project**; temporary decrypted
mounts cannot be bookmarked. Bookmarking is available once ordinary local or SSH
project preparation has finished.

## Stocks

The included **Stocks** extension (`rode.stocks`) adds a native market viewer. Open
it from the rising-chart icon in the activity bar, **View → Workspace panels → Stocks**, or the
command palette: **Stocks: Open panel**. It works without an
open project. Right-click its activity icon and choose **Open in separate window**
to detach it; **Reattach** or closing that window returns it to the main window.
Enable or disable it in the extension manager.

- Search by company name or ticker (Enter submits the search), then select a result. **Open ticker** opens
  an exact exchange symbol directly, including international symbols such as
  `SIE.DE`, `7203.T` and `VOD.L`. The provider also supports many ETFs, indices,
  currencies and other instruments; availability varies by market.
- **Add to watchlist** saves the current symbol. Select a saved symbol to reopen
  it; **Remove from watchlist** removes it. Up to 32 symbols are saved in desktop
  extension preferences, with individual records merged across editor processes.
- Choose **1D**, **5D**, **1M**, **3M**, **6M**, **1Y**, **5Y** or **All**. Short
  ranges use intraday bars; longer ranges use daily, weekly or monthly bars.
  Switch between **Line** and **Candles**, and toggle **Volume**.
- Roll the wheel over the chart to zoom around the pointer; drag to pan and
  double-click to reset. The **−**, **+**, arrow and **Reset view** buttons provide
  the same actions. Focus the chart with Tab or keyboard hints, then use **+ / −**,
  **Left / Right** and **Home**. Hover an observation for its date, OHLC and volume.

No API key, Python, npm or separately installed tool is required. The viewer requests
[Yahoo Finance](https://finance.yahoo.com/) public search/chart data over HTTPS
from the desktop. It uses unofficial endpoints (also used by the
[yfinance-rs client](https://docs.rs/yfinance-rs/latest/src/yfinance_rs/core/client/constants.rs.html));
the service can change, reject requests or return delayed/unavailable data.
Use remains subject to the provider's terms. The panel identifies its source,
currency, exchange and last observation time; times are shown in UTC. Prices are
provider OHLC values, and the displayed range change is the first-to-last close
change rather than a dividend-adjusted total return.
The percentage is omitted for a nonpositive initial price or a nonfinite result.
Display choices edited through the extension's JSON settings are reapplied when
returning to the viewer; removing those fields restores their defaults.

Requests start only when the panel is first opened or a search, uncached selection
or **Refresh** is requested. Zooming and panning operate entirely on cached data.
Up to 12 recent charts are retained in memory; selections are reused for five minutes.
**Refresh** explicitly requests newer data. There is no automatic quote polling.
A separately supervised `rode-stocks` process, response/time limits and serial
request identities keep network stalls or late results from blocking editing.
Only the chart matching the selected symbol/range is displayed. The executable
is discovered beside RODE, then on PATH, and ships with source and Debian
installations. Update it independently with `cargo install --locked --path
crates/rode-stocks --force` for the default source prefix (use `--root PREFIX`
for a custom prefix). Disable/re-enable Stocks after replacing its executable;
compatible updates do not rebuild the editor. For a packaged installation, update
the packaged executable or use an explicit installed manifest for a user-owned
build.
Disabling the extension discards pending results; a request already on the wire
may finish within its timeout. No stock requests run on an SSH host, execute
project tools, start an agent or alter document buffers. Only watchlist/display
preferences are saved; market responses are kept in memory.

## Installing a tool adapter

RODE manifest API **v1** supports project-host commands, buffer formatting and buffer diagnostics through a single, bounded JSON request/reply on stdio. Extensions can declare project-host tools, a [desktop panel](#desktop-panels), or both through [explicit contributions](#execution-scoped-contributions). RODE renders panel data using native widgets; it does not load third-party Rust libraries or execute provider-supplied UI code inside the GUI.

Create a manifest like this, replacing the adapter path with its **absolute path on the project host**:

```json
{
  "api": 1,
  "id": "example.my-tools",
  "name": "My tools",
  "version": "1.0.0",
  "description": "My formatter and project inspection commands",
  "tool": {
    "program": "python3",
    "args": ["/opt/my-tools/adapter.py"],
    "languages": ["rust", "python", "text"],
    "format": true,
    "check": true,
    "commands": [{"id": "example.inspect", "title": "Inspect project"}]
  }
}
```

Use **Install manifest** with its desktop path, then explicitly enable the installed adapter. Installation copies only the validated JSON manifest into `$XDG_CONFIG_HOME/rode/extensions/ID.json` (default `~/.config/rode/extensions/`); it does not install the program or upload it over SSH. The same directory can be managed manually; use Reload afterward. Remove the manifest and Reload to uninstall it. Bundled `rode.*` IDs are reserved. Manifests use API 1, reject unknown fields and are limited to 64 KiB; discovery reads up to 64 immediate JSON files and never scans a project or starts a process.

Installed tools start **disabled**. Review their executable and arguments before enabling. An enabled matching provider can run on explicit Format/Check, configured save actions and debounced live checks. An adapter's language IDs match the built-in language catalogue; unrecognized file types use `text`. With several enabled matches, the first matching manifest in filename order wins; disable competing adapters to choose another provider. A format operation can return diagnostics too; a check-only adapter returns diagnostics without replacing text. The bundled formatter is used when no enabled adapter matches.

The program may be a PATH name or an absolute project-host executable. Arguments are passed literally, with no shell interpolation or placeholder expansion. The project root, file path, buffer contents and settings arrive as JSON instead. For SSH, the adapter and its dependencies must already exist on the SSH host and be visible inside the active sandbox. There is no desktop fallback. Explicit grants may be needed for tools outside the standard/project mounts.

## Execution-scoped contributions

New manifests can declare one desktop panel and one project tool under a single
extension identity. The extension manager shows each contribution's execution
location, access and executable before enablement. For example:

```json
{
  "api": 1,
  "id": "example.workbench",
  "name": "My workbench",
  "version": "1.0.0",
  "description": "A desktop dashboard and project inspection tools",
  "contributions": [
    {
      "kind": "panel",
      "host": "desktop",
      "protocol": 2,
      "icon": "chart",
      "tool": {"program": "/opt/workbench/dashboard", "args": ["--stdio"]}
    },
    {
      "kind": "project-tool",
      "host": "project",
      "tool": {
        "program": "workbench-tools",
        "languages": ["rust"],
        "check": true,
        "commands": [{"id": "example.inspect", "title": "Inspect project"}]
      }
    }
  ]
}
```

A `panel` must use `host: "desktop"`; a `project-tool` must use `host: "project"`.
The host rejects mismatches and duplicate contribution kinds. The desktop panel
always runs on this computer with the user's ordinary files, network and
credentials, even in an SSH workspace. Its process never receives project
sandbox authority. Project tools run on the active local or SSH project host
under that project's actual access policy. Both share only their extension's
settings and enablement; their requests, processes and UI application guards
remain separate. Disabling a combined extension stops its panel and invalidates
its project-tool work.

Existing API-1 `tool`, `panel`, `panel_version` and `icon` manifests remain valid
with their original execution rules. Use either that legacy form or
`contributions`, never both. The contributions form requires a host supporting
it; older hosts reject it instead of guessing a location. Project-tool and panel
request/reply protocols retain their existing wire shapes.

## Protocol

RODE starts the adapter with the project root as its working directory. It sends one JSON object on stdin, then closes the adapter's stdin:

```json
{
  "api": 1,
  "extension": "example.my-tools",
  "operation": "format",
  "root": "/work/project",
  "path": "/work/project/main.rs",
  "text": "the current unsaved UTF-8 buffer",
  "settings": {}
}
```

`operation` is `format`, `check`, or a declared command ID. A command receives the active text snapshot if available, otherwise empty text and a null path. Return exactly one JSON object on stdout and exit successfully:

```json
{
  "api": 1,
  "text": "replacement buffer (format only)",
  "diagnostics": [{
    "code": "MY001",
    "message": "An issue in this buffer",
    "row": 1, "column": 1,
    "end_row": 1, "end_column": 2,
    "severity": "warning"
  }],
  "output": "optional command output or formatter message"
}
```

`text`, `diagnostics` and `output` are optional. Diagnostic positions are **1-based Unicode scalar columns**; severities are `error`, `warning` or `info`. Check/command responses cannot replace text. Command results display output rather than applying diagnostics to a possibly changed buffer. Format/Check results enter the existing document-version checks and Problems UI. Adapters must return edits; they must not write the source file behind RODE's save guards.

A minimal formatter adapter can use any implementation language:

```python
import json
import sys

request = json.load(sys.stdin)
reply = {"api": 1}
if request["operation"] == "format":
    reply["text"] = request["text"].rstrip() + "\n"
else:
    reply["output"] = "Project: " + request["root"]
json.dump(reply, sys.stdout)
```

Programs have a 20-second request limit, bounded stdout/stderr (8 MiB each), an 8 MiB input/replacement buffer limit, at most 4096 diagnostics and at most 1 MiB of displayed output. Settings are bounded JSON objects, persisted per extension under `extension_preferences`; there are no environment secrets in the manifest format. Protocol version or validation failures are visible errors. A lifetime channel allows RODE to cancel the adapter's process group on either host without ending the SSH connection or unrelated terminals.

The core launches adapters under the actual project sandbox and owns SSH transport, request limits, cancellation and edit application. Normal mode grants the host user's ordinary access. A manifest is not a separate security boundary; an enabled executable can use the filesystem/network available in that mode. Repository-provided manifests are never loaded automatically, and adapters cannot request the built-in monitor's host telemetry bypass. Model servers and agent credentials remain user-managed through the existing chat module.

The current API excludes dynamic Rust `.so` plugins, scripting/Wasm runtimes, marketplaces, webviews, language servers and installed-tool debugger APIs (the bundled debugger uses DAP separately). Bundled native modules, declarative themes, project-host tool adapters and installed desktop panels have separate lifecycles.

LaTeX tools (`rode.latex`) add explicit project-host builds and compiler diagnostics; the source toolbar and palette expose building and PDF opening. Optional LaTeX buffer formatting/linting also uses Language tools. See [LaTeX](latex.md).


## Desktop panels

An installed executable can supply a native application panel through panel
protocol **1, 2 or 3**. Existing providers default to protocol 1; add
`"panel_version": 2` for the shared settings and chart capabilities. Add `"panel": true` to its API-1 manifest and supply `tool.program`
and `tool.args`. The panel executable itself cannot also declare project-host
capabilities. An extension that needs both declares a separate `project-tool`
in [explicit contributions](#execution-scoped-contributions). A legacy panel-only
manifest remains valid, for example:

```json
{
  "api": 1,
  "id": "example.dashboard",
  "name": "My dashboard",
  "version": "1.0.0",
  "description": "A desktop dashboard",
  "panel": true,
  "tool": {
    "program": "/absolute/path/to/dashboard",
    "args": ["--stdio"]
  }
}
```

Independently installed panels start disabled; included providers retain their
documented defaults. Installation never starts a process. The extension manager uses **INCLUDED** or **INSTALLED** to identify their source;
execution and native UI capabilities are shared. Enabling one trusts its executable with
the desktop user's ordinary filesystem and network access, independently of any
project sandbox. It always runs on this computer, including while an SSH
workspace is open; no binary, credentials or panel requests are sent to the
project host. Native themes and keyboard navigation apply automatically.

An enabled panel adds its own left-rail icon, an **Open panel** action in the
extension manager and a **Name: Open panel** command-palette entry. Right-click
the icon to detach or reattach its window. Closing a detached panel returns its
view to the main window. Each provider owns a separate process, draft, refresh timer and native window
identity. Selecting another provider preserves existing background work. Reloading only
desktop contributions or changing display metadata preserves their processes and
unrelated project-tool work; changing executable arguments or the panel protocol
stops the affected provider.
Providers can detach into separate windows simultaneously. Visiting another area
or hiding a panel keeps its explicitly started background refresh active.
Disabling/replacing the provider or closing RODE stops its supervised process.
A stopped or replaced provider cannot leave its old controls active.

The process receives newline-delimited requests without stdin being closed:

```json
{"api":1,"id":1,"action":"open","values":{}}
```

Return one response for each request, echoing its identity:

```json
{
  "api": 1,
  "id": 1,
  "view": {
    "title": "My dashboard",
    "status": "Ready",
    "message": "",
    "fields": [{"key":"filter","label":"Filter","value":""}],
    "metrics": [{"label":"Progress","value":"12 / 20","progress":0.6}],
    "tables": [{"title":"Items","columns":["Name","State"],"rows":[["Example","Ready"]]}],
    "actions": [{"id":"refresh","label":"Refresh","enabled":true}],
    "refresh_seconds": null
  }
}
```

Actions receive current field values. The IDs `open`, `tick` and `settings` are
reserved for lifecycle requests and cannot be declared as user actions. Fields support optional `choices` (objects
with `value` and `label`) or `multiline: true`. Actions can specify `primary: true`
and a `confirm` string; RODE sends that action only after its native confirmation
dialog is accepted. The confirmation is tied to the reviewed action, input and settings snapshot;
a changed configuration, changed input or disabled action invalidates it. No
background request runs behind a pending confirmation. The provider must still
validate its own operation state and arguments. Responses never modify editor buffers or execute host commands.

An explicit background-refresh action can return `refresh_seconds` from 30 to
3600. RODE then sends `tick` requests, including while the view is hidden. The
scheduled request has empty `values` and preserves draft fields; only explicit
actions submit current input. The protocol defines `tick` as read-only; this is
a provider responsibility, not a
sandbox around an installed executable. No refresh is scheduled when the value
is null. Hidden panels sleep until their actual refresh or request deadline;
transport events wake the owner immediately. Pending configuration is applied
before a due refresh, and reply settings are merged before another request is
queued. The provider should keep monitoring off until explicitly started.

One request runs at a time per provider. Input is bounded to 1 MiB, output frames to 4 MiB,
individual text values to 8 KiB, fields to 128, actions to 32, metrics to 24 and
tables to 12 (16 columns and 4096 rows each). Replies must have unique field and
action IDs, matching row/column counts and finite progress in 0–1. Invalid replies,
transport loss or a request exceeding 180 seconds stop that provider. The SDK
server and host validate request/reply identities and protocol capabilities;
version-1 providers cannot emit version-2 controls or host settings. Failed sends
retain pending configuration and request snapshots. Read-only lifecycle requests
remain independent of the size of an unsent draft. Large tables scroll and render
only their visible rows. Focus a table with Tab, keyboard hints or a click, then
use the arrow keys, Page Up/Down and Home/End to scroll it.
Table height follows the window by default. Large tables have a bottom resize
handle: drag it or focus it and use Up/Down. Double-click or press Home to restore
automatic height. Manual heights are remembered per provider and stable table
identity across refreshes and restarts. In automatic mode, short tables shrink
to fit their rows.
Columns start with content-based widths. Drag a header's right edge to resize
that column, or focus its divider with Tab/keyboard hints and use Left/Right.
Double-click the divider or press Home while it is focused to restore automatic
sizing. Widths are remembered across refreshes and restarts for each provider,
table and column schema; wide tables scroll horizontally. Replies
cannot overwrite edits made after the request was sent. Runtime restarts are
explicit and preserve the current input draft, chart views
and detached-window ownership. Reopening sends empty input values and never
replays an action that may have written data. A provider performing writes must journal uncertain outcomes and avoid
blind retries after a dropped connection or process termination.

## Shared SDK and protocol 2

`rode-extension-api` owns manifest validation, execution-scoped contributions,
project-tool requests/replies and diagnostics, and panel protocols. It has no
editor, GUI or host-service dependency. Other
languages can implement the same documented JSON protocol. `rode-stocks` uses
this SDK; its domain/network code is outside the desktop crate. Host rendering
and window ownership are implemented once in `app::extension_panels`.

A protocol-2 request includes the extension's own `settings` object. The provider
can return an updated settings object alongside `view`. The host merges changed
leaves against the sent snapshot, preserving host edits made while a request was
running and retaining unrelated preferences. Missing keys and JSON `null` remain
distinct; deleting a group removes its unchanged leaves while retaining concurrent
host edits. Settings are bounded to 192 KiB per extension and 256 KiB for the whole
extension preference map, and use the existing private preference writer. Rejected
edits preserve the previous settings. Desktop-only settings edits notify that
provider without cancelling unrelated project work; combined extensions also
invalidate their project-tool jobs. No other extension's settings are
sent. A host settings edit triggers a `settings` action, with empty draft values;
providers should adopt that configuration without submitting domain writes.
Protocol-1 requests remain unchanged and cannot write host-owned preferences.

Protocol 2 adds these native view contributions:

- `action_groups`: labeled rows of ordinary actions. An action's `selected`
  flag supports watchlists, ranges and display choices. IDs must be unique across
  all groups and the main actions; the total limit is 128.
- Fields with `inline: true` remain visible outside the settings disclosure.
  `submit` names a declared action to run on Enter in a single-line input.
- `charts`: up to four native OHLC/volume charts, identified by stable `key`,
  with `title`, `subtitle`, `series`, `candles` and `volume`. A series contains
  `name`, `currency`, `exchange` and at most 25,000 strictly ordered observations
  (`at`, `open`, `high`, `low`, `close`, `volume`). Invalid/nonfinite prices,
  inconsistent OHLC bounds and negative volume are rejected. Wheel, drag,
  keyboard, reset and density aggregation all run in the shared renderer without
  further provider requests. Zoom is kept separately for each chart identity.
- `sections`: protocol 2 supports up to twelve cards with `title`, `description`,
  table indices, field keys and action IDs. Legacy rendering keeps its fixed
  table/input/action order. Protocols 1 and 2 retain their original wire shapes.
- `links`: up to eight user-activated HTTP(S) links with `label` and `url`.

### Protocol 3: ordered native layouts

Declare `panel_version: 3` (or `protocol: 3` in an explicit desktop contribution)
when using composition, layout preferences or collapsible sections. Older hosts
reject this manifest as unsupported rather than starting a provider whose replies
they cannot understand. There is no automatic version negotiation. A provider
supporting older hosts can generate a protocol-1 or protocol-2 manifest and omit
all protocol-3 properties in those replies. The SDK validates this boundary.
Settings, request identities, confirmations and lifecycle actions retain protocol-2
behavior. Existing providers continue to work on updated hosts.

A nonempty view `content` supplies the complete display order. Every top-level
control and section must appear exactly once, either in root content or section
content. No implicit placement is added. This avoids controls unexpectedly moving
into Settings or a toolbar when the provider omits a reference.

Blocks use `kind` and either `keys` for a group or `key` for a single control:

| Kind | Reference | Native presentation |
| --- | --- | --- |
| `metrics` | Array of metric keys | Responsive row of equal-height cards |
| `fields` | Array of field keys | Ordered labeled inputs |
| `actions` | Array of action IDs | Wrapping button row |
| `table` | Table key | Scrollable data table |
| `chart` | Chart key | Interactive chart with native controls |
| `link` | Link key | User-activated HTTP(S) link |
| `action_group` | Action-group key | Selectable action row |
| `section` | Section key | Native card, optionally collapsible |

Keys are stable identities, independent of array position or visible text. Fields
use `key`, actions use `id`; other controls use `key`. Chart keyboard focus and
viewport state follow its key when it moves between sections. Provider IDs isolate
identical control keys in different panels. Keys must be unique within
each control type. Actions inside action groups retain their existing globally
unique IDs and are placed through their group. Unknown, repeated or unplaced
references are rejected. Sections cannot contain other sections, and ordered
sections cannot also use nonempty legacy `fields`, `tables` or `actions` lists.
`Field.inline` must be false or omitted in ordered layouts: placement comes only
from `content`. Automatic layouts (an omitted or empty root `content`) retain
`inline` and the legacy section lists, including under protocol 3. These two
modes are validated separately; no placement hints are silently ignored.
Blocks are bounded to 256 per container; existing control and frame budgets apply.

Sections can mix charts, metrics, fields, tables, links and actions in any order.
A section can set `collapsible: true` and `default_open: false`; expansion state
belongs to the host and survives replies and title changes. `default_open` applies
on first display of each provider run and defaults to true. An explicit initial
state takes precedence over disclosure state saved by an earlier run; user
toggles survive subsequent replies. Closing a section retains input drafts.

`layout.metric_columns` requests a maximum of 1–4 metric columns; RODE reduces it
for narrow panels. `layout.metric_density` is `auto`, `comfortable` or `compact`.
These are preferences: RODE owns fonts, theme colors, accessibility, wrapping and
card alignment. These metric preferences are panel-wide and apply to every
metric group; groups independently wrap to their available width. Protocol 3 may
omit `content` to use the legacy automatic layout,
but section content requires an explicit root layout.

For example, a complete minimal protocol-3 view:

```json
{
  "title": "Service overview",
  "layout": { "metric_columns": 2, "metric_density": "compact" },
  "metrics": [{ "key": "uptime", "label": "Uptime", "value": "99.9%" }],
  "tables": [{ "key": "events", "title": "Events", "columns": ["Event"], "rows": [] }],
  "sections": [{
    "key": "details", "title": "Details", "collapsible": true,
    "default_open": false,
    "content": [{ "kind": "table", "key": "events" }]
  }],
  "content": [
    { "kind": "metrics", "keys": ["uptime"] },
    { "kind": "section", "key": "details" }
  ]
}
```

This deliberately bounded composition model supports native forms and dashboards;
it does not expose arbitrary CSS, executable widgets or recursive UI trees.

Optional panel `icon` values are `chart`, `monitor` and `folder`, painted with
RODE's active theme, or an absolute path to a local PNG image, for example
`"icon": "/home/me/.local/share/my-extension/logo.png"`. This works in both
legacy panel manifests and explicit panel contributions. White PNG icons with
transparent backgrounds are treated as alpha masks and follow the active theme's
normal, hover, selected and disabled colors. Color images retain their original
colors and transparency. Use an outline white mask for sidebar artwork rather
than a filled branding logo. Install the image alongside your extension
and use a stable path independent of its source checkout. RODE does not fetch
remote images. PNG files are limited to 2 MiB and 2048 × 2048 pixels, and cached
until **Reload manifests**. Missing or invalid images use the generic extension
icon without preventing the panel from opening. Providers do not supply executable UI
code, alter document buffers or obtain a project host's authority through a
panel. Monitor and document controllers retain their existing host/document
services; they have not been forced into this dashboard-only API.

A compatible host update is needed when adopting a new protocol capability.
After that, plugin code can be rebuilt and replaced independently. There is no
shared Rust ABI or plugin-specific code to link into RODE.
