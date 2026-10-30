//! Local, bounded theme definitions. No watchers or per-frame disk access.
//!
//! # Design decisions
//!
//! Palette packages are bounded color data, allowing customization without
//! executing theme code or adding another runtime. Startup loading and explicit
//! selection keep idle rendering free of filesystem polling; changes to a file
//! consequently take effect on the next load rather than through a watcher.
//! Independent syntax selection allows detailed token styling while retaining
//! interface/terminal colors. Selection invalidates cached highlighting instead
//! of reparsing unchanged source during every frame.
use anyhow::{Context, Result, bail};
use eframe::egui::{self, Color32, FontFamily, FontId, Stroke, TextStyle};
use egui_extras::syntax_highlighting::SyntectSettings;
pub mod syntax;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};
use syntect::highlighting::{Color, FontStyle, StyleModifier, Theme as SyntaxTheme, ThemeItem};

const MAX_THEME_BYTES: u64 = 64 * 1024;
const MAX_CUSTOM_THEMES: usize = 32;
/// Shared preference key for the interface palette, separate from syntax selection.
pub const STORAGE_KEY: &str = "color_theme";

const EMBEDDED_THEMES: &[(&str, &str)] = &[
    ("carbon", include_str!("../themes/carbon.json")),
    ("code-dark", include_str!("../themes/code-dark.json")),
    ("matrix", include_str!("../themes/matrix.json")),
    ("amber", include_str!("../themes/amber.json")),
    ("ice", include_str!("../themes/ice.json")),
    ("neon", include_str!("../themes/neon.json")),
];

/// Copyable interface/terminal colors; syntax themes may be selected independently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Main editor/background surface.
    pub background: Color32,
    /// Sidebar and panel surface.
    pub panel: Color32,
    /// Raised controls and active tab surface.
    pub raised: Color32,
    /// Borders and separators.
    pub border: Color32,
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
            background: Color32::from_rgb(13, 18, 20),
            panel: Color32::from_rgb(18, 25, 27),
            raised: Color32::from_rgb(25, 34, 36),
            border: Color32::from_rgb(38, 51, 53),
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
            terminal_foreground: Color32::WHITE,
            terminal_minimum_contrast: 1.0,
            ansi: [
                Color32::from_rgb(25, 34, 36),
                Color32::from_rgb(231, 123, 129),
                Color32::from_rgb(125, 220, 153),
                Color32::from_rgb(231, 204, 134),
                Color32::from_rgb(128, 170, 231),
                Color32::from_rgb(192, 158, 224),
                Color32::from_rgb(122, 208, 207),
                Color32::WHITE,
                Color32::from_rgb(111, 132, 135),
                Color32::from_rgb(255, 153, 153),
                Color32::from_rgb(156, 245, 174),
                Color32::from_rgb(255, 227, 156),
                Color32::from_rgb(161, 197, 255),
                Color32::from_rgb(216, 186, 255),
                Color32::from_rgb(157, 238, 234),
                Color32::WHITE,
            ],
        }
    }
}

impl Palette {
    /// Create a compact muted monospace status label.
    pub fn tag(self, text: impl Into<String>) -> egui::RichText {
        egui::RichText::new(text)
            .monospace()
            .size(11.0)
            .color(self.muted)
    }

    /// Draw the shared accent-filled primary action button.
    pub fn primary(self, ui: &mut egui::Ui, text: &str) -> egui::Response {
        ui.add(
            egui::Button::new(egui::RichText::new(text).color(self.background).strong())
                .fill(self.accent)
                .min_size(egui::vec2(0.0, 32.0)),
        )
    }

