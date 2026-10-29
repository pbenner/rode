# Interactive Slurm work

Use **View → Terminals & Slurm**, or **Sessions** in the terminal header, to inspect your jobs on the project host. Slurm support is optional; a machine without Slurm can use all terminal features.

## Allocate and run interactively

1. Connect to the login/working host and open your project.
2. Start a terminal there and enable **Persist: on** in its header before allocating resources.
3. Run your site's usual `salloc` / `srun --pty` commands, then run the model interactively. For example, where supported by your site's configuration:

   ```sh
   salloc --gres=gpu:2 --time=02:00:00
   srun --pty bash
   # Run your training command here.
   ```

4. Close RODE; the enabled persistence setting keeps the terminal running without an exit prompt. Reconnect to the same host/project and **Attach** the terminal to continue interacting with it.

Create the persistent terminal on the stable host before entering the allocation. A service started on a disposable compute node is subject to that node's job/process cleanup rules. Partitions, accounts, QoS, GPU request syntax, time limits, and logout policies remain site-specific; RODE does not alter them or allocate GPUs automatically.

## Counts and release

The manager lists your current host's allocations: job ID/name/state, nodes, elapsed/time remaining, and GPU resources reported by `squeue`. The total includes allocated jobs; pending jobs show **requested** GPUs separately. Typed and generic GPU TRES are not double-counted. Unknown GPU counts remain explicitly unknown. These are allocation counts, independent of the [monitor's physical GPU utilization](project-tools.md#host-monitor).

**Release allocation** opens a confirmation naming the exact job. Confirming requests cancellation of the **selected job/allocation and its steps**, including work launched outside RODE. RODE re-queries your own jobs and restricts cancellation to your UID and an exact listed allocation ID. Step IDs, ranges, arbitrary flags, and other users' jobs are not accepted. Array and heterogeneous rows show an additional scope warning: Slurm may affect other unsplit array elements, and cancelling heterogeneous component zero cancels the whole job; pending component cancellation may be refused. These follow [Slurm cancellation semantics](https://slurm.schedmd.com/scancel.html). GPU release may take time while Slurm cleans up the job; refresh to observe its disappearance.

Detaching a terminal, closing the editor, or ending a terminal does not invoke allocation cancellation. Use Release explicitly when you are done with the GPUs. Keeping a terminal running can continue consuming the allocation until it is released or expires.

## Executables and sandbox

The manager's **Slurm executable paths / wrappers** settings select the `squeue` and `scancel` executables, defaulting to those names on PATH. Values are executable paths, not shell command lines; use a wrapper script to load your site's environment/modules first. **Save Slurm settings** persists them per host/project using the existing coordinated preferences.

Slurm commands run on the project host and obey its active sandbox. If required configuration, authentication sockets, wrappers or executables are hidden, adjust the project's explicit access/network rules. A failed query/cancellation is shown in the manager; RODE does not retry outside the sandbox. Polling occurs only while the manager is visible, at 15-second intervals, with a manual refresh button.

The implementation uses [`squeue`](https://slurm.schedmd.com/squeue.html) with `--local --user <uid> --Format` and `tres-alloc`, and `scancel --user <uid> -- <job-id>`. It requires a Slurm version supporting these fields/options. Inherited `SCANCEL_*`, `SQUEUE_*`, and `SLURM_CLUSTERS` overrides are removed to keep cancellation scoped as displayed. Queue parsing, ownership checks and cancellation arguments are covered by deterministic local/SSH-peer fixtures; a real Slurm controller/GPU allocation was not available for validation.
