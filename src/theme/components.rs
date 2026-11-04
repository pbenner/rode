//! Shared native controls and surface builders. These use palette roles and
//! optional component tokens, so a new preset needs no view-specific rendering.
use super::Palette;
use eframe::egui::{self, Color32, FontId, Stroke};

impl Palette {
    /// Create a compact secondary label using the theme's interface typography.
    pub fn tag(self, text: impl Into<String>) -> egui::RichText {
        let text = egui::RichText::new(text).color(self.muted);
        if let Some(ui) = self.ui {
            text.font(FontId::proportional(ui.small_size))
        } else {
            text.monospace().size(11.)
        }
    }

    /// Draw the shared accent-filled primary action button.
    pub fn primary(self, ui: &mut egui::Ui, text: &str) -> egui::Response {
        let response = ui.add(
            egui::Button::new(egui::RichText::new(text).color(self.background).strong())
                .fill(self.accent)
                .min_size(egui::vec2(0.0, self.ui.map_or(32., |ui| ui.control_height))),
        );
        if let Some(style) = self.ui.filter(|_| {
            response.enabled() && (response.has_focus() || response.is_pointer_button_down_on())
        }) {
            // An accent outline disappears against the filled accent button.
            // Use its high-contrast foreground for an inset focus/pressed ring.
            ui.painter().rect_stroke(
                response.rect,
                style.control_radius,
                Stroke::new(style.focus_width, self.background),
                egui::StrokeKind::Inside,
            );
        }
        response
    }
}

/// Measure the complete button height before allocating a row or a bounded
/// child. Reserving only the nominal interaction height makes later controls
/// shift down when the first button grows to fit its font and padding.
pub fn control_height(ui: &egui::Ui) -> f32 {
    (ui.text_style_height(&egui::TextStyle::Button) + 2. * ui.spacing().button_padding.y)
        .max(ui.spacing().interact_size.y)
        .ceil()
}

/// Compact toolbar buttons omit vertical padding, but still share a complete
/// text-height interaction target with the neighbouring icon controls.
pub fn compact_control_height(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Button)
        .ceil()
        .max(18.)
}

/// The common bordered surface, also used by monitor cards. Width follows its
/// column; callers may customize the frame for selected or destructive states.
pub fn card_frame(palette: Palette) -> egui::Frame {
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

/// Common modal surface; size and scrolling remain owned by each dialog.
pub fn dialog_frame(style: &egui::Style, palette: Palette) -> egui::Frame {
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

/// Create a native panel frame with the standard border/padding and supplied fill.
pub fn frame(fill: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(12, 8))
}
