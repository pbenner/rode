//! Persistent desktop Hayro worker. Raw RGBA frames avoid codec round trips;
//! a private source snapshot, pipe bounds and a killable process retain the
//! preview's cancellation, memory and deadline guarantees for malformed PDFs.
use super::*;
use anyhow::bail;
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    process::{Child, Stdio},
    time::Instant,
};

const HEADER_LIMIT: usize = 512 * 1024;
const DEADLINE: Duration = Duration::from_secs(20);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Header {
    Document { sizes: Vec<[f32; 2]> },
    Page { width: usize, height: usize },
    Text { bytes: usize },
    Error { message: String },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Request {
    Render { page: usize, side: usize },
    Text,
}

pub(super) struct Renderer {
    child: Child,
    stream: UnixStream,
}

/// Linux passes a sealed anonymous memory file, avoiding plaintext temporary
/// filesystem snapshots. The descriptor is released once the child acknowledges parsing it.
enum Snapshot {
    #[cfg(target_os = "linux")]
    Memory(std::fs::File),
    #[cfg(not(target_os = "linux"))]
    Private(tempfile::NamedTempFile),
}
impl Snapshot {
    fn new(bytes: &[u8]) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: the name is NUL terminated; a successful descriptor is
            // immediately transferred to its sole File owner.
            let fd = unsafe {
                libc::memfd_create(
                    c"rode-pdf".as_ptr(),
                    libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
                )
            };
            ensure!(
                fd >= 0,
                "Cannot create PDF memory snapshot: {}",
                std::io::Error::last_os_error()
            );
            let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
            file.write_all(bytes)?;
            // SAFETY: File owns a valid memfd, and this command accepts an
            // integer mask. Sealing makes the byte snapshot immutable.
            let sealed = unsafe {
                libc::fcntl(
                    file.as_raw_fd(),
                    libc::F_ADD_SEALS,
                    libc::F_SEAL_WRITE
                        | libc::F_SEAL_GROW
                        | libc::F_SEAL_SHRINK
                        | libc::F_SEAL_SEAL,
                )
            };
            ensure!(
                sealed == 0,
                "Cannot seal PDF memory snapshot: {}",
                std::io::Error::last_os_error()
            );
            Ok(Self::Memory(file))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let mut file = tempfile::Builder::new().prefix("rode-pdf-").tempfile()?;
            file.write_all(bytes)?;
            file.flush()?;
            Ok(Self::Private(file))
        }
    }
    fn path(&self) -> std::path::PathBuf {
        match self {
            #[cfg(target_os = "linux")]
            Self::Memory(file) => {
                format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd()).into()
            }
            #[cfg(not(target_os = "linux"))]
            Self::Private(file) => file.path().to_path_buf(),
        }
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        // Child owns the unreaped PID; no renderer descendant processes exist.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Renderer {
    pub(super) fn open(bytes: &[u8], cancel: &AtomicBool) -> Result<(Self, Vec<[f32; 2]>)> {
        ensure!(!cancel.load(Ordering::Acquire), "PDF rendering cancelled.");
        let snapshot = Snapshot::new(bytes)?;
        let (stream, child_stream) = UnixStream::pair()?;
        stream.set_read_timeout(Some(Duration::from_millis(50)))?;
        stream.set_write_timeout(Some(Duration::from_millis(50)))?;
        let stdin: OwnedFd = child_stream.try_clone()?.into();
        let stdout: OwnedFd = child_stream.into();
        let executable = desktop_executable()?;
        let mut command = Command::new(executable);
        let path = snapshot.path();
        command.args([std::ffi::OsStr::new("--pdf-helper"), path.as_os_str()]);
        command
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::null());
        super::limit_process(&mut command);
        let child = command
            .spawn()
            .context("Cannot start the bundled Rust PDF renderer")?;
        let mut renderer = Self { child, stream };
        let Header::Document { sizes } = renderer.header(cancel, Instant::now())? else {
            bail!("PDF renderer did not return document metadata")
        };
        validate_sizes(&sizes)?;
        Ok((renderer, sizes))
    }

    fn request(&mut self, request: Request) -> Result<()> {
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        self.stream
            .write_all(&bytes)
            .context("PDF renderer input disconnected")?;
        Ok(())
    }
    fn read(&mut self, bytes: &mut [u8], cancel: &AtomicBool, start: Instant) -> Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            ensure!(!cancel.load(Ordering::Acquire), "PDF rendering cancelled.");
            ensure!(
                start.elapsed() < DEADLINE,
                "PDF rendering timed out after 20 seconds."
            );
            match self.stream.read(&mut bytes[offset..]) {
                Ok(0) => bail!(
                    "The Rust PDF renderer stopped. This PDF may exceed the renderer's memory limit or contain unsupported data."
                ),
                Ok(count) => offset += count,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
    fn header(&mut self, cancel: &AtomicBool, start: Instant) -> Result<Header> {
        let mut length = [0; 4];
        self.read(&mut length, cancel, start)?;
        let length = u32::from_be_bytes(length) as usize;
        ensure!(
            (1..=HEADER_LIMIT).contains(&length),
            "Invalid PDF renderer frame size."
        );
        let mut bytes = vec![0; length];
        self.read(&mut bytes, cancel, start)?;
        let header = serde_json::from_slice(&bytes)?;
        if let Header::Error { message } = header {
            bail!("{message}");
        }
        Ok(header)
    }
    pub(super) fn render(
        &mut self,
        page: usize,
        side: usize,
        budget: &Arc<images::Budget>,
        source_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<images::Decoded> {
        let start = Instant::now();
        self.request(Request::Render { page, side })?;
        let Header::Page { width, height } = self.header(cancel, start)? else {
            bail!("Invalid PDF page response")
        };
        ensure!(
            width > 0 && height > 0 && width <= side && height <= side,
            "Invalid PDF raster dimensions."
        );
        let size = width
            .checked_mul(height)
            .and_then(|v| v.checked_mul(4))
            .filter(|v| *v <= images::MAX_DECODED)
            .context("PDF raster exceeds its memory limit")?;
        let lease = Arc::new(budget.reserve(size)?);
        let mut pixels = vec![0; size];
        self.read(&mut pixels, cancel, start)?;
        Ok(images::Decoded {
            pixels: Arc::new(eframe::egui::ColorImage::from_rgba_unmultiplied(
                [width, height],
                &pixels,
            )),
            original: [width as u32, height as u32],
            format: "PDF".into(),
            encoded_bytes: source_bytes,
            lease,
        })
    }
    pub(super) fn text(&mut self, cancel: &AtomicBool) -> Result<String> {
        let start = Instant::now();
        self.request(Request::Text)?;
        let Header::Text { bytes } = self.header(cancel, start)? else {
            bail!("Invalid PDF text response")
        };
        ensure!(
            bytes <= rode_agent_core::attachment::CONTENT_LIMIT,
            "PDF text exceeds the attachment limit."
        );
        let mut text = vec![0; bytes];
        self.read(&mut text, cancel, start)?;
        Ok(String::from_utf8(text)?)
    }
}

fn desktop_executable() -> Result<std::path::PathBuf> {
    let executable = std::env::current_exe()?;
    // Cargo and the CI bundle place test executables in deps/ or tests/.
    // Their sibling editor implements the same private renderer entry point.
    if executable
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n == "deps" || n == "tests")
    {
        return Ok(std::env::var_os("RODE_TEST_BINARY")
            .map(Into::into)
            .unwrap_or_else(|| executable.parent().unwrap().parent().unwrap().join("rode")));
    }
    rode_host_core::command::current_executable()
}

