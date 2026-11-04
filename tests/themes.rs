//! Palette/syntax parsing, cached layouts, independent preferences and concurrent theme edits.

use eframe::egui::{self, Color32};
use rode::theme::{Chrome, Palette, Theme, Themes, parse_color};
use std::fs;

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
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

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
fn decorated_chrome_inherits_can_be_disabled_and_resets_when_switching() {
    for (base, chrome) in [
        ("afterglow", Chrome::Chamfered),
        ("hologram", Chrome::Holographic),
        ("telemetry", Chrome::Bracketed),
    ] {
        let definition = serde_json::json!({
            "name": "Trim",
            "base": base,
            "colors": {"frame_accent": "#e0aaee", "heading": "#ffddaa"}
        });
        let inherited = Theme::from_json("custom:trim", &definition.to_string()).unwrap();
        assert_eq!(inherited.palette.chrome, chrome);
        assert_eq!(
            inherited.palette.frame_accent,
            parse_color("#e0aaee").unwrap()
        );
        assert_eq!(
            inherited.palette.heading_color(),
            parse_color("#ffddaa").unwrap()
        );
        let plain = Theme::from_json(
            "custom:plain",
            &format!(r#"{{"name":"Plain","base":"{base}","chrome":"plain"}}"#),
        )
        .unwrap();
        assert_eq!(plain.palette.chrome, Chrome::Plain);
        assert_eq!(plain.palette.heading_color(), plain.palette.text);
        assert!(
            Theme::from_json(
                "custom:invalid",
                r#"{"name":"Invalid","chrome":"animated"}"#
            )
            .is_err()
        );

        let directory = tempfile::tempdir().unwrap();
        let (mut themes, _) = Themes::load(directory.path(), "phosphor");
        let ctx = egui::Context::default();
        themes.select("phosphor", &ctx).unwrap();
        let original = ctx.style().visuals.clone();
        themes.select_syntax("nord", &ctx).unwrap();
        themes.select(base, &ctx).unwrap();
        assert_eq!(themes.active_syntax().id, "nord");
        assert_ne!(ctx.style().visuals, original);
        assert_eq!(ctx.style().visuals.widgets.hovered.expansion, 0.);
        themes.select("phosphor", &ctx).unwrap();
        assert_eq!(
            ctx.style().visuals,
            original,
            "Decorative style leaked into a plain theme"
        );
        assert_eq!(themes.active_syntax().id, "nord");
    }
}

#[test]
fn studio_neutral_inherits_round_component_styling_without_lilac_accents() {
    let studio = Theme::builtin("studio").unwrap();
    let neutral = Theme::builtin("studio-neutral").unwrap();
    assert_eq!(neutral.palette.ui, studio.palette.ui);
    assert_eq!(neutral.palette.chrome, studio.palette.chrome);
    for color in [
        neutral.palette.background,
        neutral.palette.panel,
        neutral.palette.raised,
        neutral.palette.accent,
        neutral.palette.active,
        neutral.palette.selection,
    ] {
        let channels = [color.r(), color.g(), color.b()];
        assert!(
            channels.iter().max().unwrap() - channels.iter().min().unwrap() <= 22,
            "Neutral surfaces and accent should have low saturation"
        );
    }
    let inherited = Theme::from_json(
        "custom:neutral",
        r#"{"name":"Neutral copy","base":"studio-neutral"}"#,
    )
    .unwrap();
    assert_eq!(inherited.palette, neutral.palette);
}

#[test]
fn shared_component_defaults_work_with_different_colors_and_frame_treatments() {
    use rode::theme::{InterfaceStyle, card_frame, dialog_frame};

    let defaults = InterfaceStyle::default();
    for base in ["carbon", "amber", "hologram", "telemetry"] {
        let original = Theme::builtin(base).unwrap();
        let custom = Theme::from_json(
            "custom:rounded",
            &format!(r#"{{"name":"Rounded variant","base":"{base}","ui":{{}}}}"#,),
        )
        .unwrap();
        assert_eq!(custom.palette.ui, Some(defaults));
        let mut expected = original.palette;
        expected.ui = Some(defaults);
        assert_eq!(
            custom.palette, expected,
            "Component styling changed the base colors or decoration"
        );
        let ctx = egui::Context::default();
        rode::theme::install(&ctx, custom.palette);
        let card = card_frame(custom.palette);
        let dialog = dialog_frame(&ctx.style(), custom.palette);
        assert_eq!(card.fill, original.palette.raised);
        assert_eq!(
            card.corner_radius,
            egui::CornerRadius::same(defaults.surface_radius)
        );
        assert_eq!(dialog.fill, original.palette.panel);
        assert_eq!(
            dialog.corner_radius,
            egui::CornerRadius::same(defaults.popup_radius)
        );
        assert_eq!(
            dialog.inner_margin,
            egui::Margin::same(defaults.popup_padding as i8)
        );
        assert_eq!(
            ctx.style().visuals.widgets.inactive.corner_radius,
            egui::CornerRadius::same(defaults.control_radius)
        );
    }
}

#[test]
fn direct_theme_json_parsing_enforces_the_same_size_limit_as_file_loading() {
    let mut json = String::from(r#"{"name":"Bounded variant","base":"studio-neutral"}"#);
    json.extend(std::iter::repeat_n(' ', 64 * 1024 - json.len()));
    assert!(Theme::from_json("custom:bounded", &json).is_ok());
    json.push(' ');
    let error = Theme::from_json("custom:bounded", &json)
        .err()
        .expect("Oversized JSON must fail");
    assert!(error.to_string().contains("64 KiB"));
}

#[test]
fn component_styles_inherit_and_restore_legacy_layout_and_typography() {
    let studio = Theme::builtin("studio").unwrap();
    let original = studio.palette.ui.unwrap();
    let derived = Theme::from_json(
        "custom:studio",
        r#"{
        "name":"Studio Compact", "base":"studio",
        "ui":{"control_radius":4,"item_spacing":[6,5]}
    }"#,
    )
    .unwrap();
    let inherited = derived.palette.ui.unwrap();
    assert_eq!(inherited.control_radius, 4);
    assert_eq!(inherited.item_spacing, [6., 5.]);
    assert_eq!(inherited.popup_radius, original.popup_radius);
    assert_eq!(inherited.body_size, original.body_size);
    assert_eq!(inherited.control_height, original.control_height);
    assert_eq!(
        derived.palette.control_border,
        studio.palette.control_border
    );
    let directory = tempfile::tempdir().unwrap();
    let (mut themes, _) = Themes::load(directory.path(), "phosphor");
    let ctx = egui::Context::default();
    themes.select("phosphor", &ctx).unwrap();
    let before = ctx.style();
    themes.select_syntax("nord", &ctx).unwrap();
    themes.select("studio", &ctx).unwrap();
    let styled = ctx.style();
    assert_ne!(styled.spacing, before.spacing);
    assert_ne!(styled.text_styles, before.text_styles);
    assert!(
        !styled.spacing.scroll.floating,
        "Scrollbars must reserve their own gutter"
    );
    assert!(
        styled.visuals.widgets.active.bg_stroke.width
            > styled.visuals.widgets.hovered.bg_stroke.width
    );
    assert_ne!(
        styled.visuals.widgets.active.bg_fill,
        styled.visuals.widgets.hovered.bg_fill
    );
    assert!(styled.visuals.disabled_alpha < 1.);
    assert_eq!(themes.active_syntax().id, "nord");
    themes.select("phosphor", &ctx).unwrap();
    assert_eq!(
        *ctx.style(),
        *before,
        "Component styles leaked into a legacy theme"
    );
}

#[test]
fn component_controls_have_consistent_height_at_multiple_scales() {
    let studio = Theme::builtin("studio").unwrap();
    let ctx = egui::Context::default();
    rode::theme::install(&ctx, studio.palette);
    for scale in [0.75, 1., 1.25, 1.5] {
        ctx.set_zoom_factor(scale);
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let normal = ui.button("Run");
                let primary = studio.palette.primary(ui, "Commit staged");
                let disabled = ui.add_enabled(false, egui::Button::new("Unavailable"));
                assert!(!disabled.enabled());
                assert!((normal.rect.height() - primary.rect.height()).abs() < 0.1);
                assert!((normal.rect.height() - disabled.rect.height()).abs() < 0.1);
                assert!(normal.rect.height() >= studio.palette.ui.unwrap().control_height);
            });
        });
    }
}

