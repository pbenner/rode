//! A read-only change inspector inside the source editor. All content comes from
//! the admitted diff snapshot; opening or navigating a peek performs no host I/O.
use super::*;

impl State {
    /// Reveal the inspected source when opening, and restore editor focus when closing.
    pub(in crate::app) fn toggle_peek(
        &mut self,
        doc: &Document,
        index: usize,
    ) -> Option<PeekAction> {
        let hunk = self.current(doc)?.hunks.get(index)?;
        if self.peek == Some(index) {
            self.peek = None;
            Some(PeekAction::Close)
        } else {
            let row = hunk.new.start + 1;
            self.peek = Some(index);
            Some(PeekAction::Navigate(row))
        }
    }

    /// Reserve a bounded part of the editor; source rows retain their own layout.
    pub(in crate::app) fn peek_height(&self, doc: &Document, available: f32) -> f32 {
        if self
            .current(doc)
            .and_then(|changes| changes.hunks.get(self.peek?))
            .is_none()
        {
            return 0.;
        }
        (available * 0.45).min(260.).min((available - 80.).max(0.))
    }

    pub(in crate::app) fn peek_ui(
        &mut self,
        ui: &mut egui::Ui,
        doc: &Document,
        palette: theme::Palette,
        height: f32,
    ) -> Option<PeekAction> {
        let index = self.peek?;
        let Some(changes) = self
            .current(doc)
            .filter(|changes| index < changes.hunks.len())
        else {
            self.peek = None;
            return None;
        };
        if height <= 0. {
            return None;
        }
        if ui.is_enabled()
            && ui.memory(|memory| memory.allows_interaction(ui.layer_id()))
            && !egui::Popup::is_any_open(ui.ctx())
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.peek = None;
            return Some(PeekAction::Close);
        }
        let mut next = index;
        let mut close = false;
        let hunk = &changes.hunks[index];
        egui::Frame::new()
            .fill(palette.raised)
            .stroke(Stroke::new(1_f32, palette.border))
            .inner_margin(8)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.strong("Changes since last commit");
                    ui.label(format!("{} / {}", index + 1, changes.hunks.len()));
                    let previous =
                        ui.add_enabled(changes.hunks.len() > 1, egui::Button::new("Previous"));
                    ui_probe::button(ui, "git-peek-previous", &previous);
                    if previous.clicked() {
                        next = (index + changes.hunks.len() - 1) % changes.hunks.len();
                    }
                    let following =
                        ui.add_enabled(changes.hunks.len() > 1, egui::Button::new("Next"));
                    ui_probe::button(ui, "git-peek-next", &following);
                    if following.clicked() {
                        next = (index + 1) % changes.hunks.len();
                    }
                    let dismiss = ui.button("Close");
                    ui_probe::button(ui, "git-peek-close", &dismiss);
                    close = dismiss.clicked();
                });
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(palette.error, "− Removed");
                    ui.colored_label(palette.success, "+ Added");
                    if doc.dirty() {
                        ui.label("Includes unsaved edits");
                    }
                });
                let body_height = ui.available_height().min(height).max(1.);
                let response = ui
                    .push_id(("git-peek", doc.id, doc.edit_version, index), |ui| {
                        diff_view::show(ui, &hunk.patch, palette, body_height)
                    })
                    .inner;
                ui_probe::button(ui, "git-peek-diff", &response);
                ui_probe::value(ui, "git-peek-patch", &response, &hunk.patch);
            });
        if close {
            self.peek = None;
            Some(PeekAction::Close)
        } else if next != index {
            let row = changes.hunks[next].new.start + 1;
            self.peek = Some(next);
            Some(PeekAction::Navigate(row))
        } else {
            None
        }
    }
}
