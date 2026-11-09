//! Typed palette registration shared by native commands and installed adapters.
//!
//! Commands resolve to typed actions and existing guards. Installed contributions become
//! explicit invocations; palette discovery never starts their executable.
use super::*;

mod action;
pub(super) use action::Command;

enum PaletteAction {
    Command(Command),
    Style(navigation::Style),
    Navigation(navigation::NavAction),
    Theme(String),
    Syntax(String),
    Extension(String, String),
}

impl EditorApp {
    /// Render filtered typed commands and execute the selected action through the existing domain guards.
    pub(super) fn command_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.label(palette.tag("COMMAND PALETTE"));
        let list_keys = navigation::List::keys(ctx);
        let input_id = egui::Id::new("command-dialog-input");
        if self.dialog_focus || ui.memory(|memory| memory.focused().is_none()) {
            ui.memory_mut(|memory| memory.request_focus(input_id));
            self.dialog_focus = false;
        }
        let input = ui.add(
            egui::TextEdit::singleline(&mut self.dialog_input)
                .id(input_id)
                .desired_width(f32::INFINITY)
                .hint_text("Type a command…"),
        );
        super::ui_probe::button(ui, "command-palette-input", &input);
        let mut commands: Vec<_> = Command::PALETTE
            .iter()
            .copied()
            .map(|command| {
                (
                    PaletteAction::Command(command),
                    command.definition().palette_label.to_owned(),
                    self.command_hint(command),
                )
            })
            .collect();
        commands.extend(navigation::Style::ALL.into_iter().map(|style| {
            (
                PaletteAction::Style(style),
                format!("Keyboard style: {}", style.name()),
                String::new(),
            )
        }));
        commands.extend(
            self.navigation
                .bindings
                .commands()
                .map(|(action, name, shortcut)| {
                    (
                        PaletteAction::Navigation(action),
                        format!("Navigation: {name}"),
                        shortcut.to_owned(),
                    )
                }),
        );
        commands.extend(self.extensions.registry.commands().into_iter().map(
            |(extension, command, title)| {
                (
                    PaletteAction::Extension(extension, command),
                    title,
                    String::new(),
                )
            },
        ));
        commands.extend(self.themes.entries.iter().map(|entry| {
            (
                PaletteAction::Theme(entry.id.clone()),
                format!("Theme: {}", entry.name),
                String::new(),
            )
        }));
        commands.extend(self.themes.syntax_entries.iter().map(|entry| {
            (
                PaletteAction::Syntax(entry.id.clone()),
                format!("Syntax theme: {}", entry.name),
                String::new(),
            )
        }));
        let query = self.dialog_input.to_lowercase();
        let filtered: Vec<_> = commands
            .iter()
            .enumerate()
            .filter(|(_, (_, name, _))| name.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
        let reveal = self
            .navigation
            .list
            .update(&query, filtered.len(), list_keys.0);
        let mut selected = if list_keys.1 {
            filtered.get(self.navigation.list.index).copied()
        } else {
            None
        };
        egui::ScrollArea::vertical()
            .id_salt("command-results")
            .max_height(360.)
            .show(ui, |ui| {
                for (row, index) in filtered.iter().copied().enumerate() {
                    let (_, name, shortcut) = &commands[index];
                    let response = ui.add_sized(
                        [ui.available_width(), 30.],
                        egui::Button::new(format!("{name}     {shortcut}"))
                            .selected(row == self.navigation.list.index),
                    );
                    // A fixture may submit only after the complete query has
                    // reached this frame, not merely its first matching prefix.
                    if query == name.to_lowercase() {
                        super::ui_probe::button(ui, &format!("command-match:{name}"), &response);
                    }
                    if row == self.navigation.list.index && reveal {
                        response.scroll_to_me(Some(Align::Center));
                    }
                    if response.clicked() {
                        selected = Some(index);
                    }
                }
                if filtered.is_empty() {
                    ui.label("No matching commands");
                }
            });
        if let Some(index) = selected {
            self.dialog = Dialog::None;
            match commands.remove(index).0 {
                PaletteAction::Navigation(action) => self.navigate(action, ctx),
                PaletteAction::Style(style) => self.navigation.bindings.select_style(style),
                PaletteAction::Theme(id) => self.select_theme(&id, ctx),
                PaletteAction::Syntax(id) => self.select_syntax_theme(&id, ctx),
                PaletteAction::Extension(extension, command) => {
                    self.extension_command(&extension, &command, ctx)
                }
                PaletteAction::Command(command) => self.execute_command(command, ctx),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_document_accepts_text_on_its_first_focused_frame() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1380., 900.),
                )),
                events: vec![egui::Event::Text("first-frame-edit".into())],
                ..Default::default()
            },
            |ctx| app.editor(ctx),
        );
        assert_eq!(app.documents[0].text, "first-frame-edit");
    }

    #[test]
    fn the_first_palette_frame_uses_queued_text_before_confirming_a_command() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.new_file();
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.editor(ctx));
        app.set_dialog(Dialog::Commands);
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1380., 900.),
                )),
                events: vec![
                    egui::Event::Text("Open file by path".into()),
                    egui::Event::Key {
                        key: Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: Default::default(),
                    },
                ],
                ..Default::default()
            },
            |ctx| {
                app.editor(ctx);
                app.dialogs(ctx);
            },
        );
        assert!(
            app.documents.len() == 1 && app.documents[0].text.is_empty(),
            "Sizing must not activate the default command"
        );
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            app.editor(ctx);
            app.dialogs(ctx);
        });
        assert!(
            app.documents.len() == 1 && app.documents[0].text.is_empty(),
            "A delayed first frame must not execute New file"
        );
        assert!(
            app.dialog == Dialog::Open,
            "The typed command must open the file picker"
        );
    }

    #[test]
    fn a_palette_with_no_focused_widget_recovers_before_processing_text() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.set_dialog(Dialog::Commands);
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| app.dialogs(ctx));
        }
        ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("command-dialog-input")));
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Text("Open file by path".into())],
                ..Default::default()
            },
            |ctx| app.dialogs(ctx),
        );
        assert_eq!(app.dialog_input, "Open file by path");
    }

    #[test]
    fn changing_dialogs_discards_keyboard_input_waiting_for_the_old_dialog() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.set_dialog(Dialog::Commands);
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Text("New file".into())],
                ..Default::default()
            },
            |ctx| app.dialogs(ctx),
        );
        app.set_dialog(Dialog::Open);
        let _ = ctx.run(egui::RawInput::default(), |ctx| app.dialogs(ctx));
        assert!(app.dialog == Dialog::Open);
        assert!(app.dialog_input.is_empty());
        assert!(app.documents.is_empty());
    }
}
