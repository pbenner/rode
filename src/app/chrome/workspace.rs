//! One projection of actual connection/project state for the footer and native
//! title. Rendering it never prepares a project or changes host authority.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Connection {
    Local,
    Connecting,
    Connected,
    Offline,
}

struct Summary {
    title: String,
    connection: Connection,
    host_label: String,
    project: Option<String>,
    details: String,
    mode: Option<String>,
    mode_details: String,
    warning: bool,
}
impl Summary {
    fn new(app: &EditorApp) -> Self {
        let Some(workspace) = app.workspace.as_ref() else {
            let file = app.documents.active().and_then(|doc| doc.path.as_deref());
            let file_mode = file.is_some();
            return Self {
                title: file.map_or_else(|| "RODE".into(), |path| {
                    format!("RODE · {} [File mode]", ssh::display_name(path))
                }),
                connection: Connection::Local,
                host_label: "LOCAL".into(),
                project: None,
                details: "Local computer".into(),
                mode: file_mode.then(|| "File mode".into()),
                mode_details: "No project is open. File access is not sandboxed; project sandbox preferences are unchanged.".into(),
                warning: false,
            };
        };
        let host = workspace
            .remote
            .as_ref()
            .map(|remote| remote.profile.endpoint());
        let connecting = host.is_some()
            && !workspace.connected
            && (app.workspace.preparing()
                || app.ssh_master.as_ref().is_some_and(Terminal::is_alive));
        let connection = if host.is_none() {
            Connection::Local
        } else if workspace.connected {
            Connection::Connected
        } else if connecting {
            Connection::Connecting
        } else {
            Connection::Offline
        };
        let state = match connection {
            Connection::Local => "Local project".into(),
            Connection::Connected => format!("SSH connected · {}", host.as_ref().unwrap()),
            Connection::Connecting => format!("SSH connecting · {}", host.as_ref().unwrap()),
            Connection::Offline => format!("SSH offline · {}", host.as_ref().unwrap()),
        };
        let host_label = match connection {
            Connection::Local => "LOCAL".into(),
            Connection::Connected => format!("SSH · {}", host.as_ref().unwrap()),
            Connection::Connecting => format!("SSH · {} · connecting", host.as_ref().unwrap()),
            Connection::Offline => format!("SSH · {} · offline", host.as_ref().unwrap()),
        };
        let plan = &workspace.plan;
        let unavailable = plan.requested && !plan.active;
        let mode = if connection == Connection::Offline {
            "Offline".into()
        } else if !workspace.ready {
            if connecting || app.workspace.preparing() {
                "Opening project…".into()
            } else {
                "No project".into()
            }
        } else if unavailable {
            "Normal · unavailable".into()
        } else if plan.active {
            format!(
                "{} · {}",
                if plan.project_read_only {
                    "Read only"
                } else {
                    "Sandbox"
                },
                plan.backend.label()
            )
        } else {
            "Normal".into()
        };
        let mut mode_details = format!("{state}\nProject: {}", workspace.root);
        if connection == Connection::Offline {
            mode_details.push_str("\nProject file access is unavailable while SSH is offline.");
        } else if !workspace.ready {
            mode_details.push_str("\nOpen a project folder and wait for preparation to finish.");
        } else {
            mode_details.push_str(&format!("\n{}", plan.label()));
        }
        if !plan.reason.is_empty() {
            mode_details.push_str(&format!("\n{}", plan.reason));
        }
        let title = format!(
            "RODE · {}{}{}",
            ssh::display_name(&workspace.root),
            host.as_ref()
                .map(|host| format!(
                    " [SSH {host}{}] ",
                    if connection == Connection::Offline {
                        " · offline"
                    } else {
                        ""
                    }
                ))
                .unwrap_or_default(),
            if !workspace.ready && (connecting || app.workspace.preparing()) {
                " [Opening project]"
            } else if workspace.ready && plan.active && connection != Connection::Offline {
                " [Sandbox]"
            } else {
                ""
            }
        );
        Self {
            title,
            connection,
            host_label,
            project: (!workspace.root.is_empty()).then(|| workspace.root.clone()),
            details: format!("{state}\nProject: {}", workspace.root),
            mode: Some(mode),
            mode_details,
            warning: connection == Connection::Offline || !workspace.ready || unavailable,
        }
    }
}

/// Allocate a bounded, clipped 19-point footer slot; neither label length nor
/// themed button padding may enlarge it into adjacent status or editor content.
fn slot<R>(ui: &mut egui::Ui, width: f32, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 19.),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, 19.));
            ui.set_clip_rect(ui.clip_rect().intersect(ui.max_rect()));
            contents(ui)
        },
    )
    .inner
}

