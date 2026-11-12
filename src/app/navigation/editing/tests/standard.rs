//! Exercise standard actions through native TextEdit, including undo, event batches
//! and selection direction, rather than testing string transformations in isolation.
use super::*;
const CMD: egui::Modifiers = egui::Modifiers::COMMAND;
const SHIFT_CMD: egui::Modifiers = egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT);

#[test]
fn indent_keeps_unicode_and_reversed_selection_excluding_the_next_line() {
    for (anchor, point) in [(1, 6), (6, 1)] {
        let mut e = Editor::new(Style::Standard, "αβ\n世界\nnext\n");
        e.select(anchor, point);
        e.press(Key::CloseBracket, CMD);
        assert_eq!(e.text, "    αβ\n    世界\nnext\n");
        assert_eq!(
            e.selection().secondary.index,
            if anchor == 1 { 5 } else { 14 }
        );
        assert_eq!(e.point(), if point == 1 { 5 } else { 14 });
        e.press(Key::Z, CMD);
        assert_eq!(e.text, "αβ\n世界\nnext\n");
        assert_eq!(e.selection(), Text::range(anchor, point));
        e.press(Key::Y, CMD);
        assert_eq!(e.text, "    αβ\n    世界\nnext\n");
        e.press(Key::OpenBracket, CMD);
        assert_eq!(e.text, "αβ\n世界\nnext\n");
        assert_eq!(e.selection(), Text::range(anchor, point));
    }
}

#[test]
fn tab_indents_a_block_without_replacing_it_and_outdent_is_safe_at_column_zero() {
    let mut e = Editor::new(Style::Standard, "one\n\ntwo\n");
    e.select(0, 9);
    e.press(Key::Tab, egui::Modifiers::NONE);
    assert_eq!(e.text, "    one\n    \n    two\n");
    e.press(Key::Tab, egui::Modifiers::SHIFT);
    assert_eq!(e.text, "one\n\ntwo\n");
    e.select(4, 4);
    e.press(Key::OpenBracket, CMD);
    assert_eq!(
        e.text, "one\n\ntwo\n",
        "No-op outdent must not backspace a newline"
    );
    e.press(Key::Z, CMD);
    assert_eq!(
        e.text, "    one\n    \n    two\n",
        "No-op must not create an undo step"
    );
}

#[test]
fn indentation_follows_tabs_spaces_and_native_tab_stops() {
    for (source, expected) in [("  a\n  b", "    a\n    b"), ("\ta\n\tb", "\t\ta\n\t\tb")] {
        let mut e = Editor::new(Style::Standard, source);
        e.select(0, source.chars().count());
        e.press(Key::CloseBracket, CMD);
        assert_eq!(e.text, expected);
        e.press(Key::OpenBracket, CMD);
        assert_eq!(e.text, source);
    }
    let mut e = Editor::new(Style::Standard, "ab");
    e.select(2, 2);
    e.press(Key::Tab, egui::Modifiers::NONE);
    assert_eq!(e.text, "ab  ");
    for path in ["Makefile", "main.go"] {
        let mut e = Editor::new(Style::Standard, "");
        e.path = path;
        e.press(Key::Tab, egui::Modifiers::NONE);
        assert_eq!(e.text, "\t");
    }
}

#[test]
fn comments_toggle_at_existing_indentation_and_preserve_crlf() {
    for (path, prefix) in [
        ("main.rs", "//"),
        ("main.py", "#"),
        ("query.sql", "--"),
        ("main.tex", "%"),
    ] {
        let mut e = Editor::new(Style::Standard, "  α\r\n\r\n    β\r\n");
        e.path = path;
        let len = e.text.chars().count();
        e.select(0, len);
        e.press(Key::Slash, CMD);
        assert_eq!(e.text, format!("  {prefix} α\r\n\r\n    {prefix} β\r\n"));
        e.press(Key::Slash, CMD);
        assert_eq!(e.text, "  α\r\n\r\n    β\r\n");
        e.press(Key::Z, CMD);
        assert_eq!(e.text, format!("  {prefix} α\r\n\r\n    {prefix} β\r\n"));
    }
    let mut e = Editor::new(Style::Standard, "{\"x\": 1}");
    e.path = "file.json";
    e.press(Key::Slash, CMD);
    assert_eq!(e.text, "{\"x\": 1}");
    assert!(e.editing.notice.as_ref().unwrap().contains("file type"));
}

#[test]
fn select_duplicate_delete_and_insert_lines_keep_native_undo_and_line_endings() {
    let mut e = Editor::new(Style::Standard, "α\r\nβ\r\nlast");
    e.press(Key::L, CMD);
    assert_eq!(e.selection().as_sorted_char_range(), 0..3);
    e.press(Key::L, CMD);
    assert_eq!(e.selection().as_sorted_char_range(), 0..6);
    e.press(Key::D, SHIFT_CMD);
    assert_eq!(e.text, "α\r\nβ\r\nα\r\nβ\r\nlast");
    assert_eq!(e.selection().as_sorted_char_range(), 6..12);
    e.press(Key::K, SHIFT_CMD);
    assert_eq!(e.text, "α\r\nβ\r\nlast");
    e.press(Key::Z, CMD);
    assert_eq!(e.text, "α\r\nβ\r\nα\r\nβ\r\nlast");

    let mut e = Editor::new(Style::Standard, "  one\r\n  two");
    e.select(3, 3);
    e.press(Key::Enter, CMD);
    assert_eq!(e.text, "  one\r\n  \r\n  two");
    assert_eq!(e.point(), 9);
    e.press(Key::Enter, SHIFT_CMD);
    assert_eq!(e.text, "  one\r\n  \r\n  \r\n  two");
    assert_eq!(e.point(), 9);
}

