//! Bundled python feature implementation.
//!
//! Interpreter discovery is demand-driven. Explicit environment creation runs on the project
//! host; selection configures future tools/shells and never injects activation into an
//! already-running terminal without an explicit action.
use super::*;

/// Cached interpreter discovery plus a separate explicit virtual-environment creation form.
pub(super) struct State {
    pub(super) env: Option<project::Environments>,
    pub(super) env_busy: bool,
    pub(super) env_error: String,
    pub(super) venv_path: String,
    pub(super) venv_python: String,
    pub(super) venv_with_pip: bool,
    pub(super) venv_busy: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            env: None,
            env_busy: false,
            env_error: String::new(),
            venv_path: ".venv".into(),
            venv_python: "python3".into(),
            venv_with_pip: true,
            venv_busy: false,
        }
    }
}

impl EditorApp {
    /// Borrow the last successful host interpreter probe; absence also covers a probe not yet run.
    pub(in crate::app) fn python_environments(&self) -> Option<&project::Environments> {
        self.workbench.python.env.as_ref()
    }
    /// Report a discovery request currently in flight.
    pub(in crate::app) fn python_environment_busy(&self) -> bool {
        self.workbench.python.env_busy
    }
    /// Expose the last discovery error independently of a retained successful result.
    pub(in crate::app) fn python_environment_error(&self) -> &str {
        &self.workbench.python.env_error
    }
    /// Apply an interpreter selection, invalidate tool settings and refresh discovery;
    /// changing the selector leaves an existing debugger attachment before reuse.
    pub(in crate::app) fn python_environment_picker(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        let mut selected = self.workbench.prefs.python.clone();
        egui::ComboBox::from_id_salt("python-environment")
            .width(ui.available_width().min(380.))
            .selected_text(if selected == "auto" {
                "Auto · project .venv, then system"
            } else if selected.is_empty() {
                "System Python"
            } else {
                &selected
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut selected,
                    "auto".into(),
                    "Auto · detect project environment",
                );
                ui.selectable_value(&mut selected, String::new(), "System Python");
                if let Some(environment) = &self.workbench.python.env {
                    for candidate in &environment.candidates {
                        if !candidate.python.is_empty() {
                            ui.selectable_value(
                                &mut selected,
                                candidate.python.clone(),
                                &candidate.name,
                            );
                        }
                    }
                }
            });
        if selected != self.workbench.prefs.python {
            self.leave_debugger();
            self.workbench.prefs.python = selected.clone();
            self.workbench.draft.python = selected;
            self.workbench.epoch += 1;
            self.workbench.code_tools.tool_error.clear();
            self.workbench.code_tools.unavailable_tools.clear();
            self.workbench.save_preferences();
            self.refresh_environment(true, ctx);
        }
    }

    /// Render interpreter inspection and explicit environment creation controls on the project host.
    pub(in crate::app) fn python_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.heading("Project interpreter");
        self.python_environment_picker(ui, ctx);
        ui.label("Custom interpreter or environment folder (on this host)");
        ui.add(
            egui::TextEdit::singleline(&mut self.workbench.draft.python)
                .desired_width(f32::INFINITY)
                .hint_text(".venv or /path/to/bin/python"),
        );
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.workbench.python.env_busy,
                    egui::Button::new("Select interpreter"),
                )
                .clicked()
            {
                self.leave_debugger();
                self.workbench.prefs.python = self.workbench.draft.python.trim().into();
                self.workbench.epoch += 1;
                self.workbench.code_tools.tool_error.clear();
                self.workbench.code_tools.unavailable_tools.clear();
                self.workbench.save_preferences();
                self.refresh_environment(true, ctx);
            }
            if ui
                .add_enabled(
                    !self.workbench.python.env_busy,
                    egui::Button::new("Refresh"),
                )
                .clicked()
            {
                self.refresh_environment(false, ctx);
            }
            if self.workbench.python.env_busy {
                ui.spinner();
            }
        });
        if let Some(env) = &self.workbench.python.env {
            ui.add_space(8.);
            ui.label(
                RichText::new(format!("Python {}", env.version))
                    .monospace()
                    .color(palette.accent),
            );
            ui.label(RichText::new(&env.executable).monospace().small());
            ui.label(palette.tag(if env.prefix != env.base_prefix {
                "VIRTUAL ENVIRONMENT ACTIVE"
            } else {
                "SYSTEM INTERPRETER"
            }));
        }
        if !self.workbench.python.env_error.is_empty() {
            ui.colored_label(palette.error, &self.workbench.python.env_error);
        }
        ui.label(RichText::new("Selection is saved per host/project. New shells use this environment; Ruff uses it in auto mode. Existing shells keep running.").small().color(palette.muted));
        if ui
            .add_enabled(
                self.workbench.python.env.is_some() && self.terminal.is_some(),
                egui::Button::new("Activate in current shell"),
            )
            .on_hover_text(
                "Send activation to an idle shell. Use Restart shell for a fresh terminal.",
            )
            .clicked()
        {
            self.activate_python_impl(ctx, true);
        }
        ui.separator();
        ui.heading("Create environment");
        ui.label("Folder inside project");
        ui.text_edit_singleline(&mut self.workbench.python.venv_path);
        ui.label("Base Python executable");
        ui.text_edit_singleline(&mut self.workbench.python.venv_python);
        ui.checkbox(
            &mut self.workbench.python.venv_with_pip,
            "Include pip (requires ensurepip on the host)",
        );
        if ui
            .add_enabled(
                !self.workbench.python.venv_busy,
                egui::Button::new("Create with python -m venv"),
            )
            .clicked()
        {
            let path = self.workbench.python.venv_path.trim().to_owned();
            if path.is_empty() {
                self.workbench.python.env_error = "Enter an environment folder.".into();
            } else {
                self.workbench.python.venv_busy = true;
                self.dispatch_tool(
                    project::Request::CreateEnvironment {
                        root: self.root(),
                        path: path.clone(),
                        python: self.workbench.python.venv_python.trim().into(),
                        with_pip: self.workbench.python.venv_with_pip,
                    },
                    ToolJob::CreateEnvironment { path },
                    ctx,
                );
            }
        }
        if self.workbench.python.venv_busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Creating environment…");
            });
        }
    }
    /// Render Python task shortcuts; commands execute only after an explicit action.
    pub(in crate::app) fn python_tasks_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.workbench.python.env.is_some(),
                    egui::Button::new("Python REPL"),
                )
                .clicked()
            {
                let python = self
                    .workbench
                    .python
                    .env
                    .as_ref()
                    .map(|e| e.executable.as_str())
                    .unwrap_or("python3");
                self.send_project_command(ssh::quote(python), ctx);
            }
            if ui
                .add_enabled(
                    self.workbench.python.env.is_some(),
                    egui::Button::new("Run pytest"),
                )
                .clicked()
            {
                let python = self
                    .workbench
                    .python
                    .env
                    .as_ref()
                    .map(|e| e.executable.as_str())
                    .unwrap_or("python3");
                self.send_project_command(format!("{} -m pytest", ssh::quote(python)), ctx);
            }
            if ui
                .add_enabled(
                    self.workbench.python.env.is_some(),
                    egui::Button::new("List packages"),
                )
                .clicked()
            {
                let python = self
                    .workbench
                    .python
                    .env
                    .as_ref()
                    .map(|e| e.executable.as_str())
                    .unwrap_or("python3");
                self.send_project_command(format!("{} -m pip list", ssh::quote(python)), ctx);
            }
        });
        ui.label(
            RichText::new(
                "Existing environments are reused. No packages are installed automatically.",
            )
            .small()
            .color(palette.muted),
        );
    }
    /// Probe the selected project interpreter on a worker, deduplicating concurrent probes.
    /// `activate` clears the previous probe and records an activation intent in the reply.
    pub(in crate::app) fn refresh_environment(&mut self, activate: bool, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Python)
            || self.workbench.python.env_busy
            || !self.project_ready()
        {
            return;
        }
        self.workbench.python.env_busy = true;
        if activate {
            self.workbench.python.env = None;
        }
        self.dispatch_tool(
            project::Request::Environments {
                root: self.root(),
                selected: self.workbench.prefs.python.clone(),
            },
            ToolJob::Environment {
                activate,
                selected: self.workbench.prefs.python.clone(),
            },
            ctx,
        );
    }
    /// Return the selected interpreter marker, or `off` when the Python feature is disabled.
    pub(in crate::app) fn terminal_python_selection(&self) -> String {
        if self.feature(crate::extensions::Feature::Python) {
            self.workbench.prefs.python.clone()
        } else {
            "off".into()
        }
    }
    /// Apply the discovered interpreter only where automatic shell activation is permitted.
    pub(in crate::app) fn activate_python(&mut self, ctx: &egui::Context) {
        self.activate_python_impl(ctx, false);
    }
    /// Send a host-quoted activation command after shell startup. Native terminals
    /// retain their original environment unless `force` records an explicit activation action.
    pub(in crate::app) fn activate_python_impl(&mut self, _ctx: &egui::Context, force: bool) {
        if !self.feature(crate::extensions::Feature::Python) {
            return;
        }
        let Some(env) = &self.workbench.python.env else {
            return;
        };
        let Some(terminal) = &mut self.terminal else {
            return;
        };
        if terminal.native && !force {
            return;
        }
        if terminal.python.as_deref() == Some(&env.executable) {
            return;
        }
        let previous = terminal.python.replace(env.executable.clone());
        // Queue activation after the shell's startup files have completed.
        let executable = &env.executable;
        let bin = std::path::Path::new(executable)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        if bin.is_empty() {
            return;
        }
        let deactivate = "if command -v deactivate >/dev/null 2>&1; then deactivate; fi; ";
        let command = if env.prefix != env.base_prefix {
            let activate = format!("{}/bin/activate", env.prefix);
            format!(
                "{deactivate}if [ -r {activate} ]; then . {activate}; else export VIRTUAL_ENV={prefix}; export PATH={bin}:\"$PATH\"; unset PYTHONHOME; fi\n",
                activate = ssh::quote(&activate),
                prefix = ssh::quote(&env.prefix),
                bin = ssh::quote(&bin)
            )
        } else if previous.is_some()
            || (self.workbench.prefs.python != "auto" && !self.workbench.prefs.python.is_empty())
        {
            format!(
                "{deactivate}unset VIRTUAL_ENV PYTHONHOME; export PATH={}:\"$PATH\"\n",
                ssh::quote(&bin)
            )
        } else {
            return;
        };
        if let Err(error) = terminal.send(command.into_bytes()) {
            self.notify(error.to_string(), true);
        }
    }
    /// Resolve the discovered executable or configured marker for a future agent launch.
    pub(in crate::app) fn agent_python(&self) -> String {
        if !self.feature(crate::extensions::Feature::Python) {
            return "off".into();
        }
        self.workbench
            .python
            .env
            .as_ref()
            .map(|env| env.executable.clone())
            .unwrap_or_else(|| self.workbench.prefs.python.clone())
    }
    /// Clear the new terminal's recorded interpreter so initial environment setup can run once.
    pub(in crate::app) fn new_terminal_environment(&mut self) {
        if let Some(terminal) = &mut self.terminal {
            terminal.python = None;
        }
    }
}
