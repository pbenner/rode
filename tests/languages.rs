//! Embedded language detection, installed stdin tools and local/Python diagnostics parity
//! without automatic installation.

use rode::{app, languages, theme::Themes};

#[test]
fn language_detection_covers_extensions_and_special_filenames() {
    for (path, expected) in [
        ("src/main.go", "go"),
        ("types.pyi", "python"),
        ("gui.pyw", "python"),
        ("SOURCE.PY", "python"),
        ("include/test.HPP", "cpp"),
        ("index.mts", "typescript"),
        ("index.tsx", "tsx"),
        ("CMakeLists.txt", "cmake"),
        ("Dockerfile", "dockerfile"),
        ("Makefile", "makefile"),
        ("path/.bashrc", "bash"),
        (".env", "dotenv"),
        ("Cargo.lock", "toml"),
        ("module.F90", "fortran"),
        ("guide.MARKDOWN", "markdown"),
        ("main.TEX", "latex"),
        ("paper.ltx", "latex"),
        ("class.cls", "latex"),
        ("macro.sty", "latex"),
        ("references.bib", "bibtex"),
    ] {
        assert_eq!(app::language(Some(path)), expected, "{path}");
    }
    for path in [None, Some("unknown.filetype"), Some("/folder.with.rs/file")] {
        assert_eq!(app::language(path), "text");
    }
}

#[test]
fn every_catalogue_entry_has_an_embedded_grammar() {
    let syntaxes = two_face::syntax::extra_newlines();
    let mut missing = vec![];
    for language in languages::all() {
        if syntaxes
            .find_syntax_by_name(&language.syntax)
            .or_else(|| syntaxes.find_syntax_by_extension(&language.syntax))
            .is_none()
        {
            missing.push(format!("{} ({})", language.id, language.syntax));
        }
    }
    assert!(missing.is_empty(), "missing grammars: {missing:?}");
}

#[test]
fn common_languages_render_colored_tokens_with_each_theme() {
    let directory = tempfile::tempdir().unwrap();
    let (mut themes, _) = Themes::load(directory.path(), "phosphor");
    let ctx = egui_context();
    for theme in 0..themes.entries.len() {
        let id = themes.entries[theme].id.clone();
        themes.select(&id, &ctx).unwrap();
        for (path, code) in [
            (
                "main.go",
                "package main\nfunc main() { println(\"hello\") }\n",
            ),
            (
                "main.py",
                "# comment\ndef hello(value: str):\n    return value + 'hello'\n",
            ),
            ("main.ts", "// comment\nconst value: string = 'hello';\n"),
            (
                "view.tsx",
                "// comment\nexport const App = () => <div title=\"hello\" />;\n",
            ),
            (
                "main.tex",
                "% comment\n\\documentclass{article}\n\\begin{document} $E=mc^2$ \\end{document}\n",
            ),
            ("Cargo.toml", "# comment\n[package]\nname = \"rode\"\n"),
            (
                "Dockerfile",
                "# comment\nFROM alpine:latest\nRUN echo hello\n",
            ),
        ] {
            let job = themes.highlight_with_style(
                &ctx,
                &Default::default(),
                code,
                app::syntax_token(Some(path)),
                14.,
            );
            let colors: std::collections::HashSet<_> =
                job.sections.iter().map(|s| s.format.color).collect();
            assert!(colors.len() > 1, "plain text: {path} theme {theme}");
            assert_eq!(job.text, code);
        }
    }
}
fn egui_context() -> eframe::egui::Context {
    eframe::egui::Context::default()
}

#[test]
fn diagnostic_locations_match_unicode_editor_columns() {
    for (tool, stderr) in [
        ("gofmt", "<standard input>:1:6: unexpected token"),
        ("shfmt", "stdin:1:6: unexpected token"),
        ("prettier", "[error] source.ts: SyntaxError (1:4)"),
    ] {
        let mut issues = languages::syntax_diagnostics(stderr, tool);
        languages::normalize_columns(&mut issues, "🦀 ?", tool);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].row, 1);
        assert_eq!(issues[0].column, 3);
        assert_eq!(issues[0].end_column, 4);
    }
    assert!(languages::syntax_diagnostics("Cannot find configuration", "prettier").is_empty());
}
