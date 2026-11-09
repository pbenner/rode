//! Shared menu presentation. Actions still use the coordinator's normal guards.
use super::*;

#[cfg(test)]
mod tests;

/// Keep native menu focus, dismissal and submenu handling; only change presentation.
fn style(style: &mut egui::Style) {
    egui::containers::menu::menu_style(style);
    style.spacing.button_padding = egui::vec2(10., 5.);
    style.spacing.item_spacing = egui::vec2(10., 2.);
    style.spacing.interact_size.y = style.spacing.interact_size.y.max(30.);
    style.spacing.menu_margin = egui::Margin::same(8);
    style.spacing.scroll = egui::style::ScrollStyle::solid();
}

fn contents<R>(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let width = (ui.ctx().content_rect().width() - 32.).clamp(1., width);
    ui.set_width(width);
    let height = (ui.ctx().content_rect().height() - 80.).max(1.);
    // Popups start with egui's smaller default area. Let short menus take their
    // natural height before introducing scrolling at the viewport boundary.
    ui.set_max_height(height);
    egui::ScrollArea::vertical()
        .id_salt("menu-contents")
        .max_height(height)
        .show(ui, add)
        .inner
}

fn top(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    let response = ui
        .scope(|ui| {
            egui::containers::menu::menu_style(ui.style_mut());
            ui.spacing_mut().button_padding = egui::vec2(10., 6.);
            egui::containers::menu::MenuButton::new(label)
                .config(
                    egui::containers::menu::MenuConfig::new()
                        .style(style)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
                )
                .ui(ui, |ui| contents(ui, 320., add))
                .0
        })
        .inner;
    ui_probe::button(ui, &format!("menu-{}", label.to_lowercase()), &response);
    response
}

pub(super) fn submenu(ui: &mut egui::Ui, icon: Icon, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    icons::submenu(ui, icon, label, |ui| contents(ui, 260., add));
}

fn section(ui: &mut egui::Ui, label: &str, first: bool) {
    if !first {
        ui.add_space(4.);
        ui.separator();
        ui.add_space(4.);
    }
    ui.add(egui::Label::new(
        RichText::new(label).small().strong().weak(),
    ));
    ui.add_space(2.);
}

fn item(ui: &mut egui::Ui, icon: Icon, label: &str, shortcut: &str) -> bool {
    let response = ui.add(IconButton::new(icon, label).shortcut(shortcut));
    ui_probe::button(ui, &format!("menu-item:{label}"), &response);
    response.clicked()
}

/// Menu checkmarks share the icon gutter and shortcut column with command rows.
fn check(ui: &mut egui::Ui, label: &str, shortcut: &str, selected: bool) -> bool {
    let id = ui.next_auto_id().with("menu-check");
    let output = egui::Button::new((egui::Atom::custom(id, egui::Vec2::splat(16.)), label))
        .shortcut_text(shortcut)
        .atom_ui(ui);
    if selected && let Some(rect) = output.rect(id) {
        Icon::Check.paint(ui.painter(), rect, ui.visuals().selection.stroke.color);
    }
    output.response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), selected, label)
    });
    ui_probe::button(ui, &format!("menu-item:{label}"), &output.response);
    output.response.clicked()
}

impl EditorApp {
    pub(super) fn menus(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let file = top(ui, "File", |ui| self.file_menu(ui, ctx));
        self.navigation_menu(&file);
        top(ui, "View", |ui| self.view_menu(ui, ctx));
        top(ui, "Code", |ui| self.code_menu(ui, ctx));
        top(ui, "Debug", |ui| self.debug_menu(ui, ctx));
    }