/// Validate in both processes before page geometry reaches rendering or the UI.
/// The lower bound excludes degenerate pages whose scale could overflow.
fn validate_sizes(sizes: &[[f32; 2]]) -> Result<()> {
    ensure!(
        !sizes.is_empty() && sizes.len() <= MAX_PAGES,
        "PDF previews support 1–10,000 pages."
    );
    ensure!(
        sizes
            .iter()
            .flatten()
            .all(|v| v.is_finite() && (0.01..=1_000_000.).contains(v)),
        "Invalid PDF page dimensions."
    );
    Ok(())
}

fn header(output: &mut impl Write, value: &Header) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() <= HEADER_LIMIT,
        "PDF renderer metadata exceeds its limit"
    );
    output.write_all(&(bytes.len() as u32).to_be_bytes())?;
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}

/// Run inside the desktop executable's private helper mode, before GUI startup.
pub(super) fn serve(path: &Path) -> Result<()> {
    // A persistent child outlives the GUI thread that opened its document.
    // PDEATHSIG follows that spawning thread on Linux, so monitor the owning
    // process's socket instead. HUP terminates even a stuck interpreter without
    // consuming commands or requiring another installed supervisor.
    std::thread::spawn(|| {
        let mut descriptor = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: 0,
            revents: 0,
        };
        loop {
            // SAFETY: this is a live, initialized pollfd; poll only observes
            // stdin's lifetime and does not read or mutate the command stream.
            let result = unsafe { libc::poll(&mut descriptor, 1, -1) };
            if result > 0
                && descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
            {
                std::process::exit(0);
            }
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                std::process::exit(1);
            }
        }
    });
    let mut output = std::io::BufWriter::new(std::io::stdout().lock());
    if let Err(error) = serve_document(path, &mut output) {
        header(
            &mut output,
            &Header::Error {
                message: format!("{error:#}"),
            },
        )?;
    }
    Ok(())
}

