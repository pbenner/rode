//! Core bounded JSON-line process transport shared by agents and notebook kernels.
//! Launches always retain the project host, sandbox and scoped process cleanup.
//!
//! # Design decisions
//!
//! A shared stdio transport gives agents, kernels and coordinators the same
//! bounds and cleanup behavior without requiring a service port or a runtime
//! embedded in the GUI. JSON lines are easy to implement on both hosts; binary
//! payloads use explicit encoding, accepting some overhead for a simple bounded
//! protocol. The protocol client above this layer decides message meaning.
//!
//! Input and event queues are bounded so a slow or verbose child cannot grow
//! memory without limit or block the UI in a pipe write. Backpressure is visible
//! to callers; retrying clients retain their unsent buffers. Separate sessions
//! detach background tools from the launcher's controlling terminal and provide
//! process groups for scoped stop/drop, preserving the SSH authentication master
//! and independent terminal shells. Coordinators handle their own child lifetimes.
use crate::{files, project, sandbox, ssh};
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};

/// Give noninteractive workers an owned group without inheriting the launcher's
/// controlling terminal. A new process group alone still permits `/dev/tty`
/// access across the sandbox boundary. Interactive PTYs establish their own
/// session through portable-pty instead of calling this helper.
pub(crate) fn new_session(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    // The post-fork hook only calls the async-signal-safe setsid syscall and
    // captures errno on failure; it does not allocate or acquire Rust locks.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Launch description for one supervised project-host JSON-line subprocess. Argument vectors
/// are literal; executable validation does not grant sandbox access.
#[derive(Clone)]
pub struct Launch {
    /// Executable and literal arguments, or Python source when `python_program` is set.
    pub argv: Vec<String>,
    /// Prepared project root and actual sandbox command prefix.
    pub plan: sandbox::Plan,
    /// Authenticated SSH transport, or `None` for local execution.
    pub remote: Option<ssh::Remote>,
    /// Selected project-host Python executable for Python programs.
    pub python: String,
    /// Whether to execute `argv[0]` as Python source rather than an executable.
    pub python_program: bool,
    /// Maximum inbound protocol line size in bytes.
    pub line_limit: usize,
    /// Whether the process is the native debugger coordinator rather than an ordinary agent.
    pub native_debug: bool,
}
impl Launch {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.argv.is_empty() && !self.argv[0].is_empty() && self.argv.len() <= 64,
            "Choose an installed process executable."
        );
        ensure!(
            self.argv
                .iter()
                .all(|a| a.len() <= 64 * 1024 && !a.contains('\0'))
                && self.argv.iter().map(String::len).sum::<usize>() <= 128 * 1024,
            "Invalid process arguments."
        );
        ensure!(
            (8192..=16 * 1024 * 1024).contains(&self.line_limit),
            "Invalid process line limit."
        );
        Ok(())
    }
}
#[derive(Serialize)]
struct RemoteLaunch {
    root: String,
    prefix: Vec<String>,
    argv: Vec<String>,
    python: String,
    python_program: bool,
}

pub use rode_agent_core::Event;

#[derive(Default)]
struct Control {
    pid: AtomicU32,
    alive: AtomicBool,
    stopped: AtomicBool,
    finished: AtomicBool,
}
impl Control {
    fn kill(&self, signal: i32) {
        let pid = self.pid.load(Ordering::Acquire);
        if pid != 0 && self.alive.load(Ordering::Acquire) {
            // Each process owns a fresh group. This never targets the project
            // terminal or the shared OpenSSH authentication master.
            unsafe {
                libc::kill(-(pid as i32), signal);
            }
        }
    }
    fn stop(self: &Arc<Self>) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.kill(libc::SIGTERM);
        let control = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            control.kill(libc::SIGKILL);
        });
    }
}

