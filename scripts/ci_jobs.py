"""Bounded independent command jobs using CI's existing deadlines and cleanup.

Only the main thread publishes aggregate results or installs signal handlers.
Workers own distinct logs and process groups; cancellation is acknowledged before
returning. GUI callers must give each job a separate display and result directory.
"""

import concurrent.futures
import io
import os
import signal
import threading
import time
from dataclasses import dataclass

import ci

PROGRESS_INTERVAL = 30


def available_cpus():
    """Respect process affinity instead of the host's unrestricted CPU count."""
    try:
        cpus = len(os.sched_getaffinity(0))
    except (AttributeError, OSError):
        cpus = os.cpu_count() or 1
    return max(1, cpus)


def default_jobs():
    """Bound fixture load on shared machines and small CI runners."""
    return min(4, available_cpus())


@dataclass(frozen=True)
class Job:
    """One command with private environment overrides and a unique log identity."""

    name: str
    command: list[str]
    env: dict[str, str]
    timeout: float = 300
    group: str = "commands"
    umask: int | None = None
    cwd: str | None = None


@dataclass
class Result:
    """The observed exit status plus bounded console output, never a retry."""

    name: str
    exit_code: int
    output: str = ""
    error: str = ""


def _execute(job, cancel):
    console = io.StringIO()
    code, error = 0, ""
    try:
        ci.measure(
            job.name,
            job.command,
            job.env,
            job.timeout,
            job.group,
            umask=job.umask,
            cancel=cancel,
            console_file=console,
            cwd=job.cwd,
        )
    except SystemExit as failure:
        code = failure.code if isinstance(failure.code, int) else 1
    except Exception as failure:
        # Missing/corrupt metadata remains visible to aggregate validation too.
        code, error = 1, str(failure)
    return Result(job.name, code, console.getvalue(), error)


def run(jobs, limit, on_result=lambda result: None):
    """Run every job once, keeping failures and cancelling only on interruption.

    Repeat iterations are scheduled by the caller, so one iteration finishes
    before the next begins. Pending jobs are not started after cancellation.
    The callback runs only on the main thread after each completed command.
    """
    jobs = list(jobs)
    if not 1 <= limit <= 64:
        raise ValueError("Job concurrency must be between 1 and 64")
    identities = [(job.group, job.name) for job in jobs]
    if len(set(identities)) != len(identities):
        raise ValueError("Parallel jobs require unique log identities")
    cancel = threading.Event()
    results = []
    active = {}
    started = {}
    pending = iter(jobs)

    def terminate(_signum, _frame):
        raise KeyboardInterrupt

    def publish(future):
        result = future.result()
        print(result.output, end="", flush=True)
        if result.error:
            print(f"{result.name}: scheduler error: {result.error}", flush=True)
        results.append(result)
        on_result(result)
        return result

    previous = signal.signal(signal.SIGTERM, terminate)
    executor = concurrent.futures.ThreadPoolExecutor(max_workers=limit)
    interrupted = False
    next_progress = time.monotonic() + PROGRESS_INTERVAL
    try:
        while active or not cancel.is_set():
            while len(active) < limit and not cancel.is_set():
                job = next(pending, None)
                if job is None:
                    break
                print(f"Starting {job.name}", flush=True)
                started[job.name] = time.monotonic()
                active[executor.submit(_execute, job, cancel)] = job
            if not active:
                break
            done, _ = concurrent.futures.wait(
                active, timeout=0.1, return_when=concurrent.futures.FIRST_COMPLETED
            )
            for future in done:
                active.pop(future)
                if publish(future).exit_code in (130, 143):
                    cancel.set()
                    interrupted = True
            now = time.monotonic()
            if active and now >= next_progress:
                running = ", ".join(
                    f"{job.name} ({now - started[job.name]:.0f}s)" for job in active.values()
                )
                print(f"Active jobs: {running}", flush=True)
                next_progress = now + PROGRESS_INTERVAL
    except KeyboardInterrupt:
        interrupted = True
    finally:
        cancel.set()
        try:
            # ci.measure observes cancellation and reaps its owned process group.
            # Keep the handler from interrupting cleanup on a second SIGTERM.
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            executor.shutdown(wait=True, cancel_futures=True)
            for future in active:
                if not future.cancelled():
                    publish(future)
        finally:
            signal.signal(signal.SIGTERM, previous)
    if interrupted:
        raise SystemExit(130)
    return results
