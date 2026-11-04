//! Shared presentation for tool pages and dialogs.
//!
//! Keep layout separate from controllers: these helpers own only spacing, reading
//! widths and themed surfaces. Callers retain scroll IDs, state and actions, so
//! docking a page never changes its identity or starts background work.
use crate::theme::Palette;
use eframe::egui::{self, Align, Layout, RichText, Stroke};

#[cfg(test)]
pub(super) mod tests;

/// Maximum outer width for lists and forms, including the page's inner margin.
pub(super) const PAGE_WIDTH: f32 = 1000.;

/// Measure the complete button height before allocating a row or a bounded
/// child. Reserving only the nominal interaction height makes later controls
/// shift down when the first button grows to fit its font and padding.
pub(super) fn control_height(ui: &egui::Ui) -> f32 {
    (ui.text_style_height(&egui::TextStyle::Button) + 2. * ui.spacing().button_padding.y)
        .max(ui.spacing().interact_size.y)
        .ceil()
}

/// Compact toolbar buttons omit vertical padding, but still share a complete
/// text-height interaction target with the neighbouring icon controls.
pub(super) fn compact_control_height(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Button)
        .ceil()
        .max(18.)
}

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
    let width = if stacked { ui.available_width() } else { 128. };
    ui.allocate_ui_with_layout(
        egui::vec2(width.max(1.), 0.),
        Layout::top_down(Align::Min),
        |ui| {
            ui.set_width(width.max(1.));
            ui.label(text);
        },
    );
    if stacked {
        ui.end_row();
    }
}

/// The common bordered surface, also used by monitor cards. Width follows its
/// column; callers may customize the frame for selected or destructive states.
pub(super) fn card_frame(palette: Palette) -> egui::Frame {
    let mut frame = egui::Frame::new()
        .fill(palette.panel)
        .stroke(Stroke::new(
            1_f32,
            if palette.chrome.is_decorated() {
                palette.frame_accent.gamma_multiply(0.55)
            } else {
                palette.border
            },
        ))
        .corner_radius(if palette.chrome.is_decorated() { 2 } else { 6 })
        .inner_margin(12);
    if let Some(style) = palette.ui {
        frame = frame
            .fill(palette.raised)
            .stroke(Stroke::new(style.border_width, palette.border))
            .corner_radius(style.surface_radius)
            .inner_margin(style.surface_padding as i8);
    }
    frame
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

/// Common modal surface; size and scrolling remain owned by each dialog.
pub(super) fn dialog_frame(style: &egui::Style, palette: Palette) -> egui::Frame {
    let mut frame = egui::Frame::popup(style)
        .fill(palette.panel)
        .stroke(Stroke::new(
            1_f32,
            if palette.chrome.is_decorated() {
                palette.frame_accent.gamma_multiply(0.65)
            } else {
                palette.border
            },
        ))
        .corner_radius(if palette.chrome.is_decorated() { 2 } else { 8 })
        .inner_margin(16);
    if let Some(style) = palette.ui {
        frame = frame
            .stroke(Stroke::new(style.border_width, palette.border))
            .corner_radius(style.popup_radius)
            .inner_margin(style.popup_padding as i8);
    }
    frame
}
