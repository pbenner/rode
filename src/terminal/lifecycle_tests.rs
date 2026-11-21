//! Terminal transport, rendering and ownership regressions.
use super::*;
use std::sync::atomic::AtomicUsize;

#[test]
fn process_backends_keep_input_and_resize_on_their_selected_wire_protocol() {
    for protocol in [Protocol::Raw, Protocol::framed(true)] {
        let native = protocol.is_native();
        let (mut terminal, _stream, input, signals) =
            fixtures::process(&egui::Context::default(), "Transport".into(), protocol);
        assert_eq!(terminal.is_native(), native);
        assert_eq!(terminal.persistent(), native);
        terminal.send(b"command\r".to_vec()).unwrap();
        let bytes = input.recv_timeout(Duration::from_secs(1)).unwrap();
        if native {
            let (kind, payload) = rode_session::read_stream(&mut bytes.as_slice()).unwrap();
            assert_eq!(kind, b'I');
            assert_eq!(payload, b"command\r");
        } else {
            assert_eq!(bytes, b"command\r");
        }
        terminal.resize(12, 50).unwrap();
        if native {
            let bytes = input.recv_timeout(Duration::from_secs(1)).unwrap();
            let (kind, payload) = rode_session::read_stream(&mut bytes.as_slice()).unwrap();
            assert_eq!(kind, b'R');
            assert_eq!(payload, [0, 12, 0, 50]);
            assert_eq!(terminal.parser.lock().unwrap().screen().size(), (24, 80));
        } else {
            assert_eq!(terminal.parser.lock().unwrap().screen().size(), (12, 50));
            assert!(input.try_recv().is_err());
        }
        drop(terminal);
        assert_eq!(signals.load(Ordering::Acquire), 1);
    }
}

#[test]
fn output_views_never_end_their_debugger_owner() {
    let ended = Arc::new(AtomicUsize::new(0));
    let count = ended.clone();
    let (terminal, _stream) =
        Terminal::stream(&egui::Context::default(), "Debug".into(), move |event| {
            if matches!(event, StreamControl::End) {
                count.fetch_add(1, Ordering::AcqRel);
            }
        });
    let output = terminal.output_view("Retained output".into());
    assert!(!output.is_native());
    assert!(!output.persistent());
    assert!(output.send(b"do not send".to_vec()).is_err());
    drop(output);
    assert_eq!(ended.load(Ordering::Acquire), 0);
    drop(terminal);
    assert_eq!(ended.load(Ordering::Acquire), 1);
}

#[cfg(not(target_os = "macos"))]
#[test]
fn native_clipboard_events_preserve_terminal_exit_interrupt_and_copy() {
    let ctx = egui::Context::default();
    let (tx, rx) = mpsc::channel();
    let (mut terminal, _stream) = Terminal::stream(&ctx, "Keys".into(), move |event| {
        if let StreamControl::Input(bytes) = event {
            tx.send(bytes).unwrap();
        }
    });
    terminal.parser.lock().unwrap().process(b"visible output");
    let mut frame = |events| {
        ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    Terminal::focus(ctx);
                    assert!(terminal.ui(ui, 14., Palette::default()).is_none());
                });
            },
        )
    };
    frame(vec![]);
    // This is what egui-winit emits when Control and the letter are both
    // pressed and released between redraws: clipboard events, not key presses.
    frame(vec![egui::Event::Cut, egui::Event::Copy]);
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        [0x18, 0x03]
    );
    let output = frame(vec![
        egui::Event::Copy,
        egui::Event::Key {
            key: Key::C,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        },
        egui::Event::Key {
            key: Key::D,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        },
    ]);
    assert!(output.platform_output.commands.iter().any(
        |command| matches!(command, egui::OutputCommand::CopyText(text) if text == "visible output")
    ));
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), [0x04]);
}

fn glyph_color(shapes: &[egui::epaint::ClippedShape], text: &str) -> Color32 {
    shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == text => {
                Some(t.galley.job.sections[0].format.color)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing rendered glyph {text}"))
}

#[test]
fn ncurses_borders_render_supported_glyphs_and_copy_unicode_contents() {
    let ctx = egui::Context::default();
    let (mut terminal, stream) = Terminal::stream(&ctx, "Borders".into(), |_| {});
    stream.end();
    terminal
        .parser
        .lock()
        .unwrap()
        .process(b"\x1b(0lqqk\r\nx  x\r\nmqqj\x1b(B\r\nplain-q");
    assert_eq!(terminal.contents(), "┌──┐\n│  │\n└──┘\nplain-q");
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(500., 300.),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let font = FontId::monospace(14.);
                ui.fonts_mut(|fonts| {
                    assert!(fonts.has_glyphs(&font, "┌┐└┘─│┬┴├┤┼▒▁▂▃▄▅▆▇█"));
                });
                assert!(terminal.ui(ui, 14., Palette::default()).is_none());
            });
        },
    );
    for glyph in ["┌", "─", "┐", "│", "└", "┘"] {
        glyph_color(&output.shapes, glyph);
    }
}

