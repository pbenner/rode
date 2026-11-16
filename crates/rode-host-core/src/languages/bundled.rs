//! Offline buffer formatting shared by the desktop and headless project host.
//!
//! These are linked Rust libraries, not downloaded plugins or subprocesses. Installed
//! Prettier remains authoritative for its configuration/plugins; this fallback never
//! evaluates project configuration. The surrounding host worker owns cancellation and
//! its deadline. Input/output retain the normal code-tool size limits.

use super::Language;
use crate::project::{Analysis, Diagnostic};
use anyhow::{Result, bail, ensure};
use std::{borrow::Cow, path::Path};

const WIDTH: u32 = 80;
const MAX_NESTING: usize = 8;

/// Format/check a Prettier-family snapshot without reading or modifying its file.
pub(crate) fn analyze(
    language: &Language,
    path: &str,
    text: &str,
    format: bool,
) -> Result<Analysis> {
    let mut result = Analysis {
        text: text.into(),
        ..Default::default()
    };
    // Keep the encoding marker and newline convention, even for libraries that strip
    // the marker or always print LF. Parse locations are relative to the body below.
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    match format_text(&language.id, Path::new(path), body, WIDTH, 0) {
        Ok(formatted) if format => {
            let formatted = formatted.replace("\r\n", "\n");
            let crlf = body
                .find('\n')
                .is_some_and(|i| i > 0 && body.as_bytes()[i - 1] == b'\r');
            let formatted = if crlf {
                formatted.replace('\n', "\r\n")
            } else {
                formatted
            };
            ensure!(
                formatted.len() as u64 + (text.len() - body.len()) as u64
                    <= crate::files::MAX_TOOL_BYTES,
                "Bundled formatter output exceeded its size limit."
            );
            result.text = format!("{}{formatted}", &text[..text.len() - body.len()]);
        }
        Ok(_) => {}
        Err(Failure::Syntax(mut diagnostics)) => {
            if body.len() != text.len() {
                for diagnostic in &mut diagnostics {
                    if diagnostic.row == 1 {
                        diagnostic.column += 1;
                    }
                    if diagnostic.end_row == 1 {
                        diagnostic.end_column += 1;
                    }
                }
            }
            result.diagnostics = diagnostics;
            if format {
                result.warnings.push(
                    "Bundled formatter reported syntax errors; the buffer was kept unchanged."
                        .into(),
                );
            }
        }
        Err(Failure::Other(message)) => bail!("Bundled formatter: {message}"),
    }
    Ok(result)
}

#[derive(Debug)]
enum Failure {
    Syntax(Vec<Diagnostic>),
    Other(String),
}

/// Convert a parser's byte offset to the editor's one-based Unicode scalar position.
/// EOF and non-boundary offsets are clamped, without slicing through a UTF-8 character.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = &text[..offset];
    let row = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
    let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (row, column)
}

fn diagnostic(row: usize, column: usize, message: impl Into<String>) -> Diagnostic {
    Diagnostic {
        code: "syntax".into(),
        message: message.into(),
        row,
        column,
        end_row: row,
        end_column: column,
        severity: "error".into(),
    }
}

fn syntax(text: &str, offset: usize, message: impl Into<String>) -> Failure {
    let (row, column) = position(text, offset);
    Failure::Syntax(vec![diagnostic(row, column, message)])
}

fn typescript_error(error: dprint_plugin_typescript::FormatError, text: &str) -> Failure {
    use dprint_plugin_typescript::FormatError;
    let errors = match error {
        FormatError::Parse(error) => vec![error],
        FormatError::SyntaxErrors(errors) => errors.0,
        FormatError::Other(message) => return Failure::Other(message),
    };
    let mut diagnostics = errors
        .into_iter()
        .take(2000)
        .map(|error| {
            let location = error.display_position();
            diagnostic(
                location.line_number,
                location.column_number,
                error.message(),
            )
        })
        .collect::<Vec<_>>();
    // SWC exposes UTF-16 positions, just like installed Prettier.
    super::normalize_columns(&mut diagnostics, text, "prettier");
    Failure::Syntax(diagnostics)
}

/// Embedded snippets use the same defaults with the parent's remaining width. Unknown
/// languages and deeply nested examples stay untouched; no external tools are started.
fn embedded(ext: &str, code: &str, width: u32, depth: usize) -> Result<Option<String>, Failure> {
    if depth >= MAX_NESTING {
        return Ok(None);
    }
    let ext = match ext {
        "javascript" => "js",
        "typescript" => "ts",
        "markdown" => "md",
        "yml" => "yaml",
        other => other,
    };
    let path = format!("snippet.{ext}");
    let Some(language) =
        super::detect(Some(&path)).filter(|l| l.formatter.as_deref() == Some("prettier"))
    else {
        return Ok(None);
    };
    format_text(
        &language.id,
        Path::new(&path),
        code,
        width.clamp(20, WIDTH),
        depth + 1,
    )
    .map(Some)
}

