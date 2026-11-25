//! Shared Welcome content for startup, explicit navigation and empty editor windows.
//!
//! This page only dispatches user actions. Its host retains ownership of the
//! project, connection, sidebar arrangement and any documents in other windows.

use super::*;

impl EditorApp {
    /// Share the landing page between explicit Welcome navigation and empty editor
    /// viewports. Rendering never changes the selected area, sidebar or project.
    pub(super) fn welcome(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        let canvas = ui.available_rect_before_wrap();
        let compact = ui.available_height() < 620.;
        let top = if compact {
            16.
        } else {
            ((ui.available_height() - 500.) / 2.).max(24.)
        };
        let occupied = egui::ScrollArea::vertical()
            .id_salt("welcome-page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(top);
                ui.vertical_centered(|ui| {
                    let mark = ui.label(palette.tag("RODE"));
                    ui.add_space(12.0);
                    let title = ui.label(
                        RichText::new("Welcome to RODE")
                            .size(if compact { 26. } else { 32. })
                            .strong()
                            .color(palette.text),
                    );
                    ui_probe::button(ui, "welcome-screen", &title);
                    let subtitle = ui.label(
                        RichText::new("Local projects and remote compute, together.")
                            .size(15.0)
                            .color(palette.muted),
                    );
                    ui.add_space(12.0);
                    let hint = ui.label(palette.tag(if self.workspace.is_some() {
                        "Choose a file in Explorer, or open another workspace."
                    } else {
                        "Open a folder or connect to a remote project to begin."
                    }));
                    ui.add_space(if compact { 20. } else { 32. });
                    let action_size = egui::vec2((ui.available_width() - 32.).clamp(0., 320.), 36.);
                    let connect = ui.add_sized(
                        action_size,
                        crate::keyboard::Button::new(
                            RichText::new("Connect via SSH")
                                .color(palette.background)
                                .strong(),
                        )
                        .fill(palette.accent),
                    );
                    if connect.clicked() {
                        self.set_dialog(Dialog::Connect);
                    }
                    ui.add_space(8.0);
                    let folder = ui.add_sized(
                        action_size,
                        IconButton::new(Icon::Folder, "Open project folder")
                            .shortcut(&self.command_hint(Command::Folder)),
                    );
                    ui_probe::button(ui, "welcome-open-project", &folder);
                    if folder.clicked() {
                        self.set_dialog(Dialog::Folder);
                    }
                    let documents = self.welcome_document_actions(ui);
                    ui.add_space(if compact { 16. } else { 28. });
                    let privacy = ui.label(
                        RichText::new("Your SSH config. Your keys. Your machine.")
                            .color(palette.muted)
                            .size(12.0),
                    );
                    ui.add_space(18.0);
                    let shortcuts = ui.link(
                        RichText::new("Keyboard shortcuts")
                            .color(palette.accent)
                            .size(12.0),
                    );
                    if shortcuts.clicked() {
                        self.set_dialog(Dialog::Shortcuts);
                    }
                    [
                        mark.rect,
                        title.rect,
                        subtitle.rect,
                        hint.rect,
                        connect.rect,
                        folder.rect,
                        documents,
                        privacy.rect,
                        shortcuts.rect,
                    ]
                    .into_iter()
                    .fold(egui::Rect::NOTHING, egui::Rect::union)
                })
                .inner
            });
        palette.paint_welcome_artwork(ui, canvas, occupied.inner);
    }

    /// Keep the landing page's document actions in one aligned group.
    fn welcome_document_actions(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let width = (ui.available_width() - 32.).clamp(0., 320.);
        let ctx = ui.ctx().clone();
        let new = ui.add_sized(
            egui::vec2(width, 32.),
            IconButton::new(Icon::NewFile, "New file").shortcut(&self.command_hint(Command::New)),
        );
        ui_probe::button(ui, "editor-empty-new", &new);
        if new.clicked() {
            self.execute_command(Command::New, &ctx);
        }
        let open = ui.add_sized(
            egui::vec2(width, 32.),
            IconButton::new(Icon::Search, "Open file…").shortcut(&self.command_hint(Command::Open)),
        );
        ui_probe::button(ui, "editor-empty-open", &open);
        if open.clicked() {
            self.set_dialog(Dialog::Open);
        }
        new.rect.union(open.rect)
    }
}
