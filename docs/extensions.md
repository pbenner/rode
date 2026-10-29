# Extensions

Click the **Extensions** icon (four blocks) in the left activity bar, press **Ctrl+Shift+X**, or choose **View → Extensions** to open the extension manager. It is also available as **Extensions: Manage installed extensions** in the command palette, and works without an open project.

Use the checkboxes to enable/disable native features and installed adapters. The manager also installs local tool manifests, reloads installed manifests and edits extension-specific JSON settings. Changes save immediately and merge safely with other RODE windows' edits. A running window keeps its own choices until changed or restarted.

## Bundled features

Git, the system/GPU monitor, Python environments, language tools, agent chat, image previews, Markdown previews, PDF previews, hex/binary analysis, Jupyter notebooks, Marp presentations, LaTeX and the [interactive debugger](debugger.md) are bundled native modules, enabled by default. The editor, syntax renderer/grammars, Problems UI, files, SSH, sandbox, preferences and persistent terminals remain core services. Interface and syntax themes retain their existing bounded data formats; they execute no plugin code.

Disabling a feature preserves open documents and unsaved edits. Binary edits can still be saved while their view is disabled. Monitoring polls only while visible. Python environment discovery starts when a Python file or environment panel is used, rather than probing every opened project. New terminals still honor configured environments; disabling the Python module leaves existing terminals alone and omits its environment activation from new ones. Chat still starts a process only on Send; disabling it stops its process. Debug adapters start only on explicit launch/attach; disabling the debugger stops its session.

Changing extensions/settings or the workspace cancels executable adapter jobs and rejects stale edits/diagnostics. Source saves already submitted by a user can complete with their original buffer if formatting was cancelled. Disabling a bundled module stops future activation; short built-in tool requests already running may finish, but their disabled/stale results are not applied. Runtime disabling does not remove compiled dependencies or promise a smaller executable.

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
