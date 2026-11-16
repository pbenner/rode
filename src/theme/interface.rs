//! Bounded, inheritable component styling, independent of color and frame decoration.
use anyhow::{Result, ensure};
use serde::Deserialize;

/// Native icon treatment, inherited separately from colors and illustrations.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IconStyle {
    /// Uniform outline symbols used by the standard interface.
    #[default]
    Outline,
    /// Tapered ink strokes and scroll-inspired symbols for Sumi-e themes.
    SumiE,
}

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

/// Shared contemporary component defaults, used by every palette.
/// Presets override individual tokens rather than copying this baseline.
impl Default for InterfaceStyle {
    fn default() -> Self {
        Self {
            control_radius: 6,
            surface_radius: 10,
            popup_radius: 12,
            control_height: 30.,
            button_padding: [12., 6.],
            item_spacing: [8., 6.],
            surface_padding: 14,
            popup_padding: 18,
            body_size: 14.,
            button_size: 13.,
            heading_size: 22.,
            small_size: 12.,
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
}
