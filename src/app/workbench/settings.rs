//! Bundled settings feature implementation.
//!
//! Edit a local draft, validate/probe access changes before replacing a working environment,
//! and persist only explicit settings deltas. Requested isolation and successfully active
//! isolation remain separate UI states.
use super::*;

impl EditorApp {
    /// Edit a project settings draft and validate/apply explicit changes before invalidating tool epochs.
    pub(in crate::app) fn settings_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.heading("Code tools");
        ui.checkbox(
            &mut self.workbench.draft.format_on_save,
            "Format supported files on save",
        );
        ui.checkbox(
            &mut self.workbench.draft.check_on_save,
            "Check supported files on save",
        );
        ui.checkbox(
            &mut self.workbench.draft.live_checks,
            "Check supported files after typing pauses",
        );
        ui.label("Ruff command (blank: uvx ruff)");
        ui.add(
            egui::TextEdit::singleline(&mut self.workbench.draft.ruff)
                .desired_width(f32::INFINITY)
                .hint_text("uvx ruff, auto, ruff, or /path/to/ruff"),
        )
        .on_hover_text("Use auto to prefer Ruff from the selected Python environment.");
        ui.add(
            egui::Slider::new(&mut self.workbench.draft.monitor_interval, 1..=30)
                .text("Monitor interval (s)"),
        );
        if ui.button("Save tool settings").clicked() {
            if self.workbench.prefs.latex != self.workbench.draft.latex {
                self.leave_debugger();
                self.stop_latex();
            }
            let sandbox = self.workbench.prefs.sandbox.clone();
            self.workbench.prefs = self.workbench.draft.clone();
            self.workbench.prefs.sandbox = sandbox;
            self.workbench.epoch += 1;
            self.workbench.code_tools.tool_error.clear();
            self.workbench.code_tools.unavailable_tools.clear();
            self.workbench.save_preferences();
            self.refresh_environment(true, ctx);
            self.status = "Project tool settings saved".into();
        }
        ui.separator();
        ui.heading("LaTeX");
        ui.label("Build preset (or edit the executable and arguments below)");
        egui::ComboBox::from_id_salt("latex-preset")
            .selected_text("Choose a preset…")
            .show_ui(ui, |ui| {
                for (title, driver, engine) in [
                    ("latexmk · pdfLaTeX", "latexmk", "pdflatex"),
                    ("latexmk · XeLaTeX", "latexmk", "xelatex"),
                    ("latexmk · LuaLaTeX", "latexmk", "lualatex"),
                    ("pdfLaTeX", "pdflatex", "pdflatex"),
                    ("XeLaTeX", "xelatex", "xelatex"),
                    ("LuaLaTeX", "lualatex", "lualatex"),
                ] {
                    if ui.selectable_label(false, title).clicked() {
                        self.workbench.draft.latex.program = driver.into();
                        self.workbench.draft.latex.arguments = crate::latex::preset(driver, engine);
                    }
                }
            });
        ui.label("Build executable");
        ui.add(
            egui::TextEdit::singleline(&mut self.workbench.draft.latex.program)
                .desired_width(f32::INFINITY)
                .hint_text("latexmk, another compiler, or /path/to/tool"),
        );
        ui.label("Arguments · one per line · {file}, {dir}, {root}, {outdir}");
        let mut args = self.workbench.draft.latex.arguments.join("\n");
        if ui
            .add(
                egui::TextEdit::multiline(&mut args)
                    .char_limit(32 * 1024)
                    .code_editor()
                    .desired_rows(6)
                    .desired_width(f32::INFINITY),
            )
            .changed()
        {
            self.workbench.draft.latex.arguments = if args.is_empty() {
                vec![]
            } else {
                args.lines().map(str::to_owned).collect()
            };
        }
        ui.small("Arguments are passed directly, without shell splitting or quote removal. Custom TeX tools may use any executable/argument list; no tools or packages are installed by RODE.");
        for (label, value, hint) in [
            (
                "Main file (blank: current source or % !TeX root)",
                &mut self.workbench.draft.latex.main_file,
                "main.tex",
            ),
            (
                "Output directory (relative to project; blank: source directory)",
                &mut self.workbench.draft.latex.output_directory,
                "build/latex",
            ),
            (
                "PDF filename (blank: main source basename.pdf)",
                &mut self.workbench.draft.latex.pdf_file,
                "main.pdf",
            ),
            (
                "Formatter executable (blank: disabled)",
                &mut self.workbench.draft.latex.formatter,
                "latexindent",
            ),
            (
                "Checker executable (blank: disabled)",
                &mut self.workbench.draft.latex.checker,
                "chktex",
            ),
        ] {
            ui.label(label);
            ui.add(
                egui::TextEdit::singleline(value)
                    .desired_width(f32::INFINITY)
                    .hint_text(hint),
            );
        }
        ui.add(
            egui::Slider::new(&mut self.workbench.draft.latex.timeout_seconds, 1..=300)
                .text("Build timeout (s)"),
        );
        if ui.button("Save LaTeX settings").clicked() {
            match self.workbench.draft.latex.validate() {
                Ok(()) => {
                    self.leave_debugger();
                    self.stop_latex();
                    self.workbench.prefs.latex = self.workbench.draft.latex.clone();
                    self.workbench.invalidate_tools();
                    self.workbench.save_preferences();
                    self.status = "LaTeX settings saved for this project".into();
                }
                Err(error) => self.notify(error.to_string(), true),
            }
        }
        ui.separator();
        ui.heading("Marp presentations");
        ui.label("Marp CLI (blank: installed project node_modules/.bin/marp, then PATH)");
        ui.add(
            egui::TextEdit::singleline(&mut self.workbench.draft.marp.program)
                .desired_width(f32::INFINITY)
                .hint_text("marp or /path/to/marp"),
        );
        ui.label("Browser executable (blank: Marp auto-detection)");
        egui::ComboBox::from_id_salt("marp-browser-kind")
            .selected_text(&self.workbench.draft.marp.browser_kind)
            .show_ui(ui, |ui| {
                for kind in ["auto", "chrome", "edge", "firefox"] {
                    ui.selectable_value(
                        &mut self.workbench.draft.marp.browser_kind,
                        kind.into(),
                        kind,
                    );
                }
            });
        ui.add(
            egui::TextEdit::singleline(&mut self.workbench.draft.marp.browser)
                .desired_width(f32::INFINITY)
                .hint_text("/usr/bin/firefox or /usr/bin/chromium"),
        );
        ui.label("Additional theme CSS (optional project-host path)");
        ui.add(
            egui::TextEdit::singleline(&mut self.workbench.draft.marp.theme_css)
                .desired_width(f32::INFINITY)
                .hint_text("themes/slides.css"),
        );
        ui.checkbox(
            &mut self.workbench.draft.marp.allow_local_files,
            "Allow local slide assets within the current host/sandbox access",
        );
        ui.small("Tools must already be installed on the project host. Rendering/export uses its active sandbox; no automatic installation or project JavaScript config loading.");
        if ui.button("Save Marp settings").clicked() {
            match self.workbench.draft.marp.validate() {
                Ok(()) => {
                    self.workbench.prefs.marp = self.workbench.draft.marp.clone();
                    self.workbench.save_preferences();
                    for doc in &mut self.documents {
                        if let Some(view) = &mut doc.marp {
                            view.stop_render();
                        }
                    }
                    self.status = "Marp settings saved for this project".into();
                }
                Err(error) => self.notify(error.to_string(), true),
            }
        }
        ui.separator();
        ui.heading("Sandbox access");
        ui.label(RichText::new("Changes take effect after applying and probing the new policy. Editor buffers stay open; the project shell restarts.").small().color(palette.muted));
        sandbox_options_ui(ui, &mut self.workbench.draft.sandbox);
        ui.add_space(8.);
        if ui
            .add_enabled(!self.preparing, egui::Button::new("Apply sandbox settings"))
            .clicked()
        {
            let options = self.workbench.draft.sandbox.clone();
            match options.validate() {
                Ok(()) => {
                    self.sandbox_defaults = options.clone();
                    self.workbench.prefs.sandbox = options.clone();
                    self.workbench.save_preferences();
                    let remote = self.workspace.as_ref().is_some_and(|w| w.remote.is_some());
                    self.prepare_project(self.root(), options, remote, true, None, ctx);
                }
                Err(error) => self.notify(error.to_string(), true),
            }
        }
        if self.preparing {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Probing updated sandbox…");
            });
        }
        if ui.button("Reset sandbox draft to defaults").clicked() {
            let enabled = self.workbench.draft.sandbox.enabled;
            self.workbench.draft.sandbox = sandbox::Options {
                enabled,
                ..Default::default()
            };
        }
    }
    /// Queue changed project settings and sandbox defaults in shared preference storage.
    pub(in crate::app) fn persist_project_preferences(
        &mut self,
        storage: Option<&mut (dyn eframe::Storage + 'static)>,
    ) {
        if !self.workbench.preferences_pending {
            return;
        }
        self.workbench.preferences_pending = false;
        if let Some(storage) = storage {
            self.workbench.store(storage);
            if let Ok(value) = serde_json::to_string(&self.sandbox_defaults) {
                storage.set_string("sandbox_defaults", value);
            }
            storage.flush();
        } else {
            self.notify(
                "Project settings applied, but preference storage is unavailable.",
                true,
            );
        }
    }
    /// Select host/project-keyed settings after preparation, retain the actual requested
    /// sandbox options and refresh only tools needed by the visible context.
    pub(in crate::app) fn load_project_tools(&mut self, ctx: &egui::Context) {
        let Some(w) = &self.workspace else {
            return;
        };
        self.workbench.key = self.project_settings_key(&w.root, w.remote.is_some());
        let mut prefs = self
            .workbench
            .preferences
            .get(&self.workbench.key)
            .cloned()
            .unwrap_or_default();
        prefs.sandbox = w.sandbox_options.clone();
        self.workbench.prefs = prefs.clone();
        self.workbench.draft = prefs;
        self.workbench.save_preferences();
        if self.workbench.panel == Some(Panel::Environment) {
            self.refresh_runtimes(ctx);
        }
        if self.workbench.panel == Some(Panel::Environment)
            || self.documents.iter().any(|d| {
                crate::languages::detect(d.path.as_deref()).is_some_and(|l| l.id == "python")
            })
        {
            self.refresh_environment(false, ctx);
        }
        if self.workbench.panel == Some(Panel::Git) {
            self.refresh_git(ctx);
        }
    }
    /// Serialize the host transport identity and project path into the settings-map key.
    pub(in crate::app) fn project_settings_key(&self, path: &str, remote: bool) -> String {
        let host = self
            .workspace
            .as_ref()
            .and_then(|w| w.remote.as_ref())
            .filter(|_| remote)
            .map(|r| {
                (
                    &r.profile.host,
                    &r.profile.user,
                    &r.profile.port,
                    &r.profile.config,
                    &r.profile.jump,
                )
            });
        serde_json::to_string(&(host, path)).unwrap_or_default()
    }
    /// Return requested grants saved for exactly this host/project, or None if none were recorded.
    pub(in crate::app) fn saved_sandbox_for(
        &self,
        path: &str,
        remote: bool,
    ) -> Option<sandbox::Options> {
        self.workbench
            .preferences
            .get(&self.project_settings_key(path, remote))
            .map(|p| p.sandbox.clone())
    }
    /// Collect saved project policies only for the currently selected local or SSH host identity.
    pub(in crate::app) fn sandbox_policies_for_host(
        &self,
        remote: bool,
    ) -> BTreeMap<String, sandbox::Options> {
        let host: serde_json::Value =
            serde_json::from_str(&self.project_settings_key("", remote)).unwrap_or_default();
        self.workbench
            .preferences
            .iter()
            .filter_map(|(key, prefs)| {
                let key: serde_json::Value = serde_json::from_str(key).ok()?;
                if key[0] == host[0] {
                    Some((key[1].as_str()?.to_owned(), prefs.sandbox.clone()))
                } else {
                    None
                }
            })
            .collect()
    }
    /// Return configured queue/cancel executables, substituting squeue/scancel for blank overrides.
    pub(in crate::app) fn slurm_programs(&self) -> (String, String) {
        (
            if self.workbench.prefs.slurm_queue.trim().is_empty() {
                "squeue".into()
            } else {
                self.workbench.prefs.slurm_queue.clone()
            },
            if self.workbench.prefs.slurm_cancel.trim().is_empty() {
                "scancel".into()
            } else {
                self.workbench.prefs.slurm_cancel.clone()
            },
        )
    }
    /// Edit host-side Slurm executable/wrapper paths in the uncommitted project settings draft.
    pub(in crate::app) fn slurm_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Slurm executable paths / wrappers", |ui| {
            ui.label("Programs on the project host; wrappers may load site modules.");
            ui.label("squeue");
            ui.text_edit_singleline(&mut self.workbench.draft.slurm_queue);
            ui.label("scancel");
            ui.text_edit_singleline(&mut self.workbench.draft.slurm_cancel);
            if ui.button("Save Slurm settings").clicked() {
                self.workbench.prefs.slurm_queue = self.workbench.draft.slurm_queue.trim().into();
                self.workbench.prefs.slurm_cancel = self.workbench.draft.slurm_cancel.trim().into();
                self.workbench.save_preferences();
            }
        });
    }
}
