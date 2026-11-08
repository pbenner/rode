//! Verified helper bootstrap and one connection-owned, multiplexed stdio channel.
use crate::{files, sandbox, ssh};
use anyhow::{Context, Result, bail, ensure};
use rode_host_protocol::{self as wire, framing};
use rode_session::host::Response;
use std::{
    collections::HashMap,
    io::{Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

mod helper;

// Serialization is synchronous, so the generic wire envelope can borrow large
// file payloads instead of cloning them for the duration of an RPC.
type Operation<'a> =
    wire::Operation<&'a files::Request, &'a sandbox::Options, &'a crate::environment::Settings>;

type PendingReply = Result<Response, String>;
struct Pending {
    calls: HashMap<u64, mpsc::SyncSender<PendingReply>>,
    closed: Option<String>,
}
struct Shared {
    pending: Mutex<Pending>,
    ready: Condvar,
}
impl Shared {
    fn close(&self, reason: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.closed = Some(reason.into());
            for (_, call) in pending.calls.drain() {
                let _ = call.try_send(Err(reason.into()));
            }
            self.ready.notify_all();
        }
    }
}

pub(crate) struct Client {
    child: Mutex<Child>,
    input: Mutex<ChildStdin>,
    shared: Arc<Shared>,
    next: AtomicU64,
    environments: Mutex<HashMap<String, String>>,
    pub(crate) executable: String,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.shared
            .close("SSH host channel closed; reconnect before submitting another operation");
        if let Ok(child) = self.child.get_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The gateway is captured for the entire connection. A failed startup is retained
/// until reconnect; requests never retry through an unscoped command.
#[derive(Default)]
pub(crate) struct State {
    client: Mutex<Option<Result<Arc<Client>, String>>>,
    closed: AtomicBool,
}
impl State {
    pub(crate) fn client(&self, remote: &ssh::Remote) -> Result<Arc<Client>> {
        ensure!(
            !self.closed.load(Ordering::Acquire),
            "SSH host connection closed"
        );
        let mut state = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("Host startup lock failed"))?;
        if state.is_none() {
            *state = Some(
                Client::connect(remote)
                    .map(Arc::new)
                    .map_err(|e| format!("{e:#}")),
            );
        }
        if self.closed.load(Ordering::Acquire) {
            if let Some(Ok(client)) = &*state {
                client.stop("SSH disconnected during native host startup");
            }
            bail!("SSH host connection closed");
        }
        state.as_ref().unwrap().clone().map_err(anyhow::Error::msg)
    }
    pub(crate) fn close(self: &Arc<Self>) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        // Disconnect runs on the GUI thread. Startup may hold this mutex during a
        // slow SSH upload; neither that wait nor child reaping belongs in the UI.
        let state = self.clone();
        std::thread::spawn(move || {
            if let Ok(client) = state.client.lock()
                && let Some(Ok(client)) = &*client
            {
                client.stop("SSH disconnected; pending writes may have completed. Reconcile the file revision before saving again.");
            }
        });
    }
}

