//! Bounded, selectable, read-only unified diffs shared by Git views and editor peeks.
use super::*;

/// Render a bounded, selectable read-only unified diff with themed additions/deletions and
/// independent scrolling; the source/index is never mutated here.
pub(super) fn show(
    ui: &mut egui::Ui,
    text: &str,
    palette: theme::Palette,
    max_height: f32,
) -> egui::Response {
    let mut end = text.len().min(1024 * 1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end < text.len() {
        ui.label("Only the first 1 MiB of this diff is shown.");
    }
    let mut preview = &text[..end];
    let mut layouter = |ui: &egui::Ui, buffer: &dyn egui::TextBuffer, _wrap: f32| {
        let mut job = egui::text::LayoutJob::default();
        for line in buffer.as_str().split_inclusive('\n') {
            let color = if line.starts_with('+') {
                palette.success
            } else if line.starts_with('-') {
                palette.error
            } else if line.starts_with("@@") || line.starts_with("diff ") {
                palette.accent
            } else {
                palette.text
            };
            job.append(
                line,
                0.,
                egui::TextFormat {
                    font_id: FontId::monospace(12.),
                    color,
                    ..Default::default()
                },
            );
        }
        job.wrap.max_width = f32::INFINITY;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    egui::ScrollArea::both()
        .id_salt(ui.id().with("diff-scroll"))
        .max_height(max_height)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut preview)
                    .code_editor()
                    .font(FontId::monospace(12.))
                    .desired_width(ui.available_width())
                    .desired_rows(16)
                    .layouter(&mut layouter),
            )
        })
        .inner
}
