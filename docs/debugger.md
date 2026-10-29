# Interactive debugger

Open **View → Debugger**, use the bug icon in the activity bar, or choose **Debug: Show debugger** in the command palette. Debugging is an optional bundled native feature (`rode.debugger`) in **Extensions**. Opening the panel starts no processes.

RODE uses installed [Debug Adapter Protocol](https://microsoft.github.io/debug-adapter-protocol/overview) (DAP) adapters. The same controls work in local and SSH projects: launch/attach, breakpoints, pause/continue, step over/into/out, threads, call stacks, expandable variables, watches and expression evaluation. Program input/output uses a real **Debug** terminal tab; adapter output and evaluation results appear in the panel's **Debug console**.

## Setup

Install the desired debugger on the **project host**, then open a project folder and save modified sources. RODE never installs adapters, compilers or language runtimes automatically.

| Preset | Host prerequisite | Program |
| --- | --- | --- |
| Python | The selected project interpreter has `debugpy` | A saved script; defaults to the active file |
| Native · LLDB | `lldb-dap` on PATH, or its explicit executable path | An executable built with debug symbols (C, C++, Rust) |
| Custom DAP | An installed **stdio** DAP adapter | The adapter's program/module and launch or attach arguments |

For native code, build first using your project's normal build command with debug information (`-g` for C/C++; a normal Cargo debug build for Rust). Choose **Native · LLDB** and enter the executable, for example `{root}/target/debug/my-program`. RODE does not build automatically. Distribution packages may use versioned adapter names such as `/usr/bin/lldb-dap-19`; enter that in **Adapter executable** or the configuration's `adapter` field.

For Python, select the interpreter in the Python environment panel; the default discovers a project `.venv`, then system Python. With the Python feature disabled, the debugger uses system Python unless explicitly overridden in its configuration. See [debugpy configuration](https://github.com/microsoft/debugpy/wiki/Debug-configuration-settings) and [LLDB DAP configuration](https://lldb.llvm.org/use/lldbdap.html) for adapter-specific options.

## Configure and run

Expand **Launch / attach configuration**, choose a preset and program, and use **Save configuration**. **F5** also applies the current valid draft before launch. Settings, breakpoints, watches and exception selections save locally per project/host and merge with other windows' independent preference changes. Each window owns its own debug session; console output, inspected variables and running processes are never persisted.

The advanced JSON editor exposes literal argument arrays, working directory, adapter executable/arguments and additional DAP launch/attach fields. For example:

```json
{
  "backend": "lldb",
  "adapter": "lldb-dap",
  "adapter_arguments": [],
  "program": "{root}/target/debug/my-program",
  "arguments": ["--input", "data/sample file.json"],
  "cwd": "{root}",
  "stop_on_entry": false,
  "attach": false,
  "extra": {"disableASLR": false}
}
```

`{root}`, `{file}`, `{dir}`, `{stem}` and `{python}` expand once to the project root, active saved file, its parent directory/stem and selected interpreter. Paths refer to the project host. Relative program/working-directory paths resolve against the root; `~` refers to the host user's home. Arguments are passed literally, without shell splitting or interpolation. `extra` accepts an object and overrides preset DAP fields, including environment variables and source mappings. Python module launches can use an empty `program` and `extra: {"module": "package.main"}`. Python launch defaults to `console: "integratedTerminal"`; LLDB also enables `runInTerminal` for older adapters.

The selected adapter must already be accessible inside the active sandbox. Changing its executable does not install or upload anything. A custom adapter uses `backend: "custom"`, `adapter` and `adapter_arguments` for its stdio command. Examples include an installed GDB with DAP support (`["--interpreter=dap"]`), or other compatible stdio adapters. TCP-only adapters and automatic child debug sessions are not currently supported.

## Breakpoints and inspection

Click the **left edge of the line-number gutter**, or press **F9** at the cursor, to toggle a source breakpoint. In **Breakpoints**, enable/disable or remove entries; right-click an entry to set a condition, hit condition or log message. These depend on adapter capabilities. Filled markers indicate verified breakpoints; hollow markers are disabled, pending or unverified. Hover the verification label for an adapter explanation or relocated line.

When execution stops, RODE opens the selected stack frame through the current project's file backend and shows an execution arrow. Dirty buffers are preserved. Edits in open buffers since launch produce a source-staleness warning and suppress the execution marker; restart to use saved changes. Source mappings supplied to the adapter must resolve to readable project-host paths. Generated/in-memory sources without a path have no editor preview.

Choose a thread/frame to inspect its scopes, expand children, add watch expressions or evaluate an expression in the debug console. Evaluation can execute code in the debugged process. References and late replies are invalidated on resume or frame changes. The UI bounds frames (100), threads (128), scopes (32), children per expansion (200), expansion depth (8), variable collections (128), collection data (128 KiB), watches (32), messages (1 MiB), queued protocol requests (256), and retained console text (256 KiB). Use a narrower expression to inspect larger collections.

| Shortcut | Action |
| --- | --- |
| F5 | Start / continue |
| Shift+F5 | Stop launched program / detach attached program |
| F6 | Pause |
| F9 | Toggle breakpoint at source cursor |
| F10 | Step over |
| F11 | Step into |
| Shift+F11 | Step out |

Normal terminal keys, including Tab and control keys, keep working. While a debug session is active, its function-key controls remain available from the terminal; otherwise terminal function keys belong to the shell. F9 remains a source-editor shortcut.

## SSH, sandboxing and lifetime

Adapters and programs run **on the project host** using the existing authenticated OpenSSH connection. No remote RODE executable, extra RODE network listener or SSH port forwarding is required for debugging. The remote coordinator uses Python 3's standard library; the configured debugger/runtime is a separate host prerequisite. There is no desktop-local fallback for remote programs or sources.

In sandbox mode the adapter, debuggee and interactive PTYs share **one** sandbox session, including its mount, PID and network namespaces. Project commands obey its filesystem and network grants. Debugging does not widen those grants or automatically retry in normal mode. Some adapters use internal loopback connections or need kernel ptrace permissions: a host policy or unshared network may prevent them from working. Keep the visible sandbox policy and adjust it explicitly if needed. Native debugging also depends on the host's ptrace policy; containers may restrict disabling ASLR, so `extra: {"disableASLR": false}` can help without granting additional permissions.

**Attach** is supported in normal mode with backend-specific `extra` arguments, such as LLDB's `processId`, or debugpy's `listen` object with an explicitly started `python -m debugpy --connect host:port script.py` process. Stopping an attached session requests detach (`terminateDebuggee: false`). An existing process cannot be moved into the current project sandbox, so RODE rejects attach there.

Debug sessions are temporary and cannot use terminal **Persist**. Stop terminates a launched program and its owned adapter children; unrelated project terminals and the SSH master remain available. Switching project, interpreter or sandbox, disabling the debugger, disconnecting SSH, locking an encrypted project or exiting RODE stops the session. Normal exit allows a bounded graceful disconnect without a persistence prompt. Use ordinary [persistent terminals](terminals.md) for jobs that should outlive the editor.
