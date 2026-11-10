//! Interface and terminal color roles. Preset colors are data; rendering is shared.
use super::{Chrome, InterfaceStyle};
use anyhow::{Result, bail};
use eframe::egui::Color32;

/// Copyable interface/terminal colors; syntax themes may be selected independently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Optional component dimensions, typography and state styling.
    pub ui: Option<InterfaceStyle>,
    /// Static native surface treatment, independent of interface colors.
    pub chrome: Chrome,
    /// Decorative frame color, independent of interactive/focus emphasis.
    pub frame_accent: Color32,
    /// Section heading color in decorated themes.
    pub heading: Color32,
    /// Main editor/background surface.
    pub background: Color32,
    /// Sidebar and panel surface.
    pub panel: Color32,
    /// Raised controls and active tab surface.
    pub raised: Color32,
    /// Borders and separators.
    pub border: Color32,
    /// Optional control outline, separate from quiet surface borders; falls back to `border`.
    pub control_border: Option<Color32>,
    /// Primary readable interface foreground.
    pub text: Color32,
    /// Secondary labels and subdued foreground.
    pub muted: Color32,
    /// Interactive emphasis and focus color.
    pub accent: Color32,
    /// Successful-operation indicator.
    pub success: Color32,
    /// Error/destructive-operation indicator.
    pub error: Color32,
    /// Text selection fill.
    pub selection: Color32,
    /// Hovered control fill.
    pub hover: Color32,
    /// Selected control fill.
    pub active: Color32,
    /// Line-number/gutter foreground.
    pub gutter: Color32,
    /// Default terminal background.
    pub terminal_background: Color32,
    /// Default terminal foreground.
    pub terminal_foreground: Color32,
    /// Minimum rendered terminal text contrast, from 1 (unchanged) to 21.
    pub terminal_minimum_contrast: f32,
    /// Normal eight ANSI colors followed by their bright variants.
    pub ansi: [Color32; 16],
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            ui: None,
            chrome: Chrome::Plain,
            frame_accent: Color32::from_rgb(125, 235, 164),
            heading: Color32::from_rgb(205, 218, 216),
            background: Color32::from_rgb(13, 18, 20),
            panel: Color32::from_rgb(18, 25, 27),
            raised: Color32::from_rgb(25, 34, 36),
            border: Color32::from_rgb(38, 51, 53),
            control_border: None,
            text: Color32::from_rgb(205, 218, 216),
            muted: Color32::from_rgb(117, 139, 141),
            accent: Color32::from_rgb(125, 235, 164),
            success: Color32::from_rgb(125, 235, 164),
            error: Color32::from_rgb(245, 136, 137),
            selection: Color32::from_rgb(35, 76, 61),
            hover: Color32::from_rgb(35, 49, 47),
            active: Color32::from_rgb(35, 67, 51),
            gutter: Color32::from_rgb(64, 76, 78),
            terminal_background: Color32::from_rgb(13, 18, 20),
            terminal_foreground: Color32::from_rgb(205, 218, 216),
            terminal_minimum_contrast: 1.0,
            ansi: [
                Color32::from_rgb(25, 34, 36),
                Color32::from_rgb(231, 123, 129),
                Color32::from_rgb(125, 220, 153),
                Color32::from_rgb(231, 204, 134),
                Color32::from_rgb(131, 188, 163),
                Color32::from_rgb(192, 158, 224),
                Color32::from_rgb(122, 208, 207),
                Color32::from_rgb(205, 218, 216),
                Color32::from_rgb(156, 175, 170),
                Color32::from_rgb(255, 153, 153),
                Color32::from_rgb(156, 245, 174),
                Color32::from_rgb(255, 227, 156),
                Color32::from_rgb(172, 232, 194),
                Color32::from_rgb(216, 186, 255),
                Color32::from_rgb(157, 238, 234),
                Color32::from_rgb(227, 242, 232),
            ],
        }
    }
}

impl Palette {
    /// Apply one named palette color, rejecting unknown keys without changing the palette.
    /// The optional control-border override remains distinct from the general border.
    pub(super) fn override_color(&mut self, key: &str, color: Color32) -> Result<()> {
        if key == "control_border" {
            self.control_border = Some(color);
            return Ok(());
        }
        let field = match key {
            "background" => &mut self.background,
            "panel" => &mut self.panel,
            "raised" => &mut self.raised,
            "border" => &mut self.border,
            "text" => &mut self.text,
            "muted" => &mut self.muted,
            "accent" => &mut self.accent,
            "frame_accent" => &mut self.frame_accent,
            "heading" => &mut self.heading,
            "success" => &mut self.success,
            "error" => &mut self.error,
            "selection" => &mut self.selection,
            "hover" => &mut self.hover,
            "active" => &mut self.active,
            "gutter" => &mut self.gutter,
            "terminal_background" => &mut self.terminal_background,
            "terminal_foreground" => &mut self.terminal_foreground,
            _ => bail!("Unknown theme color '{key}'"),
        };
        *field = color;
        Ok(())
    }
}

/// Parse a supported hexadecimal color string, rejecting malformed components.
pub fn parse_color(value: &str) -> Result<Color32> {
    let Some(hex) = value.strip_prefix('#') else {
        bail!("Expected a color in #RRGGBB format");
    };
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("Expected a color in #RRGGBB format");
    }
    let color = u32::from_str_radix(hex, 16)?;
    Ok(Color32::from_rgb(
        (color >> 16) as u8,
        (color >> 8) as u8,
        color as u8,
    ))
}
