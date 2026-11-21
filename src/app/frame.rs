//! Application menus, workspace navigation and pane entry views.
//!
//! Presentation routes through the coordinator's guarded actions and existing
//! domain controllers. Rendering a menu or welcome page does not own a project,
//! terminal, agent or task lifetime. Keep widget IDs stable when moving views.

use super::*;

/// The rail mixes a project destination, a sidebar and host tools. It is not a
/// catalogue of central pages: Connections leaves the central view intact.
pub(super) const ACTIVITY: [Command; 5] = [
    Command::Projects,
    Command::Connections,
    Command::Panel(Panel::Monitor),
    Command::Panel(Panel::Transfers),
    Command::Panel(Panel::Stocks),
];

impl EditorApp {
    /// Menus and workspace search share the header space left by fixed actions.
    /// Keep their existing widget identities while the parent clips or scrolls this region.
    fn titlebar_navigation(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, minimal: bool) {
        let palette = self.themes.active().palette;
        ui.horizontal_centered(|ui| {
            if !minimal {
                let (mark, _) = ui.allocate_exact_size(egui::vec2(26., 26.), Sense::hover());
                ui.painter()
                    .rect_filled(mark, 7, palette.accent.gamma_multiply(0.12));
                ui.painter().text(
                    mark.center(),
                    egui::Align2::CENTER_CENTER,
                    "[r]",
                    FontId::monospace(13.),
                    palette.accent,
                );
                ui.label(
                    RichText::new("RODE")
                        .strong()
                        .size(15.)
                        .color(palette.heading_color()),
                );
                ui.add_space(14.);
            }
            self.menus(ui, ctx);
            ui.add_space(if minimal { 4. } else { 12. });
            let workspace_width = ui.available_width().clamp(0., 360.);
            if workspace_width < 40. {
                return;
            }
            let label = self
                .workspace
                .as_ref()
                .map(|w| {
                    format!(
                        "{}  ·  {}",
                        ssh::display_name(&w.root),
                        if w.remote.is_some() {
                            "SSH workspace"
                        } else {
                            "Local workspace"
                        }
                    )
                })
                .unwrap_or_else(|| "Your next workspace starts here".into());
            let mut button = IconButton::new(Icon::Search, &label)
                .color(palette.muted)
                .truncate();
            if workspace_width >= 250. {
                button = button.shortcut("Ctrl+Shift+P");
            }
            let search = ui
                .add_sized([workspace_width, 30.], button)
                .on_hover_text(format!("{label}\nOpen command palette · Ctrl+Shift+P"));
            ui_probe::button(ui, "workspace-search", &search);
            if search.clicked() {
                self.set_dialog(Dialog::Commands);
            }
        });
    }

