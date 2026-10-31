"""Revalidate one owned allocation and replace this process with its stdio step.

Runs on the project/login host inside its active policy. The compute bootstrap
resolves Python on the compute node, inside the same policy, before starting the
notebook bridge. No controller output is used to manufacture GPU visibility.
"""

import json
import os
import re
import subprocess
import sys
import tempfile

COMPUTE = r"""
import json, os, sys
spec = json.loads(sys.argv[1])
if os.environ.get("SLURM_JOB_ID") != spec["target"]["id"]:
    raise ValueError("Slurm did not supply the selected allocation; refusing a host kernel")
root = os.path.realpath(os.path.expanduser(spec["root"]))
os.chdir(root)
python = spec["python"]
if python == "auto":
    python = next((os.path.join(root, folder, "bin/python")
                   for folder in (".venv", "venv", "env", ".env")
                   if os.path.isfile(os.path.join(root, folder, "bin/python"))), "python3")
elif python in ("off", ""):
    python = "python3"
if "/" in python or python.startswith("~"):
    python = os.path.expanduser(python)
    if not os.path.isabs(python):
        python = os.path.join(root, python)
    if os.path.isdir(python):
        python = os.path.join(python, "bin/python")
    os.environ["PATH"] = os.path.dirname(python) + os.pathsep + os.environ.get("PATH", "")
    prefix = os.path.dirname(os.path.dirname(python))
    if os.path.isfile(os.path.join(prefix, "pyvenv.cfg")):
        os.environ["VIRTUAL_ENV"] = prefix
        os.environ.pop("PYTHONHOME", None)
os.execvpe(python, [python, "-u", "-c", spec["source"]], os.environ)
"""


def run():
    """Check exact ID/name/state and exec one overlapping step without a shell."""
    spec = json.loads(sys.argv[1])
    target = spec["target"]
    job = target["id"]
    if not re.fullmatch(r"[0-9]{1,20}", job):
        raise ValueError("Only ordinary Slurm allocations can run notebook kernels")
    root = os.path.realpath(os.path.expanduser(spec["root"]))

    def program(value):
        value = os.path.expanduser(value.strip())
        return os.path.join(root, value) if "/" in value and not os.path.isabs(value) else value

    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(
            (
                "SALLOC_",
                "SRUN_",
                "SQUEUE_",
                "SCANCEL_",
                "SLURM_JOB",
                "SLURM_STEP",
                "SLURM_NTASKS",
                "SLURM_MEM_PER",
                "SLURM_GPUS",
                "SLURM_TRES",
            )
        )
        and key
        not in (
            "SLURM_CLUSTERS",
            "SLURM_NNODES",
            "SLURM_CPUS_PER_TASK",
            "CUDA_VISIBLE_DEVICES",
            "ROCR_VISIBLE_DEVICES",
            "GPU_DEVICE_ORDINAL",
        )
    }
    env["LC_ALL"] = "C"
    # Disk-backed output keeps a broken site wrapper from filling process memory.
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
        result = subprocess.run(
            [
                program(target["queue"]),
                "--noheader",
                "--array",
                "--local",
                "--user",
                str(os.geteuid()),
                "--jobs",
                job,
                "--Format",
                "JobID:0|,State:0|,Name:0",
            ],
            cwd=root,
            env=env,
            stdout=output,
            stderr=errors,
            timeout=15,
            check=False,
        )
        if result.returncode:
            errors.seek(0)
            raise ValueError(
                "Cannot verify Slurm allocation: " + errors.read(4096).decode(errors="replace")
            )
        output.seek(0)
        data = output.read(8193)
    if len(data) > 8192:
        raise ValueError("Slurm target query exceeded its output limit")
    rows = [line.split("|", 2) for line in data.decode().splitlines() if line.strip()]
    expected = [job, "RUNNING", target["name"]]
    if len(rows) != 1 or [field.strip() for field in rows[0]] != expected:
        raise ValueError("Selected allocation is no longer your running job; refresh Slurm targets")
    argv = [
        program(target["runner"]),
        "--jobid=" + job,
        "--overlap",
        "--export=ALL",
        "--nodes=1",
        "--ntasks=1",
    ]
    argv += spec["prefix"] + ["python3", "-u", "-c", COMPUTE, json.dumps(spec)]
    os.chdir(root)
    os.execvpe(argv[0], argv, env)


try:
    run()
except Exception as error:
    print("Cannot start Slurm runtime: " + str(error), file=sys.stderr)
    sys.exit(1)
