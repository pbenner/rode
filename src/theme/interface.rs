//! Bounded, inheritable component styling, independent of color and frame decoration.
use super::{Chrome, Palette};
use anyhow::{Result, ensure};
use eframe::egui::{self, FontFamily, FontId, Stroke, TextStyle};
use serde::Deserialize;

/// Shared interface dimensions in logical points; editor and terminal fonts stay independent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InterfaceStyle {
    /// Corner radius of buttons, inputs and selected tabs.
    pub control_radius: u8,
    /// Corner radius of tool cards.
    pub surface_radius: u8,
    /// Corner radius of menus and dialogs.
    pub popup_radius: u8,
    /// Minimum height of ordinary interactive controls.
    pub control_height: f32,
    /// Horizontal and vertical padding inside buttons.
    pub button_padding: [f32; 2],
    /// Horizontal and vertical space between controls.
    pub item_spacing: [f32; 2],
    /// Padding inside tool cards.
    pub surface_padding: u8,
    /// Padding inside dialogs and windows.
    pub popup_padding: u8,
    /// Proportional body text size.
    pub body_size: f32,
    /// Proportional button text size.
    pub button_size: f32,
    /// Proportional heading text size.
    pub heading_size: f32,
    /// Proportional secondary text size.
    pub small_size: f32,
    /// Ordinary control and surface border width.
    pub border_width: f32,
    /// Keyboard focus and pressed-control border width.
    pub focus_width: f32,
    /// Opacity multiplier for disabled controls.
    pub disabled_alpha: f32,
}

/// Shared rounded-component defaults, selected by `"ui": {}` on any palette.
/// Presets override individual tokens rather than copying this baseline.
impl Default for InterfaceStyle {
    fn default() -> Self {
        Self {
            control_radius: 6,
            surface_radius: 10,
            popup_radius: 10,
            control_height: 30.,
            button_padding: [12., 6.],
            item_spacing: [8., 6.],
            surface_padding: 14,
            popup_padding: 18,
            body_size: 14.,
            button_size: 13.,
            heading_size: 22.,
            small_size: 11.,
            border_width: 1.,
            focus_width: 2.,
            disabled_alpha: 0.5,
        }
    }
}

/// Missing fields inherit the base theme's component settings, not generic defaults.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Overrides {
    control_radius: Option<u8>,
    surface_radius: Option<u8>,
    popup_radius: Option<u8>,
    control_height: Option<f32>,
    button_padding: Option<[f32; 2]>,
    item_spacing: Option<[f32; 2]>,
    surface_padding: Option<u8>,
    popup_padding: Option<u8>,
    body_size: Option<f32>,
    button_size: Option<f32>,
    heading_size: Option<f32>,
    small_size: Option<f32>,
    border_width: Option<f32>,
    focus_width: Option<f32>,
    disabled_alpha: Option<f32>,
}

impl InterfaceStyle {
    /// Apply optional UI metric overrides and validate finite supported point/alpha ranges.
    /// Reject a focus outline narrower than the resulting border.
    pub(super) fn merge(self, patch: Overrides) -> Result<Self> {
        let value = Self {
            control_radius: patch.control_radius.unwrap_or(self.control_radius),
            surface_radius: patch.surface_radius.unwrap_or(self.surface_radius),
            popup_radius: patch.popup_radius.unwrap_or(self.popup_radius),
            control_height: patch.control_height.unwrap_or(self.control_height),
            button_padding: patch.button_padding.unwrap_or(self.button_padding),
            item_spacing: patch.item_spacing.unwrap_or(self.item_spacing),
            surface_padding: patch.surface_padding.unwrap_or(self.surface_padding),
            popup_padding: patch.popup_padding.unwrap_or(self.popup_padding),
            body_size: patch.body_size.unwrap_or(self.body_size),
            button_size: patch.button_size.unwrap_or(self.button_size),
            heading_size: patch.heading_size.unwrap_or(self.heading_size),
            small_size: patch.small_size.unwrap_or(self.small_size),
            border_width: patch.border_width.unwrap_or(self.border_width),
            focus_width: patch.focus_width.unwrap_or(self.focus_width),
            disabled_alpha: patch.disabled_alpha.unwrap_or(self.disabled_alpha),
        };
        for (name, number, min, max) in [
            ("control_radius", f32::from(value.control_radius), 0., 16.),
            ("surface_radius", f32::from(value.surface_radius), 0., 24.),
            ("popup_radius", f32::from(value.popup_radius), 0., 24.),
            ("control_height", value.control_height, 22., 40.),
            ("button_padding[0]", value.button_padding[0], 4., 20.),
            ("button_padding[1]", value.button_padding[1], 2., 12.),
            ("item_spacing[0]", value.item_spacing[0], 2., 16.),
            ("item_spacing[1]", value.item_spacing[1], 2., 16.),
            ("surface_padding", f32::from(value.surface_padding), 8., 24.),
            ("popup_padding", f32::from(value.popup_padding), 8., 28.),
            ("body_size", value.body_size, 12., 18.),
            ("button_size", value.button_size, 12., 18.),
            ("heading_size", value.heading_size, 18., 28.),
            ("small_size", value.small_size, 10., 14.),
            ("border_width", value.border_width, 0.5, 2.),
            ("focus_width", value.focus_width, 1., 3.),
            ("disabled_alpha", value.disabled_alpha, 0.35, 0.75),
        ] {
            ensure!(
                number.is_finite() && (min..=max).contains(&number),
                "UI {name} must be between {min} and {max}"
            );
        }
        ensure!(
            value.focus_width >= value.border_width,
            "UI focus_width must be at least border_width"
        );
        Ok(value)
    }

