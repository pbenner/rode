//! Project settings with persistent category navigation and scoped draft actions.
//!
//! Switching categories retains edits and scroll positions. Saving or discarding a
//! category touches only its own fields; sandbox changes retain validation/probing.
use super::*;

/// Transient navigation within the project settings tab, independent of saved preferences.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(in crate::app) enum SettingsSection {
    #[default]
    CodeTools,
    Sandbox,
    Monitoring,
    Latex,
    Presentations,
}

/// Draft and navigation retained only when reapplying policy to the same project and host.
pub(in crate::app) struct SettingsDraft(Preferences, SettingsSection);

impl SettingsSection {
    pub(super) const ALL: [Self; 5] = [
        Self::CodeTools,
        Self::Sandbox,
        Self::Monitoring,
        Self::Latex,
        Self::Presentations,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::CodeTools => "Code tools",
            Self::Sandbox => "Sandbox",
            Self::Monitoring => "Monitoring",
            Self::Latex => "LaTeX",
            Self::Presentations => "Presentations",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::CodeTools => "Automatic checks, formatting and the Ruff executable.",
            Self::Sandbox => "Project isolation, network access and shared paths.",
            Self::Monitoring => "Refresh timing for the project host's system monitor.",
            Self::Latex => "PDF build commands, source paths and LaTeX tools.",
            Self::Presentations => "Marp rendering, browser selection and slide assets.",
        }
    }

    fn changed(self, saved: &Preferences, draft: &Preferences) -> bool {
        match self {
            Self::CodeTools => {
                saved.ruff != draft.ruff
                    || saved.format_on_save != draft.format_on_save
                    || saved.check_on_save != draft.check_on_save
                    || saved.live_checks != draft.live_checks
            }
            Self::Sandbox => saved.sandbox != draft.sandbox,
            Self::Monitoring => saved.monitor_interval != draft.monitor_interval,
            Self::Latex => saved.latex != draft.latex,
            Self::Presentations => saved.marp != draft.marp,
        }
    }

    /// Copy only the selected category, preserving every unrelated applied value or draft.
    fn copy(self, source: &Preferences, target: &mut Preferences) {
        match self {
            Self::CodeTools => {
                target.ruff = source.ruff.clone();
                target.format_on_save = source.format_on_save;
                target.check_on_save = source.check_on_save;
                target.live_checks = source.live_checks;
            }
            Self::Sandbox => target.sandbox = source.sandbox.clone(),
            Self::Monitoring => target.monitor_interval = source.monitor_interval,
            Self::Latex => target.latex = source.latex.clone(),
            Self::Presentations => target.marp = source.marp.clone(),
        }
    }
}

impl EditorApp {
    /// Capture unfinished form edits only for a policy refresh on the same project host.
    pub(in crate::app) fn settings_draft_for(
        &self,
        path: &str,
        remote: bool,
    ) -> Option<SettingsDraft> {
        (self.workbench.key == self.project_settings_key(path, remote)).then(|| {
            SettingsDraft(
                self.workbench.draft.clone(),
                self.workbench.settings_section,
            )
        })
    }

    /// Restore form state after loading the newly applied project policy.
    pub(in crate::app) fn restore_settings_draft(&mut self, draft: SettingsDraft) {
        self.workbench.draft = draft.0;
        self.workbench.settings_section = draft.1;
    }

    /// Open a specific category without toggling an already visible settings tab closed.
    pub(in crate::app) fn reveal_settings(
        &mut self,
        section: SettingsSection,
        ctx: &egui::Context,
    ) {
        self.workbench.settings_section = section;
        self.reveal_panel(Panel::Settings, ctx);
    }

