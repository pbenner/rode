//! One embedded catalogue shared by editor detection and the SSH tool peer.
use serde::Deserialize;
use std::{path::Path, sync::OnceLock};

/// Embedded language capability record shared with the SSH tool peer.
#[derive(Debug, Deserialize)]
pub struct Language {
    /// Stable language identifier used by tools and extension activation.
    pub id: String,
    /// Syntect syntax name used for lexical highlighting.
    pub syntax: String,
    /// Recognized filename extensions without a leading dot.
    pub extensions: Vec<String>,
    /// Recognized exact filenames, such as build/configuration files.
    pub filenames: Vec<String>,
    /// Optional installed formatter command identifier.
    pub formatter: Option<String>,
    /// Optional installed checker override; otherwise the formatter may provide parser checks.
    #[serde(default)]
    pub checker: Option<String>,
    /// Whether parser/diagnostic checks are available for this language.
    pub check: bool,
    /// Optional literal command template for running a saved source file.
    pub run: Option<Vec<String>>,
    /// Optional literal command template for building a saved source file.
    pub build: Option<Vec<String>>,
}

/// Return the lazily parsed embedded language catalogue; no filesystem scan occurs.
pub fn all() -> &'static [Language] {
    static LANGUAGES: OnceLock<Vec<Language>> = OnceLock::new();
    LANGUAGES.get_or_init(|| {
        serde_json::from_str(include_str!("languages.json")).expect("embedded language catalogue")
    })
}

/// Detect a language from its filename/extension without reading the source or starting tools.
pub fn detect(path: Option<&str>) -> Option<&'static Language> {
    let name = Path::new(path?).file_name()?.to_str()?;
    // Exact filenames take precedence (CMakeLists.txt, .env, Cargo.lock).
    all()
        .iter()
        .find(|l| l.filenames.iter().any(|f| f == name))
        .or_else(|| {
            let extension = name.rsplit_once('.')?.1;
            all().iter().find(|l| {
                l.extensions
                    .iter()
                    .any(|e| e.eq_ignore_ascii_case(extension))
            })
        })
}

impl Language {
    /// Return the configured checker or formatter-based parser-check tool.
    pub fn check_tool(&self) -> Option<&str> {
        self.checker.as_deref().or(self.formatter.as_deref())
    }
    /// All formatters read stdin and return stdout; never pass write-in-place flags.
    pub fn formatter_args(&self, path: &str) -> Vec<String> {
        let args: Vec<&str> = match self.formatter.as_deref() {
            Some("gofmt") => vec!["-e"],
            Some("rustfmt") => vec!["--emit", "stdout", "--edition", "2024"],
            Some("clang-format") => vec!["--style=file", "--assume-filename", path],
            Some("prettier") => vec!["--stdin-filepath", path],
            Some("shfmt") => vec!["--filename", path],
            _ => vec![],
        };
        args.into_iter().map(str::to_owned).collect()
    }

    /// Resolve project-local JS tools without npx or package installation.
    pub fn executable(root: &Path, program: &str) -> String {
        if matches!(program, "prettier" | "tsx") {
            let local = root.join("node_modules/.bin").join(program);
            if local.is_file() {
                return local.to_string_lossy().into();
            }
        }
        program.into()
    }
}

/// gofmt/shfmt use path:row:column; Prettier reports (row:column).
/// Leave tool/configuration failures as errors rather than inventing source issues.
pub fn syntax_diagnostics(stderr: &str, tool: &str) -> Vec<crate::project::Diagnostic> {
    stderr
        .lines()
        .filter_map(|line| {
            let clean = line.trim().trim_start_matches("[error] ");
            let location = if tool == "prettier" {
                let (message, location) = clean.rsplit_once('(')?;
                let (row, column) = location.strip_suffix(')')?.split_once(':')?;
                Some((
                    row.parse::<usize>().ok()?,
                    column.parse::<usize>().ok()?,
                    message.trim(),
                ))
            } else {
                let parts: Vec<_> = clean.split(':').collect();
                (1..parts.len().saturating_sub(2)).find_map(|i| {
                    Some((
                        parts[i].parse::<usize>().ok()?,
                        parts[i + 1].parse::<usize>().ok()?,
                        clean.splitn(i + 3, ':').last()?.trim(),
                    ))
                })
            }?;
            let (row, column, message) = location;
            Some(crate::project::Diagnostic {
                code: tool.into(),
                message: message.into(),
                row: row.max(1),
                column: column.max(1),
                end_row: row.max(1),
                end_column: column.saturating_add(1).max(2),
                severity: "error".into(),
            })
        })
        .take(2000)
        .collect()
}

/// Tool columns use UTF-8 bytes (Go/shell) or UTF-16 units (Prettier).
/// The editor uses Unicode scalar indices; scan source once and retain only issue rows.
pub fn normalize_columns(diagnostics: &mut [crate::project::Diagnostic], source: &str, tool: &str) {
    let mut rows = std::collections::BTreeMap::<usize, Vec<usize>>::new();
    for (index, diagnostic) in diagnostics.iter().enumerate() {
        rows.entry(diagnostic.row).or_default().push(index);
    }
    for (line_index, line) in source.lines().enumerate() {
        if let Some(indices) = rows.remove(&(line_index + 1)) {
            for index in indices {
                let diagnostic = &mut diagnostics[index];
                let mut units = 0;
                let column = line
                    .chars()
                    .take_while(|ch| {
                        let before = units;
                        units += if tool == "prettier" {
                            ch.len_utf16()
                        } else {
                            ch.len_utf8()
                        };
                        before < diagnostic.column.saturating_sub(1)
                    })
                    .count()
                    + 1;
                diagnostic.column = column;
                diagnostic.end_column = column + 1;
            }
        }
        if rows.is_empty() {
            break;
        }
    }
}