    /// Apply shared component roles after the common palette and legacy defaults.
    pub(super) fn install(self, style: &mut egui::Style, palette: Palette) {
        style.spacing.item_spacing = self.item_spacing.into();
        style.spacing.button_padding = self.button_padding.into();
        style.spacing.interact_size.y = self.control_height;
        style.spacing.window_margin = egui::Margin::same(self.popup_padding as i8);
        style.spacing.menu_margin = egui::Margin::same(6);
        // Reserve a narrow scrollbar gutter so charts and text keep their full
        // reading area when a long tool page needs to scroll.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        for (role, size) in [
            (TextStyle::Body, self.body_size),
            (TextStyle::Button, self.button_size),
            (TextStyle::Heading, self.heading_size),
            (TextStyle::Small, self.small_size),
        ] {
            style.text_styles.insert(role, FontId::proportional(size));
        }
        let visuals = &mut style.visuals;
        let control_border = palette.control_border.unwrap_or(palette.border);
        visuals.disabled_alpha = self.disabled_alpha;
        visuals.window_corner_radius = self.popup_radius.into();
        visuals.menu_corner_radius = self.popup_radius.into();
        visuals.window_stroke = Stroke::new(self.border_width, palette.border);
        let shadow = egui::epaint::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: egui::Color32::from_black_alpha(72),
        };
        visuals.window_shadow = shadow;
        visuals.popup_shadow = shadow;
        visuals.text_cursor.stroke = Stroke::new(self.focus_width, palette.accent);
        // TextEdit paints keyboard focus with selection.stroke, rather than
        // widgets.active.bg_stroke. Keep both focus outlines consistent.
        visuals.selection.stroke = Stroke::new(self.focus_width, palette.accent);
        visuals.code_bg_color = palette.background;
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.corner_radius = self.control_radius.into();
            widget.expansion = 0.;
            widget.bg_stroke = Stroke::new(self.border_width, control_border);
        }
        // Separators and passive surfaces keep the quieter decorative border.
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(self.border_width, palette.border);
        visuals.widgets.hovered.bg_stroke = Stroke::new(
            self.border_width,
            control_border.lerp_to_gamma(palette.accent, 0.35),
        );
        visuals.widgets.active.bg_stroke = Stroke::new(self.focus_width, palette.accent);
        visuals.widgets.open.bg_stroke = Stroke::new(self.border_width, palette.accent);
    }
}