impl EditorApp {
    /// Title metadata is independent of chrome geometry and the selected area.
    pub(in crate::app) fn update_window_title(&mut self, ctx: &egui::Context) {
        let title = Summary::new(self).title;
        if title != self.last_window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_window_title = title;
        }
    }

    pub(super) fn workspace_status_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        width: f32,
    ) {
        let summary = Summary::new(self);
        let palette = self.themes.active().palette;
        let color = match summary.connection {
            Connection::Local => palette.muted,
            Connection::Connected => palette.success,
            Connection::Offline => palette.error,
            Connection::Connecting => palette.accent,
        };
        let mode_color = if summary.warning {
            palette.accent
        } else {
            palette.muted
        };
        let path = summary.project.as_deref().unwrap_or("No project folder");
        let natural_width = |text: RichText, framed: bool| {
            egui::WidgetText::from(text.size(12.))
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Button,
                )
                .size()
                .x
                + if framed {
                    2. * ui.spacing().button_padding.x
                } else {
                    0.
                }
                + 1.
        };
        // Give host identity and access state their own bounded slots. Long
        // project paths use the remainder; transient messages yield space first.
        let gaps = if summary.mode.is_some() { 3. } else { 2. };
        let available = (width - gaps * ui.spacing().item_spacing.x).max(0.);
        let host_width = natural_width(RichText::new(&summary.host_label).strong(), true)
            .min(280.)
            .min(available * 0.45);
        let mode_width = summary.mode.as_ref().map_or(0., |mode| {
            natural_width(RichText::new(mode), false)
                .min(170.)
                .min(available * 0.3)
        });
        let path_width = natural_width(RichText::new(path), false)
            .min((available - host_width - mode_width).max(0.));
        let enabled = !self.foreign_modal(ctx);
        slot(ui, host_width, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                let response = footer_menu(
                    RichText::new(&summary.host_label).strong().color(color),
                    "Connection status",
                    Some(color.gamma_multiply(0.12)),
                )
                .ui(ui, |ui| {
                    menus::contents(ui, 340., |ui| {
                        self.connection_status_menu(ui, ctx, &summary)
                    })
                })
                .0
                .on_hover_text(&summary.details);
                ui_probe::button(ui, "status-connection", &response);
                ui_probe::value(
                    ui,
                    "status-connection-value",
                    &response,
                    &summary.host_label,
                );
            });
        });
        slot(ui, path_width, |ui| {
            ui.add_enabled_ui(enabled, |ui| {
                let response = footer_menu(
                    RichText::new(path).color(palette.text),
                    "Project folder",
                    None,
                )
                .ui(ui, |ui| {
                    menus::contents(ui, 380., |ui| self.project_status_menu(ui, ctx, &summary))
                })
                .0
                .on_hover_text(path);
                ui_probe::button(ui, "status-project", &response);
                ui_probe::value(ui, "status-project-value", &response, path);
            });
        });
        if let Some(mode) = &summary.mode {
            slot(ui, mode_width, |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    let response = footer_menu(
                        RichText::new(mode).color(mode_color),
                        "Project access mode",
                        None,
                    )
                    .ui(ui, |ui| {
                        menus::contents(ui, 380., |ui| {
                            ui.label(palette.tag("PROJECT ACCESS"));
                            ui.strong(mode);
                            ui.separator();
                            ui.add(
                                egui::Label::new(&summary.mode_details)
                                    .wrap()
                                    .selectable(true),
                            );
                        })
                    })
                    .0
                    .on_hover_text(&summary.mode_details);
                    ui_probe::button(ui, "status-access", &response);
                    ui_probe::value(ui, "status-access-value", &response, mode);
                });
            });
        }
        let remaining = ui.available_width().max(0.);
        ui.add_sized(
            [remaining, 19.],
            egui::Label::new(RichText::new(&self.status).size(11.).color(palette.muted))
                .truncate()
                .halign(Align::Min),
        )
        .on_hover_text(&self.status);
    }

    fn connection_status_menu(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        summary: &Summary,
    ) {
        let palette = self.themes.active().palette;
        ui.label(palette.tag("CONNECTION"));
        ui.add(
            egui::Label::new(RichText::new(&summary.host_label).strong())
                .wrap()
                .selectable(true),
        );
        ui.separator();
        let remote = summary.connection != Connection::Local;
        if remote || self.workspace.reconnect_profile().is_some() {
            let (label, name, command, icon, enabled) =
                if matches!(summary.connection, Connection::Local | Connection::Offline) {
                    (
                        "Reconnect SSH",
                        "status-reconnect",
                        Command::Reconnect,
                        Icon::Refresh,
                        true,
                    )
                } else {
                    (
                        "Disconnect SSH",
                        "status-disconnect",
                        Command::Disconnect,
                        Icon::Exit,
                        !self.runtime_work_pending(),
                    )
                };
            let button = ui.add_enabled(enabled, IconButton::new(icon, label));
            ui_probe::button(ui, name, &button);
            if button.clicked() {
                keyboard::consume_activation(&button);
                self.execute_command(command, ctx);
                ui.close();
            }
        }
        if ui
            .add(IconButton::new(Icon::Remote, "Connect via SSH…"))
            .clicked()
        {
            self.set_dialog(Dialog::Connect);
            ui.close();
        }
    }

    /// Bound the path preview separately from its actions, so a long path can
    /// scroll without displacing Copy path or Open project off the popup.
    fn project_status_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, summary: &Summary) {
        let palette = self.themes.active().palette;
        ui.label(palette.tag("PROJECT FOLDER"));
        ui.label(palette.tag(&summary.host_label));
        if let Some(path) = &summary.project {
            egui::ScrollArea::vertical()
                .id_salt("project-folder-preview")
                .max_height((ctx.content_rect().height() - 260.).clamp(24., 140.))
                .show(ui, |ui| {
                    ui.add(
                        egui::Label::new(RichText::new(path).monospace())
                            .wrap()
                            .selectable(true),
                    );
                });
            ui.separator();
            let copy = ui.add(IconButton::new(Icon::Copy, "Copy path"));
            ui_probe::button(ui, "status-copy-project", &copy);
            if copy.clicked() {
                ctx.copy_text(path.clone());
                ui.close();
            }
        } else {
            ui.weak("Choose a project folder on this host.");
            ui.separator();
        }
        let open = ui.add(IconButton::new(Icon::Folder, "Open project…"));
        ui_probe::button(ui, "status-open-project", &open);
        if open.clicked() {
            keyboard::consume_activation(&open);
            self.execute_command(Command::Folder, ctx);
            ui.close();
        }
    }
}

