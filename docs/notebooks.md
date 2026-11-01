# Jupyter notebooks

Open `.ipynb` files from the explorer or **Ctrl+P**, or choose **File → New Jupyter notebook**. The native **Jupyter notebooks** extension is enabled by default and can be managed in **View → Extensions**. Viewing and editing saved notebooks require no Jupyter installation.

## Cells and outputs

Code cells use the independent syntax theme and the kernel's language. Markdown cells have **Edit / Preview** controls, including native tables, lists and embedded PNG/JPEG attachments. Inline image files use the current local/SSH host and sandbox grants. HTTP/file loaders and HTML execution are disabled.

Select a cell by clicking its header or editor. Use **+ Code cell**, **+ Markdown cell**, the cell type selector, **Up / Down** and **Delete** to arrange cells. Cell text keeps its editor undo/cursor state through tab switches and moves. Structural changes do not currently have a separate undo history. **Ctrl+F** searches cell sources and selects the next match.

Editable cells grow and shrink to fit their source, including wrapped lines. Scroll the notebook to read long cells; cell editors have no separate scroll area.

Saved and live outputs show stdout/stderr, tracebacks, text representations, PNGs and JPEGs. Common plots work through inline Matplotlib output. Unsupported MIME bundles, HTML, widget state, attachments, metadata and unfamiliar fields are preserved in the file even when they cannot be rendered. Live display updates and clear-output messages are supported; notebooks are never executed merely by opening them.

**Ctrl+S / Save As** use the normal atomic, revision-checked local/SSH saves. Cell edits and execution outputs mark the notebook dirty; tab close/project changes retain the unsaved-edit guard. An external change prevents overwriting it. A save records its submitted snapshot and leaves changes arriving during that save dirty. The notebook never goes through whole-file text formatting or code-run commands.

Background refreshes after Git operations or agent replies preserve the notebook view and live kernel when the saved contents are unchanged.

Use **Notebook… → Edit JSON source** for direct metadata or unsupported-cell editing. Switching to JSON stops this notebook's kernel; switching back reparses the current unsaved buffer. Invalid/unsupported notebooks remain available as ordinary JSON text, with an explanation. Their contents are never replaced by an empty notebook. The cell renderer supports **nbformat 4** and preserves untouched string/list source representations and unknown JSON fields.

Use **+ Code cell** or **+ Markdown cell** in the toolbar to insert below the selected cell. The same buttons after each cell insert at that position, including at the end of the notebook. New cells receive keyboard focus immediately.

## Running cells

Execution needs an installed interpreter with `jupyter_client` and an installed compatible kernel on the **project host**. For Python, install `jupyter_client` and `ipykernel` in the chosen environment yourself; for example, in the project terminal:

```sh
uv venv .venv
uv pip install --python .venv/bin/python jupyter_client ipykernel
# Optional plotting and project libraries:
uv pip install --python .venv/bin/python matplotlib
```

Do not recreate an existing environment. You can also use pip or an already prepared interpreter. Select it through **Environment → Python interpreter and virtual environments**; automatic selection uses the project's `.venv` when available. The editor installs no notebook dependencies. The default Python kernelspec uses that interpreter; a custom kernelspec may explicitly select a different executable.

- **Run cell / Shift+Enter:** execute the selected code cell in a stateful kernel. Variables persist across runs. Shift+Enter on Markdown switches it to Preview.
- **Run all:** execute code cells in document order. An execution error stops the remaining queue. Edits to queued cells invalidate their queued snapshots; outputs from an already running edited/deleted cell are discarded.
- **Interrupt:** interrupt the current execution and discard queued cells. The kernel can be reused afterward.
- **Notebook… → Shut down kernel:** release this notebook's kernel and its processes. The next Run starts a fresh kernel. Old saved outputs remain until a cell actually begins producing a new result; startup/dependency failures preserve them.
- **Python environment:** the dropdown shares the Environment tab's project-host selection and discovered environments, including `.venv`. Selecting here also updates Environment. These controls are grouped under **Kernel settings**. **Refresh kernels** repeats discovery without installing anything or starting a kernel.
- **Kernel:** defaults to **Python · selected environment** for Python notebooks, even when saved Python metadata names an older environment. This launches `ipykernel` with the selected interpreter, rather than a foreign interpreter from a user-installed `python3` spec. The dropdown also lists installed local kernels visible to that interpreter; explicitly selecting another kernelspec honours its executable and language. Changing kernels shuts down the old kernel; the next Run starts the selected one. Other languages can use installed local kernels that support Jupyter's IPC transport. Missing dependencies and discovery errors are shown beside the controls; environments without Jupyter remain selectable.

The working directory is the open project root, or the local notebook's parent directory if no project is open. New untitled local notebooks use the editor's current directory. Remote notebooks reuse the authenticated SSH connection; the interpreter and kernel must already exist there and be visible within the active sandbox. Missing packages/paths fail visibly; there is no automatic installation, upload or fallback to local execution.

The bridge and kernel run under the actual project sandbox, including read-only grants and networking choices. Kernel channels use private Unix IPC sockets on that host, so no Jupyter HTTP server, browser runtime, port forwarding or shared networking is needed. Connection files, IPython runtime data and Matplotlib caches use an owned temporary directory. Only this notebook's kernel is shut down; existing terminals and other RODE windows are independent.

Open **Kernel settings** for the environment, kernelspec, execution target and lifetime controls. New kernels start with **Persist kernel: off**. Their host controller stops after the controlling view closes. Enable **Persist kernel: on** before or during execution to retain the same kernel, variables, queued cells and bounded outputs after tab closure, RODE exit, project/interpreter changes or SSH loss. Changing persistence waits for a host acknowledgement and does not restart the kernel. Save an untitled notebook before enabling persistence, so its kernel can be found by document path later.

