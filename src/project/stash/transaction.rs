//! Git owns the lockfiles; losing our stdin aborts its prepared transaction.
//! This also releases locks when the desktop or an SSH helper is terminated.
use super::{git_command, *};
use std::{
    io::{BufRead, BufReader, Read, Seek},
    process::{Child, ChildStdin, Stdio},
    sync::mpsc,
    time::Instant,
};

/// Prepared Git-owned writer-lock lease. Closing stdin or dropping the owner aborts
/// uncommitted work and releases locks even when an SSH controller disappears.
pub(super) struct Transaction {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<std::io::Result<String>>,
    errors: File,
}
impl Transaction {
    /// Acquire a no-dereference transaction against the expected stash tip; deletion
    /// and verification use the same bounded acknowledgement and owned-child cleanup path.
    pub fn prepare(root: &Path, tip: &str, delete: bool) -> Result<Self> {
        let errors = tempfile::tempfile()?;
        let mut command = git_command(root);
        // Verification is only our writer lease. Final deletion hooks are run
        // after the locked reflog identity check, using the repository's config.
        command.args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.filesRefLockTimeout=0",
            "-c",
            "core.packedRefsTimeout=0",
            "update-ref",
            "--stdin",
        ]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(errors.try_clone()?);
        crate::process::new_session(&mut command);
        let mut child = command.spawn()?;
        let input = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .context("Missing Git transaction output")?;
        let (sender, replies) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            for _ in 0..4 {
                let mut line = String::new();
                match (&mut reader).take(256).read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if sender.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                }
            }
        });
        let mut transaction = Self {
            child,
            input,
            replies,
            errors,
        };
        let operation = if delete { "delete" } else { "verify" };
        transaction.send(&format!(
            "option no-deref\nstart\n{operation} refs/stash {tip}\nprepare\n"
        ))?;
        transaction.acknowledge("start: ok")?;
        transaction.acknowledge("prepare: ok")?;
        Ok(transaction)
    }
    fn send(&mut self, command: &str) -> Result<()> {
        let input = self.input.as_mut().context("Git transaction is closed")?;
        input.write_all(command.as_bytes())?;
        input.flush()?;
        Ok(())
    }
    fn acknowledge(&mut self, expected: &str) -> Result<()> {
        match self.replies.recv_timeout(Duration::from_secs(120)) {
            Ok(Ok(line)) if line.trim_end() == expected => Ok(()),
            result => {
                self.errors.rewind()?;
                let mut detail = String::new();
                Read::by_ref(&mut self.errors)
                    .take(8192)
                    .read_to_string(&mut detail)?;
                anyhow::bail!(
                    "Git metadata is busy or the stash transaction failed ({result:?}): {detail}"
                )
            }
        }
    }
    pub fn finish(mut self, commit: bool) -> Result<()> {
        let operation = if commit { "commit" } else { "abort" };
        self.send(&format!("{operation}\n"))?;
        self.acknowledge(&format!("{operation}: ok"))?;
        self.input.take();
        self.wait(Duration::from_secs(2))?;
        Ok(())
    }
    fn wait(&mut self, timeout: Duration) -> Result<()> {
        let until = Instant::now() + timeout;
        while self.child.try_wait()?.is_none() {
            ensure!(Instant::now() < until, "Git transaction did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}
impl Drop for Transaction {
    fn drop(&mut self) {
        self.input.take();
        if self.wait(Duration::from_secs(2)).is_err() {
            // Git's own termination handler cleans its registered lockfiles.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGTERM);
            }
            if self.wait(Duration::from_secs(2)).is_err() {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}
