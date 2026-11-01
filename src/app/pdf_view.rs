//! Read-only PDF document view. Only the displayed page retains a texture.
//!
//! Retain encoded source separately from the displayed page. Page workers are cancellable and
//! versioned; wheel navigation crosses page boundaries only after the current page scroll
//! reaches an edge, preserving source tabs and read-only saves.
use super::*;
use crate::pdf;

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

/// Retained encoded PDF plus one displayed page texture and a cancellable requested-page token.
pub(super) struct PdfView {
    pub source: Arc<pdf::Source>,
    pub page: usize,
    requested: usize,
    token: u64,
    cancel: Arc<AtomicBool>,
    pub image: image_view::ImageView,
    scroll: PageScroll,
    entry: PageEntry,
}
impl PdfView {
    /// Initialize the viewer from a bounded, already rendered page snapshot.
    pub fn new(loaded: pdf::Loaded) -> Self {
        Self {
            source: loaded.source,
            page: loaded.page,
            requested: loaded.page,
            token: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            image: image_view::ImageView::new(loaded.pixels),
            scroll: PageScroll::default(),
            entry: PageEntry::Start,
        }
    }
    /// Clone the cancellation flag that the current page worker must observe.
    pub fn cancellation(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
    /// Cancel page work, advance the token and retain the last displayed page as the stable view.
    pub fn invalidate(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.token = self.token.wrapping_add(1);
        self.image.loading = false;
        self.requested = self.page;
        self.scroll.reset();
        self.entry = PageEntry::Start;
    }
    /// Invalidate old page work before installing a successfully reloaded PDF and page image.
    pub fn replace(&mut self, loaded: pdf::Loaded) {
        self.invalidate();
        self.source = loaded.source;
        self.page = loaded.page;
        self.requested = loaded.page;
        self.image.replace(loaded.pixels);
    }
    /// Render PDF navigation and the current image; return page/reload intents to the owning app.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        palette: theme::Palette,
        focus: bool,
        keyboard: bool,
        kind: &str,
    ) -> (bool, Option<(usize, PageEntry)>) {
        let ctx = ui.ctx().clone();
        let mut page = None;
        let mut entry = PageEntry::Start;
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(!self.image.loading, |ui| {
                if ui
                    .add_enabled(
                        self.page > 1,
                        IconButton::icon(Icon::ChevronLeft, "Previous page"),
                    )
                    .clicked()
                {
                    page = Some(self.page - 1);
                }
                ui.label(kind);
                let page_response = ui.add(
                    egui::DragValue::new(&mut self.requested)
                        .range(1..=self.source.pages)
                        .speed(0.1),
                );
                ui.label(format!("of {}", self.source.pages));
                if ui.small_button("Go").clicked()
                    || (page_response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)))
                {
                    page = Some(self.requested);
                }
                if ui
                    .add_enabled(
                        self.page < self.source.pages,
                        IconButton::icon(Icon::ChevronRight, "Next page"),
                    )
                    .clicked()
                {
                    page = Some(self.page + 1);
                }
            });
            ui.label(palette.tag(if kind == "Slide" { "MARP" } else { "PDF" }))
                .on_hover_text(
                    "Scroll to pan; at the bottom/top, continue to the next/previous page.",
                );
        });
        if keyboard && ctx.memory(|memory| memory.has_focus(image_view::ImageView::focus_id(id))) {
            ctx.input(|input| {
                if input.modifiers.is_none() {
                    if input.key_pressed(Key::PageDown) {
                        page = Some((self.page + 1).min(self.source.pages));
                    }
                    if input.key_pressed(Key::PageUp) {
                        page = Some(self.page.saturating_sub(1).max(1));
                    }
                    if input.key_pressed(Key::Home) {
                        page = Some(1);
                    }
                    if input.key_pressed(Key::End) {
                        page = Some(self.source.pages);
                    }
                }
            });
        }
        let response = self.image.ui(ui, &ctx, id, palette, focus);
        if keyboard && response.hovered && ctx.input(|input| input.modifiers.is_none()) {
            // The image scroll area consumes movement within the page. Only
            // movement remaining at an edge may navigate the PDF itself.
            ctx.input_mut(|input| input.smooth_scroll_delta.y = 0.);
            let at_edge = (response.edge_scroll > 0. && response.at_start)
                || (response.edge_scroll < 0. && response.at_end);
            if !self.image.loading && page.is_none() && at_edge {
                if let Some((target, position)) = self.scroll.turn(
                    response.edge_scroll,
                    ctx.input(|input| input.time),
                    self.page,
                    self.source.pages,
                ) {
                    page = Some(target);
                    entry = position;
                }
            } else if self.image.loading
                || page.is_some()
                || response.edge_scroll.abs() > 0.01
                || (!response.at_start && !response.at_end)
            {
                self.scroll.reset();
            }
        } else {
            self.scroll.reset();
        }
        (response.reload, page.map(|page| (page, entry)))
    }
    pub fn begin_page(
        &mut self,
        page: usize,
        entry: PageEntry,
    ) -> Option<(Arc<pdf::Source>, Arc<AtomicBool>, u64)> {
        if self.image.loading || self.page == page {
            self.requested = self.page;
            return None;
        }
        self.invalidate();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.image.loading = true;
        self.entry = entry;
        Some((self.source.clone(), self.cancel.clone(), self.token))
    }
    pub fn complete_page(
        &mut self,
        token: u64,
        page: usize,
        result: Result<crate::images::Decoded, String>,
    ) -> Option<String> {
        if self.token != token {
            return None;
        }
        self.image.loading = false;
        match result {
            Ok(pixels) => {
                self.image.replace(pixels);
                if matches!(self.entry, PageEntry::End) {
                    self.image.scroll_to_end();
                }
                self.page = page;
                self.requested = page;
                None
            }
            Err(error) => {
                self.requested = self.page;
                Some(error)
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
    /// Render the active PDF and dispatch requested page turns/reloads after releasing its borrow.
    pub(super) fn pdf_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        let Some(doc) = self
            .documents
            .get(self.active)
            .filter(|doc| doc.pdf.is_some())
        else {
            return false;
        };
        let id = doc.id;
        let focus = self.take_editor_focus(ui, ctx) == Some(id);
        let keyboard = self.dialog == Dialog::None
            && self.pending.is_none()
            && !self.workbench_modal()
            && !egui::Popup::is_any_open(ctx);
        let palette = self.themes.active().palette;
        let view = self.documents[self.active].pdf.as_mut().unwrap();
        let (reload, page) = view.ui(ui, id, palette, focus, keyboard, "Page");
        if reload {
            self.reload_pdf(id, ctx);
        } else if let Some((page, entry)) = page {
            self.render_pdf_page(id, page, entry, ctx);
        }
        true
    }

    fn render_pdf_page(&mut self, id: u64, page: usize, entry: PageEntry, ctx: &egui::Context) {
        let Some(doc) = self.documents.iter_mut().find(|doc| doc.id == id) else {
            return;
        };
        let Some(view) = &mut doc.pdf else {
            return;
        };
        let Some((source, cancel, token)) = view.begin_page(page, entry) else {
            return;
        };
        let version = doc.edit_version;
        let budget = self.image_budget.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let generation = self.generation;
        let texture_limit = ctx.input(|input| input.max_texture_side);
        let vault_work = self.vaults.work();
        std::thread::spawn(move || {
            let _vault_work = vault_work;
            let _io = budget.io.lock().unwrap_or_else(|error| error.into_inner());
            let result = source
                .render(page, &budget, texture_limit, &cancel)
                .map_err(|error| format!("{error:#}"));
            let _ = tx.send(Event::PdfPage {
                generation,
                id,
                version,
                token,
                page,
                result,
            });
            ctx.request_repaint();
        });
    }
    /// Schedule a bounded page render with captured document version, request token and cancellation.
    pub(super) fn pdf_page(
        &mut self,
        id: u64,
        version: u64,
        token: u64,
        page: usize,
        result: Result<crate::images::Decoded, String>,
    ) {
        let Some(doc) = self
            .documents
            .iter_mut()
            .find(|doc| doc.id == id && doc.edit_version == version)
        else {
            return;
        };
        let Some(view) = &mut doc.pdf else {
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
        let Some(view) = &mut doc.pdf else {
            return;
        };
        let Some(path) = doc.path.clone() else {
            return;
        };
        if view.image.loading {
            return;
        }
        view.invalidate();
        view.cancel = Arc::new(AtomicBool::new(false));
        view.image.loading = true;
        doc.edit_version = doc.edit_version.wrapping_add(1);
        let version = doc.edit_version;
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
        if sandbox::find_program("pdfinfo").is_none() || sandbox::find_program("pdftoppm").is_none()
        {
            return;
        }
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
        doc.path = Some(path.to_string_lossy().into());
        doc.modified = false;
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
        for token in [1, 0] {
            let pixels = app.documents[0]
                .pdf
                .as_ref()
                .unwrap()
                .source
                .render(2, &budget, 128, &AtomicBool::new(false))
                .unwrap();
            app.pdf_page(id, 0, token, 2, Ok(pixels));
            assert_eq!(
                app.documents[0].pdf.as_ref().unwrap().page,
                if token == 0 { 2 } else { 1 }
            );
        }
        app.documents[0].pdf.as_mut().unwrap().image.loading = true;
        app.pdf_page(id, 0, 0, 1, Err("Renderer failed".into()));
        assert_eq!(app.documents[0].pdf.as_ref().unwrap().page, 2);
        assert!(!app.documents[0].preview_loading());
        app.clear_project(true);
        let pixels = app.documents[0]
            .pdf
            .as_ref()
            .unwrap()
            .source
            .render(1, &budget, 128, &AtomicBool::new(false))
            .unwrap();
        app.pdf_page(id, 0, 0, 1, Ok(pixels));
        assert_eq!(app.documents[0].pdf.as_ref().unwrap().page, 2);
        assert!(!app.documents[0].dirty());
        drop(app);
        assert_eq!(budget.used(), 0);
    }
}
