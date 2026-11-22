//! Deterministic PTY ownership without a host process or terminal service.
//! Real egui/input tests use the production backend variants and observe their
//! queued wire bytes and signal counts rather than changing transport flags.
use super::*;
use std::sync::atomic::AtomicUsize;

struct Master(Mutex<PtySize>);
impl MasterPty for Master {
    fn resize(&self, size: PtySize) -> Result<()> {
        *self.0.lock().unwrap() = size;
        Ok(())
    }
    fn get_size(&self) -> Result<PtySize> {
        Ok(*self.0.lock().unwrap())
    }
    fn try_clone_reader(&self) -> Result<Box<dyn Read + Send>> {
        Ok(Box::new(std::io::empty()))
    }
    fn take_writer(&self) -> Result<Box<dyn Write + Send>> {
        Ok(Box::new(std::io::sink()))
    }
    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<libc::pid_t> {
        None
    }
    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::fd::RawFd> {
        None
    }
    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf> {
        None
    }
}

#[derive(Debug)]
struct Signals(Arc<AtomicUsize>);
impl portable_pty::ChildKiller for Signals {
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(Self(self.0.clone()))
    }
}

pub(super) fn process(
    ctx: &egui::Context,
    title: String,
    protocol: Protocol,
) -> (Terminal, Stream, mpsc::Receiver<Vec<u8>>, Arc<AtomicUsize>) {
    let (mut terminal, stream) = Terminal::stream(ctx, title, |_| {});
    let (input, received) = mpsc::sync_channel(256);
    let signals = Arc::new(AtomicUsize::new(0));
    terminal.backend = Backend::process(
        Process::new(
            Box::new(Master(Mutex::new(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            }))),
            Box::new(Signals(signals.clone())),
        ),
        input,
        protocol,
    );
    (terminal, stream, received, signals)
}

impl Terminal {
    /// Native-only controls in layout tests, without starting a host service.
    pub(crate) fn fixture_native(ctx: &egui::Context, title: String) -> (Self, Stream) {
        Self::fixture_native_with_persistence(ctx, title, false)
    }
    pub(crate) fn fixture_native_with_persistence(
        ctx: &egui::Context,
        title: String,
        persistent: bool,
    ) -> (Self, Stream) {
        let (terminal, stream, received, _) = process(ctx, title, Protocol::framed(persistent));
        std::thread::spawn(move || while received.recv().is_ok() {});
        (terminal, stream)
    }
}