/// Owned subprocess transport. Workers handle I/O and wake the UI; dropping requests scoped
/// termination without killing the SSH master or project terminals.
pub struct Session {
    input: SyncSender<Vec<u8>>,
    line_limit: usize,
    events: Receiver<Event>,
    control: Arc<Control>,
}
/// Cloneable handle to the bounded, nonblocking process-input queue.
#[derive(Clone)]
pub struct Writer {
    input: SyncSender<Vec<u8>>,
    limit: usize,
}
pub(crate) fn encode_request(message: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(message)?;
    ensure!(
        bytes.len() < 16 * 1024 * 1024,
        "Process request exceeds 16 MiB."
    );
    bytes.push(b'\n');
    Ok(bytes)
}
impl Writer {
    /// Queue JSON without blocking; `false` means backpressure and the caller must retry.
    pub fn try_send(&self, message: &Value) -> Result<bool> {
        self.try_send_buffer(&mut encode_request(message)?)
    }
    /// Retain the already encoded buffer when the bounded pipe is full.
    pub(crate) fn try_send_buffer(&self, bytes: &mut Vec<u8>) -> Result<bool> {
        ensure!(
            bytes.len() <= self.limit.min(16 * 1024 * 1024),
            "Process request exceeds its line limit."
        );
        match self.input.try_send(std::mem::take(bytes)) {
            Ok(()) => Ok(true),
            Err(mpsc::TrySendError::Full(buffer)) => {
                *bytes = buffer;
                Ok(false)
            }
            Err(mpsc::TrySendError::Disconnected(_)) => bail!("Process input disconnected."),
        }
    }
    /// Queue JSON, returning an error if the bounded queue is full or disconnected.
    pub fn send(&self, message: &Value) -> Result<()> {
        ensure!(
            self.try_send(message)?,
            "Process input is full; retry when it drains."
        );
        Ok(())
    }
}
impl Session {
    /// Transfer ownership to a headless notebook/debugger controller.
    pub fn into_runtime(self) -> rode_runtime_core::process::Session {
        rode_runtime_core::process::Session::new(self)
    }
    /// Clone an input-only handle without transferring process ownership.
    pub fn writer(&self) -> Writer {
        Writer {
            input: self.input.clone(),
            limit: self.line_limit,
        }
    }
    /// Validate and supervise a process off-thread. Returns before launch completes; observe
    /// [`Event`] for failures. `wake` runs on worker threads and must not directly mutate GUI
    /// state.
    pub fn start(launch: Launch, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        launch.validate()?;
        ensure!(
            !launch.plan.root.is_empty(),
            "The process working folder is empty."
        );
        ensure!(
            !launch.plan.active || !launch.plan.prefix.is_empty(),
            "Active sandbox has no launch plan; reopen the project."
        );
        let line_limit = launch.line_limit;
        let (input, input_rx) = mpsc::sync_channel::<Vec<u8>>(32);
        let (output, events) = mpsc::sync_channel::<Event>(16);
        let control = Arc::new(Control::default());
        let run_control = control.clone();
        std::thread::spawn(move || {
            let result = run_session(
                launch,
                input_rx,
                output.clone(),
                run_control.clone(),
                wake.clone(),
            );
            if let Err(error) = result {
                emit(&output, &wake, Event::Error(format!("{error:#}")));
            }
            run_control.alive.store(false, Ordering::Release);
            run_control.finished.store(true, Ordering::Release);
            wake();
        });
        Ok(Self {
            line_limit,
            input,
            events,
            control,
        })
    }
    /// Queue one protocol message through the shared writer.
    pub fn send(&self, message: &Value) -> Result<()> {
        self.writer().send(message)
    }
    pub(crate) fn is_finished(&self) -> bool {
        self.control.finished.load(Ordering::Acquire)
    }
    /// Take the next available event without waiting.
    pub fn poll(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
    /// Request idempotent SIGTERM followed by timed SIGKILL for the owned process group.
    pub fn stop(&self) {
        self.control.stop();
    }
}
impl rode_runtime_core::process::Sink for Writer {
    fn try_send_buffer(&self, bytes: &mut Vec<u8>) -> Result<bool> {
        self.try_send_buffer(bytes)
    }
}
impl rode_runtime_core::process::Transport for Session {
    fn writer(&self) -> rode_runtime_core::process::Writer {
        rode_runtime_core::process::Writer::new(self.writer())
    }
    fn poll(&self) -> Option<rode_runtime_core::process::Event> {
        use rode_runtime_core::process::Event as Runtime;
        self.poll().map(|event| match event {
            Event::Started => Runtime::Started,
            Event::Message(v) => Runtime::Message(v),
            Event::Stderr(v) => Runtime::Stderr(v),
            Event::Error(v) => Runtime::Error(v),
            Event::Exited(v) => Runtime::Exited(v),
        })
    }
    fn stop(&self) {
        self.stop();
    }
    fn is_finished(&self) -> bool {
        self.is_finished()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

fn emit(output: &SyncSender<Event>, wake: &Arc<dyn Fn() + Send + Sync>, event: Event) -> bool {
    // Repaint before sending as well: a full bounded queue needs the consumer
    // to wake before the producer can finish its next send.
    wake();
    let sent = output.send(event).is_ok();
    wake();
    sent
}

fn run_session(
    launch: Launch,
    input: Receiver<Vec<u8>>,
    output: SyncSender<Event>,
    control: Arc<Control>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> Result<()> {
    if control.stopped.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut argv = launch.argv.clone();
    let mut launch = launch;
    if argv[0] == crate::bundled::AUTO_CODEX {
        let (program, prefix) =
            crate::bundled::prepare(&launch.plan, launch.remote.as_ref(), &control.stopped)?;
        argv[0] = program;
        launch.plan.prefix = prefix;
    }
    let mut helper_binary = None;
    let (mut command, bootstrap) = if let Some(remote) = &launch.remote {
        ensure!(
            remote.connected(),
            "SSH is offline. Reconnect before starting an process."
        );
        let mut command = Command::new("ssh");
        command.args(remote.channel_args(false));
        command.arg(format!(
            "python3 -u -c {}",
            ssh::quote(include_str!("remote_agent.py"))
        ));
        let mut data = serde_json::to_vec(&RemoteLaunch {
            root: launch.plan.root.clone(),
            prefix: if launch.plan.active {
                launch.plan.prefix.clone()
            } else {
                vec![]
            },
            argv,
            python: launch.python.clone(),
            python_program: launch.python_program,
        })?;
        ensure!(
            data.len() < 256 * 1024,
            "Process launch request exceeds 256 KiB."
        );
        data.push(b'\n');
        (command, Some(data))
    } else if launch.native_debug {
        let (command, helper) = launch.plan.coordinator_command("--debug-helper")?;
        helper_binary = helper;
        (command, None)
    } else {
        let root = files::expand_home(&launch.plan.root)
            .canonicalize()
            .context("Cannot open the process working folder")?;
        let mut argv = argv;
        argv[0] = if launch.python_program {
            project::python_path(&root, &launch.python)
        } else {
            resolve_program(&root, &argv[0])
        }
        .to_string_lossy()
        .into();
        let mut command = if launch.plan.active {
            let mut command = Command::new(&launch.plan.prefix[0]);
            command.args(&launch.plan.prefix[1..]).args(&argv);
            command
        } else {
            let mut command = Command::new(&argv[0]);
            command.args(&argv[1..]);
            command
        };
        let python = project::python_path(&root, &launch.python);
        command
            .current_dir(&root)
            .envs(project::environment_variables(&python.to_string_lossy()));
        (command, None)
    };
    new_session(&mut command);
    command.env("NO_COLOR", "1");
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let until = Instant::now() + Duration::from_secs(1);
    let mut child = loop {
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(10));
                if control.stopped.load(Ordering::Acquire) {
                    return Ok(());
                }
            }
            result => break result,
        }
    }
    .context("Cannot start process. Check its executable and sandbox access on the project host")?;
    control.pid.store(child.id(), Ordering::Release);
    control.alive.store(true, Ordering::Release);
    if control.stopped.load(Ordering::Acquire) {
        control.kill(libc::SIGTERM);
    }
    let mut stdin = child.stdin.take().context("Process stdin is missing")?;
    std::thread::spawn(move || {
        if let Some(bootstrap) = bootstrap
            && stdin.write_all(&bootstrap).is_err()
        {
            return;
        }
        for data in input {
            if stdin.write_all(&data).and_then(|_| stdin.flush()).is_err() {
                break;
            }
        }
    });
    let stdout = child.stdout.take().context("Process stdout is missing")?;
    let stderr = child.stderr.take().context("Process stderr is missing")?;
    let mut readers = vec![];
    for (reader, rpc) in [
        (Box::new(stdout) as Box<dyn std::io::Read + Send>, true),
        (Box::new(stderr) as Box<dyn std::io::Read + Send>, false),
    ] {
        let output = output.clone();
        let wake = wake.clone();
        let line_limit = launch.line_limit;
        let control = control.clone();
        readers.push(std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            loop {
                match read_line(&mut reader, if rpc { line_limit } else { 8192 }) {
                    Ok(Some(line)) => {
                        let event = if rpc {
                            if line.iter().all(u8::is_ascii_whitespace) { continue; }
                            match serde_json::from_slice(&line) {
                                Ok(value) => Event::Message(value),
                                Err(_) => { emit(&output, &wake, Event::Error("Process stdout is not JSON-RPC. Check the selected protocol and arguments.".into())); control.stop(); return; }
                            }
                        } else { Event::Stderr(String::from_utf8_lossy(&line).trim_end().to_owned()) };
                        if !emit(&output, &wake, event) { return; }
                    }
                    Ok(None) => return,
                    Err(error) => { emit(&output, &wake, Event::Error(error.to_string())); control.stop(); return; }
                }
            }
        }));
    }
    emit(&output, &wake, Event::Started);
    let status = child.wait()?;
    // Kill any descendants that outlive an process which exits unexpectedly.
    control.kill(libc::SIGKILL);
    control.alive.store(false, Ordering::Release);
    // Preserve the stream's final messages before publishing process exit.
    for reader in readers {
        let _ = reader.join();
    }
    emit(&output, &wake, Event::Exited(status.code()));
    drop(helper_binary);
    Ok(())
}