#[test]
fn bash_prompt_bold_colors_follow_theme_with_normal_resets_and_backgrounds() {
    let ctx = egui::Context::default();
    let (mut terminal, stream) = Terminal::stream(&ctx, "Bash prompt".into(), |_| {});
    stream.end();
    terminal.parser.lock().unwrap().process(
            b"\x1b[1;30mH\x1b[1;34mP\x1b[0;35mG\x1b[1;30m>\x1b[0mN\x1b[1;38;2;9;10;11mR\x1b[1;38;5;17mX\x1b[1;90mB\x1b[22;30mD\x1b[1;30;41mI\x1b[22mJ\x1b[0;34mK"
        );
    let original = terminal.contents();
    for theme in crate::theme::Theme::builtins() {
        // This checks raw ANSI mapping; contrast rendering is covered separately.
        let palette = Palette {
            terminal_minimum_contrast: 1.,
            ..theme.palette
        };
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 240.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    terminal.ui(ui, 14., palette);
                });
            },
        );
        for (glyph, expected) in [
            ("H", palette.ansi[8]),
            ("P", palette.ansi[12]),
            ("G", palette.ansi[5]),
            (">", palette.ansi[8]),
            ("N", palette.terminal_foreground),
            ("R", Color32::from_rgb(9, 10, 11)),
            ("X", Color32::from_rgb(0, 0, 95)),
            ("B", palette.ansi[8]),
            ("D", palette.ansi[0]),
            ("I", palette.ansi[8]),
            ("J", palette.ansi[0]),
            ("K", palette.ansi[4]),
        ] {
            assert_eq!(
                glyph_color(&output.shapes, glyph),
                expected,
                "{}: {glyph}",
                theme.id
            );
        }
        assert!(
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == palette.ansi[1]))
        );
        assert!(
            !output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == palette.ansi[9]))
        );
        assert_eq!(terminal.contents(), original);
        let parser = terminal.parser.lock().unwrap();
        let host = parser.screen().cell(0, 0).unwrap();
        assert!(host.bold());
        assert_eq!(host.fgcolor(), vt100::Color::Idx(0));
        assert!(!parser.screen().cell(0, 8).unwrap().bold());
    }
}

#[test]
fn rendering_protects_ansi_extended_rgb_and_inverse_without_changing_output() {
    let ctx = egui::Context::default();
    let (mut terminal, stream) = Terminal::stream(&ctx, "Colors".into(), |_| {});
    stream.end();
    terminal.parser.lock().unwrap().process(
        "\x1b[30mA\x1b[38;5;17mB\x1b[38;2;9;10;11mC\x1b[7mD\x1b[0m\x1b[97mE\x1b[38;2;9;10;11m█"
            .as_bytes(),
    );
    let original = terminal.contents();
    for minimum in [1., 4.5, 1.] {
        let palette = Palette {
            terminal_minimum_contrast: minimum,
            ..Default::default()
        };
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 240.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    terminal.ui(ui, 14., palette);
                });
            },
        );
        for (letter, raw) in [
            ("A", palette.ansi[0]),
            ("B", Color32::from_rgb(0, 0, 95)),
            ("C", Color32::from_rgb(9, 10, 11)),
            ("D", palette.terminal_background),
        ] {
            let actual = glyph_color(&output.shapes, letter);
            if minimum == 1. {
                assert_eq!(actual, raw);
            } else {
                assert_ne!(actual, raw);
            }
        }
        assert_eq!(glyph_color(&output.shapes, "E"), palette.ansi[15]);
        assert_eq!(
            glyph_color(&output.shapes, "█"),
            Color32::from_rgb(9, 10, 11)
        );
        assert_eq!(terminal.contents(), original);
        let parser = terminal.parser.lock().unwrap();
        assert_eq!(
            parser.screen().cell(0, 2).unwrap().fgcolor(),
            vt100::Color::Rgb(9, 10, 11)
        );
        assert!(parser.screen().cell(0, 3).unwrap().inverse());
    }
}

#[test]
fn dropping_a_finished_view_does_not_signal_its_reaped_child() {
    for finished in [false, true] {
        let (terminal, stream, _input, signals) =
            fixtures::process(&egui::Context::default(), "Fixture".into(), Protocol::Raw);
        if finished {
            stream.end();
        }
        drop(terminal);
        assert_eq!(signals.load(Ordering::Acquire), usize::from(!finished));
    }
}

#[test]
fn output_view_keeps_late_error_bytes_without_input_or_process_ownership() {
    let (terminal, stream, _input, signals) = fixtures::process(
        &egui::Context::default(),
        "SSH fixture".into(),
        Protocol::Raw,
    );
    let output = terminal.output_view("SSH ERROR / fixture".into());
    drop(terminal);
    assert_eq!(signals.load(Ordering::Acquire), 1);
    assert!(!output.is_alive());
    assert!(!output.persistent());
    assert!(output.send(b"must not reach SSH\n".to_vec()).is_err());
    // Child exit can arrive before the PTY reader's final stderr bytes.
    stream
        .feed(b"\x1b[31mPermission denied (publickey).\x1b[0m\r\n".to_vec())
        .unwrap();
    let until = std::time::Instant::now() + Duration::from_secs(3);
    while !output.contents().contains("Permission denied (publickey).") {
        assert!(
            std::time::Instant::now() < until,
            "Late diagnostic output was lost"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!output.contents().contains('\x1b'));
    drop(output);
    assert_eq!(
        signals.load(Ordering::Acquire),
        1,
        "The output view must never signal the old process"
    );
}
