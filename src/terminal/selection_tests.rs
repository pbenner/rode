//! Pointer, clipboard and shell-input regressions using real egui frames.

use super::*;
use egui::{Event, Modifiers, PointerButton, Pos2};

struct Harness {
    ctx: egui::Context,
    terminal: Terminal,
    stream: Stream,
    input: mpsc::Receiver<Vec<u8>>,
    origin: Pos2,
    cell: Vec2,
    size: Vec2,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        let (tx, input) = mpsc::channel();
        let (terminal, stream) = Terminal::stream(&ctx, "Selection".into(), move |event| {
            if let StreamControl::Input(bytes) = event {
                let _ = tx.send(bytes);
            }
        });
        let mut harness = Self {
            ctx,
            terminal,
            stream,
            input,
            origin: Pos2::ZERO,
            cell: Vec2::ZERO,
            size: egui::vec2(540., 240.),
        };
        harness.frame(vec![]);
        harness
    }

    fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    self.origin = ui.available_rect_before_wrap().min + egui::vec2(6., 4.);
                    self.cell = ui.fonts_mut(|fonts| {
                        let font = FontId::monospace(14.);
                        Vec2::new(fonts.glyph_width(&font, 'M'), fonts.row_height(&font))
                    });
                    Terminal::prepare_input(ctx);
                    assert!(self.terminal.ui(ui, 14., Palette::default()).is_none());
                });
            },
        )
    }

    fn position(&self, row: u16, col: u16) -> Pos2 {
        self.origin
            + egui::vec2(
                f32::from(col) * self.cell.x,
                (f32::from(row) + 0.5) * self.cell.y,
            )
    }

    fn button(position: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos: position,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn drag(&mut self, start: (u16, u16), end: (u16, u16)) -> egui::FullOutput {
        let start = self.position(start.0, start.1);
        let end = self.position(end.0, end.1);
        self.frame(vec![Event::PointerMoved(start), Self::button(start, true)]);
        self.frame(vec![Event::PointerMoved(end)]);
        self.frame(vec![Self::button(end, false)])
    }

    fn write(&self, text: &str) {
        self.terminal
            .parser
            .lock()
            .unwrap()
            .process(text.as_bytes());
    }
}

fn copied(output: &egui::FullOutput) -> Vec<&str> {
    output
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn key_event(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

#[test]
fn dragging_copies_only_selected_text_in_both_directions_and_highlights_it() {
    let mut harness = Harness::new();
    harness.write("first line\r\nsecond line");
    for (start, end) in [((0, 6), (1, 6)), ((1, 6), (0, 6))] {
        let output = harness.drag(start, end);
        assert!(output.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Rect(rect) if rect.fill == Palette::default().accent)
        }));
        assert_eq!(harness.terminal.copy_contents(), "line\nsecond");
        let output = harness.frame(vec![Event::Copy]);
        assert_eq!(copied(&output), ["line\nsecond"]);
        assert!(harness.input.try_recv().is_err(), "Copy sent shell input");
    }
}

#[test]
fn selection_handles_soft_wrapping_wide_and_combining_characters() {
    let mut harness = Harness::new();
    let cols = harness.terminal.parser.lock().unwrap().screen().size().1;
    let prefix = "x".repeat(usize::from(cols - 2));
    harness.write(&format!("{prefix}界e\u{301}tail"));
    harness.drag((0, cols - 2), (1, 5));
    assert_eq!(harness.terminal.copy_contents(), "界e\u{301}tail");
    // Dragging from inside a double-width glyph must not copy half of it.
    let parser = harness.terminal.parser.lock().unwrap();
    for offset in [0.6, 1.4] {
        let position = harness.position(0, cols - 2) + egui::vec2(offset * harness.cell.x, 0.);
        let point = selection::point(
            parser.screen(),
            harness.origin,
            harness.cell,
            position,
            parser.screen().size(),
        );
        assert_eq!(point.col, if offset < 1. { cols - 2 } else { cols });
    }
}

