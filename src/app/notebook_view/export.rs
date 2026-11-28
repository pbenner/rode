//! Original notebook image snapshots, clipboard copies and protected project-host export.
//!
//! Capture the MIME payload when an image action is selected, so reruns, display
//! updates or tab changes cannot replace it while saving or copying is pending.
use super::*;
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};

/// A desktop clipboard request outlives tab switches and project changes, but
/// never the window. One pending decode bounds repeated copy requests.
#[derive(Default)]
pub(in crate::app) struct Clipboard {
    pending: Option<mpsc::Receiver<Result<arboard::Clipboard, String>>>,
    /// Linux needs a live clipboard owner to serve later paste requests.
    owner: Option<arboard::Clipboard>,
}
impl Clipboard {
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    fn start(
        &mut self,
        snapshot: ImageExport,
        budget: Arc<crate::images::Budget>,
        ctx: &egui::Context,
    ) {
        if self.busy() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _io = budget.io.lock().unwrap_or_else(|e| e.into_inner());
            let result = (|| -> Result<arboard::Clipboard> {
                let decoded = crate::images::decode_original(
                    &snapshot.data,
                    &snapshot.suggested_path,
                    &budget,
                )?;
                let mut clipboard =
                    arboard::Clipboard::new().context("Cannot open the desktop clipboard")?;
                // egui-winit 0.33 copies premultiplied ColorImage bytes directly.
                // Native clipboard images require straight RGBA to preserve alpha.
                clipboard
                    .set_image(arboard::ImageData {
                        width: decoded.rgba.width() as usize,
                        height: decoded.rgba.height() as usize,
                        bytes: std::borrow::Cow::Borrowed(decoded.rgba.as_raw()),
                    })
                    .context("Cannot transfer image to the desktop clipboard")?;
                Ok(clipboard)
            })()
            .map_err(|error| format!("Cannot copy notebook image: {error:#}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Retain clipboard ownership even when the source notebook is no longer open.
    fn poll(&mut self) -> Option<String> {
        let received = self.pending.as_ref()?.try_recv();
        let result = match received {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Notebook image copy worker stopped".into())
            }
        };
        self.pending = None;
        match result {
            Ok(clipboard) => {
                self.owner = Some(clipboard);
                None
            }
            Err(error) => Some(error),
        }
    }
}

/// One bounded immutable output selection, owned by the save dialog until submission/cancel.
#[derive(PartialEq)]
pub(in crate::app) struct ImageExport {
    pub document: u64,
    pub suggested_path: String,
    extension: String,
    data: String,
}
impl ImageExport {
    /// Retain original PNG/JPEG bytes; never export the reduced GPU preview texture.
    pub(super) fn capture(
        document: u64,
        path: &str,
        cell: usize,
        output: usize,
        bundle: &Value,
    ) -> Result<Self> {
        let (data, _) =
            image_bundle(bundle).context("This output has no exportable PNG or JPEG image")?;
        let bytes = STANDARD
            .decode(&data)
            .context("The image output contains invalid base64")?;
        let extension = match image::guess_format(&bytes)
            .context("The image output is not a supported image")?
        {
            image::ImageFormat::Png => "png",
            image::ImageFormat::Jpeg => "jpg",
            _ => anyhow::bail!("Only PNG and JPEG notebook images can be saved"),
        };
        // The codec signature decides the extension even if a notebook's MIME label is inaccurate.
        let path = std::path::Path::new(path);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("notebook");
        let suggested_path = path
            .with_file_name(format!("{stem}-cell-{cell}-output-{output}.{extension}"))
            .to_string_lossy()
            .into_owned();
        Ok(Self {
            document,
            suggested_path,
            extension: extension.into(),
            data,
        })
    }
    /// Create a new file through the existing atomic, policy-bound binary writer.
    /// Existing destinations are deliberately protected by an absent baseline revision.
    pub(super) fn request(self, mut path: String) -> Result<Request> {
        match std::path::Path::new(&path)
            .extension()
            .and_then(|s| s.to_str())
        {
            None => {
                path.push('.');
                path.push_str(&self.extension);
            }
            Some(ext) => ensure!(
                ext.eq_ignore_ascii_case(&self.extension)
                    || self.extension == "jpg" && ext.eq_ignore_ascii_case("jpeg"),
                "Use a .{} filename; image export preserves the original format",
                self.extension
            ),
        }
        Ok(Request::WriteBinary {
            path,
            data: self.data,
            expected: None,
        })
    }
}
impl EditorApp {
    pub(super) fn copy_notebook_image(&mut self, snapshot: ImageExport, ctx: &egui::Context) {
        self.notebook_clipboard
            .start(snapshot, self.image_budget.clone(), ctx);
    }

    pub(in crate::app) fn poll_notebook_image_copy(&mut self) {
        if let Some(error) = self.notebook_clipboard.poll() {
            self.notify(error, true);
        }
    }

    /// Submit an explicit image export without rebinding or marking its notebook saved.
    pub(in crate::app) fn save_notebook_image(
        &mut self,
        snapshot: ImageExport,
        path: String,
        ctx: &egui::Context,
    ) {
        match snapshot.request(path) {
            Ok(request) => self.dispatch(request, Job::ExportImage(None), ctx),
            Err(error) => self.notify(error.to_string(), true),
        }
    }
}

const MAX_IMAGE_PREVIEW_BYTES: usize = 32 * 1024 * 1024;

/// Share preview/export MIME selection and its encoded bound with Markdown attachments.
pub(super) fn image_bundle(bundle: &Value) -> Option<(String, String)> {
    for (mime, path) in [("image/png", "output.png"), ("image/jpeg", "output.jpg")] {
        let size = match &bundle[mime] {
            Value::String(text) => Some(text.len()),
            Value::Array(parts) => parts.iter().try_fold(0usize, |size, value| {
                let next = size.saturating_add(value.as_str()?.len());
                (next <= MAX_IMAGE_PREVIEW_BYTES).then_some(next)
            }),
            _ => None,
        };
        if size.is_some_and(|size| size <= MAX_IMAGE_PREVIEW_BYTES)
            && let Ok(data) = notebook::multiline(&bundle[mime])
        {
            return Some((data, path.into()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_reports_failed_copies_and_bounds_pending_requests() {
        let budget = Arc::new(crate::images::Budget::default());
        let mut clipboard = Clipboard::default();
        let ctx = egui::Context::default();
        let invalid = |data: &str| ImageExport {
            document: 42,
            suggested_path: "plot.png".into(),
            extension: "png".into(),
            data: data.into(),
        };
        clipboard.start(invalid("invalid base64!"), budget.clone(), &ctx);
        assert!(clipboard.busy());
        // A different error would result if the second request replaced the first.
        clipboard.start(invalid(&STANDARD.encode("not a PNG")), budget.clone(), &ctx);
        let until = Instant::now() + Duration::from_secs(5);
        let error = loop {
            if let Some(error) = clipboard.poll() {
                break error;
            }
            assert!(Instant::now() < until, "Image copy did not finish");
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(error.contains("Invalid image data"), "{error}");
        assert!(!clipboard.busy());
        assert_eq!(clipboard.poll(), None);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn exports_preserve_original_png_and_jpeg_snapshots_and_never_replace_existing_files() {
        for (format, mime, extension) in [
            (image::ImageFormat::Png, "image/png", "png"),
            (image::ImageFormat::Jpeg, "image/jpeg", "jpg"),
        ] {
            let pixels = image::RgbImage::from_pixel(11, 7, image::Rgb([40, 120, 200]));
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgb8(pixels)
                .write_to(&mut bytes, format)
                .unwrap();
            let bytes = bytes.into_inner();
            let encoded = STANDARD.encode(&bytes);
            let mut bundle = json!({mime: [encoded[..12], encoded[12..]]});
            let snapshot =
                ImageExport::capture(42, "/project/日本 notebook.ipynb", 3, 2, &bundle).unwrap();
            assert_eq!(
                snapshot.suggested_path,
                format!("/project/日本 notebook-cell-3-output-2.{extension}")
            );
            // A display update after opening the picker must not substitute its payload.
            bundle[mime] = json!("changed after selection");
            let root = tempfile::tempdir().unwrap();
            let path = root
                .path()
                .join("saved plot")
                .to_string_lossy()
                .into_owned();
            let request = snapshot.request(path).unwrap();
            let reply = files::local(&request).unwrap();
            assert_eq!(std::fs::read(&reply.path).unwrap(), bytes);
            assert_eq!(
                std::path::Path::new(&reply.path).extension().unwrap(),
                extension
            );
            std::fs::write(&reply.path, b"external content").unwrap();
            assert!(files::local(&request).is_err());
            assert_eq!(std::fs::read(reply.path).unwrap(), b"external content");
        }
    }

    #[test]
    fn malformed_images_and_incorrect_extensions_are_rejected_without_writes() {
        for bundle in [
            json!({}),
            json!({"image/png": "invalid"}),
            json!({"image/png": STANDARD.encode(b"not an image")}),
            json!({"image/png": "a".repeat(MAX_IMAGE_PREVIEW_BYTES + 1)}),
        ] {
            assert!(ImageExport::capture(1, "book.ipynb", 1, 1, &bundle).is_err());
        }
        let snapshot = ImageExport {
            document: 1,
            suggested_path: "plot.png".into(),
            extension: "png".into(),
            data: String::new(),
        };
        assert!(snapshot.request("notebook.ipynb".into()).is_err());
    }
}
