//! Application menus, workspace navigation and temporary terminal views.
//!
//! Presentation routes through the coordinator's guarded actions and existing
//! domain controllers. Rendering application chrome does not own a project,
//! terminal, agent or task lifetime. Keep widget IDs stable when moving views.

use super::*;

/// Order by presentation role: keep the two sidebar choices adjacent, followed
/// by project tools and then application views. Connections retains the central
/// view even though its saved locations are independent of the current project.
pub(super) const ACTIVITY: [Command; 5] = [
    Command::Explorer,
    Command::Connections,
    Command::Sidebar,
    Command::Panel(Panel::Monitor),
    Command::Panel(Panel::Transfers),
];

impl EditorApp {
    /// The brand is a native, keyboard-discoverable Welcome action, including
    /// in narrow windows. Visiting Welcome leaves project and service owners intact.
    fn welcome_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, minimal: bool) {
        let palette = self.themes.active().palette;
        let mark = ui.next_auto_id().with("rode-mark");
        let atom = egui::Atom::custom(mark, egui::vec2(26., 26.));
        let button = if minimal {
            egui::Button::new(atom)
        } else {
            egui::Button::new((
                atom,
                RichText::new("RODE")
                    .strong()
                    .size(15.)
                    .color(palette.heading_color()),
            ))
        };
        let output = ui
            .add_enabled_ui(!self.foreign_modal(ctx), |ui| {
                ui.spacing_mut().button_padding = egui::vec2(4., 2.);
                button
                    .frame(false)
                    .stroke(Stroke::NONE)
                    .sense(keyboard::button_sense(ui, "Welcome", Sense::click()))
                    .atom_ui(ui)
            })
            .inner;
        if let Some(rect) = output.rect(mark) {
            ui.painter()
                .rect_filled(rect, 7, palette.accent.gamma_multiply(0.12));
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "[r]",
                FontId::monospace(13.),
                palette.accent,
            );
        }
        let response = output.response.on_hover_text("Show Welcome");
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), "Welcome")
        });
        theme::button_focus(ui, &response, None);
        keyboard::target(ui, "Welcome", &response);
        ui_probe::button(ui, "welcome-home", &response);
        if response.clicked() {
            keyboard::consume_activation(&response);
            self.execute_command(Command::Welcome, ctx);
        }
    }

    /// Menus and workspace search share the header space left by fixed actions.
    /// Keep their existing widget identities while the parent clips or scrolls this region.
    fn titlebar_navigation(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, minimal: bool) {
        let palette = self.themes.active().palette;
        ui.horizontal_centered(|ui| {
            self.welcome_button(ui, ctx, minimal);
            ui.add_space(if minimal { 4. } else { 14. });
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
                    let closing = self.workspace.project_open() || self.vaults.has_active();
                    let (icon, label, command, probe, hint) = if closing {
                        (
                            Icon::Close,
                            if self.vaults.has_active() {
                                "Close & lock"
                            } else {
                                "Close project"
                            },
                            Command::CloseProject,
                            "close-project",
                            if self.vaults.has_active() {
                                "Close this encrypted project and unmount it after checking unsaved changes"
                            } else {
                                "Close this project after checking unsaved changes"
                            }.to_owned(),
                        )
                    } else {
                        (
                            Icon::Folder,
                            "Open project…",
                            Command::Folder,
                            "open-project",
                            format!(
                                "Open a project folder · {}",
                                self.command_hint(Command::Folder)
                            ),
                        )
                    };
                    let button = if compact {
                        IconButton::toolbar(icon, label)
                    } else {
                        IconButton::new(icon, label)
                    };
                    let project = ui
                        .add_enabled(!self.foreign_modal(ctx), button)
                        .on_hover_text(hint);
                    ui_probe::button(ui, probe, &project);
                    if project.clicked() {
                        keyboard::consume_activation(&project);
                        self.execute_command(command, ctx);
                    }
                    if self
                        .workspace
                        .as_ref()
                        .is_some_and(|w| w.remote.is_some() && w.connected)
                    {
                        let button = if minimal {
                            IconButton::toolbar(Icon::Exit, "Disconnect SSH")
                        } else {
                            IconButton::new(Icon::Exit, "Disconnect")
                        };
                        let disconnect = ui
                            .add_enabled(
                                !self.foreign_modal(ctx) && !self.runtime_work_pending(),
                                button,
                            )
                            .on_hover_text("Disconnect from the SSH host");
                        ui_probe::button(ui, "disconnect-ssh", &disconnect);
                        if disconnect.clicked() {
                            keyboard::consume_activation(&disconnect);
                            self.execute_command(Command::Disconnect, ctx);
                        }
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
        let installed_panels: Vec<_> = self
            .extensions
            .registry
            .panels()
            .map(|e| {
                (
                    e.id.clone(),
                    e.name.clone(),
                    e.panel()
                        .map(|panel| panel.icon.to_owned())
                        .unwrap_or_default(),
                )
            })
            .collect();
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
                let commands: Vec<_> = ACTIVITY
                    .into_iter()
                    .filter(|command| {
                        self.command_visible(*command)
                            && (project_controls
                                || !matches!(*command, Command::Explorer | Command::Sidebar))
                    })
                    .collect();
                // Keep the footer outside the scroll clip. At high zoom a fixed
                // minimum button size cannot fit every area above those controls.
                let area_height = (ui.available_height() - 112.).max(0.);
                let group_gap = if project_controls { 12. } else { 0. };
                let button_height = ((area_height - group_gap)
                    / (commands.len() + installed_panels.len()) as f32
                    - 6.)
                    .clamp(22., 40.);
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
                        for (id, name, icon) in &installed_panels {
                            let image = self.extensions.icon(icon, ctx);
                            let selected = self.presentation.area == areas::Area::Extension
                                && self.extension_panels.selected == *id;
                            let response = ui.add_sized(
                                [36., button_height],
                                IconButton::icon(
                                    match icon.as_str() {
                                        "chart" => Icon::Stocks,
                                        "monitor" => Icon::Monitor,
                                        "folder" => Icon::Folder,
                                        _ => Icon::Extensions,
                                    },
                                    name,
                                )
                                .icon_size(23.)
                                .image(image)
                                .selected(selected),
                            );
                            keyboard::target(ui, name, &response);
                            ui_probe::button(ui, &format!("extension-panel:{id}"), &response);
                            ui_probe::button(ui, &format!("activity:{name}"), &response);
                            if response.clicked() || response.secondary_clicked() {
                                keyboard::consume_activation(&response);
                                self.open_extension_panel(id, ctx);
                            }
                            keyboard::context_menu(&response, |ui| {
                                if let Some(tab) = self.extension_panels.tab(id) {
                                    self.tool_window_menu(ui, tab, ctx);
                                }
                            });
                            ui.add_space(6.);
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
        let tab = ToolTab::for_command(command);
        if let Some(tab) = tab {
            hint.push_str(if self.tab_detached(tab) {
                "\nRight-click to reattach to the main window"
            } else {
                "\nRight-click to open in a separate window"
            });
        }
        let selected = self.command_selected(command, ctx);
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
}
