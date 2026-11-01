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
}

impl EditorApp {
    /// Reveal the interpreter section in Environment, refreshing discovery when it is already open.
    pub(in crate::app) fn open_python_environment(&mut self, ctx: &egui::Context) {
        self.workbench.environment.open_python = true;
        if self.workbench.panel == Some(Panel::Environment) {
            self.focus_pane(navigation::Pane::Tools, ctx);
            self.refresh_environment(false, ctx);
        } else {
            self.toggle_panel(Panel::Environment, ctx);
        }
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
        ui.heading("Project environment");
        ui.label(
            "Runtime paths and variables are saved for this project on its local or SSH host.",
        );
        ui.label("New terminals and task commands use these settings. Existing and reattached terminals keep their environment.");
        ui.separator();
        ui.heading("Runtimes and toolchains");
        ui.small("Leave an override blank to use the host's PATH and existing project toolchain configuration.");
        let executable_width = (ui.available_width() * 0.4).clamp(80., 240.);
        let version_width = (ui.available_width() - executable_width - 130.).max(60.);
        egui::Grid::new("runtime-executables")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                ui.strong("Tool");
                ui.strong("Executable override");
                ui.strong("Version");
                ui.end_row();
                for (program, label) in environment::TOOLS {
                    ui.label(*label);
                    ui.add(
                        egui::TextEdit::singleline(
                            state.draft.runtimes.entry((*program).into()).or_default(),
                        )
                        .desired_width(executable_width)
                        .min_size(egui::vec2(executable_width, 20.))
                        .char_limit(4096)
                        .hint_text(format!("auto ({program}) or /path/to/{program}")),
                    );
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
                    ui.add_sized(
                        [version_width, 20.],
                        egui::Label::new(status).truncate().halign(Align::LEFT),
                    )
                    .on_hover_text(detail);
                    ui.end_row();
                }
            });
        let mut refresh = false;
        ui.horizontal(|ui| {
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
        ui.separator();
        ui.heading("Project variables");
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
        if !self.workbench.environment.error.is_empty() {
            ui.colored_label(
                self.themes.active().palette.error,
                &self.workbench.environment.error,
            );
        }
        if refresh {
            self.refresh_runtimes(ctx);
        }
        ui.separator();
        if self.feature(crate::extensions::Feature::Python) {
            let open = std::mem::take(&mut self.workbench.environment.open_python);
            let active_python = self
                .documents
                .get(self.active)
                .and_then(|doc| crate::languages::detect(doc.path.as_deref()))
                .is_some_and(|language| language.id == "python");
            let section =
                egui::CollapsingHeader::new("Python interpreter and virtual environments")
                    .id_salt("environment-python")
                    .default_open(active_python)
                    .open(open.then_some(true))
                    .show(ui, |ui| self.python_ui(ui, ctx));
            if open {
                ui.scroll_to_rect(section.header_response.rect, Some(Align::TOP));
            }
        }
    }

    /// Render explicit run/build/test entry points for the active project and language.
    pub(in crate::app) fn tasks_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Project tasks");
        ui.label("Commands run in the project terminal on this host, with the saved Environment settings.");
        let language = self
            .documents
            .get(self.active)
            .and_then(|doc| crate::languages::detect(doc.path.as_deref()));
        let language_tools = self.feature(crate::extensions::Feature::Languages);
        if !language_tools {
            ui.label("Enable Language tools in Extensions to run, build or test source projects.");
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
            ui.label("Open a source file to select its run, build and test commands.");
        }
        if self.feature(crate::extensions::Feature::Python) {
            egui::CollapsingHeader::new("Python tasks")
                .default_open(language.is_some_and(|l| l.id == "python"))
                .show(ui, |ui| self.python_tasks_ui(ui, ctx));
        }
        ui.separator();
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
