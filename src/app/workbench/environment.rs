//! Host/project environment controls and language-specific task entry points.
use super::*;
use crate::environment::{self, Settings};

#[derive(Default)]
/// Project-keyed environment edit draft and cached runtime probes; separate from
/// the settings already applied to new tools and terminals.
pub(super) struct State {
    key: Option<String>,
    draft: Settings,
    variables: String,
    pub(super) runtimes: Vec<project::Runtime>,
    pub(super) busy: bool,
    pub(super) error: String,
    open_python: bool,
    pub(super) mise: Option<crate::mise::Snapshot>,
    pub(super) mise_busy: bool,
    pub(super) mise_error: String,
}

impl EditorApp {
    /// Copy only applied opt-in settings into future launch plans; running sessions are untouched.
    pub(in crate::app) fn sync_project_environment(&mut self) {
        let mut settings = self.workbench.prefs.environment.clone();
        settings.mise &= self.feature(crate::extensions::Feature::Mise);
        if let Some(workspace) = &mut self.workspace {
            workspace.plan.environment = if settings.mise {
                settings
            } else {
                Default::default()
            };
        }
    }

    pub(in crate::app) fn request_mise(
        &mut self,
        action: crate::mise::Action,
        ctx: &egui::Context,
    ) {
        if self.workbench.environment.mise_busy || !self.feature(crate::extensions::Feature::Mise) {
            return;
        }
        self.workbench.environment.mise_busy = true;
        self.dispatch_tool(
            project::Request::Mise {
                root: self.root(),
                settings: self.workbench.prefs.environment.clone(),
                action,
            },
            ToolJob::Mise {
                action,
                revision: self.extensions.registry.revision,
                settings: self.workbench.prefs.environment.clone(),
            },
            ctx,
        );
    }