    /// Reserve actions at the right before measuring search space. A clipped,
    /// scrollable menu region keeps oversized fonts from overlapping those actions.
    pub(super) fn titlebar(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let frame = theme::frame(palette.panel);
        let height = (theme::control_height_for_style(ctx, &ctx.style())
            + frame.inner_margin.sum().y)
            .max(48.);
        let compact = ctx.content_rect().width() < 900.;
        let minimal = ctx.content_rect().width() < 650.;
        egui::TopBottomPanel::top("titlebar")
            .exact_height(height)
            .frame(frame)
            .show(ctx, |ui| {
                palette.paint_panel(ui);
                if minimal {
                    ui.spacing_mut().item_spacing.x = 4.;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let open = if compact {
                        IconButton::toolbar(Icon::Folder, "Open project…")
                    } else {
                        IconButton::new(Icon::Folder, "Open project…")
                    };
                    let open = ui
                        .add_enabled(!self.foreign_modal(ctx), open)
                        .on_hover_text(format!(
                            "Open a project folder · {}",
                            self.command_hint(Command::Folder)
                        ));
                    ui_probe::button(ui, "open-project", &open);
                    if open.clicked() {
                        keyboard::consume_activation(&open);
                        self.execute_command(Command::Folder, ctx);
                    }
                    let width = ui.available_width().max(0.);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, ui.available_height()),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            egui::ScrollArea::horizontal()
                                .id_salt("titlebar-navigation")
                                .auto_shrink([false, true])
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                )
                                .show(ui, |ui| {
                                    ui.set_max_width(width);
                                    self.titlebar_navigation(ui, ctx, minimal);
                                });
                        },
                    );
                });
            });
    }

    /// Render feature-gated activity actions without treating selection as tool-process activation.
    pub(super) fn activity(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let project_controls = ctx.viewport_id() == egui::ViewportId::ROOT;
        egui::SidePanel::left("activity")
            .exact_width(52.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette.activity_background.unwrap_or(palette.panel))
                    .inner_margin(egui::Margin::symmetric(7, 14)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.;
                ui.spacing_mut().button_padding = egui::vec2(0., 2.);
                let mut commands = Vec::new();
                for command in ACTIVITY {
                    if self.command_visible(command) {
                        commands.push(command);
                        if command == Command::Projects && project_controls {
                            commands.extend([Command::Explorer, Command::Sidebar]);
                        }
                    }
                }
                // Keep the footer outside the scroll clip. At high zoom a fixed
                // minimum button size cannot fit every area above those controls.
                let area_height = (ui.available_height() - 112.).max(0.);
                let group_gap = if project_controls { 12. } else { 0. };
                let button_height =
                    ((area_height - group_gap) / commands.len() as f32 - 6.).clamp(22., 40.);
                egui::ScrollArea::vertical()
                    .id_salt("application-areas")
                    .max_height(area_height)
                    .min_scrolled_height(0.)
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        for command in commands {
                            if matches!(command, Command::Explorer | Command::Sidebar) {
                                ui.add_enabled_ui(!self.foreign_modal(ctx), |ui| {
                                    self.activity_button(ui, command, button_height, ctx);
                                });
                            } else {
                                self.activity_button(ui, command, button_height, ctx);
                            }
                            ui.add_space(6.0);
                            if command == Command::Sidebar {
                                ui.add_space(3.);
                                ui.separator();
                                ui.add_space(3.);
                            }
                        }
                    });
                ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                    self.activity_button(ui, Command::AppSettings, 36., ctx);
                    self.activity_button(ui, Command::Extensions, 36., ctx);
                    let help = ui.add_sized(
                        [36., 28.],
                        IconButton::icon(Icon::Help, "Keyboard shortcuts").icon_size(21.),
                    );
                    ui_probe::button(ui, "activity:Help", &help);
                    if help.clicked() {
                        self.set_dialog(Dialog::Shortcuts);
                    }
                });
            });
    }

    /// Activity rails share icons, selection, shortcuts and command dispatch.
    pub(super) fn activity_button(
        &mut self,
        ui: &mut egui::Ui,
        command: Command,
        button_height: f32,
        ctx: &egui::Context,
    ) {
        if !self.command_visible(command) {
            return;
        }
        let palette = self.themes.active().palette;
        let definition = command.definition();
        let shortcut = self.command_hint(command);
        let mut hint = if shortcut.is_empty() {
            definition.label.to_owned()
        } else {
            format!("{} · {shortcut}", definition.label)
        };
        let tab = tool_windows::ToolTab::for_command(command);
        if let Some(tab) = tab {
            hint.push_str(if self.tab_detached(tab) {
                "\nRight-click to reattach to the main window"
            } else {
                "\nRight-click to open in a separate window"
            });
        }
        let selected = self.command_selected(command, ctx);
        if command == Command::Projects {
            hint.push_str(if selected {
                "\nClick to return to Welcome"
            } else {
                "\nShow the project workspace"
            });
        }
        let response = ui.add_sized(
            [36.0, button_height],
            IconButton::icon(definition.icon, &hint)
                .icon_size(if button_height < 34. { 20. } else { 23. })
                .selected(selected),
        );
        // Keep control-search names stable while tooltips include shortcuts
        // and instructions for detached windows.
        keyboard::target(ui, definition.label, &response);
        ui_probe::button(ui, &format!("activity:{}", definition.label), &response);
        // Keep native control identities independent of where navigation lives.
        // Automation and action hints use the same response as the visible icon.
        match command {
            Command::Sidebar => {
                ui_probe::button(ui, "workspace-sidebar", &response);
                ui_probe::button(
                    ui,
                    if selected {
                        "workspace-sidebar-open"
                    } else {
                        "workspace-sidebar-closed"
                    },
                    &response,
                );
            }
            Command::Explorer => ui_probe::button(ui, "workspace-explorer", &response),
            _ => {}
        }
        if response.has_focus() {
            response.scroll_to_me(None);
        }
        if selected {
            palette.paint_activity(
                ui.painter(),
                egui::Rect::from_min_size(
                    egui::pos2(response.rect.left() - 7., response.rect.top() + 6.),
                    egui::vec2(2., response.rect.height() - 12.),
                ),
            );
        }
        if let Some(tab) = tab {
            crate::keyboard::context_menu(&response, |ui| self.tool_window_menu(ui, tab, ctx));
        }
        if response.clicked() {
            keyboard::consume_activation(&response);
            self.execute_command(command, ctx);
        }
    }

    /// Render the selected terminal surface, keeping PTY input and native focus ownership together.
    pub(in crate::app) fn terminal_content(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.terminal_colors.palette(self.themes.active());
        let mut content = self.pane_ui(ui, navigation::Pane::Terminal);
        let ui = &mut content;
        self.terminal_header(ui, ctx);
        self.terminal_tabs(ui, ctx);
        ui.separator();
        if self.terminal.is_none() && self.ssh_master.is_none() {
            ui.label(palette.tag(if self.session_busy() {
                "Starting terminal…"
            } else {
                "No terminal attached. Choose New or Sessions."
            }));
            ui.allocate_space(ui.available_size().max(egui::Vec2::ZERO));
        }
        if let Some(terminal) = self.terminal.as_mut().or(self.ssh_master.as_mut()) {
            let view = terminal.ui_with_response(ui, self.font_size - 1.0, palette);
            let response = view.response;
            let mut first_cell = response.clone();
            first_cell.rect = view.first_cell;
            super::ui_probe::button(ui, "terminal-first-cell", &first_cell);
            if let Some(copy) = view.copy_selection {
                super::ui_probe::button(ui, "terminal-copy-selection", &copy);
            }
            if response.context_menu_opened() {
                super::ui_probe::button(ui, "terminal-context-menu", &response);
            }
            if !terminal.is_alive() {
                super::ui_probe::button(ui, "terminal-output-ended", &response);
            }
            if let Some(error) = view.error {
                self.notify(error, true);
            }
            if !self.session_busy()
                && self.terminal.as_ref().is_some_and(Terminal::is_alive)
                && let Some(active) = &self.sessions.active
            {
                super::ui_probe::button(ui, &format!("terminal-input:{}", active.key), &response);
                keyboard::editor_target(ui, "Terminal input", &response);
                if active.native.is_none() {
                    super::ui_probe::button(ui, "temporary-terminal-input", &response);
                }
            }
        }
    }

    /// Render local/SSH project entry points when no document surface is selected.
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
                            .shortcut("Ctrl+Shift+O"),
                    );
                    ui_probe::button(ui, "welcome-open-project", &folder);
                    if folder.clicked() {
                        self.set_dialog(Dialog::Folder);
                    }
                    let documents = self.empty_editor_actions(ui);
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
}
