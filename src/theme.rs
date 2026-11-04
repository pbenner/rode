//! Local, bounded theme definitions. No watchers or per-frame disk access.
//!
//! # Design decisions
//!
//! Palette packages are bounded color and component data, allowing customization
//! without executing theme code or adding another runtime. Startup loading and explicit
//! selection keep idle rendering free of filesystem polling; changes to a file
//! consequently take effect on the next load rather than through a watcher.
//! Independent syntax selection allows detailed token styling while retaining
//! interface/terminal colors. Selection invalidates cached highlighting instead
//! of reparsing unchanged source during every frame.
//!
//! # Module boundaries
//!
//! `palette` owns color roles and defaults; `definition` resolves bounded preset
//! data and inheritance. `interface` applies shared egui styling, `components`
//! builds reusable controls/frames, and `surfaces` draws optional decoration.
//! `syntax` owns independent code palettes and cached highlighting. This module
//! coordinates selection and startup loading; views consume roles, not preset IDs.
use anyhow::{Context, Result};
use eframe::egui::{self, Color32};
use egui_extras::syntax_highlighting::SyntectSettings;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};
use syntect::highlighting::Theme as SyntaxTheme;
mod components;
mod definition;
mod interface;
mod palette;
mod surfaces;
pub mod syntax;
pub(crate) use components::control_height_for_style;
pub use components::{card_frame, compact_control_height, control_height, dialog_frame, frame};
use definition::{MAX_CUSTOM_THEMES, MAX_THEME_BYTES};
pub use interface::{InterfaceStyle, install};
pub use palette::{Palette, parse_color};
pub use surfaces::Chrome;

/// Shared preference key for the interface palette, separate from syntax selection.
pub const STORAGE_KEY: &str = "color_theme";

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

    /// Resolve a built-in theme by its stable identifier, independently of menu order.
    pub fn builtin(id: &str) -> Option<Self> {
        definition::builtin(id)
    }

    /// Return all embedded interface themes in menu order.
    pub fn builtins() -> Vec<Self> {
        definition::builtins()
    }

    /// Parse bounded custom palette JSON using a built-in base; rejects unknown
    /// color keys, invalid values and definitions larger than 64 KiB.
    pub fn from_json(id: &str, json: &str) -> Result<Self> {
        definition::from_json(id, json)
    }
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
