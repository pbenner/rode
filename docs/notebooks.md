# Jupyter notebooks

Open `.ipynb` files from the explorer or **Ctrl+P**, or choose **File → New Jupyter notebook**. The native **Jupyter notebooks** extension is enabled by default and can be managed in **View → Extensions**. Viewing and editing saved notebooks require no Jupyter installation.

## Cells and outputs

Code cells use the independent syntax theme and the kernel's language. Markdown cells have **Edit / Preview** controls, including native tables, lists and embedded PNG/JPEG attachments. Inline image files use the current local/SSH host and sandbox grants. HTTP/file loaders and HTML execution are disabled.

Select a cell by clicking its header or editor. Use **+ Code**, **+ Markdown**, the cell type selector, **Up / Down** and **Delete** to arrange cells. Cell text keeps its editor undo/cursor state through tab switches and moves. Structural changes do not currently have a separate undo history. **Ctrl+F** searches cell sources and selects the next match.

Saved and live outputs show stdout/stderr, tracebacks, text representations, PNGs and JPEGs. Common plots work through inline Matplotlib output. Unsupported MIME bundles, HTML, widget state, attachments, metadata and unfamiliar fields are preserved in the file even when they cannot be rendered. Live display updates and clear-output messages are supported; notebooks are never executed merely by opening them.

**Ctrl+S / Save As** use the normal atomic, revision-checked local/SSH saves. Cell edits and execution outputs mark the notebook dirty; tab close/project changes retain the unsaved-edit guard. An external change prevents overwriting it. A save records its submitted snapshot and leaves changes arriving during that save dirty. The notebook never goes through whole-file text formatting or code-run commands.

Use **JSON source** for direct metadata or unsupported-cell editing. Switching to JSON stops this notebook's kernel; switching back reparses the current unsaved buffer. Invalid/unsupported notebooks remain available as ordinary JSON text, with an explanation. Their contents are never replaced by an empty notebook. The cell renderer supports **nbformat 4** and preserves untouched string/list source representations and unknown JSON fields.

## Running cells

Execution needs an installed interpreter with `jupyter_client` and an installed compatible kernel on the **project host**. For Python, install `jupyter_client` and `ipykernel` in the chosen environment yourself; for example, in the project terminal:

```sh
uv venv .venv
uv pip install --python .venv/bin/python jupyter_client ipykernel
# Optional plotting and project libraries:
uv pip install --python .venv/bin/python matplotlib
```

Do not recreate an existing environment. You can also use pip or an already prepared interpreter. Select it through **Python environment**; automatic selection uses the project's `.venv` when available. The editor installs no notebook dependencies. The default Python kernelspec uses that interpreter; a custom kernelspec may explicitly select a different executable.

- **Run cell / Shift+Enter:** execute the selected code cell in a stateful kernel. Variables persist across runs. Shift+Enter on Markdown switches it to Preview.
- **Run all:** execute code cells in document order. An execution error stops the remaining queue. Edits to queued cells invalidate their queued snapshots; outputs from an already running edited/deleted cell are discarded.
- **Interrupt:** interrupt the current execution and discard queued cells. The kernel can be reused afterward.
- **Shut down:** release this notebook's kernel and its processes. The next Run starts a fresh kernel. Old saved outputs remain until a cell actually begins producing a new result; startup/dependency failures preserve them.
- **Kernel / Use kernel:** select an installed kernelspec by name. Changing it shuts down the old kernel; the next Run starts the selected one. Other languages can use installed local kernels that support Jupyter's IPC transport.

The working directory is the open project root, or the local notebook's parent directory if no project is open. New untitled local notebooks use the editor's current directory. Remote notebooks reuse the authenticated SSH connection; the interpreter and kernel must already exist there and be visible within the active sandbox. Missing packages/paths fail visibly; there is no automatic installation, upload or fallback to local execution.

The bridge and kernel run under the actual project sandbox, including read-only grants and networking choices. Kernel channels use private Unix IPC sockets on that host, so no Jupyter HTTP server, browser runtime, port forwarding or shared networking is needed. Connection files, IPython runtime data and Matplotlib caches use an owned temporary directory. Only this notebook's kernel is shut down; existing terminals and other RODE windows are independent.

Kernels stop when their notebook tab closes, RODE exits, the notebook extension is disabled, or its project/SSH/sandbox/interpreter context changes. They are **not persistent terminals** and cannot be reattached after exit. For detached long-running commands, use the [persistent terminal](terminals.md). Interactive `input()` prompts, attaching existing Jupyter servers/kernels, widgets, JavaScript, HTML/SVG execution, completion/debugging and variable explorers are outside this implementation.

## Limits

Notebooks are limited to **8 MiB**, **1024 cells** and **256 KiB source per cell**; execution accepts up to **128 KiB code per cell** within the bounded stdio transport. A live cell retains at most **256 output records / 1 MiB of output**, with bounded process messages. Output overflow stops the owned kernel and preserves earlier results. Text display is capped at 64 KiB per output; the saved data remains intact.

Only visible cells are laid out. Image decoding runs on workers under the shared preview pixel budget; textures are released as cells leave view or tabs close. At most 16 output images are cached in a notebook view, with at most 16 inline Markdown images per preview. Large/unsupported image data remains preserved in JSON with a visible preview limitation. A malformed image never becomes an unrestricted filesystem or network load.

The notebook renderer and file operations are native Rust. The optional project-host bridge uses the installed Jupyter client to manage its owned kernel; it is launched only after an explicit Run. See [the notebook format](https://nbformat.readthedocs.io/en/latest/format_description.html) and [Jupyter messages](https://jupyter-client.readthedocs.io/en/stable/messaging.html) for the interoperable file and execution protocols.
