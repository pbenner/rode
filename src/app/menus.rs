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

    fn menu_commands(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, commands: &[Command]) {
        for &command in commands {
            let definition = command.definition();
            if item(
                ui,
                definition.icon,
                definition.label,
                &self.command_hint(command),
            ) {
                self.execute_command(command, ctx);
                ui.close();
            }
        }
    }

    fn file_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section(ui, "CREATE", true);
        self.menu_commands(
            ui,
            ctx,
            &[Command::New, Command::NewNotebook, Command::MarpNew],
        );
        section(ui, "OPEN", false);
        self.menu_commands(
            ui,
            ctx,
            &[
                Command::Open,
                Command::Folder,
                Command::Hex,
                Command::LocalFolder,
                Command::Encrypted,
            ],
        );
        section(ui, "REMOTE", false);
        self.menu_commands(ui, ctx, &[Command::Connect, Command::Tunnels]);
        section(ui, "SAVE", false);
        self.menu_commands(ui, ctx, &[Command::Save, Command::SaveAs, Command::SaveAll]);
        ui.add_space(4.);
        ui.separator();
        self.menu_commands(ui, ctx, &[Command::Quit]);
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
        self.menu_commands(ui, ctx, &[Command::TerminalColors]);
        submenu(ui, Icon::Format, "Editor font size", |ui| {
            ui.label(RichText::new("Editor text only").small().weak());
            self.font_save_pending |= ui
                .add(egui::Slider::new(&mut self.font_size, 10.0..=26.0).suffix(" pt"))
                .changed();
        });
        section(ui, "WORKSPACE", false);
        submenu(ui, Icon::Environment, "Workspace panels", |ui| {
            self.menu_commands(ui, ctx, Command::PANELS)
        });
        self.menu_commands(ui, ctx, &[Command::Sessions, Command::Extensions]);
        section(ui, "LAYOUT", false);
        if check(
            ui,
            Command::Explorer.definition().label,
            &self.command_hint(Command::Explorer),
            self.command_selected(Command::Explorer),
        ) {
            self.execute_command(Command::Explorer, ctx);
        }
        self.menu_commands(ui, ctx, &[Command::Sidebar, Command::Terminal]);
        submenu(ui, Icon::Files, "Previews", |ui| {
            self.menu_commands(ui, ctx, &[Command::Markdown, Command::MarpSlides])
        });
        if check(ui, "Show hidden files", "", self.show_hidden) {
            self.show_hidden = !self.show_hidden;
            self.explorer_state.tree_dirty = true;
        }
        ui.add_space(4.);
        ui.separator();
        self.menu_commands(ui, ctx, &[Command::Find, Command::Shortcuts]);
    }

    fn code_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section(ui, "DOCUMENT", true);
        self.menu_commands(ui, ctx, &[Command::Format, Command::Check, Command::Fix]);
        section(ui, "EXECUTION", false);
        self.menu_commands(ui, ctx, &[Command::Run, Command::Build]);
    }

    fn debug_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        section(ui, "SESSION", true);
        self.menu_commands(
            ui,
            ctx,
            &[Command::DebugStart, Command::DebugPause, Command::DebugStop],
        );
        section(ui, "STEPPING", false);
        self.menu_commands(
            ui,
            ctx,
            &[Command::DebugOver, Command::DebugIn, Command::DebugOut],
        );
        section(ui, "BREAKPOINTS", false);
        self.menu_commands(ui, ctx, &[Command::Breakpoint]);
    }
}