impl Client {
    fn connect(remote: &ssh::Remote) -> Result<Self> {
        ensure!(
            remote.connected(),
            "SSH is disconnected; reconnect to start the native host channel"
        );
        let mut probe = Command::new(remote.client_program());
        probe
            .args(remote.channel_args(false))
            .arg("uname -s; uname -m");
        let output = ssh::run_bounded(probe, vec![], Duration::from_secs(15))?;
        let probe = std::str::from_utf8(&output)?.lines().collect::<Vec<_>>();
        ensure!(
            probe.len() == 2 && probe[0] == "Linux",
            "Native host needs Linux and silent non-interactive shell startup"
        );
        let architecture = probe[1];
        let helper::Selected {
            executable,
            digest,
            info: expected_info,
        } = helper::select(remote, architecture)?;
        ensure!(
            !remote.native.closed.load(Ordering::Acquire),
            "SSH disconnected during native helper deployment"
        );
        let mut command = Command::new(remote.client_program());
        command
            // Native terminal creation captures this gateway's environment.
            // Keep its forwarded display alive for the connection, rather than
            // capturing one from a short-lived creation/attachment command.
            .args(remote.display_channel_args(false))
            .arg(format!("exec {} host-rpc", ssh::quote(&executable)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .context("Cannot start native host channel")?;
        let mut input = child.stdin.take().context("Missing host input")?;
        let mut output = child.stdout.take().context("Missing host output")?;
        let error = child.stderr.take().context("Missing host error stream")?;
        let (hello_tx, hello_rx) = mpsc::sync_channel(1);
        let expected_digest = digest.clone();
        let expected_arch = architecture.to_owned();
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending {
                calls: HashMap::new(),
                closed: None,
            }),
            ready: Condvar::new(),
        });
        let reader = shared.clone();
        std::thread::spawn(move || {
            let handshake = (|| -> Result<()> {
                let mut magic = [0; 12];
                output.read_exact(&mut magic)?;
                ensure!(
                    &magic == framing::MAGIC,
                    "Invalid native host preamble; check shell startup output"
                );
                let hello: wire::Hello =
                    framing::read(&mut output)?.context("Missing native host handshake")?;
                ensure!(
                    hello.version == framing::VERSION
                        && hello.digest == expected_digest
                        && hello.architecture == expected_arch,
                    "Native host version, digest or architecture mismatch"
                );
                let info = hello.info.context("Native helper lacks compatibility metadata; rebuild/install rode-sessiond with this editor")?;
                info.validate("linux", &expected_arch, rode_session::PROTOCOL)
                    .map_err(anyhow::Error::msg)?;
                ensure!(
                    expected_info
                        .as_ref()
                        .is_none_or(|expected| *expected == info),
                    "Installed helper metadata changed between discovery and startup; reconnect"
                );
                ensure!(
                    wire::helper::CAPABILITIES
                        .iter()
                        .all(|cap| hello.capabilities.iter().any(|value| value == cap)),
                    "Native host handshake is missing required capabilities"
                );
                Ok(())
            })();
            let ok = handshake.is_ok();
            let _ = hello_tx.send(handshake.map_err(|e| format!("{e:#}")));
            if ok {
                while let Ok(Some(reply)) =
                    framing::read_document::<wire::Reply<Response>>(&mut output)
                {
                    if let Ok(mut pending) = reader.pending.lock() {
                        if let Some(call) = pending.calls.remove(&reply.id) {
                            let _ = call.try_send(reply.result);
                        } else {
                            break;
                        }
                        reader.ready.notify_all();
                    } else {
                        break;
                    }
                }
            }
            reader.close("Native SSH channel lost. A submitted write may have completed; reconcile its revision before retrying. Reconnect to continue.");
        });
        // Drain stderr throughout the channel lifetime while retaining a bounded
        // startup diagnostic. Missing dynamic loaders must not become opaque EOFs.
        let diagnostics = Arc::new(Mutex::new(Vec::new()));
        let retained = diagnostics.clone();
        std::thread::spawn(move || {
            let mut error = error;
            let mut buffer = [0; 4096];
            while let Ok(n) = error.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                if let Ok(mut bytes) = retained.lock() {
                    let keep = n.min((64 * 1024_usize).saturating_sub(bytes.len()));
                    bytes.extend_from_slice(&buffer[..keep]);
                }
            }
        });
        let handshake = (|| -> Result<()> {
            let mut writer = DeadlineWriter::new(&mut input, Duration::from_secs(10))?;
            writer.write_all(framing::MAGIC)?;
            framing::write(
                &mut writer,
                &wire::Hello {
                    version: framing::VERSION,
                    architecture: architecture.into(),
                    digest,
                    capabilities: wire::helper::CAPABILITIES
                        .iter()
                        .map(|s| (*s).into())
                        .collect(),
                    info: None,
                },
            )?;
            hello_rx
                .recv_timeout(Duration::from_secs(15))
                .context("Native host handshake timed out")?
                .map_err(anyhow::Error::msg)
        })();
        if let Err(error) = handshake {
            let _ = child.kill();
            let _ = child.wait();
            let detail = diagnostics
                .lock()
                .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
                .unwrap_or_default();
            return if detail.is_empty() {
                Err(error)
            } else {
                Err(error.context(format!("Native helper startup failed: {detail}")))
            };
        }
        Ok(Self {
            child: Mutex::new(child),
            input: Mutex::new(input),
            shared,
            next: AtomicU64::new(1),
            environments: Mutex::new(HashMap::new()),
            executable,
        })
    }
    fn stop(&self, reason: &str) {
        self.shared.close(reason);
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    fn call(&self, operation: Operation<'_>) -> Result<Response> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let pending = self
                .shared
                .pending
                .lock()
                .map_err(|_| anyhow::anyhow!("Host request lock failed"))?;
            let (mut pending, wait) = self
                .shared
                .ready
                .wait_timeout_while(pending, Duration::from_secs(150), |p| {
                    p.closed.is_none() && p.calls.len() >= 4
                })
                .map_err(|_| anyhow::anyhow!("Host request lock failed"))?;
            if let Some(reason) = &pending.closed {
                bail!("{reason}");
            }
            ensure!(!wait.timed_out(), "Native host request queue timed out");
            pending.calls.insert(id, tx);
        }
        let sent = (|| -> Result<()> {
            let mut input = self
                .input
                .lock()
                .map_err(|_| anyhow::anyhow!("Host writer lock failed"))?;
            framing::write_document(
                &mut DeadlineWriter::new(&mut input, Duration::from_secs(45))?,
                &wire::Request { id, operation },
            )?;
            Ok(())
        })();
        if let Err(error) = sent {
            self.stop("Host request transfer failed; a write may have completed. Reconnect and reconcile before retrying.");
            return Err(error);
        }
        match rx.recv_timeout(Duration::from_secs(160)) {
            Ok(reply) => reply.map_err(anyhow::Error::msg),
            Err(_) => {
                self.stop("Native host request timed out; a write may have completed. Reconnect and reconcile before retrying.");
                bail!("Native host request timed out; no operation was replayed")
            }
        }
    }
    pub(crate) fn request(
        &self,
        request: &files::Request,
        plan: Option<&sandbox::Plan>,
    ) -> Result<files::Response> {
        let operation = if let files::Request::Prepare { path, sandbox } = request {
            ensure!(
                plan.is_none(),
                "Cannot prepare a project through an existing project worker"
            );
            Operation::Prepare {
                path: path.clone(),
                options: sandbox,
            }
        } else {
            let context = plan
                .map(|plan| -> Result<String> {
                    let base = plan
                        .host_context
                        .as_ref()
                        .context("Native operation requires a native prepared project context")?;
                    if plan.environment == Default::default() {
                        return Ok(base.clone());
                    }
                    let key = format!("{base}:{}", serde_json::to_string(&plan.environment)?);
                    let mut contexts = self
                        .environments
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Environment context lock failed"))?;
                    if let Some(context) = contexts.get(&key) {
                        return Ok(context.clone());
                    }
                    let Response::Derived { context } = self.call(Operation::Derive {
                        context: base.clone(),
                        environment: &plan.environment,
                    })?
                    else {
                        bail!("Invalid derived host context response")
                    };
                    contexts.insert(key, context.clone());
                    Ok(context)
                })
                .transpose()?;
            Operation::Run { context, request }
        };
        match self.call(operation)? {
            Response::Prepared { response } | Response::Completed { response } => {
                Ok(files::Response {
                    project: response.project,
                    tool: response.tool,
                    ..response.file.into()
                })
            }
            Response::Derived { .. } => bail!("Unexpected native host context response"),
        }
    }
}

