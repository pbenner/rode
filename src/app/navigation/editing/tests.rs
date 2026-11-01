//! Regressions for native source edit/undo behavior under Emacs and Vim input translation.

use super::*;
#[test]
fn motions_use_character_indices_and_word_classes() {
    let text = Text::new("héllo 世界\nsecond line\n");
    assert_eq!(text.line(3), (0, 8));
    assert_eq!(text.word(0, 1, false, false), 6);
    assert_eq!(text.word(8, 1, true, false), 6);
    assert_eq!(text.vertical(3, 1), 12);
}

struct Editor {
    ctx: egui::Context,
    editing: State,
    text: String,
    style: Style,
}
impl Editor {
    fn new(style: Style, text: &str) -> Self {
        let mut editor = Self {
            ctx: Default::default(),
            editing: Default::default(),
            text: text.into(),
            style,
        };
        editor.frame(vec![]);
        editor.frame(vec![]);
        editor
    }
    fn frame(&mut self, events: Vec<egui::Event>) -> Vec<NavAction> {
        let mut effects = vec![];
        let ctx = self.ctx.clone();
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                self.editing.replay(ctx, Some(1), self.style, true);
                effects = self.editing.route(ctx, 1, &self.text, self.style);
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut output = egui::TextEdit::multiline(&mut self.text)
                        .id(egui::Id::new(("document", 1u64)))
                        .code_editor()
                        .cursor_at_end(false)
                        .show(ui);
                    output.response.request_focus();
                    self.editing.after_source(
                        ui,
                        1,
                        &mut output,
                        &self.text,
                        crate::theme::Themes::load(
                            std::path::Path::new("/nonexistent-rode-style-test-themes"),
                            "carbon",
                        )
                        .0
                        .active()
                        .palette,
                    );
                });
            },
        );
        effects
    }
    fn type_text(&mut self, text: &str) {
        self.frame(vec![egui::Event::Text(text.into())]);
        self.drain();
    }
    fn press(&mut self, key_code: Key, modifiers: egui::Modifiers) {
        self.frame(vec![key(key_code, modifiers)]);
        self.drain();
    }
    fn drain(&mut self) {
        for _ in 0..32 {
            if self.editing.deferred.is_empty() {
                return;
            }
            self.frame(vec![]);
        }
        panic!("Input did not drain");
    }
    fn point(&self) -> usize {
        egui::TextEdit::load_state(&self.ctx, egui::Id::new(("document", 1u64)))
            .unwrap()
            .cursor
            .char_range()
            .unwrap()
            .primary
            .index
    }
}
#[test]
fn vim_insert_operators_counts_visual_and_native_undo_share_the_buffer_history() {
    let mut e = Editor::new(Style::Vim, "one two\nthree four\n");
    e.type_text("2dw");
    assert_eq!(e.text, "three four\n");
    e.type_text("u");
    assert_eq!(e.text, "one two\nthree four\n");
    e.type_text("ggIhello ");
    e.press(Key::Escape, egui::Modifiers::NONE);
    assert!(e.text.starts_with("hello one two\n"));
    assert_eq!(e.editing.mode, Mode::Normal);
    e.type_text("ggvldy"); // v+l selects two chars, d deletes them; y starts a pending yank.
    assert!(e.text.starts_with("llo one"));
    e.press(Key::Escape, egui::Modifiers::NONE);
    e.type_text("u");
    assert!(e.text.starts_with("hello one"));
    e.type_text("ggyyGp");
    assert!(e.text.ends_with("hello one two\n"));
}
#[test]
fn emacs_mark_kill_yank_and_motions_preserve_unicode() {
    let mut e = Editor::new(Style::Emacs, "héllo 世界\nsecond\n");
    e.press(Key::E, egui::Modifiers::CTRL);
    assert_eq!(e.point(), 8);
    e.press(Key::A, egui::Modifiers::CTRL);
    assert_eq!(e.point(), 0);
    e.press(Key::Space, egui::Modifiers::CTRL);
    e.press(Key::F, egui::Modifiers::CTRL);
    e.press(Key::F, egui::Modifiers::CTRL);
    e.press(Key::W, egui::Modifiers::CTRL);
    assert_eq!(e.text, "llo 世界\nsecond\n");
    e.press(Key::Y, egui::Modifiers::CTRL);
    assert_eq!(e.text, "héllo 世界\nsecond\n");
    e.press(Key::A, egui::Modifiers::CTRL);
    e.press(Key::K, egui::Modifiers::CTRL);
    assert_eq!(e.text, "\nsecond\n");
    e.press(Key::Y, egui::Modifiers::CTRL);
    assert_eq!(e.text, "héllo 世界\nsecond\n");
}
#[test]
fn mixed_insert_escape_and_normal_commands_in_one_frame_keep_event_order() {
    let mut e = Editor::new(Style::Vim, "abc\n");
    e.frame(vec![
        egui::Event::Text("iXYZ".into()),
        key(Key::Escape, egui::Modifiers::NONE),
        egui::Event::Text("0x".into()),
    ]);
    e.drain();
    assert_eq!(e.text, "YZabc\n");
    assert_eq!(e.editing.mode, Mode::Normal);
}
#[test]
fn deferred_input_is_discarded_when_focus_or_document_changes() {
    let mut e = Editor::new(Style::Vim, "abc");
    e.frame(vec![
        egui::Event::Text("ix".into()),
        key(Key::Escape, egui::Modifiers::NONE),
    ]);
    assert!(!e.editing.deferred.is_empty());
    e.editing.replay(&e.ctx, Some(2), Style::Vim, true);
    assert!(e.editing.deferred.is_empty());
}