fn format_text(
    id: &str,
    path: &Path,
    text: &str,
    width: u32,
    depth: usize,
) -> Result<String, Failure> {
    match id {
        "javascript" | "jsx" | "typescript" | "tsx" => {
            use dprint_plugin_typescript::{
                FormatTextOptions,
                configuration::{ConfigurationBuilder, SortOrder},
            };
            let config = ConfigurationBuilder::new()
                .line_width(width)
                .indent_width(2)
                // Import order can affect module initialization. Formatting must not
                // silently turn into organize-imports or reorder package metadata.
                .module_sort_import_declarations(SortOrder::Maintain)
                .module_sort_export_declarations(SortOrder::Maintain)
                .import_declaration_sort_named_imports(SortOrder::Maintain)
                .export_declaration_sort_named_exports(SortOrder::Maintain)
                .build();
            dprint_plugin_typescript::format_text(FormatTextOptions {
                path,
                extension: None,
                text: text.into(),
                config: &config,
                external_formatter: None,
                arena_pool: None,
            })
            .map(|output| output.unwrap_or_else(|| text.into()))
            .map_err(|e| typescript_error(e, text))
        }
        "json" => {
            let config = dprint_plugin_json::configuration::ConfigurationBuilder::new()
                .line_width(width)
                .indent_width(2)
                .package_json_apply_conventions(false)
                .build();
            dprint_plugin_json::format_text(path, text, &config)
                .map(|output| output.unwrap_or_else(|| text.into()))
                .map_err(|e| syntax(text, e.parse_error().range().start, e.message()))
        }
        "yaml" => {
            let mut config = pretty_yaml::config::FormatOptions::default();
            config.layout.print_width = width as usize;
            pretty_yaml::format_text(text, &config)
                .map_err(|e| syntax(text, e.offset(), e.message()))
        }
        "css" | "scss" => {
            let mut config = malva::config::FormatOptions::default();
            config.layout.print_width = width as usize;
            let language = if id == "scss" {
                malva::Syntax::Scss
            } else {
                malva::Syntax::Css
            };
            malva::format_text(text, language, &config).map_err(|e| {
                let malva::Error::Parser(error, _, _) = e;
                syntax(text, error.span.start, error.kind.to_string())
            })
        }
        "html" => {
            let mut config = markup_fmt::config::FormatOptions::default();
            config.layout.print_width = width as usize;
            let mut embedded_diagnostics = vec![];
            let output = markup_fmt::format_text(
                text,
                markup_fmt::Language::Html,
                &config,
                |code, hints| {
                    // Attribute callbacks may contain expressions wrapped by the HTML
                    // formatter rather than complete source programs. Leave those alone.
                    if hints.attr {
                        return Ok(Cow::Borrowed(code));
                    }
                    match embedded(
                        hints.ext,
                        code,
                        hints.print_width.min(width as usize) as u32,
                        depth,
                    ) {
                        Ok(Some(formatted)) => Ok(Cow::Owned(formatted)),
                        Ok(None) => Ok(Cow::Borrowed(code)),
                        Err(failure) => {
                            if let Failure::Syntax(diagnostics) = &failure {
                                // Only project positions when the callback is an actual
                                // slice of this source; transformed snippets have no exact map.
                                if let Some(start) = (code.as_ptr() as usize)
                                    .checked_sub(text.as_ptr() as usize)
                                    .filter(|start| {
                                        *start <= text.len() && code.len() <= text.len() - *start
                                    })
                                {
                                    let (row, column) = position(text, start);
                                    for mut diagnostic in diagnostics.iter().cloned() {
                                        if diagnostic.row == 1 {
                                            diagnostic.column += column - 1;
                                        }
                                        if diagnostic.end_row == 1 {
                                            diagnostic.end_column += column - 1;
                                        }
                                        diagnostic.row += row - 1;
                                        diagnostic.end_row += row - 1;
                                        embedded_diagnostics.push(diagnostic);
                                    }
                                }
                            }
                            Err(anyhow::anyhow!(
                                "Embedded {} could not be formatted: {failure:?}",
                                hints.ext
                            ))
                        }
                    }
                },
            );
            output.map_err(|e| match e {
                markup_fmt::FormatError::Syntax(e) => syntax(text, e.pos, e.kind.to_string()),
                markup_fmt::FormatError::External(_) if !embedded_diagnostics.is_empty() => {
                    embedded_diagnostics.truncate(2000);
                    Failure::Syntax(embedded_diagnostics)
                }
                error => Failure::Other(error.to_string()),
            })
        }
        "markdown" => {
            let config = dprint_plugin_markdown::configuration::ConfigurationBuilder::new()
                .line_width(width)
                .code_block_preserve_indentation(true)
                .code_block_preserve_blank_lines(true)
                .build();
            dprint_plugin_markdown::format_text(text, &config, |tag, code, width| {
                // Tutorials deliberately contain incomplete examples. The Markdown
                // remains formattable while those particular fences stay unchanged.
                Ok(embedded(tag, code, width, depth).ok().flatten())
            })
            .map(|output| output.unwrap_or_else(|| text.into()))
            .map_err(|e| Failure::Other(e.to_string()))
        }
        _ => Err(Failure::Other(format!("Unsupported language: {id}"))),
    }
}

#[cfg(test)]
mod tests;
