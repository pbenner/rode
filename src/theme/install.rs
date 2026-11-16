//! Install resolved palette roles and component metrics into native egui styles.
//! Component schema and validation live in `interface`; controls consume
//! the installed roles without defining another set of theme defaults.
use super::{IconStyle, Palette};
use eframe::egui::{self, FontId, Stroke, TextStyle};

impl IconStyle {
    fn key() -> egui::Id {
        egui::Id::new("theme-icon-style")
    }

    /// Publish the selected treatment without exposing its context storage key.
    fn set_current(self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(Self::key(), self));
    }

    /// Read the context's installed treatment, including detached viewports.
    pub(crate) fn current(ctx: &egui::Context) -> Self {
        ctx.data(|data| data.get_temp(Self::key()))
            .unwrap_or_default()
    }
}

/// Install palette colors, spacing and typography into the native egui context.
pub fn install(ctx: &egui::Context, palette: Palette) {
    super::typography::install(ctx);
    palette.icons.set_current(ctx);
    // Preserve settings not owned by a theme, including custom text roles.
    let mut style = (*ctx.style()).clone();
    // Explicit palette selection owns appearance across every native viewport;
    // an OS appearance notification must not replace its installed widget style.
    ctx.set_theme(if palette.dark_mode {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
    let metrics = palette.ui.unwrap_or_default();
    // Reset theme-owned state before installing it once. Switching away from
    // custom metrics must not leave spacing or widget defaults behind.
    style.spacing = Default::default();
    style.spacing.item_spacing = metrics.item_spacing.into();
    style.spacing.button_padding = metrics.button_padding.into();
    style.spacing.interact_size.y = metrics.control_height;
    style.spacing.window_margin = egui::Margin::same(metrics.popup_padding as i8);
    style.spacing.menu_margin = egui::Margin::same(6);
    // Reserve a gutter so scrolling does not displace text or form controls.
    style.spacing.scroll = egui::style::ScrollStyle::solid();
    for (role, font) in [
        (TextStyle::Body, FontId::proportional(metrics.body_size)),
        (TextStyle::Button, FontId::proportional(metrics.button_size)),
        (
            TextStyle::Heading,
            FontId::proportional(metrics.heading_size),
        ),
        (TextStyle::Small, FontId::proportional(metrics.small_size)),
        (TextStyle::Monospace, FontId::monospace(14.)),
    ] {
        style.text_styles.insert(role, font);
    }
    style.visuals = if palette.dark_mode {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let visuals = &mut style.visuals;
    visuals.panel_fill = palette.background;
    visuals.window_fill = palette.panel;
    visuals.extreme_bg_color = palette.background;
    visuals.faint_bg_color = palette.raised;
    // Widget foregrounds supply primary text. A global override would also
    // color TextEdit hints, defeating their muted fallback.
    visuals.override_text_color = None;
    visuals.weak_text_color = Some(palette.muted);
    visuals.selection.bg_fill = palette.selection;
    // TextEdit uses selection.stroke for focus; buttons use the active stroke.
    visuals.selection.stroke = Stroke::new(metrics.focus_width, palette.accent);
    visuals.text_edit_bg_color = Some(palette.background);
    visuals.code_bg_color = palette.background;
    visuals.text_cursor.stroke = Stroke::new(
        metrics.focus_width,
        palette.cursor.unwrap_or(palette.accent),
    );
    visuals.hyperlink_color = palette.accent;
    visuals.warn_fg_color = palette.accent;
    visuals.error_fg_color = palette.error;
    visuals.disabled_alpha = metrics.disabled_alpha;
    visuals.window_highlight_topmost = false;
    visuals.window_corner_radius = metrics.popup_radius.into();
    visuals.menu_corner_radius = metrics.popup_radius.into();
    visuals.window_stroke = Stroke::new(metrics.border_width, palette.border);
    let shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: egui::Color32::from_black_alpha(if palette.dark_mode { 72 } else { 28 }),
    };
    visuals.window_shadow = shadow;
    visuals.popup_shadow = shadow;
    let border = palette.control_border.unwrap_or(palette.border);
    for (widget, fill, stroke) in [
        (
            &mut visuals.widgets.noninteractive,
            palette.panel,
            Stroke::new(metrics.border_width, palette.border),
        ),
        (
            &mut visuals.widgets.inactive,
            palette.raised,
            Stroke::new(metrics.border_width, border),
        ),
        (
            &mut visuals.widgets.hovered,
            palette.hover,
            Stroke::new(
                metrics.border_width,
                border.lerp_to_gamma(palette.accent, 0.35),
            ),
        ),
        (
            &mut visuals.widgets.active,
            palette.active,
            Stroke::new(metrics.focus_width, palette.accent),
        ),
        (
            &mut visuals.widgets.open,
            palette.active,
            Stroke::new(metrics.border_width, palette.accent),
        ),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = stroke;
        widget.fg_stroke = Stroke::new(1_f32, palette.text);
        widget.corner_radius = metrics.control_radius.into();
        // Keep hover/focus paint within pane clips and allocated control bounds.
        widget.expansion = 0.;
    }
    ctx.set_style(style);
    // Theme changes also repaint unfocused editor and tool windows.
    let viewports = ctx.input(|input| input.raw.viewports.keys().copied().collect::<Vec<_>>());
    for viewport in viewports {
        ctx.request_repaint_of(viewport);
    }
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
