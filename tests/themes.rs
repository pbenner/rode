//! Palette/syntax parsing, cached layouts, independent preferences and concurrent theme edits.

use eframe::egui::{self, Color32};
use rode::theme::{Palette, Theme, Themes, parse_color};
use std::fs;

fn color_for(layout: &egui::text::LayoutJob, token: &str) -> Color32 {
    let index = layout.text.find(token).unwrap() + token.len() / 2;
    layout
        .sections
        .iter()
        .find(|section| section.byte_range.contains(&index))
        .unwrap()
        .format
        .color
}

#[test]
fn switching_recolors_cached_source_and_restores_original() {
    let directory = tempfile::tempdir().unwrap();
    let (mut themes, warnings) = Themes::load(directory.path(), "phosphor");
    assert!(warnings.is_empty());
    let ctx = egui::Context::default();
    let style = egui::Style::default();
    let source = "// comment\npub fn run(value: String) { let total = 42; println!(\"hello\"); }";
    let original = themes.highlight_with_style(&ctx, &style, source, "rs", 14.0);
    themes.select("carbon", &ctx).unwrap();
    let carbon = themes.highlight_with_style(&ctx, &style, source, "rs", 14.0);
    assert_ne!(
        original, carbon,
        "Unchanged source must not retain cached colors from the previous theme"
    );
    assert_eq!(
        color_for(&carbon, "comment"),
        parse_color("#808080").unwrap()
    );
    assert_eq!(color_for(&carbon, "pub"), parse_color("#129b78").unwrap());
    assert_eq!(color_for(&carbon, "run"), parse_color("#cb3834").unwrap());
    assert_eq!(
        color_for(&carbon, "String"),
        parse_color("#e6b93c").unwrap()
    );
    assert_eq!(color_for(&carbon, "42"), parse_color("#d59442").unwrap());
    assert_eq!(color_for(&carbon, "hello"), parse_color("#d4d4d4").unwrap());
    assert_eq!(
        ctx.style().visuals.panel_fill,
        themes.active().palette.background
    );
    assert_eq!(
        ctx.style().visuals.selection.bg_fill,
        themes.active().palette.selection
    );
    themes.select("phosphor", &ctx).unwrap();
    assert_eq!(
        themes.highlight_with_style(&ctx, &style, source, "rs", 14.0),
        original
    );
    assert_eq!(themes.active().palette, Palette::default());
}

#[test]
fn custom_theme_inherits_carbon_and_loads_by_stable_filename() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("violet.json"),
        r##"{
        "name":"Violet", "base":"carbon",
        "colors":{"accent":"#bbaaff", "terminal_background":"#15121a"},
        "syntax":{"comment":"#a08bb4"}
    }"##,
    )
    .unwrap();
    let (themes, warnings) = Themes::load(directory.path(), "custom:violet");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(themes.active().name, "Violet");
    let palette = themes.active().palette;
    assert_eq!(palette.background, Theme::carbon().palette.background);
    assert_eq!(palette.accent, parse_color("#bbaaff").unwrap());
    assert_eq!(palette.terminal_background, parse_color("#15121a").unwrap());
    let layout = themes.highlight_with_style(
        &egui::Context::default(),
        &Default::default(),
        "// comment\npub fn run() {}",
        "rs",
        14.0,
    );
    assert_eq!(
        color_for(&layout, "comment"),
        parse_color("#a08bb4").unwrap()
    );
    assert_eq!(color_for(&layout, "pub"), parse_color("#129b78").unwrap());
}

#[test]
fn invalid_or_missing_theme_falls_back_without_losing_valid_themes() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("broken.json"),
        r##"{"name":"Broken","colors":{"accent":"#nope"}}"##,
    )
    .unwrap();
    fs::write(
        directory.path().join("good.json"),
        r##"{"name":"Good","base":"carbon"}"##,
    )
    .unwrap();
    let (themes, warnings) = Themes::load(directory.path(), "custom:missing");
    assert_eq!(themes.active().id, "phosphor");
    assert_eq!(themes.entries.len(), Theme::builtins().len() + 1);
    assert!(warnings.iter().any(|w| w.contains("broken.json")));
    assert!(warnings.iter().any(|w| w.contains("unavailable")));
    for json in [
        r##"{"name":"X","base":"missing"}"##,
        r##"{"name":"X","colors":{"typo":"#ffffff"}}"##,
        r##"{"name":"X","syntax":{"typo":"#ffffff"}}"##,
        r##"{"name":"X","ansi":["#ffffff"]}"##,
        r##"{"name":"","base":"carbon"}"##,
        r##"{"name":"X","unexpected":true}"##,
    ] {
        assert!(Theme::from_json("custom:x", json).is_err(), "{json}");
    }
}

