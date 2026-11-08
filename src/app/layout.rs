//! Shared presentation for tool pages and dialogs.
//!
//! Keep layout separate from controllers: these helpers own only spacing, reading
//! widths and themed surfaces. Callers retain scroll IDs, state and actions, so
//! docking a page never changes its identity or starts background work.
use crate::theme::Palette;
pub(super) use crate::theme::{card_frame, compact_control_height, control_height, dialog_frame};
use eframe::egui::{self, Align, Layout, RichText};

#[cfg(test)]
pub(super) mod tests;

/// Maximum outer width for lists and forms, including the page's inner margin.
pub(super) const PAGE_WIDTH: f32 = 1000.;

/// Center a bounded, left-aligned page. The parent decides whether its content
/// scrolls; the available height is retained for pages with their own list scroller.
pub(super) fn page<R>(
    ui: &mut egui::Ui,
    max_width: f32,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.with_layout(Layout::top_down(Align::Center), |ui| {
        let width = ui.available_width().min(max_width).max(1.);
        ui.allocate_ui_with_layout(
            egui::vec2(width, ui.available_height()),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(width);
                egui::Frame::new()
                    .inner_margin(12)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width().max(1.));
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                        contents(ui)
                    })
                    .inner
            },
        )
        .inner
    })
    .inner
}

/// Page heading and supporting text. Actions move below the title on narrow
/// surfaces; action order stays identical when a panel is detached or resized.
pub(super) fn header(
    ui: &mut egui::Ui,
    title: &str,
    description: &str,
    palette: Palette,
    actions: impl FnOnce(&mut egui::Ui),
) {
    if ui.available_width() >= 500. {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(title)
                    .size(palette.ui.map_or(18., |style| style.heading_size - 2.))
                    .strong()
                    .color(palette.heading_color()),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), actions);
        });
    } else {
        ui.label(
            RichText::new(title)
                .size(palette.ui.map_or(18., |style| style.heading_size - 2.))
                .strong()
                .color(palette.heading_color()),
        );
        ui.horizontal(|ui| {
            ui.with_layout(
                Layout::right_to_left(Align::Center).with_main_wrap(true),
                actions,
            );
        });
    }
    if !description.is_empty() {
        subtitle(ui, description, palette);
    }
    ui.add_space(8.);
}

/// Heading for pages without toolbar actions.
pub(super) fn heading(ui: &mut egui::Ui, title: &str, description: &str, palette: Palette) {
    ui.label(
        RichText::new(title)
            .size(palette.ui.map_or(18., |style| style.heading_size - 2.))
            .strong()
            .color(palette.heading_color()),
    );
    subtitle(ui, description, palette);
    ui.add_space(8.);
}

/// Muted, wrapping secondary copy shared by pages, sections and empty states.
pub(super) fn subtitle(ui: &mut egui::Ui, text: &str, palette: Palette) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .size(palette.ui.map_or(12., |style| style.body_size - 1.))
                .color(palette.muted),
        )
        .wrap(),
    );
}

/// Reserve a readable label column in a form grid. Compact forms put the field
/// on the following row, retaining the same widget IDs and editing state.
pub(super) fn field_label(ui: &mut egui::Ui, text: &str, stacked: bool) {
    if stacked {
        ui.add(egui::Label::new(text).wrap());
        ui.end_row();
    } else {
        // Measure wrapped labels before Grid positions the cell. A nested UI
        // that grows after allocation shifts its text away from the input centre.
        let text = egui::WidgetText::from(text).into_galley(
            ui,
            Some(egui::TextWrapMode::Wrap),
            128.,
            egui::TextStyle::Body,
        );
        let height = control_height(ui).max(text.size().y.ceil());
        ui.add_sized([128., height], egui::Label::new(text).halign(Align::Min));
    }
}

/// Fill the available column with a themed card; the caller owns inter-card spacing.
pub(super) fn card<R>(
    ui: &mut egui::Ui,
    palette: Palette,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let mut frame = card_frame(palette);
    let fill = frame.fill;
    let background = palette.chrome.is_decorated().then(|| {
        frame.fill = egui::Color32::TRANSPARENT;
        frame.stroke.color = egui::Color32::TRANSPARENT;
        ui.painter().add(egui::Shape::Noop)
    });
    let result = frame.show(ui, |ui| {
        ui.set_width(ui.available_width().max(1.));
        contents(ui)
    });
    if let Some(background) = background {
        ui.painter().set(
            background,
            egui::Shape::Vec(palette.surface_shapes(result.response.rect.shrink(1.), Some(fill))),
        );
    }
    result
}

/// Group related controls with a section title and a consistent gap afterward.
pub(super) fn section<R>(
    ui: &mut egui::Ui,
    title: &str,
    palette: Palette,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let result = card(ui, palette, |ui| {
        ui.label(
            RichText::new(title)
                .size(palette.ui.map_or(15., |style| style.body_size + 1.))
                .strong()
                .color(palette.heading_color()),
        );
        ui.add_space(4.);
        contents(ui)
    })
    .inner;
    ui.add_space(12.);
    result
}

/// Explain an empty list with the same inset and typography as populated cards.
pub(super) fn empty(ui: &mut egui::Ui, title: &str, description: &str, palette: Palette) {
    card(ui, palette, |ui| {
        ui.add_space(12.);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(title).strong().color(palette.heading_color()));
            subtitle(ui, description, palette);
        });
        ui.add_space(12.);
    });
}