#[test]
fn studio_controls_and_secondary_labels_stay_readable_in_every_interaction_state() {
    for theme in ["studio", "studio-neutral"] {
        let palette = Theme::builtin(theme).unwrap().palette;
        let ctx = egui::Context::default();
        rode::theme::install(&ctx, palette);
        let style = ctx.style();
        assert_eq!(
            style.visuals.widgets.noninteractive.bg_stroke.color, palette.border,
            "Decorative separators must keep their quiet surface border"
        );
        for (name, widget) in [
            ("normal", style.visuals.widgets.inactive),
            ("hovered", style.visuals.widgets.hovered),
            ("focused/pressed", style.visuals.widgets.active),
            ("open", style.visuals.widgets.open),
        ] {
            assert!(contrast(palette.text, widget.bg_fill) >= 7., "{name} text");
            assert!(
                contrast(palette.muted, widget.bg_fill) >= 4.5,
                "{name} secondary text"
            );
            for background in [widget.bg_fill, palette.panel, palette.raised] {
                assert!(
                    contrast(widget.bg_stroke.color, background) >= 3.,
                    "{name} control boundary"
                );
            }
        }
        assert!(
            contrast(palette.background, palette.accent) >= 7.,
            "Primary action text"
        );
    }
}

#[test]
fn studio_text_fields_and_filled_primary_actions_draw_visible_keyboard_focus() {
    for theme in ["studio", "studio-neutral"] {
        let palette = Theme::builtin(theme).unwrap().palette;
        let ctx = egui::Context::default();
        rode::theme::install(&ctx, palette);
        let mut text = String::from("Editable value");
        let primary_bounds = std::cell::Cell::new(egui::Rect::NOTHING);
        let field_bounds = std::cell::Cell::new(egui::Rect::NOTHING);
        let mut frame = |target: usize| {
            ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let primary = palette.primary(ui, "Run");
                    let field = ui.text_edit_singleline(&mut text);
                    primary_bounds.set(primary.rect);
                    field_bounds.set(field.rect);
                    if target == 0 {
                        primary.request_focus();
                    } else {
                        field.request_focus();
                    }
                });
            })
        };
        for (target, bounds, color) in [
            (0, &primary_bounds, palette.background),
            (1, &field_bounds, palette.accent),
        ] {
            frame(target);
            let output = frame(target);
            assert!(
                output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Rect(rect) if rect.rect == bounds.get()
                        && rect.stroke.color == color
                        && rect.stroke.width >= palette.ui.unwrap().focus_width
                )),
                "Missing visible keyboard focus for control {target}"
            );
        }
    }
}