pub(crate) fn resolve_program(root: &std::path::Path, program: &str) -> PathBuf {
    if program.contains('/') || program.starts_with('~') {
        let path = files::expand_home(program);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    } else {
        sandbox::find_program(program)
            .or_else(|| {
                let local = files::expand_home("~/.local/bin").join(program);
                local.is_file().then_some(local)
            })
            .unwrap_or_else(|| PathBuf::from(program))
    }
}

fn read_line(reader: &mut impl BufRead, limit: usize) -> Result<Option<Vec<u8>>> {
    let mut result = Vec::new();
    loop {
        let data = reader.fill_buf()?;
        if data.is_empty() {
            return Ok((!result.is_empty()).then_some(result));
        }
        let len = data
            .iter()
            .position(|b| *b == b'\n')
            .map_or(data.len(), |p| p + 1);
        if result.len() + len > limit {
            bail!("Process output line exceeds its size limit; session stopped.");
        }
        result.extend_from_slice(&data[..len]);
        reader.consume(len);
        if result.last() == Some(&b'\n') {
            return Ok(Some(result));
        }
    }
}

#[cfg(test)]
mod writer_tests {
    use super::*;
    #[test]
    fn full_pipe_preserves_the_encoded_request_for_retry() {
        let (input, receiver) = mpsc::sync_channel(1);
        let writer = Writer { input, limit: 8192 };
        assert!(writer.try_send(&serde_json::json!({"first":1})).unwrap());
        let mut bytes = encode_request(&serde_json::json!({"next":"λ"})).unwrap();
        let pointer = bytes.as_ptr();
        assert!(!writer.try_send_buffer(&mut bytes).unwrap());
        assert_eq!(bytes.as_ptr(), pointer);
        let _ = receiver.recv().unwrap();
        assert!(writer.try_send_buffer(&mut bytes).unwrap());
        assert!(bytes.is_empty());
        let value: Value = serde_json::from_slice(&receiver.recv().unwrap()).unwrap();
        assert_eq!(value["next"], "λ");
    }
}