Use **Detach kernel** to leave persistent work running. After reopening the same host/project with the same sandbox policy, choose **Reattach kernel**. When multiple matching kernels exist, select one from **Running kernels… / Sessions → Notebook kernels & debuggers**. Reattachment starts no new kernel and executes no cells. A runtime permits one controlling window at a time. Its source snapshots match stable cell IDs and source text before output replay; edited/deleted cells do not receive stale results. Execution assigns missing or duplicate cell IDs and marks that metadata change dirty; save before closing to retain those identities. Outputs collected while detached are retained in host memory and are saved into the notebook only through the usual explicit save. Save As updates host discovery to the new notebook path.

**Shut down**, changing the kernelspec, switching execution targets or switching to JSON source explicitly ends this notebook's kernel. **End runtime** in Sessions also discards retained state. These actions leave its Slurm allocation and other kernels independent. The host owns Run All and continues accepted queues while detached; incomplete submissions, lifetime changes and explicit shutdowns must be acknowledged before a clean exit. Returning from JSON source to Cells or removing a tab preserves pending shutdown tracking; a failed shutdown keeps the window open and reports the error. Clearing outputs updates retained host output without clearing Python variables. Interrupted queues stop, and an interrupted kernel can be reused.

Persistence lasts for the host service/process lifetime; it does not survive host reboot, service termination, Slurm expiry or site logout cleanup. Retained results from an ended persistent kernel remain discoverable until **End runtime** removes them. No kernel memory or credentials are written to RODE preferences. Interactive `input()` prompts, attaching external Jupyter servers/kernels, widgets, JavaScript, HTML/SVG execution, notebook-cell stepping, completion and variable explorers are outside this implementation. The [debugger](debugger.md) supports persistent script/program debugging on the same compute targets.

## Slurm execution targets

The [GPU smoke-test notebook](../examples/slurm_gpu_smoke.ipynb) checks execution placement, GPU access, a short matrix workload, training, inline plots and persistent kernel state. Open it with a GPU-enabled PyTorch environment and select your allocation as described below.

For GPU execution, connect to the cluster's login host and use **Allocate… → Reserve for notebooks**. This creates a persistent, host-service-owned allocation; its terminal view shows scheduler progress and is not an interactive shell. Closing that view or shutting down a notebook kernel preserves the reservation. Use the confirmed **Release allocation** control when finished, or end its owning session. Reservations can continue consuming resources after RODE exits.

In **Kernel settings**, click **Refresh Slurm targets**, then choose a running allocation under **Execution target**. **Project host** remains the default. The picker lists your running ordinary job IDs, including jobs created outside RODE. Arrays and heterogeneous jobs are not supported as notebook targets in this version. A target is local to the open notebook view and is not saved in the `.ipynb` file. Changing it shuts down the current kernel; **Run** starts a fresh one and saved outputs remain intact. The running kernel reports its compute hostname and job ID.

RODE rechecks the selected job's UID-filtered ID, name and RUNNING state before each kernel start. It launches a noninteractive `srun --jobid=… --overlap` step through the existing SSH channel, with the notebook bridge and kernel together on one allocated node. They communicate through private IPC sockets there; no Jupyter server or forwarded ports are required. The selected Python environment, installed kernelspec, `jupyter_client`, project libraries and project/data paths must be available on the compute node. Interpreter selection is resolved there. GPU visibility comes from Slurm, not queue output. An active sandbox is applied on both hosts; a failed compute-node policy never falls back to the login host or unsandboxed execution.

Kernels share the selected allocation's resources with its other steps, including GPU memory; this does not reserve a separate GPU per notebook. Selecting an interactive allocation also retains that allocation's existing shell-owned lifetime. **Reserve for notebooks** avoids that dependency and does not occupy a compute step while idle. Allocation expiry/cancellation ends its kernels visibly. Kernel interruption and shutdown affect only that notebook's step, not the allocation, other notebooks or the workspace SSH connection. Enable **Persist kernel** as well as retaining the allocation to survive desktop disconnect/exit.

## Limits

Notebooks are limited to **8 MiB**, **1024 cells** and **256 KiB source per cell**; execution accepts up to **128 KiB code per cell** within the bounded stdio transport. A live cell retains at most **256 output records / 1 MiB of output**, with bounded process messages. Host queues and retained notebook state each have an **8 MiB encoded** budget; exceeding a retention limit stops the kernel visibly and preserves earlier results. Cross-cell display updates are checked against the aggregate budget before replacing any retained output. Source hashes identify queued cells without repeatedly transferring their code. At most 32 notebook/debugger runtimes are retained per host service; end old runtimes to free their slots. Text display is capped at 64 KiB per output; the saved data remains intact.

Only visible cells are laid out. Image decoding runs on workers under the shared preview pixel budget; textures are released as cells leave view or tabs close. At most 16 output images are cached in a notebook view, with at most 16 inline Markdown images per preview. Large/unsupported image data remains preserved in JSON with a visible preview limitation. A malformed image never becomes an unrestricted filesystem or network load.

The notebook renderer and file operations are native Rust. The optional project-host bridge uses the installed Jupyter client to manage its owned kernel; it is launched only after an explicit Run. See [the notebook format](https://nbformat.readthedocs.io/en/latest/format_description.html) and [Jupyter messages](https://jupyter-client.readthedocs.io/en/stable/messaging.html) for the interoperable file and execution protocols.
