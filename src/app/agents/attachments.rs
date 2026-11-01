//! Composer file snapshots and worker-side conversion. Reading/decoding stays
//! off the GUI thread; tokens prevent late files leaking into another chat.
use super::*;
use anyhow::Context;
use base64::{Engine, engine::general_purpose::STANDARD};
use rode_agent_core::attachment::{self, Attachment};

#[derive(Default)]
/// Composer-owned attachment snapshots plus a rotating request token
/// that prevents a late file read from crossing into a replacement conversation.
pub(in crate::app) struct State {
    pub files: Vec<Attachment>,
    pub pending: bool,
    pub error: String,
    token: u64,
}
impl State {
    /// Release captured files and invalidate every earlier attachment request token.
    pub fn clear(&mut self) {
        self.files.clear();
        self.pending = false;
        self.error.clear();
        self.token = self.token.wrapping_add(1);
    }
    /// Start a new attachment read and return the token required by its completion.
    pub fn begin(&mut self) -> u64 {
        self.token = self.token.wrapping_add(1);
        self.pending = true;
        self.error.clear();
        self.token
    }
    /// Accept only the current token and validate the combined attachment budget before replacement.
    pub fn finish(&mut self, token: u64, result: Result<Attachment, String>) {
        if self.token != token {
            return;
        }
        self.pending = false;
        match result {
            Ok(file) => {
                let mut files = self.files.clone();
                files.push(file);
                match attachment::validate("", &files) {
                    Ok(()) => self.files = files,
                    Err(error) => self.error = error.to_string(),
                }
            }
            Err(error) => self.error = error,
        }
    }
    /// Reserve one compact row only when files, a pending read or an error are present.
    pub fn height(&self) -> f32 {
        if self.pending || !self.files.is_empty() || !self.error.is_empty() {
            30.
        } else {
            0.
        }
    }
    /// Render attachment summaries and explicit removal controls from cached payloads.
    pub fn ui(&mut self, ui: &mut egui::Ui, enabled: bool) {
        if self.height() == 0. {
            return;
        }
        let mut remove = None;
        egui::ScrollArea::horizontal()
            .id_salt("agent-attachments")
            .max_height(26.)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.pending {
                        ui.spinner();
                        ui.label("Reading attachment…");
                    }
                    for (index, file) in self.files.iter().enumerate() {
                        ui.push_id(index, |ui| {
                            egui::Frame::group(ui.style())
                                .inner_margin(3.)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let label =
                                            if file.is_image() { "Image" } else { "Document" };
                                        ui.add_sized(
                                            [140., 18.],
                                            egui::Label::new(format!("{label} · {}", file.name()))
                                                .truncate(),
                                        )
                                        .on_hover_text(file.name());
                                        if ui
                                            .add_enabled(enabled, egui::Button::new("×").small())
                                            .on_hover_text("Remove attachment")
                                            .clicked()
                                        {
                                            remove = Some(index);
                                        }
                                    });
                                });
                        });
                    }
                    if !self.error.is_empty() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&self.error).color(ui.visuals().error_fg_color),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&self.error);
                    }
                });
            });
        if let Some(index) = remove {
            self.files.remove(index);
            self.error.clear();
        }
    }
}

impl EditorApp {
    /// Queue one bounded file read with a composer token. `local` selects an explicit
    /// desktop attachment; host attachments remain inside the current project policy.
    pub(in crate::app) fn attach_agent_file(
        &mut self,
        path: String,
        local: bool,
        ctx: &egui::Context,
    ) {
        if !self.feature(crate::extensions::Feature::Agents) || self.agents.files.pending {
            return;
        }
        let token = self.agents.files.begin();
        self.dispatch(
            Request::ReadBinary { path },
            Job::AgentAttachment { token, local },
            ctx,
        );
    }
}

