//! Bounded transports injected into host-owned runtime controllers.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Process events shared with desktop adapters.
#[derive(Debug)]
pub enum Event {
    /// Process started.
    Started,
    /// One complete protocol message.
    Message(Value),
    /// Bounded diagnostic line.
    Stderr(String),
    /// Transport failure.
    Error(String),
    /// Process exit status.
    Exited(Option<i32>),
}
/// Nonblocking message sink, shared by terminal input and protocol controllers.
pub trait Sink: Send + Sync {
    /// Return false on temporary backpressure, preserving the caller's bytes.
    fn try_send_buffer(&self, bytes: &mut Vec<u8>) -> Result<bool>;
}
/// Cloneable bounded process-input handle.
#[derive(Clone)]
pub struct Writer(Arc<dyn Sink>);
impl Writer {
    /// Adopt an adapter's bounded sink.
    pub fn new(sink: impl Sink + 'static) -> Self {
        Self(Arc::new(sink))
    }
    /// Queue an encoded message, retaining bytes on backpressure.
    pub fn try_send_buffer(&self, bytes: &mut Vec<u8>) -> Result<bool> {
        self.0.try_send_buffer(bytes)
    }
    /// Queue one JSON message without blocking.
    pub fn try_send(&self, message: &Value) -> Result<bool> {
        self.try_send_buffer(&mut encode_request(message)?)
    }
    /// Queue or report temporary backpressure.
    pub fn send(&self, message: &Value) -> Result<()> {
        ensure!(
            self.try_send(message)?,
            "Runtime input queue is full; retry when it drains"
        );
        Ok(())
    }
}
/// Owned transport with explicit scoped shutdown.
pub trait Transport: Send {
    /// Clone the input handle.
    fn writer(&self) -> Writer;
    /// Poll one bounded event.
    fn poll(&self) -> Option<Event>;
    /// Stop only this process tree.
    fn stop(&self);
    /// Whether the supervisor has reaped the process.
    fn is_finished(&self) -> bool;
}
/// Connection owned by a notebook or debugger controller.
pub struct Session(Box<dyn Transport>);
impl Session {
    /// Adopt a prepared transport.
    pub fn new(transport: impl Transport + 'static) -> Self {
        Self(Box::new(transport))
    }
    /// Clone its input handle.
    pub fn writer(&self) -> Writer {
        self.0.writer()
    }
    /// Queue one message.
    pub fn send(&self, message: &Value) -> Result<()> {
        self.writer().send(message)
    }
    /// Poll without blocking.
    pub fn poll(&self) -> Option<Event> {
        self.0.poll()
    }
    /// Request process-tree cleanup.
    pub fn stop(&self) {
        self.0.stop();
    }
    /// Whether cleanup has finished.
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}
/// Encode bounded JSON-line input.
pub fn encode_request(value: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() < 4 * 1024 * 1024,
        "Runtime process input exceeds 4 MiB"
    );
    bytes.push(b'\n');
    Ok(bytes)
}
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
struct Input(mpsc::SyncSender<Vec<u8>>);
impl Sink for Input {
    fn try_send_buffer(&self, bytes: &mut Vec<u8>) -> Result<bool> {
        ensure!(
            bytes.len() <= 4 * 1024 * 1024,
            "Runtime process input exceeds 4 MiB"
        );
        match self.0.try_send(std::mem::take(bytes)) {
            Ok(()) => Ok(true),
            Err(mpsc::TrySendError::Full(value)) => {
                *bytes = value;
                Ok(false)
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                anyhow::bail!("Runtime process input disconnected")
            }
        }
    }
}
struct Pipe {
    writer: Writer,
    events: mpsc::Receiver<Event>,
    control: Arc<Control>,
}
impl Transport for Pipe {
    fn writer(&self) -> Writer {
        self.writer.clone()
    }
    fn poll(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
    fn stop(&self) {
        self.control.stop();
    }
    fn is_finished(&self) -> bool {
        self.control.finished.load(Ordering::Acquire)
    }
}
/// Spawn on the host. The spawning thread stays alive for the full sandbox lifetime.
pub fn spawn(mut command: Command) -> Result<Session> {
    use std::os::unix::process::CommandExt;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let (input, incoming) = mpsc::sync_channel::<Vec<u8>>(32);
    let (output, events) = mpsc::sync_channel(32);
    let control = Arc::new(Control {
        pid: AtomicU32::new(0),
        alive: AtomicBool::new(false),
        stopped: AtomicBool::new(false),
        finished: AtomicBool::new(false),
    });
    let pipe = Pipe {
        writer: Writer::new(Input(input)),
        events,
        control: control.clone(),
    };
    std::thread::spawn(move || {
        let result = (|| -> Result<()> {
            if control.stopped.load(Ordering::Acquire) {
                return Ok(());
            }
            let mut child = command.spawn().context("Cannot start host runtime")?;
            control.pid.store(child.id(), Ordering::Release);
            control.alive.store(true, Ordering::Release);
            if control.stopped.load(Ordering::Acquire) {
                control.kill(libc::SIGTERM);
            }
            let mut stdin = child.stdin.take().context("Missing runtime input")?;
            std::thread::spawn(move || {
                for data in incoming {
                    if stdin.write_all(&data).and_then(|_| stdin.flush()).is_err() {
                        break;
                    }
                }
            });
            let mut readers = Vec::new();
            for (reader, rpc) in [
                (
                    Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
                    true,
                ),
                (
                    Box::new(child.stderr.take().unwrap()) as Box<dyn Read + Send>,
                    false,
                ),
            ] {
                let output = output.clone();
                let control = control.clone();
                readers.push(std::thread::spawn(move || {
                    let mut reader = BufReader::new(reader);
                    loop {
                        let limit = if rpc { 2 * 1024 * 1024 } else { 8192 };
                        let mut bytes = Vec::new();
                        let event =
                            match (&mut reader).take(limit + 1).read_until(b'\n', &mut bytes) {
                                Ok(0) => break,
                                Ok(_) if bytes.len() as u64 > limit => {
                                    control.stop();
                                    Event::Error("Runtime output exceeded its line limit".into())
                                }
                                Ok(_) if rpc => {
                                    if bytes.iter().all(u8::is_ascii_whitespace) {
                                        continue;
                                    }
                                    match serde_json::from_slice(&bytes) {
                                        Ok(value) => Event::Message(value),
                                        Err(error) => {
                                            control.stop();
                                            Event::Error(format!("Invalid runtime output: {error}"))
                                        }
                                    }
                                }
                                Ok(_) => {
                                    Event::Stderr(String::from_utf8_lossy(&bytes).trim_end().into())
                                }
                                Err(error) => {
                                    control.stop();
                                    Event::Error(error.to_string())
                                }
                            };
                        if output.send(event).is_err() {
                            break;
                        }
                    }
                }));
            }
            let _ = output.send(Event::Started);
            let status = child.wait()?;
            control.kill(libc::SIGKILL);
            control.alive.store(false, Ordering::Release);
            for reader in readers {
                let _ = reader.join();
            }
            let _ = output.send(Event::Exited(status.code()));
            Ok(())
        })();
        if let Err(error) = result {
            let _ = output.send(Event::Error(format!("{error:#}")));
        }
        control.alive.store(false, Ordering::Release);
        control.finished.store(true, Ordering::Release);
    });
    Ok(Session::new(pipe))
}