    /// Keep navigation and explicit draft actions visible while the selected form scrolls.
    pub(in crate::app) fn settings_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        ui.label(
            RichText::new("Project settings")
                .size(palette.ui.map_or(18., |style| style.heading_size - 2.))
                .strong()
                .color(palette.heading_color()),
        );
        let root = self.root();
        ui.add(egui::Label::new(RichText::new(&root).small().color(palette.muted)).truncate())
            .on_hover_text(&root);
        ui.add_space(8.);
        ui.horizontal_wrapped(|ui| {
            for section in SettingsSection::ALL {
                let label = if section.changed(&self.workbench.prefs, &self.workbench.draft) {
                    format!("{} •", section.label())
                } else {
                    section.label().into()
                };
                let tab = ui
                    .selectable_label(self.workbench.settings_section == section, label)
                    .on_hover_text(section.description());
                ui_probe::button(ui, &format!("settings-category-{}", section.label()), &tab);
                if tab.clicked() {
                    self.workbench.settings_section = section;
                }
            }
        });
        ui.add_space(8.);
        let section = self.workbench.settings_section;
        let changed = section.changed(&self.workbench.prefs, &self.workbench.draft);
        ui.horizontal_wrapped(|ui| {
            let sandbox = section == SettingsSection::Sandbox;
            let enabled = if sandbox {
                !self.preparing && !self.explorer_state.busy
            } else {
                changed
            };
            let save = ui.add_enabled(
                enabled,
                egui::Button::new(if sandbox {
                    "Apply sandbox settings"
                } else {
                    "Save changes"
                }),
            );
            ui_probe::button(ui, "settings-save", &save);
            if save.clicked() {
                self.save_settings_section(section, ctx);
            }
            let discard = ui.add_enabled(changed, egui::Button::new("Discard changes"));
            ui_probe::button(ui, "settings-discard", &discard);
            if discard.clicked() {
                section.copy(&self.workbench.prefs, &mut self.workbench.draft);
            }
            layout::subtitle(
                ui,
                if changed {
                    "Unsaved changes •"
                } else {
                    "No unsaved changes"
                },
                palette,
            );
        });
        ui.separator();
        // ScrollArea IDs scope scrolling, not the child widgets' edit/undo state.
        ui.push_id(section, |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("settings-category", section))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if ui.available_height() >= 350. {
                        layout::subtitle(ui, section.description(), palette);
                        ui.add_space(8.);
                    }
                    match section {
                        SettingsSection::CodeTools => self.code_settings_ui(ui),
                        SettingsSection::Sandbox => self.sandbox_settings_ui(ui),
                        SettingsSection::Monitoring => {
                            layout::section(ui, "System monitor", palette, |ui| {
                                ui.add(egui::Slider::new(&mut self.workbench.draft.monitor_interval, 1..=30)
                                    .text("Refresh interval (s)"));
                                layout::subtitle(ui, "Monitoring runs only while the monitor is visible. It reads the project host outside the sandbox.", palette);
                            });
                        }
                        SettingsSection::Latex => self.latex_settings_ui(ui),
                        SettingsSection::Presentations => self.marp_settings_ui(ui),
                    }
                });
        });
    }

    /// Validate before applying only this category, retaining its existing lifecycle boundaries.
    fn save_settings_section(&mut self, section: SettingsSection, ctx: &egui::Context) {
        let validation = match section {
            SettingsSection::Latex => self.workbench.draft.latex.validate(),
            SettingsSection::Presentations => self.workbench.draft.marp.validate(),
            SettingsSection::Sandbox => self.workbench.draft.sandbox.validate(),
            _ => Ok(()),
        };
        if let Err(error) = validation {
            self.notify(error.to_string(), true);
            return;
        }
        if section == SettingsSection::Latex {
            self.leave_debugger();
            self.stop_latex();
        }
        section.copy(&self.workbench.draft, &mut self.workbench.prefs);
        self.workbench.save_preferences();
        match section {
            SettingsSection::CodeTools => {
                self.workbench.invalidate_tools();
                self.workbench.code_tools.tool_error.clear();
                self.refresh_environment(true, ctx);
            }
            SettingsSection::Latex => self.workbench.invalidate_tools(),
            SettingsSection::Presentations => {
                for doc in &mut self.documents {
                    if let Some(view) = &mut doc.marp {
                        view.stop_render();
                    }
                }
            }
            SettingsSection::Sandbox => {
                let options = self.workbench.prefs.sandbox.clone();
                self.sandbox_defaults = options.clone();
                let remote = self.workspace.as_ref().is_some_and(|w| w.remote.is_some());
                self.prepare_project(self.root(), options, remote, true, None, ctx);
                return;
            }
            SettingsSection::Monitoring => {}
        }
        self.status = format!("{} settings saved for this project", section.label());
    }

    fn code_settings_ui(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        layout::section(ui, "Code tools", palette, |ui| {
            let format = ui.checkbox(
                &mut self.workbench.draft.format_on_save,
                "Format supported files on save",
            );
            ui_probe::button(ui, "settings-format-on-save", &format);
            ui.checkbox(
                &mut self.workbench.draft.check_on_save,
                "Check supported files on save",
            );
            ui.checkbox(
                &mut self.workbench.draft.live_checks,
                "Check supported files after typing pauses",
            );
        });
        layout::section(ui, "Python diagnostics and formatting", palette, |ui| {
            ui.label("Ruff command (blank: uvx ruff)");
            ui.add(
                egui::TextEdit::singleline(&mut self.workbench.draft.ruff)
                    .desired_width(f32::INFINITY)
                    .hint_text("uvx ruff, auto, ruff, or /path/to/ruff"),
            )
            .on_hover_text("Use auto to prefer Ruff from the selected Python environment.");
        });
    }

    fn latex_settings_ui(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        layout::section(ui, "Build command", palette, |ui| {
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
                            self.workbench.draft.latex.arguments =
                                crate::latex::preset(driver, engine);
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
        });
        layout::section(ui, "Files and code tools", palette, |ui| {
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
        });
    }

    fn marp_settings_ui(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        layout::section(ui, "Marp presentations", palette, |ui| {
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
        });
        layout::section(ui, "Slide assets", palette, |ui| {
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
        });
    }

    fn sandbox_settings_ui(&mut self, ui: &mut egui::Ui) {
        let palette = self.themes.active().palette;
        if let Some(workspace) = &self.workspace {
            ui.label(palette.tag(workspace.plan.label()));
            ui.add_space(8.);
        }
        layout::section(ui, "Sandbox access", palette, |ui| {
            ui.label(RichText::new("Changes take effect after applying and probing the new policy. Editor buffers stay open; the project shell restarts.").small().color(palette.muted));
            layout::subtitle(
                ui,
                "Applied rules also become the defaults for projects without a saved policy.",
                palette,
            );
            self.sandbox_presets
                .ui(ui, &mut self.workbench.draft.sandbox, palette);
            sandbox_options_ui(ui, &mut self.workbench.draft.sandbox, palette);
            ui.add_space(8.);
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
        });
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
        self.sync_project_environment();
        if self.panel_visible(Panel::Environment) {
            self.refresh_runtimes(ctx);
        }
        if self.panel_visible(Panel::Environment)
            || self.documents.iter().any(|d| {
                crate::languages::detect(d.path.as_deref()).is_some_and(|l| l.id == "python")
            })
        {
            self.refresh_environment(false, ctx);
        }
        if self.panel_visible(Panel::Git) {
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
