//! Shared native controls and surface builders. These use palette roles and
//! optional component tokens, so a new preset needs no view-specific rendering.
use super::Palette;
use eframe::egui::{self, Color32, FontId, Stroke};

impl Palette {
    /// Create a compact secondary label using the theme's interface typography.
    pub fn tag(self, text: impl Into<String>) -> egui::RichText {
        let text = egui::RichText::new(text).color(self.muted);
        text.font(FontId::proportional(self.ui.unwrap_or_default().small_size))
    }

    /// Draw the shared accent-filled primary action button.
    pub fn primary(self, ui: &mut egui::Ui, text: &str) -> egui::Response {
        let response = ui.add(
            egui::Button::new(egui::RichText::new(text).color(self.background).strong())
                .fill(self.accent)
                .stroke(Stroke::NONE)
                .min_size(egui::vec2(0.0, self.ui.unwrap_or_default().control_height)),
        );
        if response.enabled() && (response.has_focus() || response.is_pointer_button_down_on()) {
            let style = self.ui.unwrap_or_default();
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

/// Secondary actions use tonal fills instead of input-like outlines. Their
/// native response still owns focus/activation; keep the focus ring inside the
/// unchanged target so a neighbouring pane cannot clip it.
pub(crate) fn button_focus(ui: &egui::Ui, response: &egui::Response, fill: Option<Color32>) {
    if !response.enabled() || !(response.has_focus() || response.is_pointer_button_down_on()) {
        return;
    }
    let mut stroke = ui.visuals().selection.stroke;
    if fill == Some(stroke.color) {
        stroke.color = ui.visuals().extreme_bg_color;
    }
    ui.painter().rect_stroke(
        response.rect.shrink(1.5),
        ui.visuals().widgets.active.corner_radius,
        stroke,
        egui::StrokeKind::Inside,
    );
}

/// Measure the complete button height before allocating a row or a bounded
/// child. Reserving only the nominal interaction height makes later controls
/// shift down when the first button grows to fit its font and padding.
pub fn control_height(ui: &egui::Ui) -> f32 {
    control_height_for_style(ui.ctx(), ui.style())
}

/// Measure controls before creating a panel, so its initial clipping rectangle
/// already fits the current theme rather than the previous panel's cached size.
pub(crate) fn control_height_for_style(ctx: &egui::Context, style: &egui::Style) -> f32 {
    let text_height =
        ctx.fonts_mut(|fonts| fonts.row_height(&egui::TextStyle::Button.resolve(style)));
    (text_height + 2. * style.spacing.button_padding.y)
        .max(style.spacing.interact_size.y)
        .ceil()
}

/// Compact toolbar buttons omit vertical padding, but still share a complete
/// text-height interaction target with the neighbouring icon controls.
pub fn compact_control_height(ui: &egui::Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Button)
        .ceil()
        .max(18.)
}

/// Single-line form/search input with the same inset and minimum height as
/// adjacent actions. Callers retain native editing, focus and widget IDs.
pub(crate) fn text_input<'a>(
    ui: &egui::Ui,
    value: &'a mut dyn egui::TextBuffer,
) -> egui::TextEdit<'a> {
    let padding = ui.spacing().button_padding;
    egui::TextEdit::singleline(value)
        .margin(egui::Margin::symmetric(
            padding.x.round() as i8,
            padding.y.round() as i8,
        ))
        .min_size(egui::vec2(0., control_height(ui)))
}

/// The common bordered surface, also used by monitor cards. Width follows its
/// column; callers may customize the frame for selected or destructive states.
pub fn card_frame(palette: Palette) -> egui::Frame {
    let style = palette.ui.unwrap_or_default();
    egui::Frame::new()
        .fill(palette.raised)
        .stroke(Stroke::new(
            style.border_width,
            if palette.chrome.is_decorated() {
                palette.frame_accent.gamma_multiply(0.55)
            } else {
                palette.border
            },
        ))
        .corner_radius(if palette.chrome.is_decorated() && palette.ui.is_none() {
            2
        } else {
            style.surface_radius
        })
        .inner_margin(style.surface_padding as i8)
}

/// Common modal surface; size and scrolling remain owned by each dialog.
pub fn dialog_frame(style: &egui::Style, palette: Palette) -> egui::Frame {
    let metrics = palette.ui.unwrap_or_default();
    egui::Frame::popup(style)
        .fill(palette.panel)
        .stroke(Stroke::new(
            metrics.border_width,
            if palette.chrome.is_decorated() {
                palette.frame_accent.gamma_multiply(0.65)
            } else {
                palette.border
            },
        ))
        .corner_radius(if palette.chrome.is_decorated() && palette.ui.is_none() {
            2
        } else {
            metrics.popup_radius
        })
        .inner_margin(metrics.popup_padding as i8)
}

/// Standard symmetric panel padding, shared by ordinary frames and pane interiors.
pub(crate) const PANEL_MARGIN: egui::Margin = egui::Margin::symmetric(12, 8);

/// Create a native panel frame with the standard border/padding and supplied fill.
pub fn frame(fill: Color32) -> egui::Frame {
    egui::Frame::new().fill(fill).inner_margin(PANEL_MARGIN)
}
