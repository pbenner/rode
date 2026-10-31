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

Expand **Launch / attach configuration**, choose a preset and program, and use **Save configuration**. **F5** also applies the current valid draft before launch. Settings, breakpoints, watches and exception selections save locally per project/host and merge with other windows' independent preference changes. The native host service owns each managed debugger controller; one controlling window owns its UI lease. Console output, variables and running processes remain in host memory, not preferences.

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

Adapters and programs run **on the project host** using the existing authenticated OpenSSH connection. RODE provisions its native session helper for managed controllers; no remote desktop installation, extra network listener or SSH port forwarding is required. The remote coordinator uses Python 3's standard library; the configured debugger/runtime is a separate host prerequisite. There is no desktop-local fallback for remote programs or sources.

In sandbox mode the adapter, debuggee and interactive PTYs share **one** sandbox session, including its mount, PID and network namespaces. Project commands obey its filesystem and network grants. Debugging does not widen those grants or automatically retry in normal mode. Some adapters use internal loopback connections or need kernel ptrace permissions: a host policy or unshared network may prevent them from working. Keep the visible sandbox policy and adjust it explicitly if needed. Native debugging also depends on the host's ptrace policy; containers may restrict disabling ASLR, so `extra: {"disableASLR": false}` can help without granting additional permissions.

**Attach** is supported in normal mode with backend-specific `extra` arguments, such as LLDB's `processId`, or debugpy's `listen` object with an explicitly started `python -m debugpy --connect host:port script.py` process. Stopping an attached session requests detach (`terminateDebuggee: false`). An existing process cannot be moved into the current project sandbox, so RODE rejects attach there.

## Persistent host and Slurm sessions

Use **Execution target** to launch the coordinator, adapter and debuggee together on the project host or one running Slurm allocation. **Refresh targets** queries your jobs; Slurm launches use the same ownership/name/state revalidation and compute-node policy as [notebook targets](notebooks.md#slurm-execution-targets). All adapters, interpreters, programs and source/data paths must be available on that node. No direct SSH connection to the compute node is required. A project-host Python 3 bootstrap supervises managed debugger launches, including custom/native adapters.

New sessions use **Persist debugger: off**. Enable it before or during debugging to keep the controller, running/paused debuggee, breakpoints, inspection epoch, variables, console and terminal screen on the host after the desktop leaves. **Detach debugger**, project/interpreter changes, SSH loss and RODE exit detach persistent work. Use **Reattach debugger**, or **Running debuggers… / Sessions → Notebook kernels & debuggers**, from the original host/project and sandbox. Reattachment performs no second launch or DAP initialization. Pending replies/deadlines remain owned by the host. Concurrent controlling attachments are refused, and commands referring to an obsolete stopped epoch are rejected. Saved source digests captured at launch mark changed or unknown source files stale after reattachment. Verification checks the current buffer text, including edits made before reattaching; saving those edits does not make the buffer match the running program.

**Stop / Shift+F5** and **End runtime** explicitly terminate launched programs or detach externally attached targets, then reap owned adapter/coordinator children. Locking an encrypted project also requests shutdown. Unrelated kernels, terminals, the SSH master and the Slurm allocation remain independent. Release the allocation separately when finished. Persistence is bounded by host-service lifetime, reboot, Slurm expiry and site logout policy. Ended persistent sessions retain their console/state until removed from Sessions.

Managed debugger inspection retains at most 32 variable references (each collection remains bounded to 128 KiB); selecting a frame again clears references for further inspection. Console text is bounded to 256 KiB. Debug terminal reattachment restores its retained screen; output queues and scrollback remain bounded. Source digests and target metadata are bounded independently so discovery remains available. Persistent notebook kernels and this DAP program debugger are separate controllers; stepping inside notebook cells is not implemented.
