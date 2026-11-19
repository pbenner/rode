//! Persistent desktop PDF/PowerPoint worker. Raw RGBA frames avoid transport codec round trips;
//! a private source snapshot, pipe bounds and a killable process retain the
//! preview's cancellation, memory and deadline guarantees for malformed documents.
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
    Document {
        sizes: Vec<[f32; 2]>,
    },
    Page {
        width: usize,
        height: usize,
        #[serde(default)]
        skipped_images: usize,
    },
    Text {
        bytes: usize,
    },
    Error {
        message: String,
    },
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
    format: Format,
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
    pub(super) fn open(
        bytes: &[u8],
        format: Format,
        cancel: &AtomicBool,
    ) -> Result<(Self, Vec<[f32; 2]>)> {
        ensure!(
            !cancel.load(Ordering::Acquire),
            "Document rendering cancelled."
        );
        let snapshot = Snapshot::new(bytes)?;
        let (stream, child_stream) = UnixStream::pair()?;
        stream.set_read_timeout(Some(Duration::from_millis(50)))?;
        stream.set_write_timeout(Some(Duration::from_millis(50)))?;
        let stdin: OwnedFd = child_stream.try_clone()?.into();
        let stdout: OwnedFd = child_stream.into();
        let executable = desktop_executable()?;
        let mut command = Command::new(executable);
        let path = snapshot.path();
        let helper = match format {
            Format::Pdf => "--pdf-helper",
            Format::Pptx => "--pptx-helper",
        };
        command.args([std::ffi::OsStr::new(helper), path.as_os_str()]);
        command
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::null());
        super::limit_process(&mut command);
        let child = command
            .spawn()
            .context("Cannot start the bundled Rust document renderer")?;
        let mut renderer = Self {
            child,
            stream,
            format,
        };
        let Header::Document { sizes } = renderer.header(cancel, Instant::now())? else {
            bail!("Document renderer did not return metadata")
        };
        validate_sizes(&sizes)?;
        Ok((renderer, sizes))
    }

    fn request(&mut self, request: Request) -> Result<()> {
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        self.stream
            .write_all(&bytes)
            .context("Document renderer input disconnected")?;
        Ok(())
    }
    fn read(&mut self, bytes: &mut [u8], cancel: &AtomicBool, start: Instant) -> Result<()> {
        let mut offset = 0;
        while offset < bytes.len() {
            ensure!(
                !cancel.load(Ordering::Acquire),
                "Document rendering cancelled."
            );
            ensure!(
                start.elapsed() < DEADLINE,
                "Document rendering timed out after 20 seconds."
            );
            match self.stream.read(&mut bytes[offset..]) {
                Ok(0) => bail!(
                    "The bundled document renderer stopped. This file may exceed the renderer's memory limit or contain unsupported data."
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
    ) -> Result<(images::Decoded, usize)> {
        let start = Instant::now();
        self.request(Request::Render { page, side })?;
        let Header::Page {
            width,
            height,
            skipped_images,
        } = self.header(cancel, start)?
        else {
            bail!("Invalid document page response")
        };
        ensure!(
            width > 0 && height > 0 && width <= side && height <= side,
            "Invalid document raster dimensions."
        );
        let size = width
            .checked_mul(height)
            .and_then(|v| v.checked_mul(4))
            .filter(|v| *v <= images::MAX_DECODED)
            .context("Document raster exceeds its memory limit")?;
        let lease = Arc::new(budget.reserve(size)?);
        let mut pixels = vec![0; size];
        self.read(&mut pixels, cancel, start)?;
        Ok((
            images::Decoded {
                pixels: Arc::new(eframe::egui::ColorImage::from_rgba_unmultiplied(
                    [width, height],
                    &pixels,
                )),
                original: [width as u32, height as u32],
                format: self.format.label().into(),
                encoded_bytes: source_bytes,
                lease,
            },
            skipped_images,
        ))
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
pub(super) fn serve(path: &Path, format: Format) -> Result<()> {
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
    if let Err(error) = serve_document(path, format, &mut output) {
        header(
            &mut output,
            &Header::Error {
                message: format!("{error:#}"),
            },
        )?;
    }
    Ok(())
}

/// A bounded, opaque page frame shared by the PDF and presentation interpreters.
pub(super) struct Raster {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
    pub skipped_images: usize,
}

struct Hayro {
    document: hayro::hayro_syntax::Pdf,
}

enum Backend {
    Pdf(Box<Hayro>),
    Pptx(Box<super::pptx::Presentation>),
}
impl Backend {
    fn open(path: &Path, format: Format) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        Ok(match format {
            Format::Pptx => Self::Pptx(Box::new(super::pptx::Presentation::open(&bytes)?)),
            Format::Pdf => {
                let document = hayro::hayro_syntax::Pdf::new(bytes).map_err(|error| {
                    if matches!(error, hayro::hayro_syntax::LoadPdfError::Decryption(_)) {
                        anyhow::anyhow!(
                            "Password-protected PDFs are not supported by this preview."
                        )
                    } else {
                        anyhow::anyhow!("Cannot open this PDF: invalid or unsupported document.")
                    }
                })?;
                Self::Pdf(Box::new(Hayro { document }))
            }
        })
    }
    fn sizes(&self) -> Vec<[f32; 2]> {
        match self {
            Self::Pdf(pdf) => pdf
                .document
                .pages()
                .iter()
                .map(|page| {
                    let (w, h) = page.render_dimensions();
                    [w, h]
                })
                .collect(),
            Self::Pptx(deck) => deck.sizes(),
        }
    }
    fn render<'a>(
        &'a self,
        page: usize,
        side: usize,
        cache: &mut hayro::RenderCache<'a>,
    ) -> Result<Raster> {
        match self {
            Self::Pdf(pdf) => pdf.render(page, side, cache),
            Self::Pptx(deck) => deck.render(page, side),
        }
    }
    fn text(&self) -> Result<String> {
        match self {
            Self::Pdf(pdf) => super::text::extract(&pdf.document),
            Self::Pptx(_) => bail!("PPTX text extraction is not available in this preview."),
        }
    }
}
impl Hayro {
    fn render<'a>(
        &'a self,
        page: usize,
        side: usize,
        cache: &mut hayro::RenderCache<'a>,
    ) -> Result<Raster> {
        use hayro::{
            hayro_interpret::{InterpreterSettings, util::TransformExt},
            vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit},
        };
        let page = &self.document.pages()[page - 1];
        let (width, height) = page.render_dimensions();
        let scale = side as f32 / width.max(height);
        let w = (width * scale).ceil().clamp(1., side as f32) as u16;
        let h = (height * scale).ceil().clamp(1., side as f32) as u16;
        let mut context = RenderContext::new(w, h);
        hayro::render_into(
            page,
            cache,
            &InterpreterSettings::default(),
            &hayro::RenderSettings::default(),
            &mut context,
            hayro::kurbo::Affine::scale(scale as f64) * page.initial_transform(true).to_kurbo(),
        );
        context.flush();
        let mut pixels = Pixmap::new(w, h);
        context.render_with(
            &mut pixels,
            &mut Resources::default(),
            RasterizerSettings {
                target_init: TargetInit::Clear(hayro::vello_cpu::color::palette::css::WHITE),
                ..Default::default()
            },
        );
        Ok(Raster {
            width: usize::from(w),
            height: usize::from(h),
            pixels: pixels.data_as_u8_slice().to_vec(),
            skipped_images: 0,
        })
    }
}

