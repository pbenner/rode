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
    label: String,
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
                label: "Local".into(),
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
        let label = match connection {
            Connection::Local => "Local".into(),
            Connection::Connected => format!("SSH · {}", host.as_ref().unwrap()),
            Connection::Connecting => "SSH · connecting…".into(),
            Connection::Offline => format!("SSH · {} · offline", host.as_ref().unwrap()),
        };
        let state = match connection {
            Connection::Local => "Local project".into(),
            Connection::Connected => format!("SSH connected · {}", host.as_ref().unwrap()),
            Connection::Connecting => format!("SSH connecting · {}", host.as_ref().unwrap()),
            Connection::Offline => format!("SSH offline · {}", host.as_ref().unwrap()),
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
            label,
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
            Connection::Offline => palette.error,
            Connection::Connecting => palette.accent,
            _ => palette.success,
        };
        let mode_color = if summary.warning {
            palette.accent
        } else {
            palette.muted
        };
        let natural_width = |text: &str| {
            ui.painter()
                .layout_no_wrap(text.into(), FontId::proportional(12.), color)
                .size()
                .x
                + 8.
        };
        let host_width = natural_width(&summary.label).min(190.).min(width * 0.45);
        let mode_width = summary
            .mode
            .as_ref()
            .map_or(0., |mode| natural_width(mode).min(170.).min(width * 0.35));
        slot(ui, host_width, |ui| {
            ui.add_enabled_ui(!self.foreign_modal(ctx), |ui| {
                let response = crate::keyboard::MenuButton::from_button(
                    egui::Button::new(RichText::new(&summary.label).size(12.).color(color))
                        .small()
                        .frame(false)
                        .truncate()
                        .min_size(egui::vec2(0., 19.)),
                    "Connection status",
                )
                // The wrapper applies its stroke with Button::stroke, which
                // restores a frame. Apply this after wrapping so themed padding
                // cannot consume the label's measured width.
                .frame(false)
                .ui(ui, |ui| {
                    ui.set_max_width((ctx.content_rect().width() - 64.).clamp(120., 460.));
                    ui.add(egui::Label::new(&summary.details).wrap());
                    ui.separator();
                    let remote = summary.connection != Connection::Local;
                    if remote || self.workspace.reconnect_profile().is_some() {
                        let (label, name, command, enabled) = if matches!(
                            summary.connection,
                            Connection::Local | Connection::Offline
                        ) {
                            ("Reconnect", "status-reconnect", Command::Reconnect, true)
                        } else {
                            (
                                "Disconnect",
                                "status-disconnect",
                                Command::Disconnect,
                                !self.runtime_work_pending(),
                            )
                        };
                        let button = ui.add_enabled(enabled, crate::keyboard::Button::new(label));
                        ui_probe::button(ui, name, &button);
                        if button.clicked() {
                            keyboard::consume_activation(&button);
                            self.execute_command(command, ctx);
                            ui.close();
                        }
                    }
                    if crate::keyboard::button(ui, "Connect via SSH…").clicked() {
                        self.set_dialog(Dialog::Connect);
                        ui.close();
                    }
                })
                .0
                .on_hover_text(&summary.details);
                ui_probe::button(ui, "status-connection", &response);
                ui_probe::value(ui, "status-connection-value", &response, &summary.label);
            });
        });
        if let Some(mode) = &summary.mode {
            slot(ui, mode_width, |ui| {
                let response = crate::keyboard::MenuButton::from_button(
                    egui::Button::new(RichText::new(mode).size(12.).color(mode_color))
                        .small()
                        .frame(false)
                        .truncate()
                        .min_size(egui::vec2(0., 19.)),
                    "Project access mode",
                )
                // The wrapper applies its stroke with Button::stroke, which
                // restores a frame. Apply this after wrapping so themed padding
                // cannot consume the label's measured width.
                .frame(false)
                .ui(ui, |ui| {
                    ui.set_max_width((ctx.content_rect().width() - 64.).clamp(120., 460.));
                    ui.add(egui::Label::new(&summary.mode_details).wrap());
                })
                .0
                .on_hover_text(&summary.mode_details);
                ui_probe::button(ui, "status-access", &response);
                ui_probe::value(ui, "status-access-value", &response, mode);
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(summary.mode.as_deref(), Some("Offline"));
        assert!(summary.warning);
        assert!(summary.title.contains("offline"));
        assert!(!summary.title.contains("[Sandbox]"));
        assert!(!summary.mode_details.contains("SANDBOX"));
        app.workspace.as_mut().unwrap().ready = false;
        app.workspace.begin_preparation();
        let summary = Summary::new(&app);
        assert_eq!(summary.connection, Connection::Connecting);
        assert_eq!(summary.mode.as_deref(), Some("Opening project…"));
        assert!(summary.title.contains("[Opening project]"));
        assert!(app.workspace.preparing());

        let mut local = crate::app::tests::remote_workspace();
        local.remote = None;
        app.workspace.set_fixture(Some(local));
        let summary = Summary::new(&app);
        assert_eq!(summary.connection, Connection::Local);
        assert_eq!(summary.mode.as_deref(), Some("Normal"));
        assert!(summary.details.contains("Local project"));
        assert!(!summary.title.contains("SSH"));
    }
}
