# Interactive Slurm work

Use **View → Terminals & Slurm**, or **Sessions** in the terminal header, to inspect your jobs on the project host. Slurm support is optional; a machine without Slurm can use all terminal features.

## Allocate and run interactively

1. Connect to the login/working host and open your project.
2. Click **Slurm…**, immediately left of **Persist**, in the terminal header. The command palette also provides **Terminal: Allocate Slurm resources**.
3. Choose GPU count, time and memory, then **Allocate in new terminal**. Defaults are **1 GPU, 2 hours, 80 GB**. Bare time values mean hours; `2:30` means two hours and thirty minutes. Full `HH:MM:SS` / `D-HH:MM:SS` notation is also supported. Bare memory values mean GB; explicit Slurm units such as `80000M` work too. Advanced settings include partition, account, QoS, reservation, nodes, CPUs, constraints and a GRES override for typed/per-node requests. A GRES override replaces the total GPU count. Zero GPUs with no GRES requests CPU-only resources. Explicit requests save these settings per host/project.
4. The new terminal shows scheduler progress while the header says **Slurm: pending**. Once the job is **RUNNING**, the button fills with the theme's accent and shows the allocated GPU count. Query failures or stale state remove the active fill. Click it to inspect the exact job or cancel/release it with confirmation. Your previous terminal keeps running independently.
5. When granted, RODE opens your login shell on the first allocated node using `salloc … srun --export=ALL --nodes=1 --ntasks=1 --pty …`. Slurm supplies the `SLURM_*` variables and GPU visibility; RODE does not synthesize them from queue output. Bash login configuration can use `SLURM_STEP_GPUS`, `SLURM_JOB_GPUS`, `SLURM_GPUS_ON_NODE` or `CUDA_VISIBLE_DEVICES` in the prompt. For example, inspect the shell with:

   ```sh
   printf 'Job: %s\nStep GPUs: %s\nJob GPUs: %s\nVisible: %s\n' \
       "$SLURM_JOB_ID" "$SLURM_STEP_GPUS" "$SLURM_JOB_GPUS" "$CUDA_VISIBLE_DEVICES"
   ```

6. Enable **Persist: on** in the allocation terminal if it should survive closing RODE or losing SSH. New allocation terminals start with persistence off. Reconnect to the same host/project and **Sessions → Attach** to resume that shell. Its allocation identity survives renaming and shared attachments from other RODE windows.

The native service stays on the login/working host; the shell runs on the compute node. A service started directly on a disposable compute node is subject to that node's job/process cleanup rules. Site time limits and logout policies remain authoritative. The project path must exist on the allocated nodes. For multi-node work, the interactive shell starts on one node; use the site's Slurm commands for further steps. Existing manual `salloc` / `srun` workflows remain available in ordinary terminals.

## Counts and release

The manager lists your current host's allocations: job ID/name/state, nodes, elapsed/time remaining, and GPU resources reported by `squeue`. The total includes allocated jobs; pending jobs show **requested** GPUs separately. Typed and generic GPU TRES are not double-counted. Unknown GPU counts remain explicitly unknown. These are allocation counts, independent of the [monitor's physical GPU utilization](project-tools.md#host-monitor).

**Release allocation** opens a confirmation naming the exact job. Confirming requests cancellation of the **selected job/allocation and its steps**, including work launched outside RODE. RODE re-queries your own jobs and restricts cancellation to your UID and an exact listed allocation ID. Step IDs, ranges, arbitrary flags, and other users' jobs are not accepted. Array and heterogeneous rows show an additional scope warning: Slurm may affect other unsplit array elements, and cancelling heterogeneous component zero cancels the whole job; pending component cancellation may be refused. These follow [Slurm cancellation semantics](https://slurm.schedmd.com/scancel.html). GPU release may take time while Slurm cleans up the job; refresh to observe its disappearance.

Exiting the allocated shell naturally ends its owning `salloc`, which relinquishes the allocation. Detaching/closing a persistent terminal preserves it. Nonpersistent terminals end when their final view closes, so enable Persist before closing if you need the allocation to continue. RODE does not issue an automatic `scancel` on exit; use the confirmed Release control to explicitly release resources and check completion in the manager. Keeping a terminal running can continue consuming the allocation until it is released or expires.

## Executables and sandbox

Advanced allocation settings select installed `salloc` and `srun` programs or site wrappers. **Slurm executable paths / wrappers** selects `squeue` and `scancel`; **Save Slurm settings** persists those paths. Values are executable paths, not shell command lines; a wrapper can load your site's modules/environment first. Nothing installs Slurm or allocates resources before an explicit request.

Slurm commands run on the project host and obey its active sandbox. The same sandbox prefix is applied to the compute-node shell, since scheduler execution does not inherit the login host's mount namespace. Compute nodes must provide bubblewrap and the granted paths; failure never retries outside an active sandbox. GPU devices (for example `/dev/nvidia0`, `/dev/nvidiactl` and `/dev/nvidia-uvm`) and site authentication/configuration paths may need explicit grants. The selected Python environment is inherited, but no activation keystrokes are sent while allocation is pending. Shell startup files can still adjust that environment.

Queue polling runs at five-second intervals only while the manager/allocation dialog or a managed allocation terminal is visible, with manual refresh in the manager. Ordinary terminals do not start scheduler polling. Hidden persistent terminals keep running without UI polling. A failed query/cancellation is shown in the allocation dialog/manager.

The implementation uses [`squeue`](https://slurm.schedmd.com/squeue.html) with `--local --user <uid> --Format` and `tres-alloc`, and `scancel --user <uid> -- <job-id>`. It requires a Slurm version supporting these fields/options. Inherited `SCANCEL_*`, `SQUEUE_*`, and `SLURM_CLUSTERS` overrides are removed to keep cancellation scoped as displayed. Queue parsing, ownership checks and cancellation arguments are covered by deterministic local/SSH-peer fixtures; a real Slurm controller/GPU allocation was not available for validation.
