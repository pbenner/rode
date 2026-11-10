//! Synchronous byte capture shared by host helpers and project commands.
//!
//! Every child starts in its own session. The scoped owner stops its process
//! group on success and every error path, then reaps the leader before pipe
//! collection. `waitid(WNOWAIT)` observes exit without releasing the PID before
//! the final group signal. Independent stdout/stderr limits preserve binary
//! transfers and prevent diagnostic overflow from becoming silent truncation.
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{self, Read},
    process::{Child, Command, ExitStatus, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Inclusive capture budgets; the first extra byte triggers scoped cleanup.
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub stdout: u64,
    pub stderr: u64,
}

/// A leader that must remain unreaped until its final process-group signal.
/// No other component may reap this child's PID while this owner is active.
struct Owned {
    child: Child,
    armed: bool,
}
impl Owned {
    /// Peek at this leader only; never reap an unrelated child or release our PID.
    fn exited(&mut self) -> io::Result<bool> {
        let result = rode_runtime_core::process::wait::exited(&self.child);
        if result
            .as_ref()
            .is_err_and(|error| error.raw_os_error() == Some(libc::ECHILD))
        {
            // A competing reaper/ignored SIGCHLD disposition broke ownership.
            // Never signal a potentially reused numeric PID.
            self.armed = false;
        }
        result
    }

    fn signal(&self, signal: libc::c_int) {
        if self.armed {
            // SAFETY: new_session made this unreaped child its group leader.
            unsafe { libc::kill(-(self.child.id() as i32), signal) };
        }
    }

    /// Invalidate signalling before wait can release the leader's PID.
    fn finish(&mut self) -> io::Result<ExitStatus> {
        self.signal(libc::SIGKILL);
        self.armed = false;
        self.child.wait()
    }

    fn stop(&mut self, grace: Duration) {
        if !grace.is_zero() {
            // Marp/Puppeteer uses SIGINT to close its separately grouped browser.
            self.signal(libc::SIGINT);
            let until = Instant::now() + grace;
            while Instant::now() < until && matches!(self.exited(), Ok(false)) {
                std::thread::sleep(Duration::from_millis(15));
            }
        }
        let _ = self.finish();
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.finish();
        }
    }
}

/// Capture owned binary I/O with bounded output, cancellation and stdin lifetime.
/// Descendants that deliberately create another session escape this group; this
/// is process supervision, not a replacement for the caller's sandbox policy.
pub(crate) fn run(
    mut command: Command,
    mut input: impl Read + Send + 'static,
    timeout: Duration,
    cancel: &AtomicBool,
    lifeline: bool,
    limits: Limits,
    terminate_grace: Duration,
) -> Result<Output> {
    let cancelled = || cancel.load(Ordering::Acquire) || super::CANCELLED.load(Ordering::Acquire);
    ensure!(!cancelled(), "Extension task cancelled.");
    let description = command.get_program().to_string_lossy().into_owned();
    super::new_session(&mut command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let spawn_until = Instant::now() + timeout.min(Duration::from_secs(1));
    let child = loop {
        ensure!(!cancelled(), "Extension task cancelled.");
        match command.spawn() {
            Err(error)
                if error.raw_os_error() == Some(libc::ETXTBSY) && Instant::now() < spawn_until =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => break result.with_context(|| format!("Could not start {description}"))?,
        }
    };
    let mut owner = Owned { child, armed: true };
    let mut stdin = owner.child.stdin.take().context("Missing command stdin")?;
    let stdout = owner
        .child
        .stdout
        .take()
        .context("Missing command stdout")?;
    let stderr = owner
        .child
        .stderr
        .take()
        .context("Missing command stderr")?;
    let (input_tx, input_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = io::copy(&mut input, &mut stdin);
        if lifeline {
            let _ = input_tx.send(stdin);
        }
    });
    let mut input_rx = Some(input_rx);
    let overflow = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    for (is_out, pipe, limit) in [
        (
            true,
            Box::new(stdout) as Box<dyn Read + Send>,
            limits.stdout,
        ),
        (
            false,
            Box::new(stderr) as Box<dyn Read + Send>,
            limits.stderr,
        ),
    ] {
        let tx = tx.clone();
        let overflow = overflow.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.take(limit.saturating_add(1)).read_to_end(&mut bytes);
            if bytes.len() as u64 > limit {
                overflow.store(true, Ordering::Release);
            }
            let _ = tx.send((is_out, bytes, result));
        });
    }
    drop(tx);
    let start = Instant::now();
    let status = loop {
        if cancelled() {
            // Closing the lease first lets a coordinator stop its own children.
            input_rx.take();
            if lifeline {
                let until = Instant::now() + Duration::from_secs(1).max(terminate_grace);
                while Instant::now() < until && !owner.exited()? {
                    std::thread::sleep(Duration::from_millis(15));
                }
            }
            owner.stop(terminate_grace);
            bail!("Extension task cancelled.");
        }
        if overflow.load(Ordering::Acquire) || start.elapsed() > timeout {
            input_rx.take();
            owner.stop(terminate_grace);
            if overflow.load(Ordering::Acquire) {
                bail!("{description} output exceeded its size limit.");
            }
            bail!("{description} timed out; the operation did not finish within its deadline.");
        }
        if owner.exited()? {
            break owner.finish()?;
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let mut output = Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    for _ in 0..2 {
        let (is_out, bytes, result) = rx
            .recv_timeout(Duration::from_secs(2))
            .context("Command output pipes did not close")?;
        result?;
        let limit = if is_out { limits.stdout } else { limits.stderr };
        ensure!(
            bytes.len() as u64 <= limit,
            "{description} output exceeded its size limit."
        );
        if is_out {
            output.stdout = bytes;
        } else {
            output.stderr = bytes;
        }
    }
    Ok(output)
}