    fn file_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section(ui, "CREATE", true);
        if item(ui, Icon::NewFile, "New file", self.editing_hint("Ctrl+N")) {
            self.new_file();
            ui.close();
        }
        if item(ui, Icon::NewFile, "New Jupyter notebook", "") {
            self.new_notebook();
            ui.close();
        }
        if item(ui, Icon::NewFile, "New Marp presentation", "") {
            self.new_marp();
            ui.close();
        }
        section(ui, "OPEN", false);
        for (icon, label, shortcut, dialog) in [
            (
                Icon::Search,
                "Open file…",
                self.editing_hint("Ctrl+P"),
                Dialog::Open,
            ),
            (
                Icon::Folder,
                "Open project folder…",
                "Ctrl+Shift+O",
                Dialog::Folder,
            ),
            (Icon::Files, "Open binary / hex…", "", Dialog::HexOpen),
        ] {
            if item(ui, icon, label, shortcut) {
                self.set_dialog(dialog);
                ui.close();
            }
        }
        if item(ui, Icon::Folder, "Open local folder…", "") {
            self.open_local_folder_dialog();
            ui.close();
        }
        if item(ui, Icon::Folder, "Open encrypted project…", "") {
            self.encrypted_dialog();
            ui.close();
        }
        section(ui, "REMOTE", false);
        for (label, dialog) in [
            ("Connect via SSH…", Dialog::Connect),
            ("SSH tunnels…", Dialog::Tunnels),
        ] {
            if item(ui, Icon::Remote, label, "") {
                self.set_dialog(dialog);
                ui.close();
            }
        }
        section(ui, "SAVE", false);
        if item(ui, Icon::Save, "Save", self.editing_hint("Ctrl+S")) {
            self.save_active(ctx, false);
            ui.close();
        }
        if item(
            ui,
            Icon::SaveAs,
            "Save as…",
            self.editing_hint("Ctrl+Shift+S"),
        ) {
            self.save_active(ctx, true);
            ui.close();
        }
        if item(ui, Icon::SaveAll, "Save all", "") {
            let ids: Vec<_> = self
                .documents
                .iter()
                .filter(|d| d.dirty() && d.path.is_some())
                .map(|d| (d.id, d.path.clone().unwrap_or_default()))
                .collect();
            for (id, path) in ids {
                self.save_document(id, path, false, ctx);
            }
            if self.documents.iter().any(|d| d.path.is_none()) {
                self.notify(
                    "Save untitled files individually with Ctrl+S to choose their paths.",
                    false,
                );
            }
            ui.close();
        }
        ui.add_space(4.);
        ui.separator();
        if item(ui, Icon::Exit, "Quit", "") {
            self.guarded(Action::Quit, ctx);
            ui.close();
        }
    }

    fn view_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section(ui, "APPEARANCE", true);
        submenu(ui, Icon::Palette, "Color theme", |ui| {
            self.theme_menu(ui, ctx)
        });
        submenu(ui, Icon::Palette, "Syntax highlighting", |ui| {
            self.syntax_theme_menu(ui, ctx)
        });
        self.ui_scale_menu(ui, ctx);
        if item(ui, Icon::Palette, "Terminal colors…", "") {
            self.terminal_colors.open = true;
            ui.close();
        }
        submenu(ui, Icon::Format, "Editor font size", |ui| {
            ui.label(RichText::new("Editor text only").small().weak());
            self.font_save_pending |= ui
                .add(egui::Slider::new(&mut self.font_size, 10.0..=26.0).suffix(" pt"))
                .changed();
        });
        section(ui, "WORKSPACE", false);
        submenu(ui, Icon::Environment, "Workspace panels", |ui| {
            for (panel, icon, label, shortcut) in [
                (Panel::Git, Icon::Git, "Source control", "Ctrl+Shift+G"),
                (Panel::Monitor, Icon::Monitor, "System monitor", ""),
                (Panel::Problems, Icon::Problems, "Problems", "Ctrl+Shift+M"),
                (Panel::Environment, Icon::Environment, "Environment", ""),
                (Panel::Tasks, Icon::Play, "Tasks", ""),
                (Panel::Settings, Icon::Settings, "Project settings", ""),
                (Panel::Agent, Icon::Chat, "Agent chat", "Ctrl+Shift+A"),
                (Panel::Debug, Icon::Debug, "Debugger", "F5"),
            ] {
                if item(ui, icon, label, shortcut) {
                    self.toggle_panel(panel, ctx);
                    ui.close();
                }
            }
        });
        if item(ui, Icon::Terminal, "Terminals & Slurm", "") {
            self.open_sessions(ctx);
            ui.close();
        }
        if item(ui, Icon::Extensions, "Extensions…", "Ctrl+Shift+X") {
            self.set_dialog(Dialog::Extensions);
            ui.close();
        }
        section(ui, "LAYOUT", false);
        if check(ui, "Explorer", "Ctrl+B", self.show_explorer) {
            self.show_explorer = !self.show_explorer;
        }
        if item(ui, Icon::ChevronRight, "Toggle tools sidebar", "Ctrl+~") {
            self.toggle_sidebar(ctx);
            ui.close();
        }
        if item(ui, Icon::Terminal, "Toggle terminal", "Ctrl+Shift+T") {
            self.toggle_terminal(ctx);
            ui.close();
        }
        submenu(ui, Icon::Files, "Previews", |ui| {
            if item(ui, Icon::Files, "Markdown preview", "Ctrl+Shift+R") {
                self.toggle_markdown(ctx);
                ui.close();
            }
            if item(ui, Icon::Play, "Marp slide preview", "") {
                self.toggle_marp(ctx);
                ui.close();
            }
        });
        if check(ui, "Show hidden files", "", self.show_hidden) {
            self.show_hidden = !self.show_hidden;
            self.tree_dirty = true;
        }
        ui.add_space(4.);
        ui.separator();
        if item(
            ui,
            Icon::Search,
            "Find / replace",
            self.editing_hint("Ctrl+F"),
        ) {
            self.markdown_source();
            self.show_find = true;
            self.find_focus = true;
            ui.close();
        }
        if item(ui, Icon::Settings, "Keyboard shortcuts…", "") {
            self.set_dialog(Dialog::Shortcuts);
            ui.close();
        }
    }

    fn code_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section(ui, "DOCUMENT", true);
        for (icon, label, shortcut, format, fix) in [
            (Icon::Format, "Format document", "Shift+Alt+F", true, false),
            (Icon::Check, "Check document", "", false, false),
            (Icon::Fix, "Ruff: apply safe fixes", "", true, true),
        ] {
            if item(ui, icon, label, shortcut) {
                self.format_active(format, fix, ctx);
                ui.close();
            }
        }
        section(ui, "EXECUTION", false);
        for (label, build) in [
            ("Run current file / project", false),
            ("Build current file / project", true),
        ] {
            if item(ui, Icon::Play, label, "") {
                self.run_active(build, ctx);
                ui.close();
            }
        }
    }

    fn debug_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section(ui, "SESSION", true);
        if item(ui, Icon::Debug, "Start / continue", "F5") {
            self.start_debugger(ctx);
            ui.close();
        }
        if item(ui, Icon::Pause, "Pause", "F6") {
            self.debug_control("pause");
            ui.close();
        }
        if item(ui, Icon::Stop, "Stop / detach", "Shift+F5") {
            self.stop_debugger();
            ui.close();
        }
        section(ui, "STEPPING", false);
        for (icon, label, command, shortcut) in [
            (Icon::StepOver, "Step over", "next", "F10"),
            (Icon::StepIn, "Step into", "stepIn", "F11"),
            (Icon::StepOut, "Step out", "stepOut", "Shift+F11"),
        ] {
            if item(ui, icon, label, shortcut) {
                self.debug_control(command);
                ui.close();
            }
        }
        section(ui, "BREAKPOINTS", false);
        if item(ui, Icon::Check, "Toggle breakpoint", "F9") {
            self.toggle_cursor_breakpoint();
            ui.close();
        }
    }
}
