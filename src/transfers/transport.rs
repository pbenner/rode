//! Dedicated channels share authentication, but never occupy the document RPC
//! writer. Pipe polling bounds stalled peers and observes per-job cancellation.
use anyhow::{Context, Result, ensure};
use rode_host_protocol::{
    framing,
    transfer::{Reply, Request},
};
use std::{
    io::{self, Read, Write},
    os::fd::AsRawFd,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Immutable destination captured before a task leaves the UI thread.
#[derive(Clone)]
pub(crate) struct Endpoint {
    pub remote: Option<crate::ssh::Remote>,
    pub plan: Option<crate::sandbox::Plan>,
}
impl Endpoint {
    pub fn local() -> Self {
        Self {
            remote: None,
            plan: None,
        }
    }
    pub fn open(&self, cancel: Arc<AtomicBool>) -> Result<Client> {
        if let Some(remote) = &self.remote {
            ensure!(remote.connected(), "SSH disconnected");
            let plan = self
                .plan
                .as_ref()
                .context("Missing captured project policy")?;
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
            ensure!(self.plan.is_none(), "Local transfer scope must be explicit");
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
        )?;
        let output = Pipe::new(
            child.stdout.take().context("Missing transfer output")?,
            cancel,
        )?;
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
        for pipe in [&mut self.input.cancel, &mut self.output.cancel] {
            *pipe = Arc::default();
        }
        self.input.timeout = Duration::from_secs(2);
        self.output.timeout = Duration::from_secs(2);
        self.input.touched = Instant::now();
        self.output.touched = Instant::now();
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
struct Pipe<T> {
    inner: T,
    cancel: Arc<AtomicBool>,
    touched: Instant,
    timeout: Duration,
}
impl<T: AsRawFd> Pipe<T> {
    fn new(inner: T, cancel: Arc<AtomicBool>) -> io::Result<Self> {
        let fd = inner.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            inner,
            cancel,
            touched: Instant::now(),
            timeout: Duration::from_secs(60),
        })
    }
    fn ready(&mut self, events: libc::c_short) -> io::Result<()> {
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "Transfer cancelled",
                ));
            }
            if self.touched.elapsed() > self.timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Transfer channel stalled",
                ));
            }
            let mut descriptor = libc::pollfd {
                fd: self.inner.as_raw_fd(),
                events,
                revents: 0,
            };
            match unsafe { libc::poll(&mut descriptor, 1, 100) } {
                -1 => {
                    let e = io::Error::last_os_error();
                    if e.kind() != io::ErrorKind::Interrupted {
                        return Err(e);
                    }
                }
                0 => {}
                _ => return Ok(()),
            }
        }
    }
}
impl<T: Read + AsRawFd> Read for Pipe<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.ready(libc::POLLIN)?;
            match self.inner.read(bytes) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                result => {
                    if matches!(result,Ok(n) if n>0) {
                        self.touched = Instant::now();
                    }
                    return result;
                }
            }
        }
    }
}
impl<T: Write + AsRawFd> Write for Pipe<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            self.ready(libc::POLLOUT)?;
            match self.inner.write(bytes) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                result => {
                    if matches!(result,Ok(n) if n>0) {
                        self.touched = Instant::now();
                    }
                    return result;
                }
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