#[test]
fn custom_theme_file_and_count_limits_are_enforced() {
    let directory = tempfile::tempdir().unwrap();
    fs::File::create(directory.path().join("000-huge.json"))
        .unwrap()
        .set_len(64 * 1024 + 1)
        .unwrap();
    for index in 0..35 {
        fs::write(
            directory.path().join(format!("theme-{index:02}.json")),
            format!(r##"{{"name":"Theme {index}"}}"##),
        )
        .unwrap();
    }
    let (themes, warnings) = Themes::load(directory.path(), "phosphor");
    assert!(themes.entries.len() <= Theme::builtins().len() + 32);
    assert!(warnings.iter().any(|w| w.contains("64 KiB")));
    assert!(warnings.iter().any(|w| w.contains("first 32")));
}

#[test]
fn terminal_uses_theme_ansi_but_preserves_truecolor_and_extended_colors() {
    use rode::terminal::ansi_color;
    let original = Theme::phosphor().palette;
    let carbon = Theme::carbon().palette;
    assert_ne!(
        ansi_color(vt100::Color::Idx(3), original.text, &original.ansi),
        ansi_color(vt100::Color::Idx(3), carbon.text, &carbon.ansi)
    );
    assert_eq!(
        ansi_color(
            vt100::Color::Default,
            carbon.terminal_background,
            &carbon.ansi
        ),
        carbon.terminal_background
    );
    assert_eq!(
        ansi_color(vt100::Color::Rgb(1, 2, 3), carbon.text, &carbon.ansi),
        Color32::from_rgb(1, 2, 3)
    );
    assert_eq!(
        ansi_color(vt100::Color::Idx(196), carbon.text, &carbon.ansi),
        Color32::RED
    );
}

#[test]
fn themes_can_set_terminal_contrast_without_changing_ansi_or_syntax() {
    let base = Theme::carbon();
    assert_eq!(base.palette.terminal_minimum_contrast, 1.0);
    for minimum in [1., 3., 7., 21.] {
        let file = format!(
            r#"{{"name":"Readable","base":"carbon","terminal":{{"minimum_contrast":{minimum}}}}}"#
        );
        let theme = Theme::from_json("custom:readable", &file).unwrap();
        assert_eq!(theme.palette.terminal_minimum_contrast, minimum);
        assert_eq!(theme.palette.ansi, base.palette.ansi);
        assert_eq!(
            theme.palette.terminal_background,
            base.palette.terminal_background
        );
    }
    for value in ["0.9", "21.1", "null", "\"4.5\"", "1e999"] {
        let file = format!(r#"{{"name":"Invalid","terminal":{{"minimum_contrast":{value}}}}}"#);
        assert!(Theme::from_json("custom:invalid", &file).is_err());
    }
    assert!(
        Theme::from_json(
            "custom:invalid",
            r#"{"name":"Invalid","terminal":{"minimum_contrast":4.5,"unknown":1}}"#
        )
        .is_err()
    );
}

#[test]
fn unknown_extensions_use_custom_default_text_color() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("plain.json"),
        r##"{"name":"Plain","colors":{"text":"#abbaee"}}"##,
    )
    .unwrap();
    let (themes, warnings) = Themes::load(directory.path(), "custom:plain");
    assert!(warnings.is_empty());
    let layout = themes.highlight_with_style(
        &egui::Context::default(),
        &Default::default(),
        "plain file",
        "unknown-extension",
        14.0,
    );
    assert_eq!(color_for(&layout, "plain"), parse_color("#abbaee").unwrap());
}

