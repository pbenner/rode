//! Bulk-copy channels share authentication without occupying the document RPC
//! writer. File management reuses Explorer RPC with the same captured policy.
//! Pipe polling bounds stalled streams and observes per-job cancellation.
use crate::pipe::Pipe;
use anyhow::{Context, Result, ensure};
use rode_host_protocol::{
    framing,
    transfer::{Reply, Request},
};
use std::{
    io::{Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};

/// Immutable destination captured before a task leaves the UI thread.
#[derive(Clone)]
pub(crate) enum Endpoint {
    Local,
    Ssh(Box<SshEndpoint>),
}

/// Connection and policy form one captured authority; neither can be omitted
/// or attached to a desktop endpoint. Construct through `Endpoint::ssh`.
#[derive(Clone)]
pub(crate) struct SshEndpoint {
    remote: crate::ssh::Remote,
    plan: crate::sandbox::Plan,
}
impl Endpoint {
    pub fn local() -> Self {
        Self::Local
    }
    pub fn ssh(remote: crate::ssh::Remote, plan: crate::sandbox::Plan) -> Self {
        Self::Ssh(Box::new(SshEndpoint { remote, plan }))
    }
    pub fn remote(&self) -> Option<&crate::ssh::Remote> {
        match self {
            Self::Local => None,
            Self::Ssh(endpoint) => Some(&endpoint.remote),
        }
    }
    pub fn plan(&self) -> Option<&crate::sandbox::Plan> {
        match self {
            Self::Local => None,
            Self::Ssh(endpoint) => Some(&endpoint.plan),
        }
    }
    /// Reuse Explorer's literal-path operations on this captured endpoint. A
    /// remote mutation keeps its prepared policy and never falls back locally.
    pub fn mutate(&self, mutation: crate::files::Mutation) -> Result<crate::files::Response> {
        let request = crate::files::Request::Mutate { mutation };
        match self {
            Self::Local => crate::files::local(&request),
            Self::Ssh(endpoint) => {
                ensure!(
                    endpoint.plan.host_context.is_some(),
                    "Remote file operations require a prepared host context"
                );
                endpoint.remote.request_in(&request, Some(&endpoint.plan))
            }
        }
    }
    pub fn open(&self, cancel: Arc<AtomicBool>) -> Result<Client> {
        if let Self::Ssh(endpoint) = self {
            let SshEndpoint { remote, plan } = endpoint.as_ref();
            ensure!(remote.connected(), "SSH disconnected");
            ensure!(
                plan.host_context.is_some(),
                "Remote transfers require a prepared project context"
            );
            let gateway = remote.native.client(remote)?;
            let mut command = Command::new(remote.client_program());
            command.args(remote.channel_args(false)).arg(format!(
                "exec {} --file-transfer",
                crate::ssh::quote(&gateway.executable)
            ));
            let mut client = Stream::spawn(command, cancel)?;
            let mut header = serde_json::to_vec(plan)?;
            header.push(b'\n');
            client.input.write_all(&header)?;
            client.ready()?;
            Ok(Client::Stream(Box::new(client)))
        } else {
            Ok(Client::Local(Box::default()))
        }
    }
}

pub(crate) enum Client {
    Local(Box<rode_host_core::transfer::Session>),
    Stream(Box<Stream>),
}
impl Client {
    pub fn open_source(&mut self, path: String, version: String, size: u64) -> Result<()> {
        self.request(Request::OpenRead { path, version })?;
        if let Self::Stream(stream) = self {
            stream.window =
                (size / rode_host_protocol::transfer::BLOCK as u64 + 1).clamp(1, 32) as usize;
        }
        Ok(())
    }
    pub fn request(&mut self, request: Request) -> Result<Reply> {
        match self {
            Self::Local(session) => session.request(&request),
            Self::Stream(stream) => stream.request(request),
        }
    }
}
pub(crate) struct Stream {
    child: Child,
    input: Pipe<ChildStdin>,
    output: Pipe<ChildStdout>,
    reads: usize,
    writes: usize,
    window: usize,
    diagnostics: Arc<Mutex<Vec<u8>>>,
}
impl Stream {
    fn spawn(mut command: Command, cancel: Arc<AtomicBool>) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let mut stderr = child
            .stderr
            .take()
            .context("Missing transfer diagnostics")?;
        let diagnostics = Arc::new(Mutex::new(Vec::<u8>::new()));
        let retained = diagnostics.clone();
        std::thread::spawn(move || {
            let mut block = [0; 2048];
            while let Ok(count) = stderr.read(&mut block) {
                if count == 0 {
                    break;
                }
                let mut bytes = retained.lock().unwrap();
                let count = count.min(8192usize.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&block[..count]);
            }
        });
        let input = Pipe::new(
            child.stdin.take().context("Missing transfer input")?,
            cancel.clone(),
        )?
        .with_idle_timeout(Duration::from_secs(60));
        let output = Pipe::new(
            child.stdout.take().context("Missing transfer output")?,
            cancel,
        )?
        .with_idle_timeout(Duration::from_secs(60));
        Ok(Self {
            child,
            input,
            output,
            reads: 0,
            writes: 0,
            window: 1,
            diagnostics,
        })
    }
    /// A fixed window amortizes SSH round trips while retaining at most 8 MiB
    /// of read-ahead. Write acknowledgements are drained before publication.
    fn request(&mut self, request: Request) -> Result<Reply> {
        if matches!(request, Request::Read) {
            if self.reads == 0 {
                for _ in 0..self.window {
                    framing::write(&mut self.input, &Request::Read)?;
                    self.reads += 1;
                }
            }
            let reply = self.reply()?;
            self.reads -= 1;
            if reply.data.is_empty() {
                while self.reads > 0 {
                    // Requests queued after EOF receive the expected "No source" response.
                    let _ = framing::read::<Result<Reply, String>>(&mut self.output)?
                        .context("Transfer channel ended during read-ahead")?;
                    self.reads -= 1;
                }
            } else {
                framing::write(&mut self.input, &Request::Read)?;
                self.reads += 1;
            }
            return Ok(reply);
        }
        if matches!(request, Request::Write { .. }) {
            if self.writes >= 16 {
                self.reply()?;
                self.writes -= 1;
            }
            framing::write(&mut self.input, &request)?;
            self.writes += 1;
            return Ok(Reply::default());
        }
        while self.writes > 0 {
            self.reply()?;
            self.writes -= 1;
        }
        framing::write(&mut self.input, &request)?;
        self.reply()
    }
    fn ready(&mut self) -> Result<()> {
        self.reply().map(|_| ())
    }
    fn reply(&mut self) -> Result<Reply> {
        let result = (|| {
            framing::read::<Result<Reply, String>>(&mut self.output)?
                .context("Transfer channel closed; check host access and reconnect")?
                .map_err(anyhow::Error::msg)
        })();
        result.with_context(|| {
            let diagnostics = self.diagnostics.lock().unwrap();
            let text = String::from_utf8_lossy(&diagnostics);
            if text.trim().is_empty() {
                "File transfer channel".into()
            } else {
                format!("File transfer channel: {}", text.trim())
            }
        })
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        // A cooperative Abort acknowledges stage cleanup before closing SSH.
        // Use an independent, short cleanup budget after user cancellation.
        let deadline = Instant::now() + Duration::from_secs(2);
        self.input.cleanup_until(deadline);
        self.output.cleanup_until(deadline);
        while self.writes > 0 {
            if self.reply().is_err() {
                break;
            }
            self.writes -= 1;
        }
        if framing::write(&mut self.input, &Request::Abort).is_ok() {
            let _ = self.reply();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
