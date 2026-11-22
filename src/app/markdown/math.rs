//! Native math widgets for CommonMark's dollar-delimited math events.
//! Both preview surfaces use the shared asynchronous, bounded chat renderer.
use crate::{
    app::{math, ui_probe},
    theme,
};
use eframe::egui::{self, Align, RichText};

#[cfg(test)]
mod tests;

/// Keep inline equations in the surrounding paragraph and center display math
/// on its own row. Wide equations fit the available line without changing aspect.
pub(super) fn ui(
    ui: &mut egui::Ui,
    source: &str,
    inline: bool,
    font_size: f32,
    palette: theme::Palette,
) {
    let width = ui.max_rect().width().min(ui.clip_rect().width()).max(1.);
    let draw = |ui: &mut egui::Ui| {
        let delimiter = if inline { "$" } else { "$$" };
        let literal = format!("{delimiter}{source}{delimiter}");
        if let Some((texture, size)) = math::texture(ui.ctx(), source, !inline, font_size) {
            let size = size * (width / size.x.max(1.)).min(1.);
            let response = ui
                .add(
                    egui::Image::new((texture.id(), size))
                        .tint(palette.text)
                        .sense(egui::Sense::click()),
                )
                .on_hover_text(format!("{literal}\nClick or press Enter to copy LaTeX"));
            let label = format!(
                "Copy LaTeX: {}",
                source.chars().take(80).collect::<String>()
            );
            crate::keyboard::target(ui, &label, &response);
            theme::button_focus(ui, &response, None);
            if response.clicked() {
                ui.ctx().copy_text(literal.clone());
            }
            ui_probe::button(
                ui,
                if inline {
                    "markdown-math-inline"
                } else {
                    "markdown-math-display"
                },
                &response,
            );
            response.context_menu(|ui| {
                let copy = crate::keyboard::button(ui, "Copy LaTeX");
                ui_probe::button(ui, "markdown-math-copy", &copy);
                if copy.clicked() {
                    ui.ctx().copy_text(literal);
                    ui.close();
                }
            });
        } else {
            // Pending, unsupported and oversized expressions remain readable.
            // A completed render invalidates document scroll geometry next pass.
            let response = ui.add(
                egui::Label::new(RichText::new(literal).monospace().color(palette.text)).wrap(),
            );
            ui_probe::button(ui, "markdown-math-source", &response);
        }
    };
    if inline {
        draw(ui);
    } else {
        ui.end_row();
        ui.allocate_ui_with_layout(
            egui::vec2(width, 0.),
            egui::Layout::top_down(Align::Center),
            |ui| {
                ui.set_min_width(width);
                draw(ui);
            },
        );
        ui.end_row();
    }
}