#[test]
fn builtin_palettes_are_readable_and_custom_bases_preserve_their_syntax() {
    fn contrast(a: Color32, b: Color32) -> f64 {
        let luminance = |c: Color32| {
            let linear = |v: u8| {
                let s = f64::from(v) / 255.;
                if s <= 0.04045 {
                    s / 12.92
                } else {
                    ((s + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b())
        };
        let a = luminance(a);
        let b = luminance(b);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    let directory = tempfile::tempdir().unwrap();
    let source = "// comment\npub fn run(value: String) { let n = 42; println!(\"hello\"); }";
    let mut identities = std::collections::HashSet::new();
    let mut signatures = std::collections::HashSet::new();
    for builtin in Theme::builtins() {
        let ctx = egui::Context::default();
        assert!(identities.insert(builtin.id.clone()), "Duplicate theme ID");
        let p = builtin.palette;
        assert!(
            signatures.insert((p.background, p.accent)),
            "Duplicate palette"
        );
        assert!(contrast(p.text, p.background) >= 7., "{} text", builtin.id);
        assert!(
            contrast(p.muted, p.panel) >= 4.5,
            "{} muted labels",
            builtin.id
        );
        assert!(
            contrast(p.text, p.selection) >= 4.5,
            "{} selection",
            builtin.id
        );
        assert!(
            contrast(p.terminal_foreground, p.terminal_background) >= 7.,
            "{} terminal",
            builtin.id
        );
        let file = format!(r#"{{"name":"Derived","base":"{}"}}"#, builtin.id);
        fs::write(directory.path().join("derived.json"), &file).unwrap();
        assert_eq!(
            Theme::from_json("custom:derived", &file).unwrap().palette,
            p
        );
        let (mut themes, warnings) = Themes::load(directory.path(), &builtin.id);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(themes.active().id, builtin.id);
        let original = themes.highlight_with_style(&ctx, &Default::default(), source, "rs", 14.);
        // Phosphor retains its original syntect palette for compatibility.
        if builtin.id != "phosphor" {
            assert!(
                contrast(color_for(&original, "comment"), p.background) >= 4.5,
                "{} comments",
                builtin.id
            );
        }
        themes.select("custom:derived", &ctx).unwrap();
        assert_eq!(
            themes.highlight_with_style(&ctx, &Default::default(), source, "rs", 14.),
            original
        );
    }
    assert!(
        identities.is_superset(
            &["code-dark", "matrix", "amber", "ice", "neon"]
                .map(str::to_owned)
                .into()
        )
    );
}

#[test]
fn syntax_palettes_are_independent_and_follow_ui_remains_available() {
    let directory = tempfile::tempdir().unwrap();
    let (mut themes, warnings) = Themes::load(directory.path(), "carbon");
    assert!(warnings.is_empty());
    let ctx = egui::Context::default();
    let style = egui::Style::default();
    themes.select("carbon", &ctx).unwrap();
    let source =
        "// comment\npub fn greet(value: String) { let answer = 42; println!(\"hello\"); }\n";
    let linked = themes.highlight_with_style(&ctx, &style, source, "rs", 17.);
    let palette = themes.active().palette;
    let visuals = ctx.style().visuals.clone();
    themes.select_syntax("dracula", &ctx).unwrap();
    let dracula = themes.highlight_with_style(&ctx, &style, source, "rs", 17.);
    assert_ne!(dracula, linked);
    assert_eq!(themes.active().palette, palette);
    assert_eq!(ctx.style().visuals, visuals);
    for ui in ["phosphor", "code-dark", "matrix", "amber", "ice", "neon"] {
        themes.select(ui, &ctx).unwrap();
        assert_eq!(themes.active_syntax().id, "dracula");
        assert_eq!(
            themes.highlight_with_style(&ctx, &style, source, "rs", 17.),
            dracula
        );
    }
    themes.select("carbon", &ctx).unwrap();
    themes.select_syntax("ui", &ctx).unwrap();
    assert_eq!(
        themes.highlight_with_style(&ctx, &style, source, "rs", 17.),
        linked
    );
    for syntax in [
        "dracula",
        "catppuccin-mocha",
        "nord",
        "monokai",
        "one-half-dark",
    ] {
        themes.select_syntax(syntax, &ctx).unwrap();
        let layout = themes.highlight_with_style(&ctx, &style, source, "rs", 17.);
        assert_eq!(layout.text, source);
        assert!(
            layout
                .sections
                .iter()
                .all(|section| section.format.font_id.size == 17.)
        );
        let colors: std::collections::HashSet<_> = layout
            .sections
            .iter()
            .map(|section| section.format.color)
            .collect();
        assert!(
            colors.len() >= 5,
            "{syntax} should retain its detailed palette, got {colors:?}"
        );
    }
    assert!(themes.select_syntax("missing", &ctx).is_err());
    assert_eq!(themes.active_syntax().id, "one-half-dark");
}

#[test]
fn custom_syntax_scopes_preserve_italics_and_underlines_separately() {
    let directory = tempfile::tempdir().unwrap();
    let ui_directory = directory.path().join("themes");
    let syntax_directory = directory.path().join("syntax-themes");
    fs::create_dir_all(&ui_directory).unwrap();
    fs::create_dir_all(&syntax_directory).unwrap();
    fs::write(syntax_directory.join("detailed.json"),r##"{
        "name":"Detailed", "base":"dracula", "foreground":"#e0e0e0", "rules":[
            {"scope":"source.rust comment.line.double-slash.rust","foreground":"#9988aa","font_style":"italic"},
            {"scope":"source.rust entity.name.function","foreground":"#88ffcc","font_style":"underline"},
            {"scope":"source.python entity.name.function","foreground":"#ff88cc","font_style":"normal"}
        ]
    }"##).unwrap();
    let (mut themes, warnings) = Themes::load(&ui_directory, "carbon");
    assert!(warnings.is_empty(), "{warnings:?}");
    let ctx = egui::Context::default();
    themes.select_syntax("custom:detailed", &ctx).unwrap();
    let source = "// comment 🦀\nfn greet() { let s = \"hello\"; }\n";
    let layout = themes.highlight_with_style(&ctx, &Default::default(), source, "rs", 14.);
    assert_eq!(layout.text, source);
    for section in &layout.sections {
        assert!(source.is_char_boundary(section.byte_range.start));
        assert!(source.is_char_boundary(section.byte_range.end));
    }
    let comment = layout
        .sections
        .iter()
        .find(|s| s.byte_range.contains(&source.find("comment").unwrap()))
        .unwrap();
    assert_eq!(comment.format.color, parse_color("#9988aa").unwrap());
    assert!(comment.format.italics);
    assert_eq!(
        comment.format.underline,
        egui::Stroke::NONE,
        "italics must not acquire underlines"
    );
    let function = layout
        .sections
        .iter()
        .find(|s| s.byte_range.contains(&source.find("greet").unwrap()))
        .unwrap();
    assert_eq!(function.format.color, parse_color("#88ffcc").unwrap());
    assert!(!function.format.italics);
    assert!(function.format.underline.width > 0.);
    let python = themes.highlight_with_style(
        &ctx,
        &Default::default(),
        "def greet():\n    pass\n",
        "py",
        14.,
    );
    assert_eq!(color_for(&python, "greet"), parse_color("#ff88cc").unwrap());
    let plain =
        themes.highlight_with_style(&ctx, &Default::default(), "plain text", "unknown", 14.);
    assert_eq!(color_for(&plain, "plain"), parse_color("#e0e0e0").unwrap());
}

#[test]
fn malformed_or_excessive_syntax_themes_leave_presets_usable() {
    use rode::theme::syntax::SyntaxPalette;
    for json in [
        r##"{"name":"Bad","base":"absent"}"##,
        r##"{"name":"Bad","foreground":"blue"}"##,
        r##"{"name":"Bad","rules":[{"scope":"","foreground":"#123456"}]}"##,
        r##"{"name":"Bad","rules":[{"scope":"comment","font_style":"bold"}]}"##,
        r##"{"name":"Bad","rules":[{"scope":"comment"}]}"##,
        r##"{"name":"Bad","unexpected":1}"##,
    ] {
        assert!(
            SyntaxPalette::from_json("custom:bad", json).is_err(),
            "{json}"
        );
    }
    let rules = vec![serde_json::json!({"scope":"comment","foreground":"#abcdef"}); 257];
    assert!(
        SyntaxPalette::from_json(
            "custom:large",
            &serde_json::json!({"name":"Large","rules":rules}).to_string()
        )
        .is_err()
    );
    assert!(SyntaxPalette::from_json("custom:large", &" ".repeat(128 * 1024 + 1)).is_err());
    let root = tempfile::tempdir().unwrap();
    let syntax = root.path().join("syntax-themes");
    fs::create_dir(&syntax).unwrap();
    for i in 0..36 {
        fs::write(syntax.join(format!("{i:02}.json")), r#"{"name":"Custom"}"#).unwrap();
    }
    fs::write(syntax.join("00.json"), "invalid JSON").unwrap();
    let (mut themes, warnings) = Themes::load(&root.path().join("themes"), "carbon");
    assert!(!warnings.is_empty());
    assert!(themes.syntax_entries.len() <= 38);
    themes.select_syntax("nord", &Default::default()).unwrap();
    assert_eq!(themes.active_syntax().id, "nord");
}
