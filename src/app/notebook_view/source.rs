//! Stateful source editing, highlighting and keyboard submission for one cell.
//!
//! Stable document/cell IDs preserve cursor state across layout and reordering.
//! Source changes retain the byte budget even though TextEdit limits characters.
//! The notebook owns scrolling; the editor expands to the wrapped text height.

use super::*;

/// Borrowed rendering settings and document identity shared by a notebook's cell editors.
pub(super) struct Editor<'a> {
    pub themes: &'a theme::Themes,
    pub document: u64,
    pub font_size: f32,
    pub palette: theme::Palette,
}

/// One cell interaction result; callers synchronize accepted edits and schedule execution.
pub(super) struct Edit {
    pub focused: bool,
    /// The edit passed the UTF-8 byte budget and advanced the cell source version.
    pub changed: bool,
    /// Shift+Enter was consumed by this focused editor; execution is still the caller's action.
    pub submitted: bool,
    pub error: Option<&'static str>,
}

impl Editor<'_> {
    /// Render an expanding, wrapped cell editor using stable document/cell cursor state.
    /// Selection endpoints are character offsets. Shift+Enter submits; edits exceeding
    /// the UTF-8 byte limit are rolled back before advancing the cell source version.
    pub(super) fn show(
        &self,
        ui: &mut egui::Ui,
        cell: &mut Cell,
        language: &str,
        focus: bool,
        selection: Option<(usize, usize)>,
    ) -> Edit {
        let ctx = ui.ctx().clone();
        let id = egui::Id::new(("notebook-source", self.document, cell.uid));
        if let Some((start, end)) = selection {
            let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(start),
                    egui::text::CCursor::new(end),
                )));
            state.store(&ctx, id);
        }
        let submitted = ctx.memory(|memory| memory.has_focus(id))
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::SHIFT, Key::Enter));
        let syntax = crate::languages::all()
            .iter()
            .find(|entry| entry.id.eq_ignore_ascii_case(language))
            .map(|entry| entry.syntax.as_str())
            .unwrap_or(language);
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
            let mut job = if text.as_str().len() < 128 * 1024 {
                self.themes
                    .highlight(ui, text.as_str(), syntax, self.font_size)
            } else {
                egui::text::LayoutJob::simple(
                    text.as_str().into(),
                    FontId::monospace(self.font_size),
                    self.palette.text,
                    wrap,
                )
            };
            job.wrap.max_width = wrap;
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let before = cell.source.clone();
        let response = egui::TextEdit::multiline(&mut cell.source)
            .id(id)
            .code_editor()
            .font(FontId::monospace(self.font_size))
            .desired_width(f32::INFINITY)
            .desired_rows(3)
            .char_limit(notebook::MAX_SOURCE)
            .layouter(&mut layouter)
            .show(ui);
        if focus {
            response.response.request_focus();
        }
        let mut edit = Edit {
            focused: response.response.has_focus(),
            changed: false,
            submitted,
            error: None,
        };
        if cell.source != before {
            if cell.source.len() > notebook::MAX_SOURCE {
                cell.source = before;
                edit.error = Some("Cell source exceeds 256 KiB.");
            } else {
                cell.set_source(cell.source.clone());
                edit.changed = true;
            }
        }
        edit
    }
}