#[test]
fn copy_remains_available_after_exit_and_without_a_native_clipboard_event() {
    let mut harness = Harness::new();
    harness.write("retained failure diagnostic");
    harness.stream.end();
    harness.drag((0, 9), (0, 16));
    assert_eq!(copied(&harness.frame(vec![Event::Copy])), ["failure"]);
    assert_eq!(
        copied(&harness.frame(vec![key_event(Key::C, Modifiers::COMMAND)])),
        ["failure"]
    );
    harness.frame(vec![Event::Text("ignored".into())]);
    assert!(harness.input.try_recv().is_err());
    harness.frame(vec![key_event(Key::Escape, Modifiers::NONE)]);
    assert!(harness.terminal.selection.is_none());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn clearing_selection_restores_interrupt_and_typing_reaches_the_shell() {
    let mut harness = Harness::new();
    harness.write("copy me");
    harness.drag((0, 0), (0, 4));
    harness.frame(vec![key_event(Key::Escape, Modifiers::NONE)]);
    harness.frame(vec![Event::Copy]);
    assert_eq!(
        harness.input.recv_timeout(Duration::from_secs(2)).unwrap(),
        [3]
    );
    harness.drag((0, 0), (0, 4));
    harness.frame(vec![Event::Text("echo".into())]);
    assert_eq!(
        harness.input.recv_timeout(Duration::from_secs(2)).unwrap(),
        b"echo"
    );
    assert!(harness.terminal.selection.is_none());
}

#[test]
fn clicking_and_changed_output_clear_stale_ranges() {
    let mut harness = Harness::new();
    harness.write("old output");
    harness.drag((0, 0), (0, 3));
    let position = harness.position(0, 5);
    harness.frame(vec![
        Event::PointerMoved(position),
        Harness::button(position, true),
    ]);
    harness.frame(vec![Harness::button(position, false)]);
    assert!(harness.terminal.selection.is_none());
    harness.frame(vec![
        Harness::button(position, true),
        Harness::button(position, false),
    ]);
    assert!(harness.terminal.selection.is_none());
    harness.drag((0, 0), (0, 3));
    harness.write("\rnew output");
    // Header copy can occur before the terminal's next render.
    assert_eq!(harness.terminal.copy_contents(), "new output");
    harness.frame(vec![]);
    assert!(harness.terminal.selection.is_none());
}

#[test]
fn selection_copies_scrollback_and_invalidates_after_viewport_changes() {
    let mut harness = Harness::new();
    for index in 0..40 {
        harness.write(&format!("line {index:02}\r\n"));
    }
    harness
        .terminal
        .parser
        .lock()
        .unwrap()
        .screen_mut()
        .set_scrollback(5);
    let expected = harness
        .terminal
        .parser
        .lock()
        .unwrap()
        .screen()
        .contents_between(0, 0, 0, 7);
    harness.drag((0, 0), (0, 7));
    assert_eq!(copied(&harness.frame(vec![Event::Copy])), [expected]);
    harness
        .terminal
        .parser
        .lock()
        .unwrap()
        .screen_mut()
        .set_scrollback(0);
    harness.frame(vec![]);
    assert!(harness.terminal.selection.is_none());
    harness.drag((0, 0), (0, 7));
    harness
        .terminal
        .parser
        .lock()
        .unwrap()
        .screen_mut()
        .set_size(10, 30);
    harness.frame(vec![]);
    assert!(harness.terminal.selection.is_none());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn typing_then_interrupt_in_one_frame_does_not_copy_an_old_selection() {
    let mut harness = Harness::new();
    harness.write("selected output");
    harness.drag((0, 0), (0, 8));
    let output = harness.frame(vec![Event::Text("a".into()), Event::Copy]);
    assert_eq!(
        harness.input.recv_timeout(Duration::from_secs(2)).unwrap(),
        b"a\x03"
    );
    assert!(copied(&output).is_empty());
}

#[test]
fn native_view_resize_clears_selection_before_shared_size_acknowledgement() {
    let mut harness = Harness::new();
    harness.write("selected output");
    harness.drag((0, 0), (0, 8));
    harness.terminal.native = true;
    let parser_size = harness.terminal.parser.lock().unwrap().screen().size();
    harness.size = egui::vec2(320., 200.);
    harness.frame(vec![]);
    assert_eq!(
        harness.terminal.parser.lock().unwrap().screen().size(),
        parser_size
    );
    assert!(harness.terminal.selection.is_none());
}

#[test]
fn release_on_first_drag_frame_keeps_the_range() {
    let mut harness = Harness::new();
    harness.write("selected output");
    let start = harness.position(0, 0);
    let end = harness.position(0, 8);
    harness.frame(vec![
        Event::PointerMoved(start),
        Harness::button(start, true),
    ]);
    harness.frame(vec![Event::PointerMoved(end), Harness::button(end, false)]);
    assert_eq!(harness.terminal.copy_contents(), "selected");
}

#[test]
fn context_menu_input_does_not_reach_the_shell() {
    let mut harness = Harness::new();
    harness.write("selected output");
    harness.drag((0, 0), (0, 8));
    let position = harness.position(1, 5);
    for pressed in [true, false] {
        harness.frame(vec![
            Event::PointerMoved(position),
            Event::PointerButton {
                pos: position,
                button: PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            },
        ]);
    }
    assert!(egui::Popup::is_any_open(&harness.ctx));
    harness.frame(vec![
        Event::Text("x".into()),
        key_event(Key::Enter, Modifiers::NONE),
    ]);
    assert!(
        harness
            .input
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
}