/// Nonblocking writes make a stopped remote reader obey the transfer deadline.
struct DeadlineWriter<'a> {
    input: &'a mut ChildStdin,
    until: Instant,
}
impl<'a> DeadlineWriter<'a> {
    fn new(input: &'a mut ChildStdin, timeout: Duration) -> Result<Self> {
        use std::os::fd::AsRawFd;
        let fd = input.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        ensure!(
            flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
            "Cannot configure native host pipe"
        );
        Ok(Self {
            input,
            until: Instant::now() + timeout,
        })
    }
}
impl Write for DeadlineWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        loop {
            if Instant::now() >= self.until {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Native host transfer timed out",
                ));
            }
            match self.input.write(bytes) {
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disconnect_does_not_wait_for_an_in_progress_bootstrap() {
        let state = Arc::new(State::default());
        let bootstrap_lock = state.client.lock().unwrap();
        let (done, receive) = mpsc::sync_channel(1);
        let closing = state.clone();
        let thread = std::thread::spawn(move || {
            closing.close();
            done.send(()).unwrap();
        });
        let result = receive.recv_timeout(Duration::from_secs(1));
        // Release the simulated bootstrap even if the assertion would fail.
        drop(bootstrap_lock);
        thread.join().unwrap();
        assert!(result.is_ok(), "Disconnect waited on SSH bootstrap");
        assert!(state.closed.load(Ordering::Acquire));
    }
}
