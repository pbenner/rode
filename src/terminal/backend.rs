//! Valid terminal transports and their cleanup ownership. A retained output view
//! has no input or process handle; a debugger stream never owns its coordinator's
//! PTY, and a native attachment owns its client process rather than the host job.
use super::*;

/// A PTY master and the matching child signal handle are always owned together.
pub(super) struct Process {
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}
impl Process {
    pub fn new(
        master: Box<dyn MasterPty + Send>,
        killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    ) -> Self {
        Self { master, killer }
    }
}

/// Protocol selected once at startup and shared with the attachment reader.
/// Only framed streams carry host-acknowledged persistence state.
#[derive(Clone)]
pub(super) enum Protocol {
    Raw,
    Framed(Arc<AtomicBool>),
}
impl Protocol {
    pub fn framed(persistent: bool) -> Self {
        Self::Framed(Arc::new(AtomicBool::new(persistent)))
    }
    pub fn is_native(&self) -> bool {
        matches!(self, Self::Framed(_))
    }
}

/// Each variant contains exactly the resources its transport can operate on.
pub(super) enum Backend {
    Pty {
        process: Process,
        input: mpsc::SyncSender<Vec<u8>>,
    },
    Native {
        process: Process,
        input: mpsc::SyncSender<Vec<u8>>,
        persistent: Arc<AtomicBool>,
    },
    Debugger {
        input: mpsc::SyncSender<Vec<u8>>,
        control: Box<dyn Fn(StreamControl) + Send + Sync>,
    },
    Output,
}
impl Backend {
    pub fn process(process: Process, input: mpsc::SyncSender<Vec<u8>>, protocol: Protocol) -> Self {
        match protocol {
            Protocol::Raw => Self::Pty { process, input },
            Protocol::Framed(persistent) => Self::Native {
                process,
                input,
                persistent,
            },
        }
    }
    pub fn is_native(&self) -> bool {
        matches!(self, Self::Native { .. })
    }
    pub fn persistent(&self) -> bool {
        matches!(self, Self::Native { persistent, .. } if persistent.load(Ordering::Acquire))
    }
    pub fn send(&self, bytes: Vec<u8>) -> Result<()> {
        if self.is_native() {
            self.send_frame(b'I', &bytes)
        } else {
            self.queue(bytes)
        }
    }
    fn send_frame(&self, kind: u8, bytes: &[u8]) -> Result<()> {
        let mut frame = Vec::with_capacity(bytes.len() + 5);
        rode_session::write_stream(&mut frame, kind, bytes)?;
        self.queue(frame)
    }
    fn queue(&self, bytes: Vec<u8>) -> Result<()> {
        let input = match self {
            Self::Pty { input, .. } | Self::Native { input, .. } | Self::Debugger { input, .. } => {
                input
            }
            Self::Output => anyhow::bail!("Terminal session has ended."),
        };
        input.try_send(bytes).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => anyhow::anyhow!("Terminal input queue is full. Input was not sent; try again when the shell responds."),
            mpsc::TrySendError::Disconnected(_) => anyhow::anyhow!("Terminal session has ended."),
        })
    }
    /// Native size changes share the ordered input queue and await the host's
    /// parser-size acknowledgement; other views resize their local parser.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        match self {
            Self::Native { .. } => {
                self.send_frame(b'R', &[rows.to_be_bytes(), cols.to_be_bytes()].concat())
            }
            Self::Pty { process, .. } => process.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            }),
            Self::Debugger { control, .. } => {
                control(StreamControl::Resize(rows, cols));
                Ok(())
            }
            Self::Output => Ok(()),
        }
    }
    /// Stop only the owned attachment/process, or notify the debugger owner.
    /// A finished PTY's numeric signal handle must never be used after reaping.
    pub fn close(&mut self, alive: bool) {
        match self {
            Self::Pty { process, .. } | Self::Native { process, .. } if alive => {
                let _ = process.killer.kill();
            }
            Self::Debugger { control, .. } => control(StreamControl::End),
            _ => {}
        }
    }
}
