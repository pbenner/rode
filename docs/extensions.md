# Extensions

The optional [mise extension](mise.md) (`rode.mise`) adds project toolchain/environment integration and task discovery. It is bundled and disabled by default; enable it in this manager and opt in per project from **Settings → Environment**.

Click the **Extensions** icon (four blocks) in the left activity bar, press **Ctrl+Shift+X**, or choose **View → Extensions** to open the extension manager. It is also available as **Extensions: Manage installed extensions** in the command palette, and works without an open project.

Use the checkboxes to enable/disable native features and installed adapters. The manager also installs local tool manifests, reloads installed manifests and edits extension-specific JSON settings. Changes save immediately and merge safely with other RODE windows' edits. A running window keeps its own choices until changed or restarted.

Filter the list by extension name or ID. The filter receives keyboard focus when the manager opens; after filtering, press **Tab** to focus a checkbox and **Space** to toggle it.

## Bundled features

Project bookmarks, Stocks, Git, the system/GPU monitor, Python environments, language tools, agent chat, image previews, Markdown previews, PDF previews, [PowerPoint previews](powerpoint.md), [Word previews](word.md), hex/binary analysis, Jupyter notebooks, Marp presentations, LaTeX and the [interactive debugger](debugger.md) are bundled native modules, enabled by default. The editor, syntax renderer/grammars, Problems UI, files, SSH, sandbox, preferences and persistent terminals remain core services. Interface and syntax themes retain their existing bounded data formats; they execute no plugin code.

Disabling a feature preserves open documents and unsaved edits. Binary edits can still be saved while their view is disabled. Monitoring starts only through **Start monitoring** and continues while hidden until stopped. Disabling the monitor stops collection. Python environment discovery starts when a Python file or environment panel is used, rather than probing every opened project. New terminals still honor configured environments; disabling the Python module leaves existing terminals alone and omits its environment activation from new ones. Chat still starts a process only on Send; disabling it stops its process. Debug adapters start only on explicit launch/attach; disabling the debugger stops its session.

Disabled extensions hide their global-area buttons, project tools, menu and command-palette
actions, dedicated toolbar controls and settings categories. Re-enabling an
extension restores these controls and its saved preferences. Shared core controls,
including file/save actions, terminals, project settings and the extension manager,
remain available. Enabled tool adapters can still expose formatting/checking
actions independently of the bundled Language tools extension.

Changing extensions/settings or the workspace cancels executable adapter jobs and rejects stale edits/diagnostics. Source saves already submitted by a user can complete with their original buffer if formatting was cancelled. Disabling a bundled module stops future activation; short built-in tool requests already running may finish, but their disabled/stale results are not applied. Runtime disabling does not remove compiled dependencies or promise a smaller executable.

## File transfers

The **File transfers** module adds local/remote browsing and a background copy
queue while SSH is connected. Its browsers do not change the open project. See
[File transfers](remote.md#file-transfers) for navigation, collision choices and
the captured filesystem policy. Disabling it cancels queued/running transfers and
hides its tab, palette command, connection-banner and Explorer entry points.

## Project bookmarks

The bundled **Project bookmarks** extension (`rode.project-bookmarks`) adds a
**Saved projects** section above the Explorer file tree. It is enabled by default.
Use the bookmark icon in the Explorer header to save the current local or SSH
project; the selected icon removes that bookmark. Open a saved project with one
click, and right-click an entry to rename or remove it. The list is alphabetical
and scrolls independently so it leaves room for project files.

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

Encrypted projects continue to use **Open encrypted project**; temporary decrypted
mounts cannot be bookmarked. Bookmarking is available once ordinary local or SSH
project preparation has finished.

## Stocks

The bundled **Stocks** extension (`rode.stocks`) adds a native market viewer. Open
it from the rising-chart icon in the activity bar, **View → Stocks**, or the
command palette: **Stocks: Market charts and watchlist**. It works without an
open project and can use the existing **Detach tab** control for its own window.
Enable or disable it in the extension manager.

- Search by company name or ticker, then select a result. **Open ticker** opens
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

No API key, Python, npm or external program is required. The viewer requests
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
A single background worker, response/time limits and selection generations keep
network stalls or late results from blocking editing or replacing a newer chart.
Disabling the extension discards pending results; a request already on the wire
may finish within its timeout. No stock requests run on an SSH host, execute
project tools, start an agent or alter document buffers. Only watchlist/display
preferences are saved; market responses are kept in memory.

## Installing a tool adapter

RODE API **v1** supports commands, buffer formatting and buffer diagnostics through a single, bounded JSON request/reply on stdio. It does not load third-party Rust libraries into the GUI. Adapters contribute command-palette actions and use native output/Problems views; they cannot inject arbitrary UI, panels or document renderers.

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

The current API intentionally excludes dynamic Rust `.so` plugins, scripting/Wasm runtimes, marketplaces, webviews, language servers and installed-tool debugger APIs (the bundled debugger uses DAP separately). Bundled native UI modules and declarative themes remain separate from installed tool adapters.

LaTeX tools (`rode.latex`) add explicit project-host builds and compiler diagnostics; the source toolbar and palette expose building and PDF opening. Optional LaTeX buffer formatting/linting also uses Language tools. See [LaTeX](latex.md).