/// Footer anchors retain their 19-point geometry; popup rows reuse the normal
/// menu spacing and viewport constraints instead of inheriting footer sizing.
fn footer_menu(
    label: RichText,
    name: &str,
    fill: Option<egui::Color32>,
) -> keyboard::MenuButton<'static> {
    let mut button = egui::Button::new(label.size(12.))
        .small()
        .truncate()
        .min_size(egui::vec2(0., 19.));
    if let Some(fill) = fill {
        button = button.fill(fill).corner_radius(3);
    }
    keyboard::MenuButton::from_button(button, name)
        .config(egui::containers::menu::MenuConfig::new().style(menus::style))
        .frame(fill.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_popup_bounds_long_paths_keeps_actions_visible_and_copies_the_full_path() {
        for (size, scale) in [(egui::vec2(850., 560.), 1.), (egui::vec2(480., 360.), 1.5)] {
            let ctx = egui::Context::default();
            let mut app = EditorApp::new(
                &eframe::CreationContext::_new_kittest(ctx.clone()),
                Startup::default(),
            );
            ctx.set_zoom_factor(scale);
            ctx.style_mut(|style| style.animation_time = 0.);
            app.workspace
                .set_fixture(Some(crate::app::tests::remote_workspace()));
            let folder = format!(
                "/scratch/{}研究 project",
                "a-long-project-parent/".repeat(40)
            );
            app.workspace.as_mut().unwrap().root = folder.clone();
            let directory = tempfile::tempdir().unwrap();
            let probe = directory.path().join("popup.json");
            ui_probe::install_at(&ctx, probe.clone());
            let frame = |app: &mut EditorApp, events| {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        app.titlebar(ctx);
                        app.statusbar(ctx);
                    },
                )
            };
            let rect = |name: &str| {
                let state: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&probe).unwrap()).unwrap();
                let control = state["buttons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|b| b["name"] == name)
                    .unwrap_or_else(|| panic!("Missing {name}: {state}"));
                let [x1, y1, x2, y2]: [f32; 4] =
                    serde_json::from_value(control["rect"].clone()).unwrap();
                egui::Rect::from_min_max(egui::pos2(x1, y1), egui::pos2(x2, y2))
            };
            for _ in 0..3 {
                frame(&mut app, vec![]);
            }
            let events = |point, pressed| {
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ]
            };
            let point = rect("status-project").center();
            frame(&mut app, events(point, true));
            frame(&mut app, events(point, false));
            for _ in 0..3 {
                frame(&mut app, vec![]);
            }
            let copy = rect("status-copy-project");
            let open = rect("status-open-project");
            for control in [copy, open] {
                assert!(
                    ctx.content_rect().contains_rect(control),
                    "{size}/{scale}: {control:?}"
                );
                assert!(
                    control.height() >= 28.,
                    "Clipped action at {size}/{scale}: {control:?}"
                );
            }
            assert!(!copy.intersects(open));
            frame(&mut app, events(copy.center(), true));
            let output = frame(&mut app, events(copy.center(), false));
            assert!(
                output
                    .platform_output
                    .commands
                    .iter()
                    .any(|c| matches!(c, egui::OutputCommand::CopyText(text) if text == &folder))
            );
        }
    }

    #[test]
    fn status_reports_actual_access_and_transport_without_changing_authority() {
        let ctx = egui::Context::default();
        let mut app = EditorApp::new(
            &eframe::CreationContext::_new_kittest(ctx),
            Startup::default(),
        );
        let summary = Summary::new(&app);
        assert_eq!(summary.connection, Connection::Local);
        assert!(summary.mode.is_none());
        app.new_file();
        // Unbound recovery buffers must not claim ordinary local file access.
        assert!(Summary::new(&app).mode.is_none());
        app.documents[0].fixture_content().path = Some("/tmp/example.rs".into());
        let summary = Summary::new(&app);
        assert_eq!(summary.mode.as_deref(), Some("File mode"));
        assert_eq!(summary.title, "RODE · example.rs [File mode]");

        app.workspace
            .set_fixture(Some(crate::app::tests::remote_workspace()));
        let summary = Summary::new(&app);
        assert_eq!(summary.connection, Connection::Connected);
        assert_eq!(summary.host_label, "SSH · researcher@test-host");
        assert_eq!(
            summary.project.as_deref(),
            Some("/scratch/researcher/project")
        );
        assert!(
            summary
                .details
                .contains("SSH connected · researcher@test-host")
        );
        assert_eq!(summary.mode.as_deref(), Some("Normal"));
        assert!(!summary.warning);
        {
            let plan = &mut app.workspace.as_mut().unwrap().plan;
            plan.requested = true;
            plan.reason = "User namespaces are unavailable".into();
        }
        let summary = Summary::new(&app);
        assert_eq!(summary.mode.as_deref(), Some("Normal · unavailable"));
        assert!(summary.warning);
        assert!(
            summary
                .mode_details
                .contains("User namespaces are unavailable")
        );
        assert!(!summary.title.contains("[Sandbox]"));
        {
            let plan = &mut app.workspace.as_mut().unwrap().plan;
            plan.active = true;
            plan.project_read_only = true;
            plan.backend = sandbox::Backend::Landlock;
            plan.reason.clear();
        }
        let summary = Summary::new(&app);
        assert_eq!(summary.mode.as_deref(), Some("Read only · Landlock"));
        assert!(!summary.warning);
        assert!(summary.title.contains("[Sandbox]"));

        app.workspace.as_mut().unwrap().connected = false;
        let summary = Summary::new(&app);
        assert_eq!(summary.connection, Connection::Offline);
        assert!(
            summary
                .host_label
                .contains("SSH · researcher@test-host · offline")
        );
        assert_eq!(
            summary.project.as_deref(),
            Some("/scratch/researcher/project")
        );
        assert_eq!(summary.mode.as_deref(), Some("Offline"));
        assert!(summary.warning);
        assert!(summary.title.contains("offline"));
        assert!(!summary.title.contains("[Sandbox]"));
        assert!(!summary.mode_details.contains("SANDBOX"));
        app.workspace.as_mut().unwrap().ready = false;
        app.workspace.begin_preparation();
        let summary = Summary::new(&app);
        assert_eq!(summary.connection, Connection::Connecting);
        assert_eq!(
            summary.project.as_deref(),
            Some("/scratch/researcher/project")
        );
        assert_eq!(summary.mode.as_deref(), Some("Opening project…"));
        assert!(summary.title.contains("[Opening project]"));
        assert!(app.workspace.preparing());

        let mut local = crate::app::tests::remote_workspace();
        local.remote = None;
        local.root = "/home/researcher/local-project".into();
        app.workspace.set_fixture(Some(local));
        let summary = Summary::new(&app);
        assert_eq!(summary.connection, Connection::Local);
        assert_eq!(
            summary.project.as_deref(),
            Some("/home/researcher/local-project")
        );
        assert_eq!(summary.mode.as_deref(), Some("Normal"));
        assert!(summary.details.contains("Local project"));
        assert!(!summary.title.contains("SSH"));
    }
}