#[test]
fn malformed_component_settings_are_rejected() {
    for ui in [
        serde_json::json!({"control_height": 0}),
        serde_json::json!({"control_radius": 255}),
        serde_json::json!({"button_padding": [5]}),
        serde_json::json!({"item_spacing": [-1, 8]}),
        serde_json::json!({"body_size": 80}),
        serde_json::json!({"disabled_alpha": 0}),
        serde_json::json!({"border_width": 2, "focus_width": 1}),
        serde_json::json!({"unknown_property": true}),
    ] {
        let definition = serde_json::json!({"name":"Invalid", "base":"studio", "ui":ui});
        assert!(Theme::from_json("custom:invalid", &definition.to_string()).is_err());
    }
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
    let directory = tempfile::tempdir().unwrap();
    let source = "// comment\npub fn run(value: String) { let n = 42; println!(\"hello\"); }";
    let mut identities = std::collections::HashSet::new();
    let mut signatures = std::collections::HashSet::new();
    for builtin in Theme::builtins() {
        let ctx = egui::Context::default();
        assert!(identities.insert(builtin.id.clone()), "Duplicate theme ID");
        let p = builtin.palette;
        assert!(
            signatures.insert((p.background, p.accent, std::mem::discriminant(&p.chrome))),
            "Duplicate palette and frame style"
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
        assert_eq!(
            p.terminal_foreground, p.text,
            "{} default terminal text",
            builtin.id
        );
        assert_eq!(p.ansi[7], p.text, "{} ANSI white", builtin.id);
        for index in 8..16 {
            assert!(
                contrast(p.ansi[index], p.terminal_background) >= 4.5,
                "{} bright ANSI {index} must remain readable in PS1",
                builtin.id
            );
        }
        assert!(
            p.ansi[9].r() > p.ansi[9].g(),
            "{} error stays red",
            builtin.id
        );
        assert!(
            p.ansi[10].g() > p.ansi[10].r(),
            "{} success stays green",
            builtin.id
        );
        let path = p.ansi[12];
        match builtin.id.as_str() {
            "carbon" | "carbon-edge" | "amber" => assert!(
                path.r() > path.g() && path.g() > path.b(),
                "{} warm path",
                builtin.id
            ),
            "phosphor" | "matrix" => assert!(
                path.g() > path.r() && path.g() > path.b(),
                "{} green path",
                builtin.id
            ),
            "ice" | "code-dark" | "afterglow" | "knight-rider" | "hologram" | "studio"
            | "studio-neutral" => {
                assert!(path.b() > path.r(), "{} cool path", builtin.id)
            }
            "neon" => assert!(path.r() > path.g() && path.b() > path.g(), "violet path"),
            "telemetry" => assert!(path.g() > path.r() && path.b() > path.r(), "turquoise path"),
            _ => unreachable!(),
        }
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
            &[
                "carbon-edge",
                "code-dark",
                "matrix",
                "amber",
                "ice",
                "neon",
                "afterglow",
                "knight-rider",
                "hologram",
                "telemetry",
                "studio",
                "studio-neutral"
            ]
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
    for ui in [
        "phosphor",
        "carbon-edge",
        "code-dark",
        "matrix",
        "amber",
        "ice",
        "neon",
        "knight-rider",
        "hologram",
        "telemetry",
        "studio",
        "studio-neutral",
    ] {
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
