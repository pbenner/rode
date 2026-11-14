//! Bounded interface-preset parsing and inheritance. Catalog order controls the
//! menu only; resolving a base follows identifiers and rejects cycles.
use super::{Chrome, Palette, Theme, interface, parse_color, syntax::syntax_color};
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::collections::BTreeMap;
use syntect::highlighting::{FontStyle, StyleModifier, Theme as SyntaxTheme, ThemeItem};

#[cfg(test)]
mod tests;

/// Maximum interface preset size, shared by direct JSON parsing and file loading.
pub(super) const MAX_THEME_BYTES: u64 = 64 * 1024;
/// Maximum custom presets admitted to the startup catalog.
pub(super) const MAX_CUSTOM_THEMES: usize = 32;

const EMBEDDED_THEMES: &[(&str, &str)] = &[
    (
        "atelier-dark",
        include_str!("../../themes/atelier-dark.json"),
    ),
    (
        "atelier-light",
        include_str!("../../themes/atelier-light.json"),
    ),
    ("carbon", include_str!("../../themes/carbon.json")),
    ("carbon-edge", include_str!("../../themes/carbon-edge.json")),
    ("code-dark", include_str!("../../themes/code-dark.json")),
    ("matrix", include_str!("../../themes/matrix.json")),
    ("amber", include_str!("../../themes/amber.json")),
    ("ice", include_str!("../../themes/ice.json")),
    ("neon", include_str!("../../themes/neon.json")),
    ("afterglow", include_str!("../../themes/afterglow.json")),
    (
        "knight-rider",
        include_str!("../../themes/knight-rider.json"),
    ),
    ("hologram", include_str!("../../themes/hologram.json")),
    ("telemetry", include_str!("../../themes/telemetry.json")),
    ("studio", include_str!("../../themes/studio.json")),
    (
        "studio-neutral",
        include_str!("../../themes/studio-neutral.json"),
    ),
    (
        "studio-hacker",
        include_str!("../../themes/studio-hacker.json"),
    ),
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    name: String,
    appearance: Option<Appearance>,
    #[serde(default = "default_base")]
    base: String,
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    syntax: BTreeMap<String, String>,
    ansi: Option<[String; 16]>,
    terminal: Option<TerminalSettings>,
    chrome: Option<Chrome>,
    ui: Option<interface::Overrides>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Appearance {
    Dark,
    Light,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalSettings {
    minimum_contrast: f32,
}

fn default_base() -> String {
    "phosphor".into()
}

impl ThemeFile {
    fn parse(json: &str) -> Result<Self> {
        ensure!(json.len() as u64 <= MAX_THEME_BYTES, "Theme exceeds 64 KiB");
        serde_json::from_str(json).context("Invalid theme JSON")
    }

    fn apply(self, id: &str, base: &Theme) -> Result<Theme> {
        let file = self;
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
        if let Some(appearance) = file.appearance {
            theme.palette.dark_mode = matches!(appearance, Appearance::Dark);
        }
        if let Some(chrome) = file.chrome {
            theme.palette.chrome = chrome;
        }
        if let Some(ui) = file.ui {
            theme.palette.ui = Some(theme.palette.ui.unwrap_or_default().merge(ui)?);
        }
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
}

/// Decode once, resolve a built-in base, then apply validated overrides.
pub(super) fn from_json(id: &str, json: &str) -> Result<Theme> {
    let file = ThemeFile::parse(json)?;
    let base = builtin(&file.base)
        .with_context(|| format!("Unknown built-in theme base '{}'", file.base))?;
    file.apply(id, &base)
}

/// Invalid embedded definitions are programming errors; unknown selections return `None`.
pub(super) fn builtin(id: &str) -> Option<Theme> {
    if id != "phosphor" && !EMBEDDED_THEMES.iter().any(|(name, _)| *name == id) {
        return None;
    }
    Some(resolve(id, EMBEDDED_THEMES, &mut Vec::new()).expect("embedded theme is valid"))
}

/// Keep presentation order in the catalog while resolving bases by identifier.
pub(super) fn builtins() -> Vec<Theme> {
    std::iter::once(Theme::phosphor())
        .chain(
            EMBEDDED_THEMES
                .iter()
                .map(|(id, _)| builtin(id).expect("embedded theme exists")),
        )
        .collect()
}

/// Every recursion step follows a registered base. A repeated ancestor is an
/// error, so resolution is bounded by the number of definitions in the catalog.
fn resolve(id: &str, definitions: &[(&str, &str)], chain: &mut Vec<String>) -> Result<Theme> {
    if id == "phosphor" {
        return Ok(Theme::phosphor());
    }
    ensure!(
        !chain.iter().any(|ancestor| ancestor == id),
        "Theme inheritance cycle: {} -> {id}",
        chain.join(" -> ")
    );
    let (_, json) = definitions
        .iter()
        .find(|(name, _)| *name == id)
        .with_context(|| format!("Unknown built-in theme base '{id}'"))?;
    let file = ThemeFile::parse(json).with_context(|| format!("Theme '{id}'"))?;
    chain.push(id.into());
    let base = resolve(&file.base, definitions, chain);
    chain.pop();
    file.apply(id, &base?)
        .with_context(|| format!("Theme '{id}'"))
}

impl Theme {
    /// Build syntax colors from semantic overrides or retain the original scope rules when none
    /// exist.
    /// An explicit semantic palette starts a fresh scope map so inherited rules cannot override it.
    pub(super) fn syntax_theme(&self, original: &SyntaxTheme) -> SyntaxTheme {
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
