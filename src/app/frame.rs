//! Application menus, workspace connection banners and pane entry views.
//!
//! Presentation routes through the coordinator's guarded actions and existing
//! domain controllers. Rendering a menu or welcome page does not own a project,
//! terminal, agent or task lifetime. Keep widget IDs stable when moving views.

use super::*;

impl EditorApp {
    /// Render native app menus and workspace identity using the same guarded actions as commands.
    pub(super) fn titlebar(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        egui::TopBottomPanel::top("titlebar")
            .exact_height(48.0)
            .frame(theme::frame(palette.panel))
            .show(ctx, |ui| {
                palette.paint_panel(ui);
                ui.horizontal_centered(|ui| {
                    ui.label(
                        RichText::new("[r]")
                            .monospace()
                            .size(22.0)
                            .strong()
                            .color(palette.accent),
                    );
                    ui.label(
                        RichText::new("RODE")
                            .strong()
                            .size(16.0)
                            .color(palette.heading_color()),
                    );
                    ui.add_space(14.0);
                    self.menus(ui, ctx);
                    ui.add_space(12.0);
                    let label = self
                        .workspace
                        .as_ref()
                        .map(|w| {
                            format!(
                                "{}  /  {}",
                                if w.remote.is_some() { "SSH" } else { "LOCAL" },
                                ssh::display_name(&w.root)
                            )
                        })
                        .unwrap_or_else(|| "Your next workspace starts here".into());
                    let roomy = ctx.content_rect().width() >= 1180.;
                    let workspace_width =
                        (ui.available_width() - if roomy { 250. } else { 230. }).clamp(90., 360.);
                    if ui
                        .add_sized(
                            [workspace_width, 28.],
                            egui::Button::new(RichText::new(&label).color(palette.muted))
                                .truncate(),
                        )
                        .on_hover_text(format!("{label}\nOpen command palette · Ctrl+Shift+P"))
                        .clicked()
                    {
                        self.set_dialog(Dialog::Commands);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(IconButton::new(Icon::Remote, "Connect SSH").color(palette.accent))
                            .clicked()
                        {
                            self.set_dialog(Dialog::Connect);
                        }
                        let tunnels = self.tunnel_status_button(ui, palette);
                        ui_probe::button(ui, "open-tunnels", &tunnels);
                        if tunnels.clicked() {
                            self.set_dialog(Dialog::Tunnels);
                        }
                    });
                });
            });
    }

    /// Display actual host/project preparation and sandbox state independently of requested settings.
    pub(super) fn workspace_banner(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let Some(workspace) = &self.workspace else {
            let file = self
                .documents
                .get(self.active)
                .and_then(|d| d.path.as_deref());
            let title = file.map_or_else(
                || "RODE".into(),
                |path| format!("RODE · {} [File mode]", ssh::display_name(path)),
            );
            if self.last_window_title != title {
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
                self.last_window_title = title;
            }
            if file.is_some() {
                chrome::workspace_banner_panel(ctx, palette).show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("LOCAL FILE").strong());
                            ui.separator();
                            ui.label(RichText::new("NORMAL MODE").monospace().size(11.0))
                                .on_hover_text("No project is open. File access is not sandboxed; project sandbox preferences are unchanged.");
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Open project…").clicked() {
                                    self.open_local_folder_dialog();
                                }
                            });
                        });
                    });
            }
            return;
        };
        let remote_profile = workspace.remote.as_ref().map(|r| r.profile.clone());
        let remote = remote_profile.as_ref().map(Profile::endpoint);
        let root = workspace.root.clone();
        let connected = workspace.connected;
        let connecting = !connected && self.ssh_master.as_ref().is_some_and(Terminal::is_alive);
        let ready = workspace.ready;
        let plan = workspace.plan.clone();
        let title = format!(
            "RODE · {}{}{}",
            ssh::display_name(&root),
            remote
                .as_ref()
                .map(|r| {
                    format!(
                        " [SSH {r}{}] ",
                        if connected || connecting {
                            ""
                        } else {
                            " · offline"
                        }
                    )
                })
                .unwrap_or_default(),
            if !ready && (connecting || self.preparing) {
                " [Opening project]"
            } else if ready && plan.active {
                " [Sandbox]"
            } else {
                ""
            }
        );
        if title != self.last_window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_window_title = title;
        }
        if remote.is_none() && !plan.requested && !self.preparing {
            return;
        }
        chrome::workspace_banner_panel(ctx, palette).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if let Some(host) = &remote {
                        let state = if connected {
                            "CONNECTED"
                        } else if connecting {
                            "CONNECTING"
                        } else {
                            "OFFLINE"
                        };
                        chrome::bounded_label(
                            ui,
                            RichText::new(format!("SSH {state}  ·  {host}"))
                                .strong()
                                .color(if connected {
                                    palette.success
                                } else {
                                    palette.error
                                }),
                            230.,
                            20.,
                        )
                        .on_hover_text(format!("SSH {state} · {host}"));
                        if connected || connecting {
                            if ui
                                .add_enabled(
                                    !self.sessions_need_exit_check(),
                                    IconButton::new(Icon::Close, "Disconnect"),
                                )
                                .on_hover_text("Disconnect SSH and preserve open files. Terminals with Persist enabled keep running; other terminals end after their last view detaches.")
                                .on_disabled_hover_text("Wait for terminal startup or persistence changes to finish.")
                                .clicked()
                            {
                                self.disconnect_ssh(ctx);
                            }
                        } else if ui.add(IconButton::new(Icon::Remote, "Reconnect")).clicked() {
                            self.connect(remote_profile.clone().unwrap(), ctx, true);
                        }
                    } else {
                        ui.label(RichText::new("LOCAL PROJECT").strong());
                    }
                    ui.separator();
                    let path_width = (ui.available_width() - 410.0).clamp(60.0, 500.0);
                    chrome::bounded_label(
                        ui,
                        RichText::new(&root).monospace().size(12.0),
                        path_width,
                        20.,
                    )
                    .on_hover_text(&root);
                    if self.preparing {
                        ui.spinner();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Open project…").clicked() {
                            self.set_dialog(Dialog::Folder);
                        }
                        ui.label(
                            RichText::new(if remote.is_some() && !connected && !connecting {
                                "SSH OFFLINE"
                            } else if ready {
                                plan.label()
                            } else {
                                "PROJECT NOT READY"
                            })
                            .monospace()
                            .size(11.0)
                            .color(
                                if remote.is_some() && !connected && !connecting {
                                    palette.error
                                } else if !ready || (plan.requested && !plan.active) {
                                    palette.accent
                                } else {
                                    palette.success
                                },
                            ),
                        )
                        .on_hover_text(if plan.reason.is_empty() {
                            "Project file access and terminal commands use the displayed mode."
                        } else {
                            &plan.reason
                        });
                    });
                });
            });
        if plan.requested && !plan.active && !plan.reason.is_empty() {
            egui::TopBottomPanel::top("sandbox-fallback")
                .frame(theme::frame(palette.panel))
                .show(ctx, |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&plan.reason).size(12.0).color(palette.accent),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&plan.reason);
                });
        }
    }

    /// Render feature-gated activity actions without treating selection as tool-process activation.
    pub(super) fn activity(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        egui::SidePanel::left("activity")
            .exact_width(52.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(palette.panel)
                    .inner_margin(egui::Margin::symmetric(7, 14)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.;
                ui.spacing_mut().button_padding = egui::vec2(0., 2.);
                let commands = [
                    Command::Explorer,
                    Command::Open,
                    Command::Connect,
                    Command::Terminal,
                    Command::Panel(Panel::Git),
                    Command::Panel(Panel::Monitor),
                    Command::Panel(Panel::Problems),
                    Command::Panel(Panel::Environment),
                    Command::Panel(Panel::Tasks),
                    Command::Panel(Panel::Settings),
                    Command::Panel(Panel::Agent),
                    Command::Extensions,
                    Command::Panel(Panel::Debug),
                ];
                let button_height =
                    ((ui.available_height() - 68.) / commands.len() as f32 - 6.).clamp(22., 40.);
                for command in commands {
                    let definition = command.definition();
                    let shortcut = self.command_hint(command);
                    let hint = if shortcut.is_empty() {
                        definition.label.to_owned()
                    } else {
                        format!("{} · {shortcut}", definition.label)
                    };
                    let selected = self.command_selected(command);
                    let response = ui.add_sized(
                        [36.0, button_height],
                        IconButton::icon(definition.icon, &hint)
                            .icon_size(if button_height < 34. { 20. } else { 23. })
                            .selected(selected),
                    );
                    if selected {
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(response.rect.left() - 7., response.rect.top() + 6.),
                                egui::vec2(2., response.rect.height() - 12.),
                            ),
                            0,
                            palette.accent,
                        );
                    }
                    if response.clicked() {
                        self.execute_command(command, ctx);
                    }
                    ui.add_space(6.0);
                }
                ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                    if ui
                        .add_sized(
                            [36., 28.],
                            IconButton::icon(Icon::Help, "Keyboard shortcuts").icon_size(21.),
                        )
                        .clicked()
                    {
                        self.set_dialog(Dialog::Shortcuts);
                    }
                    ui.label(palette.tag("0.1"));
                });
            });
    }

    /// Render the selected terminal surface, keeping PTY input and native focus ownership together.
    pub(in crate::app) fn terminal_content(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.terminal_colors.palette(self.themes.active());
        self.pane_ui(ui, navigation::Pane::Terminal);
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
            let (response, error) = terminal.ui_with_response(ui, self.font_size - 1.0, palette);
            if !terminal.is_alive() {
                super::ui_probe::button(ui, "terminal-output-ended", &response);
            }
            if let Some(error) = error {
                self.notify(error, true);
            }
            if !self.session_busy()
                && self.terminal.as_ref().is_some_and(Terminal::is_alive)
                && let Some(active) = &self.sessions.active
            {
                super::ui_probe::button(ui, &format!("terminal-input:{}", active.key), &response);
                if active.native.is_none() {
                    super::ui_probe::button(ui, "temporary-terminal-input", &response);
                }
            }
        }
    }

    /// Render local/SSH project entry points when no document surface is selected.
    pub(super) fn welcome(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        let top = ((ui.available_height() - 440.0) / 2.0).max(25.0);
        ui.add_space(top);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("[ r ]")
                    .monospace()
                    .size(64.0)
                    .color(palette.accent),
            );
            ui.add_space(12.0);
            ui.label(
                RichText::new("RODE")
                    .size(34.0)
                    .strong()
                    .color(palette.text),
            );
            ui.label(
                RichText::new("Small footprint. Far reach.")
                    .size(18.0)
                    .color(palette.muted),
            );
            ui.add_space(12.0);
            ui.label(palette.tag("RUST EDITOR  /  OPENSSH NATIVE  /  REMOTE READY"));
            ui.add_space(32.0);
            let action_size = egui::vec2(ui.available_width().min(320.), 32.);
            if ui
                .add_sized(
                    action_size,
                    egui::Button::new(
                        RichText::new("Connect via SSH")
                            .color(palette.background)
                            .strong(),
                    )
                    .fill(palette.accent),
                )
                .clicked()
            {
                self.set_dialog(Dialog::Connect);
            }
            ui.add_space(8.0);
            if ui
                .add_sized(
                    action_size,
                    IconButton::new(Icon::Folder, "Open project folder").shortcut("Ctrl+Shift+O"),
                )
                .clicked()
            {
                self.set_dialog(Dialog::Folder);
            }
            if ui
                .add_sized(
                    action_size,
                    IconButton::new(Icon::NewFile, "New file").shortcut("Ctrl+N"),
                )
                .clicked()
            {
                self.new_file();
            }
            ui.add_space(28.0);
            ui.label(
                RichText::new("Your SSH config. Your keys. Your machine.")
                    .color(palette.muted)
                    .size(12.0),
            );
            ui.add_space(18.0);
            if ui
                .link(
                    RichText::new("Keyboard shortcuts")
                        .color(palette.accent)
                        .size(12.0),
                )
                .clicked()
            {
                self.set_dialog(Dialog::Shortcuts);
            }
        });
    }
}
