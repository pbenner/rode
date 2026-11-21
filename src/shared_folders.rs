//! Backend-independent SSHFS plumbing. Dedicated non-PTY channels reuse the
//! selected master; the GUI never authenticates again or handles SFTP packets.
//! Each relay has bounded buffers and cancellation-aware nonblocking pipe I/O.
use anyhow::{Context, Result, ensure};
pub(crate) use rode_host_core::shared_folders::{Config, Direction, Mount};
use rode_host_core::shared_folders::{Export, write_header};
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Mount status is emitted from a worker, never inferred from a successful spawn.
pub(crate) enum Event {
    Ready,
    Error(String),
}
/// Commands to a running mount; explicit unmount and forced teardown differ.
#[derive(Default)]
pub(crate) struct Control {
    pub cancel: AtomicBool,
    pub unmount: AtomicBool,
}

fn command(
    remote: Option<&crate::ssh::Remote>,
    helper: Option<&str>,
    flag: &str,
) -> Result<Command> {
    if let Some(remote) = remote {
        ensure!(
            remote.connected(),
            "SSH disconnected; reconnect before mounting"
        );
        let helper = helper.context("Missing verified remote helper")?;
        let mut command = Command::new(remote.client_program());
        command.args(remote.channel_args(false)).arg(format!(
            "exec {} {}",
            crate::ssh::quote(helper),
            crate::ssh::quote(flag)
        ));
        Ok(command)
    } else {
        let executable = rode_session::current_executable()?;
        #[cfg(test)]
        let executable = std::env::var_os("RODE_TEST_BINARY")
            .map(std::path::PathBuf::from)
            .unwrap_or(executable);
        let mut command = Command::new(executable);
        command.arg(flag);
        Ok(command)
    }
}
struct Process {
    child: Child,
    diagnostics: Arc<Mutex<Vec<u8>>>,
}
impl Process {
    fn spawn(
        mut command: Command,
        header: &impl serde::Serialize,
        ready: Option<Arc<AtomicBool>>,
    ) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let retained = diagnostics.clone();
        let stderr = child
            .stderr
            .take()
            .context("Missing shared-folder diagnostics")?;
        std::thread::spawn(move || {
            use std::io::BufRead;
            let mut input = std::io::BufReader::new(stderr);
            let mut line = Vec::new();
            // read_until itself is bounded: a broken tool cannot allocate an
            // unlimited diagnostic line or retain the parent through stdout.
            loop {
                line.clear();
                let result = input.by_ref().take(8192).read_until(b'\n', &mut line);
                if !matches!(result,Ok(n) if n>0) {
                    break;
                }
                if line == b"RODE_SHARE_READY\n" {
                    if let Some(ready) = &ready {
                        ready.store(true, Ordering::Release);
                    }
                } else {
                    let mut out = retained.lock().unwrap();
                    let count = line.len().min(8192usize.saturating_sub(out.len()));
                    out.extend_from_slice(&line[..count]);
                }
            }
        });
        if let Err(error) = write_header(child.stdin.as_mut().unwrap(), header) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(Self { child, diagnostics })
    }
    fn diagnostic(&self) -> String {
        String::from_utf8_lossy(&self.diagnostics.lock().unwrap())
            .trim()
            .to_owned()
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Pipe<T> {
    inner: T,
    cancel: Arc<Control>,
}
impl<T: AsRawFd> Pipe<T> {
    fn new(inner: T, cancel: Arc<Control>) -> Result<Self> {
        let fd = inner.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        ensure!(
            flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
            "Cannot make sharing pipe nonblocking"
        );
        Ok(Self { inner, cancel })
    }
    fn wait(&self, events: libc::c_short, deadline: Option<Instant>) -> std::io::Result<()> {
        loop {
            if self.cancel.cancel.load(Ordering::Acquire) {
                return Err(std::io::ErrorKind::ConnectionAborted.into());
            }
            if deadline.is_some_and(|end| Instant::now() >= end) {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            let mut fd = libc::pollfd {
                fd: self.inner.as_raw_fd(),
                events,
                revents: 0,
            };
            let count = unsafe { libc::poll(&mut fd, 1, 100) };
            if count > 0 {
                return Ok(());
            }
            if count < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
}
fn relay(
    mut read: Pipe<std::process::ChildStdout>,
    mut write: Pipe<std::process::ChildStdin>,
) -> Result<()> {
    let mut bytes = [0; 64 * 1024];
    loop {
        // Idle shares may remain mounted for days. Only a pending write has a
        // deadline; inactivity is not a failed connection.
        read.wait(libc::POLLIN, None)?;
        let count = match read.inner.read(&mut bytes) {
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            result => result?,
        };
        if count == 0 {
            return Ok(());
        }
        let mut at = 0;
        let deadline = Instant::now() + Duration::from_secs(30);
        while at < count {
            write.wait(libc::POLLOUT, Some(deadline))?;
            match write.inner.write(&bytes[at..count]) {
                Ok(0) => anyhow::bail!("Shared-folder pipe closed"),
                Ok(n) => at += n,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// Run one explicit share until acknowledged unmount, cancellation or channel
/// loss. Serving-side permissions and mount ownership stay in headless helpers.
pub(crate) fn run(
    endpoint: crate::transfers::Endpoint,
    config: Config,
    control: Arc<Control>,
    mut event: impl FnMut(Event),
) -> Result<()> {
    config.validate()?;
    let remote = endpoint
        .remote()
        .context("Shared folders require an SSH host")?;
    let helper = remote.native.client(remote)?.executable.clone();
    ensure!(!control.cancel.load(Ordering::Acquire), "Mount cancelled");
    let source_remote = config.direction == Direction::RemoteToLocal;
    // Validate the remote path through exactly the selected transfer policy.
    let remote_folder = remote.request_in(
        &crate::files::Request::List {
            path: if source_remote {
                config.source.clone()
            } else {
                config.destination.clone()
            },
        },
        endpoint.plan(),
    )?;
    let source = Export {
        root: if source_remote {
            remote_folder.path.clone()
        } else {
            config.source.clone()
        },
        read_only: config.read_only,
        plan: if source_remote {
            endpoint.plan().cloned()
        } else {
            None
        },
    };
    let mut entropy = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut entropy)?;
    let mount = Mount {
        path: if source_remote {
            crate::files::expand_home(&config.destination)
                .canonicalize()?
                .to_str()
                .context("Mount path must be UTF-8")?
                .into()
        } else {
            remote_folder.path.clone()
        },
        tag: format!(
            "rode-share-{}",
            entropy
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        ),
        read_only: config.read_only,
    };
    let ready = Arc::new(AtomicBool::new(false));
    let mut exporting = Process::spawn(
        command(
            source_remote.then_some(remote),
            Some(&helper),
            "--shared-folder-export",
        )?,
        &source,
        None,
    )?;
    let mut mounting = Process::spawn(
        command(
            (!source_remote).then_some(remote),
            Some(&helper),
            "--shared-folder-mount",
        )?,
        &mount,
        Some(ready.clone()),
    )?;
    let requests = Pipe::new(mounting.child.stdout.take().unwrap(), control.clone())?;
    let request_target = Pipe::new(exporting.child.stdin.take().unwrap(), control.clone())?;
    let replies = Pipe::new(exporting.child.stdout.take().unwrap(), control.clone())?;
    let reply_target = Pipe::new(mounting.child.stdin.take().unwrap(), control.clone())?;
    let (tx, rx) = std::sync::mpsc::channel();
    let tx2 = tx.clone();
    let a = std::thread::spawn(move || {
        let _ = tx.send(relay(requests, request_target));
    });
    let b = std::thread::spawn(move || {
        let _ = tx2.send(relay(replies, reply_target));
    });
    let start = Instant::now();
    let mut announced = false;
    let result = (|| -> Result<()> {
        loop {
            if control.cancel.load(Ordering::Acquire) {
                return Ok(());
            }
            if ready.load(Ordering::Acquire) && !announced {
                event(Event::Ready);
                announced = true;
            }
            if control.unmount.swap(false, Ordering::AcqRel) {
                let result = if source_remote {
                    rode_host_core::shared_folders::unmount(&mount, false)
                } else {
                    let mut request = serde_json::to_vec(&mount)?;
                    request.push(b'\n');
                    rode_host_core::command::run_controlled(
                        command(Some(remote), Some(&helper), "--shared-folder-unmount")?,
                        request,
                        Duration::from_secs(15),
                        &control.cancel,
                        false,
                    )
                    .and_then(|output| output.checked().map(|_| ()))
                };
                match result {
                    Ok(()) => return Ok(()),
                    Err(error) => event(Event::Error(format!("{error:#}"))),
                }
            }
            if let Some(status) = mounting.child.try_wait()? {
                ensure!(
                    status.success(),
                    "Mount helper exited with {status}: {}",
                    mounting.diagnostic()
                );
                return Ok(());
            }
            if let Some(status) = exporting.child.try_wait()? {
                anyhow::bail!(
                    "Shared-folder server ended ({status}): {}",
                    exporting.diagnostic()
                );
            }
            if let Ok(result) = rx.try_recv() {
                result?;
                anyhow::bail!(
                    "Shared-folder connection ended: {} {}",
                    exporting.diagnostic(),
                    mounting.diagnostic()
                );
            }
            ensure!(
                announced || start.elapsed() < Duration::from_secs(30),
                "Shared-folder mount did not become ready: {} {}",
                exporting.diagnostic(),
                mounting.diagnostic()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    })();
    control.cancel.store(true, Ordering::Release);
    let _ = a.join();
    let _ = b.join();
    // EOF gives the mounting helper time to detach its exact mount before the
    // channel process is reaped. It also handles abrupt desktop process death.
    let until = Instant::now() + Duration::from_secs(12);
    while mounting.child.try_wait()?.is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    result
}
