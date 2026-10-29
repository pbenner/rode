//! Independent syntax palettes and a cached native renderer.
use super::{parse_color, syntax_color};
use anyhow::{Context, Result, bail, ensure};
use eframe::egui::{
    self, Color32, FontId, Stroke,
    text::{LayoutJob, LayoutSection, TextFormat},
};
use egui_extras::syntax_highlighting::SyntectSettings;
use serde::Deserialize;
use std::{
    io::Read,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};
use syntect::highlighting::{FontStyle, StyleModifier, Theme, ThemeItem};
use two_face::theme::EmbeddedThemeName as Embedded;

/// Shared preference key for syntax styling, independent of the interface palette.
pub const STORAGE_KEY: &str = "syntax_theme";
const PRESETS: &[(&str, &str, Embedded)] = &[
    ("dracula", "Dracula", Embedded::Dracula),
    (
        "catppuccin-mocha",
        "Catppuccin Mocha",
        Embedded::CatppuccinMocha,
    ),
    ("nord", "Nord", Embedded::Nord),
    ("monokai", "Monokai", Embedded::MonokaiExtended),
    ("one-half-dark", "One Half Dark", Embedded::OneHalfDark),
];
/// Maximum custom syntax-theme definition size in bytes.
pub const MAX_BYTES: u64 = 128 * 1024;

/// Named full-scope syntax palette, or the special choice that follows interface colors.
pub struct SyntaxPalette {
    /// Stable syntax-theme identifier used in preferences.
    pub id: String,
    /// Human-readable syntax-theme name.
    pub name: String,
    // None follows the current UI palette, preserving older configurations.
    pub(super) theme: Option<Theme>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Definition {
    name: String,
    #[serde(default = "default_base")]
    base: String,
    foreground: Option<String>,
    #[serde(default)]
    rules: Vec<Rule>,
}
fn default_base() -> String {
    "dracula".into()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    scope: String,
    foreground: Option<String>,
    font_style: Option<String>,
}

impl SyntaxPalette {
    /// Parse a bounded custom full-scope theme from a supported built-in base and explicit
    /// style rules.
    pub fn from_json(id: &str, json: &str) -> Result<Self> {
        ensure!(
            json.len() as u64 <= MAX_BYTES,
            "Syntax theme exceeds 128 KiB"
        );
        let definition: Definition =
            serde_json::from_str(json).context("Invalid syntax theme JSON")?;
        let name = definition.name.trim();
        ensure!(
            !name.is_empty() && name.chars().count() <= 64 && !name.chars().any(char::is_control),
            "Syntax theme name must contain 1–64 printable characters"
        );
        ensure!(
            definition.rules.len() <= 256,
            "Syntax themes support at most 256 rules"
        );
        let (_, _, embedded) = PRESETS
            .iter()
            .find(|(id, _, _)| *id == definition.base)
            .context("Unknown built-in syntax theme base")?;
        let mut theme = presets().get(*embedded).clone();
        theme.name = Some(name.into());
        if let Some(color) = definition.foreground {
            theme.settings.foreground = Some(syntax_color(parse_color(&color)?));
        }
        for rule in definition.rules {
            ensure!(
                !rule.scope.trim().is_empty() && rule.scope.len() <= 512,
                "Scope selectors must contain 1–512 bytes"
            );
            let font_style = rule
                .font_style
                .map(|value| -> Result<FontStyle> {
                    if value == "normal" {
                        return Ok(FontStyle::empty());
                    }
                    let mut style = FontStyle::empty();
                    ensure!(
                        !value.trim().is_empty(),
                        "Font style must be normal, italic, underline, or italic underline"
                    );
                    for word in value.split_whitespace() {
                        style |= match word {
                            "italic" => FontStyle::ITALIC,
                            "underline" => FontStyle::UNDERLINE,
                            _ => bail!("Unsupported font style '{word}'"),
                        };
                    }
                    Ok(style)
                })
                .transpose()?;
            ensure!(
                rule.foreground.is_some() || font_style.is_some(),
                "A scope rule needs a foreground or font_style"
            );
            theme.scopes.push(ThemeItem {
                scope: rule
                    .scope
                    .parse()
                    .with_context(|| format!("Invalid scope selector '{}'", rule.scope))?,
                style: StyleModifier {
                    foreground: rule
                        .foreground
                        .map(|color| parse_color(&color).map(syntax_color))
                        .transpose()?,
                    background: None,
                    font_style,
                },
            });
        }
        Ok(Self {
            id: id.into(),
            name: name.into(),
            theme: Some(theme),
        })
    }
}

fn presets() -> &'static two_face::theme::EmbeddedLazyThemeSet {
    static PRESETS: std::sync::OnceLock<two_face::theme::EmbeddedLazyThemeSet> =
        std::sync::OnceLock::new();
    PRESETS.get_or_init(two_face::theme::extra)
}

