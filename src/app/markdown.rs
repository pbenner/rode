//! Native Markdown previews. Images use the existing host/sandbox-aware reader;
//! the renderer never installs filesystem, HTTP, or data-URL loaders.
//!
//! Preparation reads the current unsaved buffer and preserves source cursor/undo state.
//! Linked assets resolve relative to the source on its host; custom image loading enforces
//! the existing sandbox and shared preview budget without HTTP/file fallback loaders.
use super::*;
use egui::load::{
    LoadError, SizeHint, SizedTexture, TextureLoadResult, TextureLoader, TexturePoll,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use pulldown_cmark::{Event as MdEvent, Options, Parser, Tag, TagEnd};
use std::sync::{Mutex, atomic::AtomicU64};

const MAX_BYTES: usize = 256 * 1024;
const MAX_EVENTS: usize = 12_000;
const MAX_IMAGES: usize = 16;
const SCHEME: &str = "rode-markdown://";
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

pub(super) fn is_path(path: &str) -> bool {
    PathBuf::from(path)
        .extension()
        .and_then(|p| p.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "md" | "markdown" | "mdown" | "mkd"
            )
        })
}

enum Image {
    Pending,
    Ready {
        texture: egui::TextureHandle,
        _lease: Arc<crate::images::Lease>,
    },
    Error(String),
}
#[derive(Default)]
pub(super) struct Images(Mutex<HashMap<String, Image>>);
impl TextureLoader for Images {
    fn id(&self) -> &str {
        concat!(module_path!(), "::Images")
    }
    fn load(
        &self,
        _: &egui::Context,
        uri: &str,
        _: egui::TextureOptions,
        _: SizeHint,
    ) -> TextureLoadResult {
        if !uri.starts_with(SCHEME) {
            return Err(LoadError::NotSupported);
        }
        match self.0.lock().unwrap_or_else(|e| e.into_inner()).get(uri) {
            Some(Image::Pending) => Ok(TexturePoll::Pending { size: None }),
            Some(Image::Ready { texture, .. }) => Ok(TexturePoll::Ready {
                texture: SizedTexture::from_handle(texture),
            }),
            Some(Image::Error(error)) => Err(LoadError::Loading(error.clone())),
            None => Err(LoadError::Loading("Image preview has expired".into())),
        }
    }
    fn forget(&self, uri: &str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(uri);
    }
    fn forget_all(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    fn byte_size(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .map(|entry| match entry {
                Image::Ready { texture, .. } => texture.size()[0] * texture.size()[1] * 4,
                _ => 0,
            })
            .sum()
    }
}

pub(super) struct Preview {
    pub(in crate::app) showing: bool,
    pub token: u64,
    key: Option<(u64, String)>,
    source: String,
    cache: CommonMarkCache,
    images: Arc<Images>,
    uris: Vec<String>,
    cancelled: Arc<AtomicBool>,
    error: Option<String>,
    warnings: Vec<String>,
    appearance: Option<(String, u32, [u32; 2])>,
}
impl Preview {
    pub(in crate::app) fn new(images: Arc<Images>) -> Self {
        Self {
            showing: false,
            token: 0,
            key: None,
            source: String::new(),
            cache: Default::default(),
            images,
            uris: vec![],
            cancelled: Arc::new(AtomicBool::new(false)),
            error: None,
            warnings: vec![],
            appearance: None,
        }
    }
    fn clear(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        for uri in self.uris.drain(..) {
            self.images.forget(&uri);
        }
        self.source.clear();
        self.cache = Default::default();
        self.warnings.clear();
        self.error = None;
    }
    pub(super) fn invalidate(&mut self) {
        self.clear();
        self.key = None;
    }
    pub(in crate::app) fn prepare(
        &mut self,
        version: u64,
        path: &str,
        text: &str,
    ) -> Vec<(String, String)> {
        self.prepare_embedded(version, path, text, &HashMap::new())
    }
    pub(in crate::app) fn prepare_embedded(
        &mut self,
        version: u64,
        path: &str,
        text: &str,
        embedded: &HashMap<String, String>,
    ) -> Vec<(String, String)> {
        if self
            .key
            .as_ref()
            .is_some_and(|key| key.0 == version && key.1 == path)
        {
            return vec![];
        }
        self.clear();
        self.key = Some((version, path.into()));
        self.token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        self.cancelled = Arc::new(AtomicBool::new(false));
        match prepare_images(text, path, self.token, embedded) {
            Ok(prepared) => {
                self.source = prepared.source;
                for link in prepared.links {
                    self.cache.add_link_hook(link);
                }
                self.warnings = prepared.warnings;
                for (uri, _) in &prepared.images {
                    self.uris.push(uri.clone());
                    self.images
                        .0
                        .lock()
                        .unwrap()
                        .insert(uri.clone(), Image::Pending);
                }
                prepared.images
            }
            Err(error) => {
                self.error = Some(error);
                vec![]
            }
        }
    }
    pub(in crate::app) fn complete(
        &mut self,
        uri: String,
        result: Result<crate::images::Decoded, String>,
        ctx: &egui::Context,
    ) {
        if !self.uris.contains(&uri) {
            return;
        }
        let image = match result {
            Ok(decoded) => Image::Ready {
                texture: ctx.load_texture(
                    &uri,
                    egui::ImageData::Color(decoded.pixels),
                    egui::TextureOptions::LINEAR,
                ),
                _lease: decoded.lease,
            },
            Err(error) => {
                self.warnings.push(error.clone());
                Image::Error(error)
            }
        };
        self.images
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(uri, image);
        self.cache.clear_scrollable(); // Recalculate layout when image dimensions arrive.
    }
    pub(in crate::app) fn cancelled(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
    pub(in crate::app) fn inline_ui(
        &mut self,
        ui: &mut egui::Ui,
        font_size: f32,
        palette: theme::Palette,
    ) -> Option<String> {
        if let Some(error) = &self.error {
            ui.colored_label(palette.error, error);
            return None;
        }
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(font_size));
        CommonMarkViewer::new()
            .explicit_image_uri_scheme(true)
            .show(ui, &mut self.cache, &self.source);
        self.cache
            .link_hooks()
            .iter()
            .find(|(_, clicked)| **clicked)
            .map(|(url, _)| url.clone())
    }
    pub(in crate::app) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        theme: &str,
        font_size: f32,
        palette: theme::Palette,
    ) -> Option<String> {
        if let Some(error) = &self.error {
            ui.colored_label(palette.error, error);
            return None;
        }
        let size = ui.available_size();
        let appearance = (
            theme.into(),
            font_size.to_bits(),
            [size.x.to_bits(), size.y.to_bits()],
        );
        if self.appearance.as_ref() != Some(&appearance) {
            self.cache.clear_scrollable();
            self.appearance = Some(appearance);
        }
        if !self.warnings.is_empty() {
            ui.collapsing(format!("{} image notice(s)", self.warnings.len()), |ui| {
                for warning in &self.warnings {
                    ui.label(warning);
                }
            });
        }
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(font_size));
        ui.style_mut().text_styles.insert(
            egui::TextStyle::Heading,
            FontId::proportional(font_size * 2.),
        );
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Monospace, FontId::monospace(font_size));
        CommonMarkViewer::new()
            .explicit_image_uri_scheme(true)
            .show_scrollable(("markdown-preview", id), ui, &mut self.cache, &self.source);
        self.cache
            .link_hooks()
            .iter()
            .find(|(_, clicked)| **clicked)
            .map(|(url, _)| url.clone())
    }
}
impl Drop for Preview {
    fn drop(&mut self) {
        self.clear();
    }
}