#[test]
fn unsupported_text_objects_do_not_accidentally_enter_insert_mode() {
    let mut e = Editor::new(Style::Vim, "one two\n");
    e.type_text("diw");
    assert_eq!(e.text, "one two\n");
    assert_eq!(e.editing.mode, Mode::Normal);
}
#[test]
fn command_palette_and_modified_editor_shortcuts_remain_available_in_vim() {
    let ctx = egui::Context::default();
    let mut editing = State::default();
    let event = key(Key::P, egui::Modifiers::CTRL | egui::Modifiers::SHIFT);
    let _ = ctx.run(
        egui::RawInput {
            events: vec![event],
            ..Default::default()
        },
        |ctx| {
            assert!(editing.route(ctx, 1, "source", Style::Vim).is_empty());
            assert!(ctx.input(|i| i.events.iter().any(|event| matches!(
                event,
                egui::Event::Key {
                    key: Key::P,
                    pressed: true,
                    ..
                }
            ))));
        },
    );
}
#[test]
fn native_undo_survives_switching_editing_styles() {
    let mut e = Editor::new(Style::Vim, "hello\n");
    e.type_text("x");
    assert_eq!(e.text, "ello\n");
    e.style = Style::Emacs;
    e.frame(vec![]);
    e.press(Key::Slash, egui::Modifiers::CTRL);
    assert_eq!(e.text, "hello\n");
}

#[test]
fn batched_native_insert_and_undo_do_not_drop_the_undo_key() {
    let mut e = Editor::new(Style::Vim, "abc\n");
    e.type_text("i");
    e.frame(vec![
        egui::Event::Text("xyz".into()),
        key(Key::Z, egui::Modifiers::COMMAND),
    ]);
    e.drain();
    assert_eq!(e.text, "abc\n");
}

#[test]
fn notebook_json_source_uses_source_keys_while_structured_cells_keep_native_input() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_notebook();
    assert!(app.styled_source().is_none());
    app.toggle_notebook_source(&ctx);
    assert!(app.styled_source().is_some());
    app.toggle_notebook_source(&ctx);
    let until = Instant::now() + Duration::from_secs(2);
    while app.styled_source().is_some() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(1));
        app.poll(&ctx);
    }
    assert!(app.styled_source().is_none());
}