    /// Native surface of the optional bundled extension; metadata excludes resolved secrets.
    fn mise_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, tasks: bool) {
        if !self.feature(crate::extensions::Feature::Mise) {
            return;
        }
        let palette = self.themes.active().palette;
        layout::section(ui, "mise", palette, |ui| {
            ui.small("Uses mise installed on this project host, inside the current sandbox. Tool installation and configuration trust are explicit actions.");
            if !tasks {
                let state = &mut self.workbench.environment;
                ui.checkbox(&mut state.draft.mise, "Use mise for new project processes");
                ui.label("mise executable (blank: host PATH or ~/.local/bin/mise)");
                ui.add(
                    egui::TextEdit::singleline(
                        state.draft.runtimes.entry("mise".into()).or_default(),
                    )
                    .hint_text("mise or /path/to/mise"),
                );
                ui.small("Apply environment saves this choice. Explicit Rode variables and interpreter paths take precedence. Existing sessions keep their environment.");
            }
            let busy = self.workbench.environment.mise_busy;
            let mut action = None;
            ui.horizontal_wrapped(|ui| {
                for (label, operation) in [
                    ("Refresh mise tools and tasks", crate::mise::Action::Inspect),
                    ("Trust project configuration", crate::mise::Action::Trust),
                    ("Install declared tools", crate::mise::Action::Install),
                ] {
                    if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                        action = Some(operation);
                    }
                }
                if busy {
                    ui.spinner();
                }
            });
            if let Some(action) = action {
                self.request_mise(action, ctx);
            }
            if !self.workbench.environment.mise_error.is_empty() {
                ui.colored_label(
                    self.themes.active().palette.error,
                    &self.workbench.environment.mise_error,
                );
            }
            let snapshot = self.workbench.environment.mise.clone();
            if let Some(snapshot) = snapshot {
                if !snapshot.version.is_empty() {
                    ui.label(&snapshot.version);
                }
                if !snapshot.tools.is_empty() {
                    ui.label(format!("Project tools: {}", snapshot.tools.join(", ")));
                }
                if !snapshot.output.is_empty() {
                    ui.label(&snapshot.output);
                }
                if tasks {
                    for task in snapshot.tasks {
                        ui.horizontal_wrapped(|ui| {
                        if ui.button(&task.name).clicked() {
                            let program = crate::mise::shell_program(&self.workbench.prefs.environment);
                            let command = format!("MISE_AUTO_INSTALL=0 MISE_EXEC_AUTO_INSTALL=0 MISE_PARANOID=1 {} -C {} run {}", program, crate::ssh::quote(&self.root()), crate::ssh::quote(&task.name));
                            self.send_project_command(command, ctx);
                        }
                        ui.label(&task.description);
                    });
                    }
                }
            }
        });
    }
    /// Reveal the interpreter section in Environment, refreshing discovery when it is already open.
    pub(in crate::app) fn open_python_environment(&mut self, ctx: &egui::Context) {
        self.workbench.environment.open_python = true;
        self.reveal_panel(Panel::Environment, ctx);
        self.refresh_environment(false, ctx);
    }

    /// Queue at most one runtime-version probe using captured applied environment settings.
    pub(in crate::app) fn refresh_runtimes(&mut self, ctx: &egui::Context) {
        if self.workbench.environment.busy || !self.project_ready() {
            return;
        }
        self.workbench.environment.busy = true;
        let settings = self.workbench.prefs.environment.clone();
        self.dispatch_tool(
            project::Request::Runtimes {
                root: self.root(),
                settings: settings.clone(),
            },
            ToolJob::Runtimes { settings },
            ctx,
        );
    }

    /// Edit literal project variables/runtime overrides and apply them after validation;
    /// existing terminal processes retain their original environment.
    pub(in crate::app) fn environment_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let key = self.workbench.key.clone();
        let state = &mut self.workbench.environment;
        if state.key.as_ref() != Some(&key) {
            state.key = Some(key);
            state.draft = self.workbench.prefs.environment.clone();
            state.variables = state
                .draft
                .variables
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("\n");
        }
        layout::heading(
            ui,
            "Project environment",
            "Runtime paths and variables for new terminals and tasks on this host",
            palette,
        );
        let refresh = layout::section(ui, "Runtimes and toolchains", palette, |ui| {
            ui.small("Leave an override blank to use the host's PATH and existing project toolchain configuration.");
            let compact = ui.available_width() < 520.;
            let executable_width = if compact {
                ui.available_width()
            } else {
                (ui.available_width() * 0.4).clamp(80., 240.)
            };
            let version_width = (ui.available_width() - executable_width - 168.).max(60.);
            let fields_id = ui.make_persistent_id("runtime-executables");
            let mut fields = |ui: &mut egui::Ui| {
                if !compact {
                    ui.strong("Tool");
                    ui.strong("Executable override");
                    ui.strong("Version");
                    ui.end_row();
                }
                for (program, label) in environment::TOOLS {
                    layout::field_label(ui, label, compact);
                    ui.add(
                        egui::TextEdit::singleline(
                            state.draft.runtimes.entry((*program).into()).or_default(),
                        )
                        .id(fields_id.with(program))
                        .desired_width(executable_width)
                        .min_size(egui::vec2(executable_width, 20.))
                        .char_limit(4096)
                        .hint_text(format!("auto ({program}) or /path/to/{program}")),
                    );
                    if compact {
                        ui.end_row();
                    }
                    let info = state.runtimes.iter().find(|info| info.name == *program);
                    let status = info
                        .map(|info| {
                            if info.error.is_empty() {
                                info.version.as_str()
                            } else {
                                "Unavailable"
                            }
                        })
                        .unwrap_or("Not checked");
                    let detail = info
                        .map(|info| {
                            format!(
                                "{}\n{}",
                                info.executable,
                                if info.error.is_empty() {
                                    &info.version
                                } else {
                                    &info.error
                                }
                            )
                        })
                        .unwrap_or_default();
                    ui.allocate_ui_with_layout(
                        egui::vec2(
                            if compact {
                                ui.available_width()
                            } else {
                                version_width
                            },
                            20.,
                        ),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.add(egui::Label::new(status).truncate())
                                .on_hover_text(detail);
                        },
                    );
                    ui.end_row();
                }
            };
            if compact {
                // A one-column grid can retain its previous width when the
                // scrollbar appears or the panel shrinks. Stack fields directly
                // so every control follows the currently visible width.
                fields(ui);
            } else {
                egui::Grid::new("runtime-executables")
                    .num_columns(3)
                    .spacing([12., 8.])
                    .striped(true)
                    .show(ui, fields);
            }
            let mut refresh = false;
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!state.busy, egui::Button::new("Refresh runtime versions"))
                    .on_hover_text("Probe applied settings on the project host, inside its sandbox")
                    .clicked()
                {
                    refresh = true;
                }
                if state.busy {
                    ui.spinner();
                }
            });
            refresh
        });
        self.mise_ui(ui, ctx, false);
        let mut refresh = refresh;
        layout::section(ui, "Project variables", palette, |ui| {
            let state = &mut self.workbench.environment;
            ui.small(
            "One NAME=value per line. Values are literal and saved in local project preferences.",
        );
            ui.add(
                egui::TextEdit::multiline(&mut state.variables)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .char_limit(65536)
                    .hint_text("APP_MODE=development\nDATA_DIR=/scratch/data"),
            );
            if ui.button("Apply environment").clicked() {
                let result = (|| -> anyhow::Result<Settings> {
                    let mut settings = state.draft.clone();
                    settings
                        .runtimes
                        .retain(|_, value| !value.trim().is_empty());
                    settings.variables = Settings::parse_variables(&state.variables)?;
                    settings.validate()?;
                    Ok(settings)
                })();
                match result {
                    Ok(settings) => {
                        self.workbench.prefs.environment = settings.clone();
                        self.workbench.draft.environment = settings;
                        self.workbench.save_preferences();
                        self.workbench.environment.error.clear();
                        self.status =
                            "Project environment saved · applies to new terminals and tasks".into();
                        refresh = true;
                    }
                    Err(error) => self.workbench.environment.error = error.to_string(),
                }
            }
        });
        if !self.workbench.environment.error.is_empty() {
            ui.colored_label(
                self.themes.active().palette.error,
                &self.workbench.environment.error,
            );
        }
        if refresh {
            self.sync_project_environment();
            self.refresh_runtimes(ctx);
        }
        if self.feature(crate::extensions::Feature::Python) {
            layout::card(ui, palette, |ui| {
                let open = std::mem::take(&mut self.workbench.environment.open_python);
                let active_python = self
                    .documents
                    .get(self.active)
                    .and_then(|doc| crate::languages::detect(doc.path.as_deref()))
                    .is_some_and(|language| language.id == "python");
                // CollapsingHeader does not wrap its label.
                let title = if ui.available_width() < 520. {
                    "Python environments"
                } else {
                    "Python interpreter and virtual environments"
                };
                let section = egui::CollapsingHeader::new(title)
                    .id_salt("environment-python")
                    .default_open(active_python)
                    .open(open.then_some(true))
                    .show(ui, |ui| self.python_ui(ui, ctx));
                if open {
                    ui.scroll_to_rect(section.header_response.rect, Some(Align::TOP));
                }
            });
        }
    }

    /// Render explicit run/build/test entry points for the active project and language.
    pub(in crate::app) fn tasks_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        layout::heading(
            ui,
            "Project tasks",
            "Run, build and test on this host using the saved Environment settings",
            palette,
        );
        self.mise_ui(ui, ctx, true);
        layout::section(ui, "Run and build", palette, |ui| {
            let language = self
                .documents
                .get(self.active)
                .and_then(|doc| crate::languages::detect(doc.path.as_deref()));
            let language_tools = self.feature(crate::extensions::Feature::Languages);
            if !language_tools {
                ui.label(
                    "Enable Language tools in Extensions to run, build or test source projects.",
                );
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        language_tools && language.is_some_and(|l| l.run.is_some()),
                        egui::Button::new("Run current file / project"),
                    )
                    .clicked()
                {
                    self.run_active(false, ctx);
                }
                if ui
                    .add_enabled(
                        language_tools && language.is_some_and(|l| l.build.is_some()),
                        egui::Button::new("Build project"),
                    )
                    .clicked()
                {
                    self.run_active(true, ctx);
                }
            });
            if let Some(language) = language {
                ui.label(format!("Active language: {}", language.id));
                let test = match language.id.as_str() {
                    "rust" => Some(("cargo", vec!["test"])),
                    "go" => Some(("go", vec!["test", "./..."])),
                    "javascript" | "typescript" | "jsx" | "tsx" => Some(("npm", vec!["test"])),
                    _ => None,
                };
                if let Some((program, arguments)) = test
                    && ui
                        .add_enabled(language_tools, egui::Button::new("Run project tests"))
                        .clicked()
                {
                    if self.documents.iter().any(Document::dirty) {
                        self.notify("Save modified files before running tests.", true);
                    } else {
                        let command =
                            std::iter::once(self.workbench.prefs.environment.program(program))
                                .chain(arguments.into_iter().map(str::to_owned))
                                .map(|arg| ssh::quote(&arg))
                                .collect::<Vec<_>>()
                                .join(" ");
                        self.send_project_command(
                            format!("cd -- {} && {command}", ssh::quote(&self.root())),
                            ctx,
                        );
                    }
                }
            } else {
                layout::empty(
                    ui,
                    "Select a source file",
                    "Open a source file to select its run, build and test commands.",
                    palette,
                );
            }
        });
        if self.feature(crate::extensions::Feature::Python) {
            let language = self
                .documents
                .get(self.active)
                .and_then(|doc| crate::languages::detect(doc.path.as_deref()));
            layout::card(ui, palette, |ui| {
                egui::CollapsingHeader::new("Python tasks")
                    .default_open(language.is_some_and(|l| l.id == "python"))
                    .show(ui, |ui| self.python_tasks_ui(ui, ctx));
            });
            ui.add_space(12.);
        }
        if ui.button("Open project terminal").clicked() {
            self.focus_pane(navigation::Pane::Terminal, ctx);
        }
        ui.small("Tasks may create build artifacts or download dependencies according to their tools. Save files first and use an idle terminal.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_environment_survives_disabling_python_and_old_preferences_migrate() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        app.set_extension(crate::extensions::Feature::Python.id(), false);
        assert!(app.panel_enabled(Panel::Environment));
        assert!(app.panel_enabled(Panel::Tasks));
        let mut prefs: Preferences = serde_json::from_str(r#"{"python":".venv"}"#).unwrap();
        assert_eq!(prefs.python, ".venv");
        assert_eq!(prefs.environment, Settings::default());
        prefs
            .environment
            .variables
            .insert("MODE".into(), "development".into());
        let restored: Preferences =
            serde_json::from_str(&serde_json::to_string(&prefs).unwrap()).unwrap();
        assert_eq!(restored.environment.variables["MODE"], "development");
        assert_eq!(restored.python, ".venv");
    }
}