/// Parse once and retain the original presentation model and font/render caches.
/// This loop owns only immutable snapshots; it exposes no file mutation or external resource loading.
fn serve_document(path: &Path, format: Format, mut output: &mut impl Write) -> Result<()> {
    let document = Backend::open(path, format)?;
    let sizes = document.sizes();
    validate_sizes(&sizes)?;
    let pages = sizes.len();
    header(&mut output, &Header::Document { sizes })?;
    let mut input = BufReader::new(std::io::stdin().lock());
    // Hayro's cache borrows its parsed document. Keep both local to this
    // lifetime instead of making the retained backend self-referential.
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
            "Invalid document renderer request"
        );
        match serde_json::from_slice::<Request>(&line)? {
            Request::Render { page, side } => {
                ensure!(
                    (1..=pages).contains(&page) && (1..=4096).contains(&side),
                    "Invalid document page request"
                );
                if renders == 16 {
                    cache = hayro::RenderCache::new();
                    renders = 0;
                }
                let raster = document.render(page, side, &mut cache)?;
                renders += 1;
                ensure!(
                    raster.width > 0
                        && raster.height > 0
                        && raster.width <= side
                        && raster.height <= side,
                    "Invalid document raster dimensions"
                );
                ensure!(
                    raster.pixels.len() == raster.width * raster.height * 4,
                    "Invalid document raster payload"
                );
                header(
                    &mut output,
                    &Header::Page {
                        width: raster.width,
                        height: raster.height,
                        skipped_images: raster.skipped_images,
                    },
                )?;
                output.write_all(&raster.pixels)?;
                output.flush()?;
            }
            Request::Text => {
                let text = document.text()?;
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
        (
            Renderer {
                child,
                stream,
                format: Format::Pdf,
            },
            peer,
        )
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