/// Install palette colors, spacing and typography into the native egui context.
pub fn install(ctx: &egui::Context, palette: Palette) {
    let mut style = (*ctx.style()).clone();
    // Reset all spacing and the secondary text role before applying a palette,
    // so optional component styles cannot leak into a subsequent legacy theme.
    style.spacing = Default::default();
    style.text_styles.insert(
        TextStyle::Small,
        egui::Style::default().text_styles[&TextStyle::Small].clone(),
    );
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = palette.background;
    style.visuals.window_fill = palette.panel;
    style.visuals.extreme_bg_color = palette.background;
    style.visuals.faint_bg_color = palette.raised;
    // Widget foregrounds below already supply the primary text colour. A global
    // override also bakes it into TextEdit hints, defeating their muted fallback.
    style.visuals.override_text_color = None;
    style.visuals.weak_text_color = Some(palette.muted);
    style.visuals.selection.bg_fill = palette.selection;
    style.visuals.selection.stroke = Stroke::new(1.0_f32, palette.accent);
    style.visuals.widgets.noninteractive.bg_fill = palette.panel;
    style.visuals.widgets.noninteractive.weak_bg_fill = palette.panel;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, palette.border);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, palette.text);
    style.visuals.widgets.inactive.weak_bg_fill = palette.raised;
    style.visuals.widgets.inactive.bg_fill = palette.raised;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, palette.border);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, palette.text);
    style.visuals.widgets.hovered.weak_bg_fill = palette.hover;
    style.visuals.widgets.hovered.bg_fill = palette.hover;
    style.visuals.widgets.hovered.bg_stroke =
        Stroke::new(1.0_f32, palette.accent.gamma_multiply(0.6));
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, palette.text);
    style.visuals.widgets.active.weak_bg_fill = palette.active;
    style.visuals.widgets.active.bg_fill = palette.active;
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, palette.text);
    style.visuals.widgets.open.weak_bg_fill = palette.active;
    style.visuals.widgets.open.bg_fill = palette.active;
    style.visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, palette.border);
    style.visuals.widgets.open.fg_stroke = Stroke::new(1.0_f32, palette.text);
    style.visuals.text_edit_bg_color = Some(palette.background);
    style.visuals.hyperlink_color = palette.accent;
    style.visuals.warn_fg_color = palette.accent;
    style.visuals.error_fg_color = palette.error;
    style.visuals.window_stroke = Stroke::new(1.0_f32, palette.border);
    style.visuals.window_highlight_topmost = false;
    if palette.chrome.is_decorated() {
        let corner_radius = if palette.chrome == Chrome::Bracketed {
            0
        } else {
            2
        };
        style.visuals.window_corner_radius = egui::CornerRadius::same(corner_radius);
        style.visuals.menu_corner_radius = egui::CornerRadius::same(corner_radius);
        style.visuals.window_stroke =
            Stroke::new(1.0_f32, palette.frame_accent.gamma_multiply(0.65));
        style.visuals.window_shadow = egui::epaint::Shadow {
            offset: [0, 0],
            blur: 18,
            spread: 2,
            color: palette.frame_accent.gamma_multiply(0.12),
        };
        for widget in [
            &mut style.visuals.widgets.inactive,
            &mut style.visuals.widgets.hovered,
            &mut style.visuals.widgets.active,
            &mut style.visuals.widgets.open,
        ] {
            widget.corner_radius = egui::CornerRadius::same(corner_radius);
            widget.expansion = 0.0;
        }
        style.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, palette.accent);
        style.visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, palette.accent);
    }
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.window_margin = egui::Margin::same(20);
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(13.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Monospace, FontId::monospace(14.0));
    style
        .text_styles
        .insert(TextStyle::Heading, FontId::proportional(24.0));
    if let Some(ui) = palette.ui {
        ui.install(&mut style, palette);
    }
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_field_hints_are_muted_while_identical_entered_values_keep_text_colour() {
        let ctx = egui::Context::default();
        // Reuse the context to cover switching themes as well as initial installation.
        for theme in crate::theme::Theme::builtins() {
            install(&ctx, theme.palette);
            let mut value = String::new();
            for entered in [false, true] {
                if entered {
                    value = "~/.ssh/id_ed25519".into();
                }
                let output = ctx.run(Default::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.label("Identity file");
                        ui.add(
                            egui::TextEdit::singleline(&mut value).hint_text("~/.ssh/id_ed25519"),
                        );
                    });
                });
                let expected = if entered {
                    theme.palette.text
                } else {
                    theme.palette.muted
                };
                for label in ["Identity file", "~/.ssh/id_ed25519"] {
                    let text = output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            egui::Shape::Text(text) if text.galley.text() == label => Some(text),
                            _ => None,
                        })
                        .unwrap();
                    for section in &text.galley.job.sections {
                        let colour = text.override_text_color.unwrap_or_else(|| {
                            if section.format.color == egui::Color32::PLACEHOLDER {
                                text.fallback_color
                            } else {
                                section.format.color
                            }
                        });
                        assert_eq!(
                            colour,
                            if label == "Identity file" {
                                theme.palette.text
                            } else {
                                expected
                            },
                            "{}: {label}, entered={entered}",
                            theme.id
                        );
                    }
                }
                assert_eq!(value.is_empty(), !entered, "A hint must not become a value");
            }
        }
    }
}