/// Parse once and retain interpreter/font state for the connection's lifetime.
fn serve_document(path: &Path, mut output: &mut impl Write) -> Result<()> {
    use hayro::{
        hayro_interpret::{InterpreterSettings, util::TransformExt},
        hayro_syntax::Pdf,
        vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit},
    };
    let document = match Pdf::new(std::fs::read(path)?) {
        Ok(document) => document,
        Err(error) => {
            let message = if matches!(error, hayro::hayro_syntax::LoadPdfError::Decryption(_)) {
                "Password-protected PDFs are not supported by this preview."
            } else {
                "Cannot open this PDF: invalid or unsupported document."
            };
            header(
                &mut output,
                &Header::Error {
                    message: message.into(),
                },
            )?;
            return Ok(());
        }
    };
    let pages = document.pages();
    ensure!(
        !pages.is_empty() && pages.len() <= MAX_PAGES,
        "PDF previews support 1–10,000 pages."
    );
    let sizes: Vec<_> = pages
        .iter()
        .map(|p| {
            let (w, h) = p.render_dimensions();
            [w, h]
        })
        .collect();
    validate_sizes(&sizes)?;
    header(
        &mut output,
        &Header::Document {
            sizes: sizes.clone(),
        },
    )?;
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut cache = hayro::RenderCache::new();
    let mut renders = 0;
    loop {
        let mut line = Vec::new();
        let count = input.by_ref().take(4097).read_until(b'\n', &mut line)?;
        if count == 0 {
            return Ok(());
        }
        ensure!(
            count <= 4096 && line.ends_with(b"\n"),
            "Invalid PDF renderer request"
        );
        let request: Request = serde_json::from_slice(&line)?;
        match request {
            Request::Render { page, side } => {
                ensure!(
                    (1..=pages.len()).contains(&page) && (1..=4096).contains(&side),
                    "Invalid PDF page request"
                );
                if renders == 16 {
                    cache = hayro::RenderCache::new();
                    renders = 0;
                }
                let page = &pages[page - 1];
                let (width, height) = page.render_dimensions();
                let scale = side as f32 / width.max(height);
                let w = (width * scale).ceil().clamp(1., side as f32) as u16;
                let h = (height * scale).ceil().clamp(1., side as f32) as u16;
                let mut context = RenderContext::new(w, h);
                hayro::render_into(
                    page,
                    &cache,
                    &InterpreterSettings::default(),
                    &hayro::RenderSettings::default(),
                    &mut context,
                    hayro::kurbo::Affine::scale(scale as f64)
                        * page.initial_transform(true).to_kurbo(),
                );
                context.flush();
                let mut pixels = Pixmap::new(w, h);
                context.render_with(
                    &mut pixels,
                    &mut Resources::default(),
                    RasterizerSettings {
                        target_init: TargetInit::Clear(
                            hayro::vello_cpu::color::palette::css::WHITE,
                        ),
                        ..Default::default()
                    },
                );
                header(
                    &mut output,
                    &Header::Page {
                        width: usize::from(w),
                        height: usize::from(h),
                    },
                )?;
                output.write_all(pixels.data_as_u8_slice())?;
                output.flush()?;
                renders += 1;
            }
            Request::Text => {
                let text = super::text::extract(&document)?;
                header(&mut output, &Header::Text { bytes: text.len() })?;
                output.write_all(text.as_bytes())?;
                output.flush()?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mock() -> (Renderer, UnixStream) {
        let (stream, peer) = UnixStream::pair().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        (Renderer { child, stream }, peer)
    }
    #[test]
    fn page_geometry_rejects_nonfinite_and_degenerate_sizes_before_layout() {
        for size in [
            [0., 500.],
            [f32::NAN, 500.],
            [f32::INFINITY, 500.],
            [1e-40, 1e-40],
            [1_000_001., 500.],
        ] {
            assert!(validate_sizes(&[size]).is_err());
        }
        assert!(validate_sizes(&[]).is_err());
        assert!(validate_sizes(&[[400., 500.], [500., 400.]]).is_ok());
    }
    #[test]
    fn frames_reject_oversized_headers_and_cancel_silent_workers_without_leaking_children() {
        let (mut renderer, mut peer) = mock();
        peer.write_all(&((HEADER_LIMIT + 1) as u32).to_be_bytes())
            .unwrap();
        assert!(
            renderer
                .header(&AtomicBool::new(false), Instant::now())
                .unwrap_err()
                .to_string()
                .contains("frame size")
        );
        let pid = renderer.child.id();
        drop(renderer);
        // Drop owns/reaps this child, so its PID must no longer be waitable.
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) },
            -1
        );

        let (mut renderer, _peer) = mock();
        let cancelled = Arc::new(AtomicBool::new(false));
        let signal = cancelled.clone();
        let worker = std::thread::spawn(move || {
            renderer
                .header(&cancelled, Instant::now())
                .unwrap_err()
                .to_string()
        });
        signal.store(true, Ordering::Release);
        assert!(worker.join().unwrap().contains("cancelled"));
    }
}
