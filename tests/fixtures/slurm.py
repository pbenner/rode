#!/usr/bin/python3
"""Deterministic Slurm peer: explicit grant/cancel files, no scheduler or GPU access.

Copy this executable as salloc/srun/squeue/scancel into a fixture's tools directory.
The parent project contains slurm-state; creating grant releases the pending request.
"""

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

state = Path(__file__).parent.parent / "slurm-state"
state.mkdir(exist_ok=True)
record = state / "job.json"
program = Path(sys.argv[0]).name


def write_job(job):
    temporary = record.with_suffix(".tmp")
    temporary.write_text(json.dumps(job))
    temporary.replace(record)


def run():
    if program == "salloc":
        arguments = sys.argv[1:]
        index = next(i for i, value in enumerate(arguments) if not value.startswith("--"))
        options, command = arguments[:index], arguments[index:]
        assert "SLURM_JOB_ID" not in os.environ
        assert "SLURM_STEP_GPUS" not in os.environ
        assert "SLURM_CLUSTERS" not in os.environ
        assert not any(key.startswith(("SALLOC_", "SRUN_")) for key in os.environ)
        job = dict(
            id="4242",
            state="PENDING",
            name=next(
                value.split("=", 1)[1] for value in options if value.startswith("--job-name=")
            ),
            args=options,
            pid=os.getpid(),
        )
        write_job(job)

        def stop(_signal, _frame):
            raise SystemExit(1)

        signal.signal(signal.SIGTERM, stop)
        signal.signal(signal.SIGHUP, stop)
        signal.signal(signal.SIGINT, stop)
        child = None
        try:
            deadline = time.monotonic() + 120
            while not (state / "grant").exists():
                assert time.monotonic() < deadline, "Fixture allocation was never granted"
                time.sleep(0.05)
            job["state"] = "RUNNING"
            write_job(job)
            env = dict(
                os.environ,
                SLURM_JOB_ID="4242",
                SLURM_JOBID="4242",
                SLURM_JOB_GPUS="0,1",
                SLURM_JOB_NODELIST="gpu-test",
                SLURM_CPUS_PER_TASK="4",
            )
            child = subprocess.Popen(command, env=env)
            return child.wait()
        finally:
            if child and child.poll() is None:
                child.send_signal(signal.SIGHUP)
                try:
                    child.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            record.unlink(missing_ok=True)
            (state / "ended").write_text("released")
    if program == "srun":
        args = sys.argv[1:]
        if args[0].startswith("--jobid="):
            assert args[:5] == [
                "--jobid=4242",
                "--overlap",
                "--export=ALL",
                "--nodes=1",
                "--ntasks=1",
            ]
            assert record.exists() and json.loads(record.read_text())["state"] == "RUNNING"
            assert "SLURM_JOB_ID" not in os.environ
            assert not any(key.startswith(("SALLOC_", "SRUN_")) for key in os.environ)
            os.environ["SLURM_JOB_ID"] = "4242"
            (state / "step.json").write_text(json.dumps(args[:5]))
            command = args[5:]
        else:
            assert args[:4] == ["--export=ALL", "--nodes=1", "--ntasks=1", "--pty"]
            assert os.environ["SLURM_JOB_ID"] == "4242"
            command = args[4:]
        os.environ.update(
            SLURM_STEP_ID="0",
            SLURM_STEP_GPUS="0,1",
            SLURM_GPUS_ON_NODE="2",
            CUDA_VISIBLE_DEVICES="0,1",
        )
        if not args[0].startswith("--jobid="):
            os.execvpe(command[0], command, os.environ)
        child = subprocess.Popen(command, start_new_session=True)

        def end_step(_signal=None, _frame=None):
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)

        signal.signal(signal.SIGTERM, end_step)
        signal.signal(signal.SIGHUP, end_step)
        try:
            while child.poll() is None and record.exists():
                time.sleep(0.05)
        finally:
            end_step()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
        return child.returncode
    if program == "squeue":
        assert sys.argv[sys.argv.index("--user") + 1] == str(os.geteuid())
        if (state / "queue-error").exists():
            print("Fixture scheduler unavailable", file=sys.stderr)
            return 1
        if record.exists():
            job = json.loads(record.read_text())
            if "--jobs" in sys.argv:
                assert sys.argv[sys.argv.index("--jobs") + 1] == job["id"]
                print(f"{job['id']}|{job['state']}|{job['name']}")
            else:
                print(
                    f"{job['id']}|{job['state']}|0:00|1:00:00|gpu-test|cpu=4,gres/gpu=2|{job['name']}"
                )
        return 0
    if program == "scancel":
        assert sys.argv[1:] == ["--user", str(os.geteuid()), "--", "4242"]
        job = json.loads(record.read_text())
        os.kill(job["pid"], signal.SIGTERM)
        (state / "cancelled").write_text(job["id"])
        return 0
    raise AssertionError(program)


sys.exit(run())