pub(super) fn load(directory: &Path) -> (Vec<SyntaxPalette>, Vec<String>) {
    let mut entries = vec![SyntaxPalette {
        id: "ui".into(),
        name: "Follow UI theme".into(),
        theme: None,
    }];
    entries.extend(PRESETS.iter().map(|(id, name, preset)| SyntaxPalette {
        id: (*id).into(),
        name: (*name).into(),
        theme: Some(presets().get(*preset).clone()),
    }));
    let mut warnings = vec![];
    if !directory.exists() {
        return (entries, warnings);
    }
    let result = (|| -> Result<()> {
        let mut paths = vec![];
        for entry in std::fs::read_dir(directory)?.take(1025) {
            let path = entry?.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                paths.push(path);
            }
        }
        paths.sort();
        if paths.len() > 32 {
            warnings.push("Only the first 32 custom syntax themes are loaded".into());
        }
        for path in paths.into_iter().take(32) {
            let result = (|| -> Result<SyntaxPalette> {
                ensure!(
                    std::fs::metadata(&path)?.is_file(),
                    "Syntax theme must be a regular file"
                );
                let file = std::fs::File::open(&path)?;
                ensure!(
                    file.metadata()?.is_file(),
                    "Syntax theme must be a regular file"
                );
                ensure!(
                    file.metadata()?.len() <= MAX_BYTES,
                    "Syntax theme exceeds 128 KiB"
                );
                let mut bytes = vec![];
                file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
                let id = format!(
                    "custom:{}",
                    path.file_stem()
                        .and_then(|name| name.to_str())
                        .context("Syntax theme filename must be UTF-8")?
                );
                SyntaxPalette::from_json(&id, std::str::from_utf8(&bytes)?)
            })();
            match result {
                Ok(theme) => entries.push(theme),
                Err(error) => warnings.push(format!("{}: {error:#}", path.display())),
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        warnings.push(format!("Cannot load syntax themes: {error:#}"));
    }
    (entries, warnings)
}

pub(super) fn generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy)]
struct Settings<'a> {
    settings: &'a SyntectSettings,
    generation: u64,
}
impl std::hash::Hash for Settings<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.generation.hash(state);
    }
}
#[derive(Default)]
struct Highlighter;
impl egui::cache::ComputerMut<(&str, &str, u32, Settings<'_>), LayoutJob> for Highlighter {
    fn compute(
        &mut self,
        (code, language, size, settings): (&str, &str, u32, Settings<'_>),
    ) -> LayoutJob {
        let settings = settings.settings;
        let syntax = settings
            .ps
            .find_syntax_by_name(language)
            .or_else(|| settings.ps.find_syntax_by_extension(language))
            .unwrap_or_else(|| settings.ps.find_syntax_plain_text());
        let theme = &settings.ts.themes["base16-mocha.dark"];
        let font_id = FontId::monospace(f32::from_bits(size));
        let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);
        let mut job = LayoutJob {
            text: code.into(),
            ..Default::default()
        };
        let mut offset = 0;
        for line in syntect::util::LinesWithEndings::from(code) {
            let Ok(tokens) = highlighter.highlight_line(line, &settings.ps) else {
                return LayoutJob::simple(
                    code.into(),
                    font_id,
                    theme
                        .settings
                        .foreground
                        .map(|c| Color32::from_rgb(c.r, c.g, c.b))
                        .unwrap_or(Color32::WHITE),
                    f32::INFINITY,
                );
            };
            for (style, token) in tokens {
                let color =
                    Color32::from_rgb(style.foreground.r, style.foreground.g, style.foreground.b);
                job.sections.push(LayoutSection {
                    leading_space: 0.,
                    byte_range: offset..offset + token.len(),
                    format: TextFormat {
                        font_id: font_id.clone(),
                        color,
                        italics: style.font_style.contains(FontStyle::ITALIC),
                        underline: if style.font_style.contains(FontStyle::UNDERLINE) {
                            Stroke::new(1_f32, color)
                        } else {
                            Stroke::NONE
                        },
                        ..Default::default()
                    },
                });
                offset += token.len();
            }
        }
        job
    }
}

pub(super) fn highlight(
    ctx: &egui::Context,
    code: &str,
    language: &str,
    font_size: f32,
    settings: &SyntectSettings,
    generation: u64,
) -> LayoutJob {
    ctx.memory_mut(|memory| {
        memory
            .caches
            .cache::<egui::cache::FrameCache<LayoutJob, Highlighter>>()
            .get((
                code,
                language,
                font_size.to_bits(),
                Settings {
                    settings,
                    generation,
                },
            ))
    })
}