/// Convert a bounded file-channel snapshot into portable prompt content.
pub(in crate::app) fn prepare(
    path: &str,
    encoded: &str,
    budget: &Arc<crate::images::Budget>,
    cancel: &AtomicBool,
) -> anyhow::Result<Attachment> {
    anyhow::ensure!(!cancel.load(Ordering::Acquire), "Attachment cancelled.");
    anyhow::ensure!(
        encoded.len() <= crate::images::MAX_ENCODED.div_ceil(3) * 4,
        "Attachment exceeds 32 MiB."
    );
    let bytes = STANDARD.decode(encoded)?;
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("attachment")
        .to_owned();
    let file = if crate::pdf::is_path(path) {
        let text = crate::pdf::extract_text(bytes, cancel)?;
        Attachment::Document { name, text }
    } else if crate::images::format_for(path).is_some() {
        let decoded = crate::images::decode_bytes(&bytes, path, budget, 2048)?;
        let mut image = image::RgbaImage::from_raw(
            decoded.pixels.size[0] as u32,
            decoded.pixels.size[1] as u32,
            decoded
                .pixels
                .pixels
                .iter()
                .flat_map(|pixel| pixel.to_srgba_unmultiplied())
                .collect(),
        )
        .context("Invalid image pixels")?;
        let opaque = image.pixels().all(|p| p.0[3] == 255);
        let (mime_type, data) = loop {
            let mut buffer = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image.clone())
                .write_to(&mut buffer, image::ImageFormat::Png)?;
            let mut mime_type = "image/png";
            if opaque && buffer.get_ref().len().div_ceil(3) * 4 > attachment::CONTENT_LIMIT {
                buffer = std::io::Cursor::new(Vec::new());
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buffer, 88)
                    .encode_image(&image::DynamicImage::ImageRgba8(image.clone()).into_rgb8())?;
                mime_type = "image/jpeg";
            }
            let data = STANDARD.encode(buffer.into_inner());
            if data.len() <= attachment::CONTENT_LIMIT {
                break (mime_type.to_owned(), data);
            }
            anyhow::ensure!(
                image.width().max(image.height()) > 256,
                "Image cannot fit the attachment limit."
            );
            let side = (image.width().max(image.height()) * 3 / 4).max(256);
            image = image::imageops::thumbnail(&image, side, side);
        };
        Attachment::Image {
            name,
            mime_type,
            data,
        }
    } else {
        anyhow::ensure!(
            bytes.len() <= attachment::CONTENT_LIMIT,
            "Text documents are limited to 1 MiB."
        );
        let text = String::from_utf8(bytes)
            .context("Unsupported document: attach a UTF-8 text file, PDF, or raster image.")?;
        anyhow::ensure!(
            !text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')),
            "Unsupported binary document: attach a UTF-8 text file or PDF."
        );
        Attachment::Document {
            name,
            text: text.trim_start_matches('\u{feff}').into(),
        }
    };
    attachment::validate("", std::slice::from_ref(&file))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_text_and_images_and_rejects_binary_or_cancelled_files() {
        let budget = Arc::new(crate::images::Budget::default());
        let cancel = AtomicBool::new(false);
        let doc = prepare(
            "/project/notes.md",
            &STANDARD.encode("\u{feff}Hello λ\nSecond line"),
            &budget,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            doc,
            Attachment::Document {
                name: "notes.md".into(),
                text: "Hello λ\nSecond line".into()
            }
        );
        assert!(
            prepare(
                "binary.bin",
                &STANDARD.encode(b"abc\0def"),
                &budget,
                &cancel
            )
            .is_err()
        );
        assert!(
            prepare(
                "invalid.png",
                &STANDARD.encode(b"not an image"),
                &budget,
                &cancel
            )
            .is_err()
        );
        let image = image::RgbaImage::from_pixel(48, 32, image::Rgba([0, 150, 220, 255]));
        let mut data = std::io::Cursor::new(vec![]);
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut data, image::ImageFormat::Png)
            .unwrap();
        let image = prepare(
            "screenshot.png",
            &STANDARD.encode(data.into_inner()),
            &budget,
            &cancel,
        )
        .unwrap();
        let Attachment::Image {
            mime_type, data, ..
        } = image
        else {
            panic!()
        };
        assert_eq!(mime_type, "image/png");
        let image = image::load_from_memory(&STANDARD.decode(data).unwrap()).unwrap();
        assert_eq!((image.width(), image.height()), (48, 32));
        assert_eq!(budget.used(), 0);
        assert!(
            prepare(
                "notes.txt",
                &STANDARD.encode("hello"),
                &budget,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
    #[test]
    fn replacing_a_chat_discards_late_files_and_failed_reads_keep_existing_files() {
        let mut state = State::default();
        let token = state.begin();
        state.clear();
        let doc = Attachment::Document {
            name: "notes.txt".into(),
            text: "snapshot".into(),
        };
        state.finish(token, Ok(doc.clone()));
        assert!(state.files.is_empty());
        let token = state.begin();
        state.finish(token, Ok(doc.clone()));
        let token = state.begin();
        state.finish(token, Err("unreadable".into()));
        assert_eq!(state.files, vec![doc]);
        assert_eq!(state.error, "unreadable");
        assert!(!state.pending);
    }
    #[test]
    fn queued_files_stay_with_their_prompt_and_failed_sends_keep_the_draft() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let doc = Attachment::Document {
            name: "notes.txt".into(),
            text: "snapshot".into(),
        };
        app.agents.chat.busy = true;
        app.agents.files.files.push(doc.clone());
        app.send_agent(&ctx);
        assert_eq!(app.agents.queue.next(), Some(""));
        assert_eq!(
            app.agents.queue.next_attachments(),
            std::slice::from_ref(&doc)
        );
        assert!(app.agents.files.files.is_empty());
        app.agents.chat.busy = false;
        app.agents.queue.clear();
        app.agents.config.codex = "bad\0program".into();
        app.agents.files.files.push(doc.clone());
        app.agents.draft = "keep this draft".into();
        app.send_agent(&ctx);
        assert_eq!(app.agents.draft, "keep this draft");
        assert_eq!(app.agents.files.files, vec![doc]);
    }

    #[test]
    fn desktop_attachment_reads_use_the_local_host_in_an_offline_ssh_workspace() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("desktop.txt");
        std::fs::write(&path, "Local contents").unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.workspace = Some(Workspace {
            root: "/remote/project".into(),
            remote: Some(
                crate::ssh::Remote::new(crate::ssh::Profile {
                    host: "attachments.invalid".into(),
                    ..Default::default()
                })
                .unwrap(),
            ),
            connected: false,
            ready: false,
            plan: Default::default(),
            sandbox_options: Default::default(),
        });
        app.poll_agents(&ctx);
        app.attach_agent_file(path.to_string_lossy().into(), true, &ctx);
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.agents.files.pending && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            app.poll(&ctx);
        }
        assert!(!app.agents.files.pending);
        assert!(
            app.agents.files.error.is_empty(),
            "{}",
            app.agents.files.error
        );
        assert_eq!(
            app.agents.files.files,
            vec![Attachment::Document {
                name: "desktop.txt".into(),
                text: "Local contents".into()
            }]
        );
    }
}