    fn override_color(&mut self, key: &str, color: Color32) -> Result<()> {
        let field = match key {
            "background" => &mut self.background,
            "panel" => &mut self.panel,
            "raised" => &mut self.raised,
            "border" => &mut self.border,
            "text" => &mut self.text,
            "muted" => &mut self.muted,
            "accent" => &mut self.accent,
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

/// Named interface palette with its private syntax fallback data.
#[derive(Clone)]
pub struct Theme {
    /// Stable built-in/custom identifier used in preferences.
    pub id: String,
    /// Human-readable interface-theme name.
    pub name: String,
    /// Interface and terminal colors.
    pub palette: Palette,
    syntax: BTreeMap<String, Color32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    #[serde(default = "default_base")]
    base: String,
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    syntax: BTreeMap<String, String>,
    ansi: Option<[String; 16]>,
    terminal: Option<TerminalSettings>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalSettings {
    minimum_contrast: f32,
}

fn default_base() -> String {
    "phosphor".into()
}

impl Theme {
    /// Construct the original embedded Phosphor theme.
    pub fn phosphor() -> Self {
        Self {
            id: "phosphor".into(),
            name: "Phosphor".into(),
            palette: Palette::default(),
            syntax: BTreeMap::new(),
        }
    }

    /// Construct the screenshot-inspired embedded Carbon theme.
    pub fn carbon() -> Self {
        Self::builtin("carbon").expect("embedded Carbon theme exists")
    }

    /// Resolve a built-in theme by its stable identifier.
    pub fn builtin(id: &str) -> Option<Self> {
        if id == "phosphor" {
            return Some(Self::phosphor());
        }
        EMBEDDED_THEMES
            .iter()
            .find(|(name, _)| *name == id)
            .map(|(_, json)| {
                Self::parse(id, json, &Self::phosphor()).expect("embedded theme is valid")
            })
    }

    /// Return all embedded interface themes in menu order.
    pub fn builtins() -> Vec<Self> {
        std::iter::once(Self::phosphor())
            .chain(
                EMBEDDED_THEMES
                    .iter()
                    .map(|(id, _)| Self::builtin(id).expect("embedded theme exists")),
            )
            .collect()
    }

    fn parse(id: &str, json: &str, base: &Self) -> Result<Self> {
        let file: ThemeFile = serde_json::from_str(json).context("Invalid theme JSON")?;
        let name = file.name.trim();
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
            bail!("Theme name must contain 1–64 printable characters");
        }
        if file.base != base.id {
            bail!("Theme base '{}' does not match '{}'", file.base, base.id);
        }
        let mut theme = base.clone();
        theme.id = id.into();
        theme.name = name.into();
        if let Some(terminal) = file.terminal {
            if !terminal.minimum_contrast.is_finite()
                || !(1.0..=21.0).contains(&terminal.minimum_contrast)
            {
                bail!("Terminal minimum_contrast must be between 1 and 21");
            }
            theme.palette.terminal_minimum_contrast = terminal.minimum_contrast;
        }
        for (key, value) in file.colors {
            theme.palette.override_color(
                &key,
                parse_color(&value).with_context(|| format!("Color '{key}'"))?,
            )?;
        }
        for (key, value) in file.syntax {
            syntax_scopes(&key).with_context(|| format!("Unknown syntax color '{key}'"))?;
            if theme.syntax.is_empty() {
                // A partial override of Phosphor should retain a complete code
                // palette instead of turning every other token into plain text.
                for (kind, value) in [
                    ("comment", "#958b83"),
                    ("keyword", "#a89bb9"),
                    ("type", "#f4bc87"),
                    ("function", "#7bbda4"),
                    ("string", "#beb55b"),
                    ("number", "#d28b71"),
                    ("constant", "#d28b71"),
                    ("variable", "#d0c8c6"),
                    ("operator", "#d0c8c6"),
                    ("punctuation", "#d0c8c6"),
                    ("builtin", "#7bbda4"),
                    ("attribute", "#d28b71"),
                ] {
                    theme
                        .syntax
                        .insert(kind.into(), parse_color(value).expect("static color"));
                }
            }
            theme.syntax.insert(
                key.clone(),
                parse_color(&value).with_context(|| format!("Syntax color '{key}'"))?,
            );
        }
        if let Some(ansi) = file.ansi {
            for (index, value) in ansi.iter().enumerate() {
                theme.palette.ansi[index] =
                    parse_color(value).with_context(|| format!("ANSI color {index}"))?;
            }
        }
        Ok(theme)
    }

    /// Parse bounded custom palette JSON using a built-in base; rejects unknown color keys and
    /// invalid values.
    pub fn from_json(id: &str, json: &str) -> Result<Self> {
        let file: ThemeFile = serde_json::from_str(json).context("Invalid theme JSON")?;
        let base = Self::builtin(&file.base)
            .with_context(|| format!("Unknown built-in theme base '{}'", file.base))?;
        Self::parse(id, json, &base)
    }

    fn syntax_theme(&self, original: &SyntaxTheme) -> SyntaxTheme {
        if self.syntax.is_empty() {
            let mut theme = original.clone();
            if self.palette.text != Palette::default().text {
                theme.settings.foreground = Some(syntax_color(self.palette.text));
            }
            return theme;
        }
        // Start clean so inherited scopes cannot override a semantic palette.
        let mut theme = SyntaxTheme {
            name: Some(self.name.clone()),
            ..Default::default()
        };
        theme.settings.foreground = Some(syntax_color(self.palette.text));
        theme.settings.background = Some(syntax_color(self.palette.background));
        for (key, color) in &self.syntax {
            theme.scopes.push(ThemeItem {
                scope: syntax_scopes(key)
                    .expect("validated syntax key")
                    .parse()
                    .expect("static syntax scope is valid"),
                style: StyleModifier {
                    foreground: Some(syntax_color(*color)),
                    background: None,
                    font_style: Some(FontStyle::empty()),
                },
            });
        }
        theme
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

fn syntax_color(color: Color32) -> Color {
    Color {
        r: color.r(),
        g: color.g(),
        b: color.b(),
        a: 255,
    }
}

fn syntax_scopes(key: &str) -> Option<&'static str> {
    Some(match key {
        "comment" => "comment, punctuation.definition.comment",
        "keyword" => "keyword, storage, storage.type, storage.modifier",
        "type" => {
            "entity.name.type, entity.name.struct, entity.name.enum, entity.name.trait, support.type, support.class, meta.function.parameters.rust - variable.parameter - punctuation - keyword - storage - comment - string, meta.function.return-type.rust - punctuation - keyword - storage - comment - string"
        }
        "function" => {
            "entity.name.function, variable.function, meta.function-call variable.function, support.function.macro"
        }
        "string" => "string, constant.other.symbol",
        "number" => "constant.numeric",
        "constant" => "constant.language, constant.other, entity.name.constant",
        "variable" => "variable, variable.parameter, variable.other",
        "operator" => "keyword.operator",
        "punctuation" => "punctuation, meta.brace",
        "builtin" => "support.function, support.module, support.constant",
        "attribute" => {
            "meta.annotation, meta.attribute, entity.other.attribute-name, entity.name.decorator"
        }
        _ => return None,
    })
}

/// Loaded interface/syntax choices and reusable highlighting state. Theme selection invalidates
/// cached highlighting without touching documents or terminals.
pub struct Themes {
    /// Bounded built-in and custom interface themes in menu order.
    pub entries: Vec<Theme>,
    selected: usize,
    /// Independent built-in and custom syntax themes.
    pub syntax_entries: Vec<syntax::SyntaxPalette>,
    selected_syntax: usize,
    syntax_generation: u64,
    syntax: Box<SyntectSettings>,
    original_syntax: SyntaxTheme,
}

impl Themes {
    /// Load bounded custom palettes once, returning warnings instead of aborting startup for
    /// invalid files.
    pub fn load(directory: &Path, requested: &str) -> (Self, Vec<String>) {
        let mut entries = Theme::builtins();
        let mut warnings = Vec::new();
        if directory.exists() {
            match std::fs::read_dir(directory) {
                Err(error) => warnings.push(format!("Cannot load themes: {error}")),
                Ok(files) => {
                    let mut paths = Vec::new();
                    for file in files.take(1025) {
                        match file {
                            Ok(file) if file.path().extension().is_some_and(|s| s == "json") => {
                                paths.push(file.path())
                            }
                            Err(error) => {
                                warnings.push(format!("Cannot read theme entry: {error}"))
                            }
                            _ => {}
                        }
                    }
                    paths.sort();
                    if paths.len() > MAX_CUSTOM_THEMES {
                        warnings.push(format!(
                            "Only the first {MAX_CUSTOM_THEMES} custom themes are loaded"
                        ));
                    }
                    for path in paths.into_iter().take(MAX_CUSTOM_THEMES) {
                        let result = (|| -> Result<Theme> {
                            let metadata = std::fs::metadata(&path)?;
                            anyhow::ensure!(metadata.is_file(), "Theme must be a regular file");
                            anyhow::ensure!(
                                metadata.len() <= MAX_THEME_BYTES,
                                "Theme exceeds 64 KiB"
                            );
                            let file = std::fs::File::open(&path)?;
                            anyhow::ensure!(
                                file.metadata()?.is_file(),
                                "Theme must be a regular file"
                            );
                            let mut bytes = Vec::new();
                            file.take(MAX_THEME_BYTES + 1).read_to_end(&mut bytes)?;
                            anyhow::ensure!(
                                bytes.len() as u64 <= MAX_THEME_BYTES,
                                "Theme exceeds 64 KiB"
                            );
                            let id = format!(
                                "custom:{}",
                                path.file_stem()
                                    .context("Missing theme filename")?
                                    .to_str()
                                    .context("Theme filename must be UTF-8")?
                            );
                            Theme::from_json(&id, std::str::from_utf8(&bytes)?)
                        })();
                        match result {
                            Ok(theme) => entries.push(theme),
                            Err(error) => warnings.push(format!("{}: {error:#}", path.display())),
                        }
                    }
                }
            }
        }
        let selected = entries
            .iter()
            .position(|t| t.id == requested)
            .unwrap_or_else(|| {
                if !requested.is_empty() {
                    warnings.push(format!(
                        "Theme '{requested}' is unavailable; using Phosphor"
                    ));
                }
                0
            });
        let mut syntax = Box::new(SyntectSettings {
            ps: two_face::syntax::extra_newlines(),
            ts: syntect::highlighting::ThemeSet::load_defaults(),
        });
        let original_syntax = syntax.ts.themes["base16-mocha.dark"].clone();
        syntax.ts.themes.insert(
            "base16-mocha.dark".into(),
            entries[selected].syntax_theme(&original_syntax),
        );
        let (syntax_entries, syntax_warnings) =
            syntax::load(&directory.with_file_name("syntax-themes"));
        warnings.extend(syntax_warnings);
        (
            Self {
                entries,
                selected,
                syntax_entries,
                selected_syntax: 0,
                syntax_generation: syntax::generation(),
                syntax,
                original_syntax,
            },
            warnings,
        )
    }

    /// Borrow the active interface theme.
    pub fn active(&self) -> &Theme {
        &self.entries[self.selected]
    }

    /// Select/install interface colors and rebuild the syntax fallback cache; the caller
    /// persists only the changed preference key.
    pub fn select(&mut self, id: &str, ctx: &egui::Context) -> Result<()> {
        let selected = self
            .entries
            .iter()
            .position(|t| t.id == id)
            .context("Theme is unavailable")?;
        if selected != self.selected {
            self.selected = selected;
            self.rebuild_syntax();
            // Release computed layouts when the palette changes.
            ctx.memory_mut(|m| m.caches = Default::default());
        }
        install(ctx, self.active().palette);
        ctx.request_repaint();
        Ok(())
    }

    /// Borrow the active independent syntax choice.
    pub fn active_syntax(&self) -> &syntax::SyntaxPalette {
        &self.syntax_entries[self.selected_syntax]
    }

    fn rebuild_syntax(&mut self) {
        let theme = self
            .active_syntax()
            .theme
            .clone()
            .unwrap_or_else(|| self.active().syntax_theme(&self.original_syntax));
        self.syntax
            .ts
            .themes
            .insert("base16-mocha.dark".into(), theme);
        self.syntax_generation = syntax::generation();
    }

    /// Select syntax styling without changing interface/terminal colors; invalidate cached
    /// layouts.
    pub fn select_syntax(&mut self, id: &str, ctx: &egui::Context) -> Result<()> {
        let selected = self
            .syntax_entries
            .iter()
            .position(|theme| theme.id == id)
            .context("Syntax theme is unavailable")?;
        if selected != self.selected_syntax {
            self.selected_syntax = selected;
            self.rebuild_syntax();
            ctx.memory_mut(|memory| memory.caches = Default::default());
            ctx.request_repaint();
        }
        Ok(())
    }

    /// Build or reuse cached highlighted text for the active syntax choice and UI font size.
    pub fn highlight(
        &self,
        ui: &egui::Ui,
        code: &str,
        language: &str,
        font_size: f32,
    ) -> egui::text::LayoutJob {
        self.highlight_with_style(ui.ctx(), ui.style(), code, language, font_size)
    }

    /// Build or reuse a highlighted layout from an explicit egui context/style, without reading
    /// files.
    pub fn highlight_with_style(
        &self,
        ctx: &egui::Context,
        style: &egui::Style,
        code: &str,
        language: &str,
        font_size: f32,
    ) -> egui::text::LayoutJob {
        let _ = style;
        syntax::highlight(
            ctx,
            code,
            language,
            font_size,
            &self.syntax,
            self.syntax_generation,
        )
    }
}

/// Resolve the desktop's custom interface-theme directory.
pub fn directory() -> PathBuf {
    crate::ssh::config_path()
        .parent()
        .expect("config path has a parent")
        .join("themes")
}

/// Install palette colors, spacing and typography into the native egui context.
pub fn install(ctx: &egui::Context, palette: Palette) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = palette.background;
    style.visuals.window_fill = palette.panel;
    style.visuals.extreme_bg_color = palette.background;
    style.visuals.faint_bg_color = palette.raised;
    style.visuals.override_text_color = Some(palette.text);
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
    ctx.set_style(style);
}

/// Create a native panel frame with the standard border/padding and supplied fill.
pub fn frame(fill: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(12, 8))
}