struct Prepared {
    source: String,
    links: HashSet<String>,
    images: Vec<(String, String)>,
    warnings: Vec<String>,
}
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
}
#[cfg(test)]
fn prepare(text: &str, path: &str, token: u64) -> Result<Prepared, String> {
    prepare_images(text, path, token, &HashMap::new())
}
fn prepare_images(
    text: &str,
    path: &str,
    token: u64,
    embedded: &HashMap<String, String>,
) -> Result<Prepared, String> {
    if text.len() > MAX_BYTES {
        return Err("Markdown preview is limited to 256 KiB. The full document remains available in Edit mode.".into());
    }
    let events: Vec<_> = Parser::new_ext(text, options())
        .into_offset_iter()
        .take(MAX_EVENTS + 1)
        .collect();
    if events.len() > MAX_EVENTS {
        return Err(
            "Markdown preview has too many elements. Use Edit mode for this document.".into(),
        );
    }
    let mut depth = 0_usize;
    for (event, _) in &events {
        match event {
            MdEvent::Start(_) => {
                depth += 1;
                if depth > 64 {
                    return Err("Markdown preview nesting exceeds 64 levels.".into());
                }
            }
            MdEvent::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let mut result = Prepared {
        source: text.into(),
        links: HashSet::new(),
        images: vec![],
        warnings: vec![],
    };
    let mut edits = vec![];
    let mut events = events.into_iter();
    let mut destinations = HashMap::new();
    while let Some((event, mut range)) = events.next() {
        match event {
            MdEvent::Start(Tag::Link { dest_url, .. }) => {
                result.links.insert(dest_url.to_string());
            }
            MdEvent::Start(Tag::Image { dest_url, .. }) => {
                let mut alt = String::new();
                let mut depth = 1;
                for (event, span) in events.by_ref() {
                    match event {
                        MdEvent::Text(text) | MdEvent::Code(text) => alt.push_str(&text),
                        MdEvent::Start(Tag::Image { .. }) => depth += 1,
                        MdEvent::End(TagEnd::Image) => {
                            depth -= 1;
                            if depth == 0 {
                                range.end = range.end.max(span.end);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let alt = alt
                    .chars()
                    .take(160)
                    .collect::<String>()
                    .replace(['\n', '\r'], " ");
                let alt = alt
                    .chars()
                    .flat_map(|c| {
                        if "\\[]*_`<>()!".contains(c) {
                            vec!['\\', c]
                        } else {
                            vec![c]
                        }
                    })
                    .collect::<String>();
                let target = embedded
                    .get(dest_url.as_ref())
                    .cloned()
                    .map(Ok)
                    .unwrap_or_else(|| {
                        file_target(path, &dest_url).and_then(|path| {
                            if crate::images::format_for(&path).is_some() {
                                Ok(path)
                            } else {
                                Err("Image format is not supported in Markdown previews.".into())
                            }
                        })
                    });
                let replacement = match target {
                    Ok(target)
                        if destinations.contains_key(&target)
                            || result.images.len() < MAX_IMAGES =>
                    {
                        let uri = destinations.entry(target.clone()).or_insert_with(|| {
                            let uri = format!("{SCHEME}{token}/{}", result.images.len());
                            result.images.push((uri.clone(), target));
                            uri
                        });
                        format!("![{alt}]({uri})")
                    }
                    other => {
                        let message = other.err().unwrap_or_else(|| {
                            "Only 16 distinct inline images are loaded per preview.".into()
                        });
                        if !result.warnings.contains(&message) {
                            result.warnings.push(message);
                        }
                        format!("*Image: {alt} (not loaded)*")
                    }
                };
                edits.push((range, replacement));
            }
            _ => {}
        }
    }
    for (range, replacement) in edits.into_iter().rev() {
        result.source.replace_range(range, &replacement);
    }
    Ok(result)
}

fn decode_path(path: &str) -> Result<String, String> {
    let mut bytes = vec![];
    let mut source = path.as_bytes().iter().copied();
    while let Some(byte) = source.next() {
        if byte == b'%' {
            let a = source.next().and_then(|b| (b as char).to_digit(16));
            let b = source.next().and_then(|b| (b as char).to_digit(16));
            match (a, b) {
                (Some(a), Some(b)) => bytes.push((a * 16 + b) as u8),
                _ => return Err("Invalid percent-encoded path".into()),
            }
        } else {
            bytes.push(byte);
        }
    }
    let path = String::from_utf8(bytes).map_err(|_| "Link path must be UTF-8")?;
    if path.contains('\0') || path.len() > 4096 {
        return Err("Invalid link path".into());
    }
    Ok(path)
}
fn file_target(base: &str, target: &str) -> Result<String, String> {
    let target = target.trim();
    let path = if let Some(path) = target.strip_prefix("file://") {
        if !path.starts_with('/') {
            return Err("File links must use an absolute path.".into());
        }
        path
    } else {
        if target.contains(":") || target.starts_with("//") {
            return Err("External images are not downloaded automatically.".into());
        }
        target
    };
    let path = decode_path(path.split('#').next().unwrap_or_default())?;
    if path.is_empty() {
        return Err("Heading links are not supported yet.".into());
    }
    let path = PathBuf::from(&path);
    Ok(if path.is_absolute() {
        path
    } else {
        PathBuf::from(base)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(path)
    }
    .to_string_lossy()
    .into())
}

impl EditorApp {
    pub(super) fn markdown_source(&mut self) {
        if let Some(view) = self
            .documents
            .get_mut(self.active)
            .and_then(|doc| doc.marp.as_mut())
        {
            view.showing = false;
            view.stop_render();
        }
        if let Some(preview) = self
            .documents
            .get_mut(self.active)
            .and_then(|doc| doc.markdown.as_mut())
        {
            preview.showing = false;
        }
    }
    pub(super) fn toggle_markdown(&mut self, ctx: &egui::Context) {
        if self.feature(crate::extensions::Feature::Marp)
            && self.documents.get(self.active).is_some_and(|doc| {
                crate::marp::is_deck(doc.path.as_deref(), &doc.text)
                    || doc.marp.as_ref().is_some_and(|view| view.showing)
            })
        {
            self.toggle_marp(ctx);
            return;
        }
        if !self.feature(crate::extensions::Feature::Markdown) {
            self.notify("Enable Markdown previews in View → Extensions.", false);
            return;
        }
        let Some(doc) = self
            .documents
            .get_mut(self.active)
            .filter(|doc| doc.path.as_deref().is_some_and(is_path) && !doc.is_preview())
        else {
            self.notify("Open a Markdown file to switch its preview.", false);
            return;
        };
        let preview = doc
            .markdown
            .get_or_insert_with(|| Preview::new(self.markdown_images.clone()));
        if let Some(view) = &mut doc.marp {
            view.showing = false;
            view.stop_render();
        }
        preview.showing = !preview.showing;
        if !preview.showing {
            self.focus_editor(ctx);
        }
    }
    pub(super) fn markdown_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        if self.documents[self.active].recovery.is_some() {
            return false;
        }
        let markdown_enabled = self.feature(crate::extensions::Feature::Markdown);
        let marp_enabled = self.feature(crate::extensions::Feature::Marp);
        if !markdown_enabled && !marp_enabled {
            return false;
        }
        let untitled_path = self.join_root("untitled.marp.md");
        let doc = &mut self.documents[self.active];
        if !doc.path.as_deref().is_some_and(is_path) && doc.marp.is_none() {
            doc.markdown = None;
            return false;
        }
        let dirty = doc.dirty();
        let preview = doc
            .markdown
            .get_or_insert_with(|| Preview::new(self.markdown_images.clone()));
        let was_showing = preview.showing;
        if !markdown_enabled {
            preview.showing = false;
        }
        let was_slides = doc.marp.as_ref().is_some_and(|view| view.showing);
        let mut slides = was_slides;
        let mut reload = false;
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(!preview.showing && !slides, "Edit")
                .clicked()
            {
                preview.showing = false;
                slides = false;
            }
            if ui
                .add_enabled(
                    markdown_enabled,
                    egui::Button::selectable(preview.showing && !slides, "Preview"),
                )
                .on_hover_text("Toggle Markdown preview · Ctrl+Shift+R")
                .clicked()
            {
                preview.showing = true;
                slides = false;
            }
            if preview.showing {
                reload = ui.small_button("Reload images").clicked();
                ui.label(self.themes.active().palette.tag(if dirty {
                    "UNSAVED BUFFER"
                } else {
                    "MARKDOWN"
                }));
            }
            if ui
                .add_enabled(marp_enabled, egui::Button::selectable(slides, "Slides"))
                .on_hover_text("Preview the current buffer with Marp on the project host")
                .clicked()
            {
                preview.showing = false;
                slides = true;
            }
        });
        if slides {
            doc.marp
                .get_or_insert_with(marp_view::View::default)
                .showing = true;
            self.marp_ui(ui, ctx);
            return true;
        }
        if let Some(view) = &mut doc.marp {
            view.showing = false;
        }
        if !preview.showing {
            if was_showing || was_slides {
                self.focus_editor(ctx);
            }
            return false;
        }
        if reload {
            preview.invalidate();
        }
        let path = doc.path.clone().unwrap_or(untitled_path);
        let id = doc.id;
        let requests = preview.prepare(doc.edit_version, &path, &doc.text);
        let token = preview.token;
        let cancelled = preview.cancelled.clone();
        let focus = self.take_editor_focus(ui, ctx).is_some() || !was_showing;
        let response = ui.interact(
            ui.available_rect_before_wrap(),
            egui::Id::new(("markdown-focus", id)),
            Sense::click(),
        );
        if focus || response.clicked() {
            response.request_focus();
        }
        let preview = self.documents[self.active].markdown.as_mut().unwrap();
        let selected = egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(18, 8))
            .show(ui, |ui| {
                preview.ui(
                    ui,
                    id,
                    &self.themes.active().id,
                    self.font_size,
                    self.themes.active().palette,
                )
            })
            .inner;
        if !requests.is_empty() {
            self.load_markdown_images(id, token, requests, cancelled, ctx);
        }
        if let Some(target) = selected {
            if target.starts_with("https://")
                || target.starts_with("http://")
                || target.starts_with("mailto:")
            {
                ctx.open_url(egui::OpenUrl::new_tab(target));
            } else {
                match file_target(&path, &target) {
                    Ok(path) => self.open_file(path, ctx),
                    Err(error) => self.notify(error, false),
                }
            }
        }
        true
    }
    fn load_markdown_images(
        &self,
        id: u64,
        token: u64,
        requests: Vec<(String, String)>,
        cancelled: Arc<AtomicBool>,
        ctx: &egui::Context,
    ) {
        let remote = self.workspace.as_ref().and_then(|w| w.remote.clone());
        let plan = self.workspace.as_ref().map(|w| w.plan.clone());
        let budget = self.image_budget.clone();
        let generation = self.generation;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let limit = ctx.input(|i| i.max_texture_side);
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            for (uri, path) in requests {
                if cancelled.load(Ordering::Acquire) {
                    return;
                }
                let _guard = budget.io.lock().unwrap_or_else(|e| e.into_inner());
                if cancelled.load(Ordering::Acquire) {
                    return;
                }
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> anyhow::Result<_> {
                        let request = Request::ReadImage { path: path.clone() };
                        let response = match &remote {
                            Some(remote) => remote.request_in(&request, plan.as_ref()),
                            None => match &plan {
                                Some(plan) => plan.local_request(&request),
                                None => files::local(&request),
                            },
                        }?;
                        crate::images::decode(
                            &response
                                .image
                                .ok_or_else(|| anyhow::anyhow!("Image data missing"))?,
                            &path,
                            &budget,
                            limit,
                        )
                    },
                ))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Image decoder failed")))
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::MarkdownImage {
                    generation,
                    id,
                    token,
                    uri,
                    result,
                });
                ctx.request_repaint();
            }
        });
    }
    pub(super) fn markdown_image(
        &mut self,
        id: u64,
        token: u64,
        uri: String,
        result: Result<crate::images::Decoded, String>,
        ctx: &egui::Context,
    ) {
        if let Some(preview) = self
            .documents
            .iter_mut()
            .find(|d| d.id == id)
            .and_then(|d| d.markdown.as_mut())
            .filter(|p| p.token == token)
        {
            preview.complete(uri, result, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn render(app: &mut EditorApp, ctx: &egui::Context) -> String {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000., 800.),
                )),
                ..Default::default()
            },
            |ctx| app.editor(ctx),
        )
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
    }
    #[test]
    fn images_are_rewritten_to_private_uris_and_paths_stay_on_the_document_host() {
        let source = "# Heading\n\n![quoted \\[alt\\]](assets/a%20🦀.png)\n\n![reference][pic]\n\n[pic]: assets/a%20🦀.png\n\n![web](https://example.org/tracker.png)\n\n[code](../main.rs) [web](https://example.org/) [blocked](javascript:alert(1))\n\n<script>alert(1)</script>";
        let prepared = prepare(source, "/project/docs/README.md", 42).unwrap();
        assert_eq!(prepared.images.len(), 1);
        assert_eq!(prepared.images[0].1, "/project/docs/assets/a 🦀.png");
        assert!(prepared.links.contains("../main.rs"));
        assert!(prepared.links.contains("javascript:alert(1)"));
        assert!(!prepared.warnings.is_empty());
        for event in Parser::new_ext(&prepared.source, options()) {
            if let MdEvent::Start(Tag::Image { dest_url, .. }) = event {
                assert!(dest_url.starts_with(SCHEME), "{dest_url}");
            }
        }
        assert_eq!(
            file_target("/project/docs/a.md", "../main.rs#L2").unwrap(),
            "/project/docs/../main.rs"
        );
        assert!(file_target("/project/a.md", "javascript:alert(1)").is_err());
        assert!(file_target("/project/a.md", "%00.png").is_err());
        assert!(prepare(&"x".repeat(MAX_BYTES + 1), "a.md", 1).is_err());
        assert!(prepare(&"- item\n".repeat(MAX_EVENTS), "a.md", 1).is_err());
        assert!(is_path("README.MARKDOWN"));
        assert!(is_path("notes.mdown"));
    }
    #[test]
    fn preview_uses_unsaved_text_and_preserves_cursor_dirty_state_and_source_saves() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("README.md");
        std::fs::write(&path, "# Original\n").unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        let doc = &mut app.documents[0];
        doc.path = Some(path.to_str().unwrap().into());
        doc.revision = Some(files::digest(b"# Original\n"));
        doc.saved_text = "# Original\n".into();
        doc.text = "# Unsaved heading\n\n**Bold body** and `inline code`.\n\n- [x] Done\n\n| Name | Value |\n| --- | --- |\n| item | 42 |\n".into();
        doc.modified = true;
        doc.edit_version = 1;
        doc.cursor = (2, 3);
        let source = doc.text.clone();
        app.toggle_markdown(&ctx);
        let labels = render(&mut app, &ctx);
        assert!(labels.contains("Unsaved heading"), "{labels}");
        assert!(labels.contains("Bold body"), "{labels}");
        assert!(labels.contains("UNSAVED BUFFER"), "{labels}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Original\n");
        assert_eq!(app.documents[0].text, source);
        assert!(app.documents[0].dirty());
        assert_eq!(app.documents[0].cursor, (2, 3));
        app.save_active(&ctx, false);
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.documents[0].saving && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            app.poll(&ctx);
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        assert!(!app.documents[0].dirty());
        app.toggle_markdown(&ctx);
        assert!(!app.documents[0].markdown.as_ref().unwrap().showing);
        assert_eq!(app.documents[0].text, source);
    }
    #[test]
    fn closed_or_invalidated_previews_release_images_and_reject_old_results() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        app.documents[0].path = Some("/project/README.md".into());
        app.toggle_markdown(&ctx);
        let id = app.documents[0].id;
        let preview = app.documents[0].markdown.as_mut().unwrap();
        let requests = preview.prepare(0, "/project/README.md", "![sample](sample.png)");
        let token = preview.token;
        let cancel = preview.cancelled.clone();
        use std::io::Cursor;
        let mut bytes = Cursor::new(vec![]);
        ::image::DynamicImage::ImageRgba8(::image::RgbaImage::new(2, 2))
            .write_to(&mut bytes, ::image::ImageFormat::Png)
            .unwrap();
        let decoded =
            crate::images::decode_bytes(bytes.get_ref(), "sample.png", &app.image_budget, 4096)
                .unwrap();
        app.markdown_image(id, token, requests[0].0.clone(), Ok(decoded), &ctx);
        assert!(matches!(
            app.markdown_images
                .load(&ctx, &requests[0].0, Default::default(), Default::default()),
            Ok(TexturePoll::Ready { .. })
        ));
        assert_eq!(app.image_budget.used(), 16);
        app.clear_project(true);
        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(app.image_budget.used(), 0);
        let decoded =
            crate::images::decode_bytes(bytes.get_ref(), "sample.png", &app.image_budget, 4096)
                .unwrap();
        // The source key was invalidated; prepare the new host context before accepting results.
        app.documents[0].markdown.as_mut().unwrap().prepare(
            0,
            "/project/README.md",
            "![sample](sample.png)",
        );
        app.markdown_image(id, token, requests[0].0.clone(), Ok(decoded), &ctx);
        assert_eq!(app.image_budget.used(), 0);
        app.documents.clear();
        assert_eq!(app.markdown_images.byte_size(), 0);
    }
}