#[test]
fn final_line_and_empty_buffer_edits_do_not_join_lines_or_delete_unrelated_text() {
    let mut e = Editor::new(Style::Standard, "one\ntwo");
    e.select(6, 6);
    e.press(Key::D, SHIFT_CMD);
    assert_eq!(e.text, "one\ntwo\ntwo");
    assert_eq!(e.point(), 10);
    e.press(Key::K, SHIFT_CMD);
    assert_eq!(e.text, "one\ntwo");
    e.press(Key::Enter, CMD);
    assert_eq!(e.text, "one\ntwo\n");
    for shortcut in [
        (Key::OpenBracket, CMD),
        (Key::K, SHIFT_CMD),
        (Key::Slash, CMD),
    ] {
        let mut e = Editor::new(Style::Standard, "");
        e.press(shortcut.0, shortcut.1);
        assert_eq!(e.text, "");
    }
}

#[test]
fn batched_select_all_indent_and_typing_use_the_latest_selection_and_preserve_order() {
    let mut e = Editor::new(Style::Standard, "one\ntwo");
    e.frame(vec![key(Key::A, CMD), key(Key::CloseBracket, CMD)]);
    e.drain();
    assert_eq!(e.text, "    one\n    two");
    e.select(7, 7);
    e.frame(vec![
        key(Key::CloseBracket, CMD),
        egui::Event::Text("!".into()),
        key(Key::OpenBracket, CMD),
    ]);
    e.drain();
    assert_eq!(e.text, "    one!\n    two");
    e.press(Key::Z, CMD);
    let before_release = e.text.clone();
    let mut release = key(Key::Z, CMD);
    if let egui::Event::Key { pressed, .. } = &mut release {
        *pressed = false;
    }
    e.frame(vec![release]);
    assert_eq!(e.text, before_release, "Releasing Undo must not undo again");
}

#[test]
fn altgr_and_vim_escape_keep_their_meanings() {
    let mut e = Editor::new(Style::Standard, "");
    e.frame(vec![
        key(
            Key::CloseBracket,
            egui::Modifiers::CTRL | egui::Modifiers::ALT,
        ),
        egui::Event::Text("]".into()),
    ]);
    e.drain();
    assert_eq!(e.text, "]");
    let mut vim = Editor::new(Style::Vim, "text");
    vim.type_text("i");
    vim.press(Key::OpenBracket, egui::Modifiers::CTRL);
    assert_eq!(vim.editing.mode, Mode::Normal);
    assert_eq!(vim.text, "text");
}

#[test]
fn standard_commands_leave_active_ime_composition_to_text_edit() {
    let mut e = Editor::new(Style::Standard, "text");
    e.frame(vec![
        egui::Event::Ime(egui::ImeEvent::Enabled),
        egui::Event::Ime(egui::ImeEvent::Preedit("あ".into())),
    ]);
    let before = e.text.clone();
    e.press(Key::CloseBracket, CMD);
    assert_eq!(e.text, before);
    assert!(e.editing.after_range.is_none());
    e.frame(vec![
        egui::Event::Ime(egui::ImeEvent::Commit("あ".into())),
        egui::Event::Ime(egui::ImeEvent::Disabled),
    ]);
    let committed = e.text.clone();
    e.press(Key::CloseBracket, CMD);
    assert_eq!(e.text, format!("    {committed}"));
}

#[test]
fn oversized_line_edits_leave_the_buffer_and_selection_intact() {
    let source = "x".repeat(LIMIT);
    let text = Text::new(&source);
    let mut state = State::default();
    let mut range = Text::range(0, 0);
    let mut native = vec![];
    assert!(!state.standard(
        &text,
        &mut range,
        super::super::standard::Action::Duplicate,
        Some("main.rs"),
        "    ",
        &mut native
    ));
    assert!(native.is_empty());
    assert_eq!(range, Text::range(0, 0));
    assert!(state.after_range.is_none());
    assert!(state.notice.unwrap().contains("limit"));
}

#[test]
fn source_shortcuts_do_not_consume_terminal_form_or_modal_input() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    for focus in [egui::Id::new("terminal-input"), egui::Id::new("form-field")] {
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key(Key::CloseBracket, CMD)],
                ..Default::default()
            },
            |ctx| {
                ctx.memory_mut(|m| m.request_focus(focus));
                app.editing_shortcuts(ctx);
                assert!(ctx.input(|i| i.key_pressed(Key::CloseBracket)));
            },
        );
    }
    app.dialog = Dialog::Commands;
    let id = app.documents[app.documents.active_index()].id;
    let _ = ctx.run(
        egui::RawInput {
            events: vec![key(Key::CloseBracket, CMD)],
            ..Default::default()
        },
        |ctx| {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new(("document", id))));
            app.editing_shortcuts(ctx);
            assert!(ctx.input(|i| i.key_pressed(Key::CloseBracket)));
        },
    );
    assert!(app.documents[app.documents.active_index()].text.is_empty());
}
