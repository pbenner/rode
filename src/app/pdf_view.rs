//! Shared read-only PDF/PowerPoint/Word reader with virtual continuous pages and bounded caches.
//!
//! Retain encoded source separately from the displayed page. Page workers are cancellable and
//! versioned; wheel navigation crosses page boundaries only after the current page scroll
//! reaches an edge, preserving source tabs and read-only saves.
use super::*;
use crate::pdf;

mod fullscreen;
mod toolbar;
pub(super) use fullscreen::Fullscreen;

/// Toolbar capabilities for a standalone reader or an embedded slide preview.
#[derive(Clone, Copy)]
pub(super) enum Display {
    Document { fullscreen: bool },
    Slides,
}

/// Reader intents dispatched by the application after releasing the view borrow.
pub(super) struct Response {
    pub reload: bool,
    pub page: Option<(usize, PageEntry)>,
    pub toggle_fullscreen: bool,
}

/// The PDF view that owns a page request. Capture this with the document version
/// so a slide-preview reply cannot replace a standalone PDF page or vice versa.
#[derive(Clone, Copy)]
pub(super) enum Target {
    Document,
    Presentation,
}
impl Target {
    fn view(self, doc: &mut Document) -> Option<&mut PdfView> {
        match self {
            Self::Document => doc.pdf.as_mut(),
            Self::Presentation => doc.marp.as_mut().and_then(|view| view.pdf.as_mut()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// Which vertical edge to reveal after a page turn; page numbers themselves are one-based.
pub(super) enum PageEntry {
    #[default]
    Start,
    End,
}

#[derive(Default)]
/// Gesture accumulation and cooldown timestamps that prevent one wheel gesture skipping pages.
struct PageScroll {
    accumulated: f32,
    last_input: Option<f64>,
    last_turn: Option<f64>,
}
impl PageScroll {
    /// Convert accumulated edge scrolling into at most one adjacent one-based page request
    /// while respecting gesture direction, idle reset and page-turn cooldown.
    fn turn(
        &mut self,
        delta: f32,
        time: f64,
        page: usize,
        pages: usize,
    ) -> Option<(usize, PageEntry)> {
        if delta.abs() < 0.01 {
            return None;
        }
        if self.last_input.is_none_or(|last| time - last > 0.25)
            || self.accumulated.signum() != delta.signum()
        {
            self.accumulated = 0.;
        }
        self.last_input = Some(time);
        // Do not let renderer latency or a gesture's remaining momentum turn
        // several pages as soon as a new page appears.
        if self.last_turn.is_some_and(|last| time - last < 0.2) {
            self.accumulated = 0.;
            return None;
        }
        self.accumulated += delta;
        let target = if self.accumulated <= -32. && page < pages {
            Some((page + 1, PageEntry::Start))
        } else if self.accumulated >= 32. && page > 1 {
            Some((page - 1, PageEntry::End))
        } else {
            None
        };
        if target.is_some() {
            self.last_turn = Some(time);
            self.accumulated = 0.;
        }
        target
    }
    fn reset(&mut self) {
        self.accumulated = 0.;
        self.last_input = None;
    }
}

/// One admitted page job. A view keeps at most one worker alive; a newer
/// navigation updates the desired page and cancels obsolete work, rather than
/// spawning an unbounded queue of threads behind the renderer.
struct Work {
    token: u64,
    page: usize,
}

/// Cached virtual-document geometry. Only the visible page range is painted;
/// page dimensions are known before any of their rasters have been generated.
#[derive(Default)]
struct PageLayout {
    size: egui::Vec2,
    scale: f32,
    tops: Vec<f32>,
    sizes: Vec<egui::Vec2>,
    total: egui::Vec2,
}
impl PageLayout {
    fn new(source: &pdf::Source, viewport: egui::Vec2, scale: f32) -> Self {
        let mut result = Self {
            size: viewport,
            scale,
            ..Default::default()
        };
        let mut top = 16.;
        let mut width = (viewport.x - 16.).max(1.);
        for [w, h] in &source.sizes {
            let factor = pdf::RENDER_SIDE as f32 / w.max(*h);
            let size = egui::vec2(w * factor, h * factor) * scale;
            result.tops.push(top);
            result.sizes.push(size);
            top += size.y + 24.;
            width = width.max(size.x + 32.);
        }
        result.total = egui::vec2(width, top);
        result
    }
    fn page_at(&self, y: f32) -> usize {
        self.tops
            .partition_point(|top| *top <= y)
            .saturating_sub(1)
            .min(self.tops.len().saturating_sub(1))
    }
}

/// Per-tab reader state, a bounded texture cache and a cancellable render
/// admission. CPU page/font caches belong to the shared original source.
pub(super) struct PdfView {
    pub source: Arc<pdf::Source>,
    pub page: usize,
    requested: usize,
    wanted: usize,
    image_page: usize,
    token: u64,
    cancel: Arc<AtomicBool>,
    work: Option<Work>,
    reloading: bool,
    pub image: image_view::ImageView,
    textures: std::collections::VecDeque<(usize, image_view::ImageView)>,
    failed: std::collections::HashSet<usize>,
    scroll: PageScroll,
    entry: PageEntry,
    continuous: bool,
    settings_open: bool,
    layout: PageLayout,
    offset: egui::Vec2,
    jump: Option<(usize, PageEntry)>,
    visible: Vec<usize>,
}
impl PdfView {
    /// Initialize the reader from a bounded initial raster.
    pub fn new(loaded: pdf::Loaded) -> Self {
        Self {
            source: loaded.source,
            page: loaded.page,
            requested: loaded.page,
            wanted: loaded.page,
            image_page: loaded.page,
            token: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            work: None,
            reloading: false,
            image: image_view::ImageView::new(loaded.pixels),
            textures: Default::default(),
            failed: Default::default(),
            scroll: Default::default(),
            entry: PageEntry::Start,
            continuous: false,
            settings_open: false,
            layout: Default::default(),
            offset: egui::Vec2::ZERO,
            jump: None,
            visible: vec![],
        }
    }
    /// Cancellation flag for the current page request or a guarded reload.
    pub fn cancellation(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
    /// Cancel obsolete work while retaining the last successful document.
    pub fn invalidate(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.token = self.token.wrapping_add(1);
        self.work = None;
        self.reloading = false;
        self.image.loading = false;
        self.wanted = self.page;
        self.requested = self.page;
        self.scroll.reset();
        self.entry = PageEntry::Start;
    }
    /// Install a fresh source only after a successful reload, preserving reading mode.
    pub fn replace(&mut self, loaded: pdf::Loaded) {
        self.invalidate();
        self.source = loaded.source;
        self.page = loaded.page;
        self.wanted = loaded.page;
        self.image_page = loaded.page;
        self.requested = loaded.page;
        self.image.replace(loaded.pixels);
        self.textures.clear();
        self.failed.clear();
        self.layout = Default::default();
        self.jump = Some((self.page, PageEntry::Start));
    }
    /// End a failed reload while preserving its previous source and caches.
    pub fn reload_failed(&mut self) {
        self.reloading = false;
        self.image.loading = false;
    }
    fn has_texture(&self, page: usize) -> bool {
        self.image_page == page || self.textures.iter().any(|(p, _)| *p == page)
    }
    fn activate(&mut self, page: usize, mut image: image_view::ImageView) {
        image.adopt_presentation(&self.image);
        let old = std::mem::replace(&mut self.image, image);
        if self.continuous {
            self.textures.push_back((self.image_page, old));
        }
        while self.textures.len() > 2 {
            self.textures.pop_front();
        }
        self.image_page = page;
        if !self.continuous {
            self.page = page;
            self.requested = page;
        }
        if matches!(self.entry, PageEntry::End) {
            self.image.scroll_to_end();
        }
    }
    fn cached_page(&mut self, page: usize, limit: usize) -> bool {
        if self.image_page == page {
            self.image.loading = false;
            return true;
        }
        let image = if let Some(index) = self.textures.iter().position(|(p, _)| *p == page) {
            self.textures.remove(index).map(|(_, image)| image)
        } else {
            self.source
                .cached(page, limit)
                .map(image_view::ImageView::new)
        };
        if let Some(image) = image {
            self.activate(page, image);
            true
        } else {
            false
        }
    }
    fn navigate(&mut self, page: usize, entry: PageEntry, limit: usize) {
        let page = page.clamp(1, self.source.pages);
        self.wanted = page;
        self.requested = page;
        self.entry = entry;
        self.failed.remove(&page);
        if self.continuous {
            self.page = page;
            self.jump = Some((page, entry));
        }
        let cached = self.cached_page(page, limit);
        self.image.loading = !cached;
        if self.work.as_ref().is_some_and(|w| w.page != page) {
            self.cancel.store(true, Ordering::Release);
        }
    }
    /// Select one foreground/visible page before bounded neighbour prefetch.
    fn next_page(&mut self, limit: usize) -> Option<(usize, PageEntry)> {
        if self.reloading {
            return None;
        }
        // Scrolling may return to a ready page before obsolete work completes.
        // Derive loading from the desired page, rather than the outstanding job,
        // so its completion cannot leave the spinner and Reload stuck.
        let cached = self.cached_page(self.wanted, limit);
        self.image.loading = !cached && !self.failed.contains(&self.wanted);
        if self.work.is_some() {
            return None;
        }
        if self.image.loading {
            return Some((self.wanted, self.entry));
        }
        let mut pages = if self.continuous {
            self.visible.clone()
        } else {
            vec![]
        };
        pages.sort_by_key(|page| page.abs_diff(self.page));
        pages.truncate(3);
        for page in [self.page.saturating_add(1), self.page.saturating_sub(1)] {
            if (1..=self.source.pages).contains(&page) && !pages.contains(&page) {
                pages.push(page);
            }
        }
        pages
            .into_iter()
            .find(|page| {
                !self.has_texture(*page)
                    && !self.failed.contains(page)
                    && self.source.cached(*page, limit).is_none()
            })
            .map(|page| (page, PageEntry::Start))
    }
    /// Render navigation immediately from caches; schedule only missing rasters.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        palette: theme::Palette,
        focus: bool,
        keyboard: bool,
        display: Display,
    ) -> Response {
        let ctx = ui.ctx().clone();
        let limit = ctx.input(|i| i.max_texture_side);
        let controls = self.toolbar(ui, palette, display);
        let mut page = controls.page;
        let mut entry = PageEntry::Start;
        let focused = keyboard
            && !self.reloading
            && ctx.memory(|m| m.has_focus(image_view::ImageView::focus_id(id)));
        if focused && !self.continuous {
            ctx.input(|i| {
                if i.modifiers.is_none() {
                    if i.key_pressed(Key::PageDown)
                        || (self.source.format == pdf::Format::Pptx
                            && i.key_pressed(Key::ArrowRight))
                    {
                        page = Some((self.wanted + 1).min(self.source.pages));
                    }
                    if i.key_pressed(Key::PageUp)
                        || (self.source.format == pdf::Format::Pptx
                            && i.key_pressed(Key::ArrowLeft))
                    {
                        page = Some(self.wanted.saturating_sub(1).max(1));
                    }
                    if i.key_pressed(Key::Home) {
                        page = Some(1);
                    }
                    if i.key_pressed(Key::End) {
                        page = Some(self.source.pages);
                    }
                }
            });
        }
        if self.continuous {
            self.continuous_ui(ui, &ctx, id, palette, focus, focused);
        } else {
            let response = self.image.pdf_ui(ui, &ctx, id, palette, focus);
            if keyboard && response.hovered && ctx.input(|i| i.modifiers.is_none()) {
                ctx.input_mut(|i| i.smooth_scroll_delta.y = 0.);
                let edge = (response.edge_scroll > 0. && response.at_start)
                    || (response.edge_scroll < 0. && response.at_end);
                if !self.image.loading && page.is_none() && edge {
                    if let Some((p, e)) = self.scroll.turn(
                        response.edge_scroll,
                        ctx.input(|i| i.time),
                        self.page,
                        self.source.pages,
                    ) {
                        page = Some(p);
                        entry = e;
                    }
                } else {
                    self.scroll.reset();
                }
            } else {
                self.scroll.reset();
            }
            if !self.image.loading {
                ui_probe::button(
                    ui,
                    &format!("pdf-page-ready:{}", self.image_page),
                    &ui.interact(
                        ui.clip_rect(),
                        egui::Id::new(("pdf-ready", id)),
                        Sense::hover(),
                    ),
                );
            }
        }
        if let Some(page) = page {
            self.navigate(page, entry, limit);
            ctx.request_repaint();
        }
        Response {
            reload: controls.reload,
            page: self.next_page(limit),
            toggle_fullscreen: controls.toggle_fullscreen,
        }
    }
    fn continuous_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        id: u64,
        palette: theme::Palette,
        focus: bool,
        keyboard: bool,
    ) {
        let limit = ctx.input(|i| i.max_texture_side);
        let viewport = ui.available_size().max(egui::vec2(1., 1.));
        let mut largest = egui::Vec2::ZERO;
        for [w, h] in &self.source.sizes {
            let factor = pdf::RENDER_SIDE as f32 / w.max(*h);
            largest = largest.max(egui::vec2(w * factor, h * factor));
        }
        let width_fit = ((viewport.x - 40.).max(1.) / largest.x).max(0.001);
        let fit = if self.image.fitting_width() {
            width_fit
        } else {
            width_fit
                .min((viewport.y - 32.).max(1.) / largest.y)
                .min(1.)
        };
        let scale = self.image.zoom_factor().unwrap_or(fit).max(0.001);
        self.image.displayed_at(scale);
        let mut reposition = false;
        if self.layout.size != viewport
            || (self.layout.scale - scale).abs() > f32::EPSILON
            || self.layout.tops.is_empty()
        {
            let (anchor, fraction) = if self.layout.tops.is_empty() {
                (self.page - 1, 0.)
            } else {
                let index = self.layout.page_at(self.offset.y + viewport.y * 0.3);
                (
                    index,
                    ((self.offset.y + viewport.y * 0.3 - self.layout.tops[index])
                        / self.layout.sizes[index].y)
                        .clamp(0., 1.),
                )
            };
            reposition = true;
            self.layout = PageLayout::new(&self.source, viewport, scale);
            self.offset.y = (self.layout.tops[anchor] + fraction * self.layout.sizes[anchor].y
                - viewport.y * 0.3)
                .max(0.);
        }
        if let Some((page, entry)) = self.jump.take() {
            reposition = true;
            self.offset.y = self.layout.tops[page - 1];
            if matches!(entry, PageEntry::End) {
                self.offset.y += (self.layout.sizes[page - 1].y - viewport.y).max(0.);
            }
        }
        if keyboard {
            let before = self.offset;
            ctx.input(|i| {
                if i.modifiers.is_none() {
                    if i.key_pressed(Key::PageDown)
                        || (i.key_pressed(Key::Space) && !i.modifiers.shift)
                    {
                        self.offset.y += viewport.y * 0.9;
                    }
                    if i.key_pressed(Key::PageUp) {
                        self.offset.y -= viewport.y * 0.9;
                    }
                    if i.key_pressed(Key::ArrowDown) {
                        self.offset.y += 40.;
                    }
                    if i.key_pressed(Key::ArrowUp) {
                        self.offset.y -= 40.;
                    }
                    if i.key_pressed(Key::Home) {
                        self.offset.y = 0.;
                    }
                    if i.key_pressed(Key::End) {
                        self.offset.y = self.layout.total.y;
                    }
                }
            });
            reposition |= self.offset != before;
        }
        self.offset.y = self
            .offset
            .y
            .clamp(0., (self.layout.total.y - viewport.y).max(0.));
        let mut scroller = egui::ScrollArea::both()
            .id_salt(("pdf-continuous", id))
            .auto_shrink([false, false]);
        if reposition {
            scroller = scroller.scroll_offset(self.offset).animated(false);
        }
        let output = scroller.show_viewport(ui, |ui, visible| {
            ui.set_min_size(self.layout.total);
            self.visible.clear();
            let first = self.layout.page_at(visible.top()).saturating_sub(1);
            let last = (self.layout.page_at(visible.bottom()) + 1).min(self.source.pages - 1);
            for index in first..=last {
                let page = index + 1;
                let size = self.layout.sizes[index];
                let rect = egui::Rect::from_min_size(
                    ui.min_rect().min
                        + egui::vec2(
                            (self.layout.total.x - size.x) * 0.5,
                            self.layout.tops[index],
                        ),
                    size,
                );
                if !rect.intersects(ui.clip_rect()) {
                    continue;
                }
                self.visible.push(page);
                ui.painter().rect_filled(rect, 2, egui::Color32::WHITE);
                ui.painter().rect_stroke(
                    rect.expand(0.5),
                    2,
                    egui::Stroke::new(1_f32, palette.border),
                    egui::StrokeKind::Outside,
                );
                let ready = if self.image_page == page {
                    self.image.paint(ui, ctx, id, rect);
                    true
                } else {
                    if !self.textures.iter().any(|(p, _)| *p == page)
                        && let Some(pixels) = self.source.cached(page, limit)
                    {
                        self.textures
                            .push_back((page, image_view::ImageView::new(pixels)));
                        while self.textures.len() > 2 {
                            self.textures.pop_front();
                        }
                    }
                    if let Some((_, image)) = self.textures.iter_mut().find(|(p, _)| *p == page) {
                        image.paint(ui, ctx, id.wrapping_add(page as u64), rect);
                        true
                    } else {
                        false
                    }
                };
                let response =
                    ui.interact(rect, egui::Id::new(("pdf-page", id, page)), Sense::hover());
                if ready {
                    ui_probe::button(ui, &format!("pdf-page-ready:{page}"), &response);
                } else {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        if self.failed.contains(&page) {
                            format!("Page {page} · Could not render. Use Go to retry.")
                        } else {
                            format!("Page {page} · Rendering…")
                        },
                        egui::FontId::proportional(14.),
                        egui::Color32::GRAY,
                    );
                }
            }
            let response = ui.interact(
                ui.min_rect(),
                image_view::ImageView::focus_id(id),
                Sense::click(),
            );
            ui_probe::button(ui, "pdf-document", &response);
            if focus || response.clicked() {
                response.request_focus();
            }
            let selected = self.layout.page_at(visible.center().y) + 1;
            if self.page != selected {
                self.page = selected;
                self.requested = selected;
                ctx.request_repaint();
            }
            self.wanted = selected;
        });
        self.offset = output.state.offset;
        self.textures
            .retain(|(page, _)| self.visible.contains(page));
        if self.work.as_ref().is_some_and(|work| {
            !self.visible.contains(&work.page) && work.page.abs_diff(self.page) > 1
        }) {
            self.cancel.store(true, Ordering::Release);
        }
    }
    /// Admit a single worker. Navigation remains available while it runs; later
    /// targets coalesce and are scheduled after cancellation is acknowledged.
    pub fn begin_page(
        &mut self,
        page: usize,
        entry: PageEntry,
    ) -> Option<(Arc<pdf::Source>, Arc<AtomicBool>, u64)> {
        if self.work.is_some() || self.reloading || !(1..=self.source.pages).contains(&page) {
            return None;
        }
        self.token = self.token.wrapping_add(1);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.work = Some(Work {
            token: self.token,
            page,
        });
        self.entry = entry;
        if page == self.wanted && page != self.image_page {
            self.image.loading = true;
        }
        Some((self.source.clone(), self.cancel.clone(), self.token))
    }
    /// Admit only the active token, retaining the previous page on failures and
    /// keeping speculative-render failures quiet until an explicit navigation.
    pub fn complete_page(
        &mut self,
        token: u64,
        page: usize,
        result: Result<crate::images::Decoded, String>,
    ) -> Option<String> {
        if self
            .work
            .as_ref()
            .is_none_or(|work| work.token != token || work.page != page)
        {
            return None;
        }
        self.work = None;
        if self.cancel.load(Ordering::Acquire) {
            return None;
        }
        match result {
            Ok(pixels) => {
                if page == self.wanted {
                    self.activate(page, image_view::ImageView::new(pixels));
                    self.image.loading = false;
                }
                None
            }
            Err(error) => {
                self.failed.insert(page);
                if page == self.wanted {
                    self.wanted = self.image_page;
                    if !self.continuous {
                        self.page = self.image_page;
                    }
                    self.requested = self.page;
                    self.image.loading = false;
                    Some(error)
                } else {
                    None
                }
            }
        }
    }
}
impl Drop for PdfView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl EditorApp {
    /// Cancel the old project's queued PDF work before installing a fresh lifetime.
    pub(super) fn reset_pdf_requests(&mut self) {
        self.pdf_cancel.store(true, Ordering::Release);
        self.pdf_cancel = Arc::new(AtomicBool::new(false));
    }

    /// Render the active PDF and dispatch requested page turns/reloads after releasing its borrow.
    pub(super) fn pdf_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        let Some(doc) = self.documents.active().filter(|doc| doc.pdf.is_some()) else {
            return false;
        };
        let id = doc.id;
        let focus = self.take_editor_focus(ui, ctx) == Some(id);
        let keyboard = self.dialog == Dialog::None
            && !self.pending.modal()
            && !self.workbench_modal()
            && !egui::Popup::is_any_open(ctx);
        let palette = self.themes.active().palette;
        let fullscreen = self.editor.pdf_fullscreen.is_some();
        let view = self.documents.active_mut().unwrap().pdf.as_mut().unwrap();
        let response = view.ui(
            ui,
            id,
            palette,
            focus,
            keyboard,
            Display::Document { fullscreen },
        );
        if response.toggle_fullscreen {
            self.toggle_pdf_fullscreen_in(ctx.viewport_id(), ctx);
        }
        if response.reload {
            self.reload_pdf(id, ctx);
        } else if let Some((page, entry)) = response.page {
            self.render_pdf_page(id, Target::Document, page, entry, ctx);
        }
        true
    }

    /// Schedule one bounded page render for either PDF presentation, retaining
    /// its cancellation, image budget and encrypted-mount guard through publication.
    pub(super) fn render_pdf_page(
        &mut self,
        id: u64,
        target: Target,
        page: usize,
        entry: PageEntry,
        ctx: &egui::Context,
    ) {
        let Some(doc) = self.documents.iter_mut().find(|doc| doc.id == id) else {
            return;
        };
        let Some(view) = target.view(doc) else {
            return;
        };
        let Some((source, cancel, token)) = view.begin_page(page, entry) else {
            return;
        };
        let version = doc.version();
        let budget = self.image_budget.clone();
        let replies = self.workers.workspace(ctx);
        let texture_limit = ctx.input(|input| input.max_texture_side);
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let result = source
                .render(page, &budget, texture_limit, &cancel)
                .map_err(|error| format!("{error:#}"));
            let _ = replies.send(WorkspaceEvent::PdfPage {
                target,
                id,
                version,
                token,
                page,
                result,
            });
        });
    }
    /// Admit a page reply only to its captured view, document version and request token.
    pub(super) fn pdf_page(
        &mut self,
        target: Target,
        id: u64,
        version: u64,
        token: u64,
        page: usize,
        result: Result<crate::images::Decoded, String>,
    ) {
        let Some(doc) = self
            .documents
            .iter_mut()
            .find(|doc| doc.id == id && doc.version() == version)
        else {
            return;
        };
        let Some(view) = target.view(doc) else {
            return;
        };
        if let Some(error) = view.complete_page(token, page, result) {
            self.notify(error, true);
        }
    }
    /// Reload the bound PDF through its original host/policy while retaining the old view on failure.
    pub(super) fn reload_pdf(&mut self, id: u64, ctx: &egui::Context) {
        let Some(doc) = self.documents.iter_mut().find(|doc| doc.id == id) else {
            return;
        };
        let Some(path) = doc.path.clone() else {
            return;
        };
        let Some(view) = &mut doc.pdf else {
            return;
        };
        if view.reloading {
            return;
        }
        view.invalidate();
        view.cancel = Arc::new(AtomicBool::new(false));
        view.image.loading = true;
        view.reloading = true;
        let version = doc.begin_preview_reload();
        self.dispatch(
            Request::ReadBinary { path },
            Job::Reload { id, version },
            ctx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn office_previews_stay_read_only_and_use_their_own_feature_gates() {
        use base64::Engine as _;
        for (format, feature, bytes, notice) in [
            (
                pdf::Format::Pptx,
                crate::extensions::Feature::Pptx,
                include_bytes!("../../tests/fixtures/slides.pptx").as_slice(),
                "PowerPoint previews are read-only.",
            ),
            (
                pdf::Format::Docx,
                crate::extensions::Feature::Docx,
                include_bytes!("../../tests/fixtures/word.docx").as_slice(),
                "Word previews are read-only.",
            ),
        ] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            let loaded = pdf::load_format_cancellable(
                &encoded,
                format,
                1,
                &app.image_budget,
                128,
                &AtomicBool::new(false),
            )
            .unwrap();
            app.new_file();
            let doc = app.documents.active_mut().unwrap();
            doc.pdf = Some(PdfView::new(loaded));
            doc.set_dirty_fixture(false);
            assert_eq!(doc.paged_feature(), Some(feature));
            assert!(app.documents.active_source().is_none());
            for save_as in [false, true] {
                app.save_active(&ctx, save_as);
                assert!(app.dialog == Dialog::None);
                assert_eq!(app.notice.as_ref().unwrap().0, notice);
            }
            app.set_extension(crate::extensions::Feature::Pdf.id(), false);
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                app.toggle_pdf_fullscreen(ctx)
            });
            assert!(
                app.editor.pdf_fullscreen.is_some(),
                "PDF settings must not disable Office previews"
            );
            app.set_extension(feature.id(), false);
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                assert!(!app.pdf_fullscreen_ui(ctx));
                app.toggle_pdf_fullscreen(ctx);
            });
            assert!(
                app.editor.pdf_fullscreen.is_none(),
                "Fullscreen must honor the disabled document feature"
            );
            assert!(!app.documents.active().unwrap().dirty());
        }
    }
    #[test]
    fn wheel_paging_accumulates_trackpad_motion_and_keeps_document_boundaries() {
        let mut scroll = PageScroll::default();
        for (delta, time) in [(-8., 0.), (-8., 0.05), (-8., 0.1)] {
            assert!(scroll.turn(delta, time, 1, 3).is_none());
        }
        assert_eq!(scroll.turn(-8., 0.15, 1, 3), Some((2, PageEntry::Start)));
        assert!(
            scroll.turn(-100., 0.2, 2, 3).is_none(),
            "Ignore the rest of a fast wheel gesture"
        );
        assert_eq!(scroll.turn(32., 0.4, 2, 3), Some((1, PageEntry::End)));
        assert!(
            scroll.turn(100., 1., 1, 3).is_none(),
            "Never go before the first page"
        );
        assert!(
            scroll.turn(-100., 1.5, 3, 3).is_none(),
            "Never go past the last page"
        );
        scroll.reset();
        assert!(scroll.turn(-20., 2., 1, 3).is_none());
        assert!(
            scroll.turn(-20., 3., 1, 3).is_none(),
            "Do not retain motion from an old gesture"
        );
        assert!(
            scroll.turn(20., 3.05, 2, 3).is_none(),
            "Reversing direction starts a new gesture"
        );
    }

    #[test]
    fn pdf_tabs_are_read_only_and_discard_stale_page_results() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("sample.pdf");
        let bytes = include_bytes!("../../tests/fixtures/pages.pdf");
        std::fs::write(&path, bytes).unwrap();
        let budget = app.image_budget.clone();
        let loaded = pdf::load_bytes(bytes.to_vec(), 1, &budget, 128).unwrap();
        app.new_file();
        let doc = &mut app.documents[0];
        doc.fixture_content().path = Some(path.to_string_lossy().into());
        doc.set_dirty_fixture(false);
        doc.pdf = Some(PdfView::new(loaded));
        let id = doc.id;
        app.save_active(&ctx, false);
        app.save_active(&ctx, true);
        app.save_document(
            id,
            root.path().join("copy.pdf").to_string_lossy().into(),
            true,
            &ctx,
        );
        assert!(matches!(app.dialog, Dialog::None));
        assert!(!root.path().join("copy.pdf").exists());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let view = app.documents[0].pdf.as_mut().unwrap();
        view.navigate(2, PageEntry::Start, 128);
        let (_, _, admitted) = view.begin_page(2, PageEntry::Start).unwrap();
        for token in [admitted.wrapping_add(1), admitted] {
            let pixels = app.documents[0]
                .pdf
                .as_ref()
                .unwrap()
                .source
                .render(2, &budget, 128, &AtomicBool::new(false))
                .unwrap();
            app.pdf_page(Target::Document, id, 0, token, 2, Ok(pixels));
            assert_eq!(
                app.documents[0].pdf.as_ref().unwrap().page,
                if token == admitted { 2 } else { 1 }
            );
        }
        let view = app.documents[0].pdf.as_mut().unwrap();
        view.wanted = 1;
        let (_, _, failed) = view.begin_page(1, PageEntry::Start).unwrap();
        app.pdf_page(
            Target::Document,
            id,
            0,
            failed,
            1,
            Err("Renderer failed".into()),
        );
        assert_eq!(app.documents[0].pdf.as_ref().unwrap().page, 2);
        assert!(!app.documents[0].preview_loading());
        app.clear_project(
            documents::Retention::Keep,
            workspace::LeaveReason::ProjectChanged,
        );
        let pixels = app.documents[0]
            .pdf
            .as_ref()
            .unwrap()
            .source
            .render(1, &budget, 128, &AtomicBool::new(false))
            .unwrap();
        app.pdf_page(Target::Document, id, 0, 0, 1, Ok(pixels));
        assert_eq!(app.documents[0].pdf.as_ref().unwrap().page, 2);
        assert!(!app.documents[0].dirty());
        drop(app);
        assert_eq!(budget.used(), 0);
    }
    fn fixture() -> (PdfView, Arc<crate::images::Budget>) {
        let budget = Arc::new(crate::images::Budget::default());
        let loaded = pdf::load_bytes(
            include_bytes!("../../tests/fixtures/pages.pdf").to_vec(),
            1,
            &budget,
            128,
        )
        .unwrap();
        (PdfView::new(loaded), budget)
    }
    #[test]
    fn rapid_navigation_coalesces_and_cached_back_navigation_stays_immediate() {
        let (mut view, _budget) = fixture();
        view.navigate(2, PageEntry::Start, 128);
        let (_, cancel, token) = view.begin_page(2, PageEntry::Start).unwrap();
        view.navigate(1, PageEntry::Start, 128);
        assert_eq!(view.page, 1);
        assert!(!view.image.loading);
        assert!(cancel.load(Ordering::Acquire));
        view.navigate(2, PageEntry::Start, 128);
        assert!(
            view.next_page(128).is_none(),
            "Keep one admitted worker until it acknowledges cancellation"
        );
        assert!(view.begin_page(2, PageEntry::Start).is_none());
        assert!(
            view.complete_page(token, 2, Err("cancelled".into()))
                .is_none()
        );
        assert_eq!(view.next_page(128), Some((2, PageEntry::Start)));
    }
    #[test]
    fn scrolling_back_to_cached_content_clears_loading_before_other_work_finishes() {
        let (mut view, _budget) = fixture();
        view.continuous = true;
        view.wanted = 2;
        let (_, _, token) = view.begin_page(2, PageEntry::Start).unwrap();
        assert!(view.image.loading);
        // Continuous scrolling updates the desired page independently of the
        // toolbar's navigation method, while an adjacent page may keep rendering.
        view.wanted = 1;
        assert!(view.next_page(128).is_none());
        assert!(!view.image.loading);
        assert!(view.work.is_some());
        assert!(
            view.complete_page(token, 2, Err("unsupported page".into()))
                .is_none()
        );
        assert!(!view.image.loading);
        assert_eq!(view.page, 1);
    }
    #[test]
    fn speculative_failures_do_not_spin_and_reload_blocks_page_admission() {
        let (mut view, _budget) = fixture();
        let (_, _, token) = view.begin_page(2, PageEntry::Start).unwrap();
        assert!(
            view.complete_page(token, 2, Err("unsupported page".into()))
                .is_none()
        );
        assert!(view.next_page(128).is_none());
        view.navigate(2, PageEntry::Start, 128);
        assert_eq!(view.next_page(128), Some((2, PageEntry::Start)));
        view.reloading = true;
        assert!(view.next_page(128).is_none());
        assert!(view.begin_page(2, PageEntry::Start).is_none());
        view.reload_failed();
        assert_eq!(view.next_page(128), Some((2, PageEntry::Start)));
    }
    #[test]
    fn continuous_geometry_preserves_orientation_spacing_and_page_lookup() {
        let (view, _) = fixture();
        let layout = PageLayout::new(&view.source, egui::vec2(900., 700.), 0.4);
        assert!(layout.sizes[0].y > layout.sizes[0].x);
        assert!(layout.sizes[1].x > layout.sizes[1].y);
        assert_eq!(layout.tops[1], layout.tops[0] + layout.sizes[0].y + 24.);
        for (index, top) in layout.tops.iter().enumerate() {
            assert_eq!(layout.page_at(top + layout.sizes[index].y * 0.5), index);
        }
        assert_eq!(layout.page_at(-10.), 0);
        assert_eq!(layout.page_at(f32::MAX), 1);
    }
}
