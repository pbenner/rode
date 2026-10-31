//! Native extension controls; installed manifests never provide arbitrary UI code.
//!
//! Toggles update field-merged preferences and invalidate activation generations. Disabling
//! cancels owned executable work while preserving documents and already-submitted source
//! saves.
use super::*;
use crate::extensions::{self as api, Feature};

pub(super) struct Extensions {
    pub registry: api::Registry,
    install_path: String,
    settings_id: String,
    settings_text: String,
    pub output: String,
    pub output_open: bool,
    pub busy: bool,
    pub cancel: Arc<AtomicBool>,
}
impl Extensions {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let preferences = storage
            .and_then(|s| s.get_string(api::STORAGE_KEY))
            .and_then(|s| api::preferences(&s).ok())
            .unwrap_or_default();
        Self {
            registry: api::Registry::load(&api::directory(), preferences),
            install_path: String::new(),
            settings_id: String::new(),
            settings_text: String::new(),
            output: String::new(),
            output_open: false,
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}
impl EditorApp {
    pub(super) fn cancel_extensions(&mut self) {
        self.extensions.cancel.store(true, Ordering::Release);
        self.extensions.cancel = Arc::new(AtomicBool::new(false));
    }
    pub(super) fn feature(&self, feature: Feature) -> bool {
        self.extensions.registry.feature(feature)
    }
    pub(super) fn panel_feature(panel: Panel) -> Option<Feature> {
        match panel {
            Panel::Git => Some(Feature::Git),
            Panel::Monitor => Some(Feature::Monitor),
            Panel::Agent => Some(Feature::Agents),
            Panel::Debug => Some(Feature::Debugger),
            _ => None,
        }
    }
    pub(super) fn panel_enabled(&self, panel: Panel) -> bool {
        Self::panel_feature(panel).is_none_or(|f| self.feature(f))
    }
    pub(super) fn persist_extensions<S: eframe::Storage + ?Sized>(
        &mut self,
        storage: Option<&mut S>,
    ) {
        if !self.extensions.registry.pending {
            return;
        }
        if let Some(storage) = storage {
            storage.set_string(
                api::STORAGE_KEY,
                serde_json::to_string(&self.extensions.registry.preferences).unwrap_or_default(),
            );
            storage.flush();
            self.extensions.registry.pending = false;
        }
    }
    pub(super) fn set_extension(&mut self, id: &str, enabled: bool) {
        self.cancel_extensions();
        self.extensions.registry.set_enabled(id, enabled);
        // Invalidates all source-tool results, including jobs queued before disabling.
        self.workbench.invalidate_tools();
        if !enabled {
            if id == Feature::Debugger.id() {
                self.leave_debugger();
            }
            if id == Feature::Latex.id() {
                self.stop_latex();
            }
            if id == Feature::Marp.id() {
                for doc in &mut self.documents {
                    if let Some(view) = &mut doc.marp {
                        view.invalidate();
                    }
                }
            }
            if id == Feature::Notebooks.id() {
                self.stop_notebook_kernels("Notebook extension disabled");
            }
            if id == Feature::Agents.id() {
                self.stop_project_agent();
            }
            if id == Feature::Monitor.id() {
                self.workbench.reset_monitor();
            }
            if self
                .workbench
                .panel
                .is_some_and(|panel| !self.panel_enabled(panel))
            {
                self.workbench.panel = None;
            }
            for doc in &mut self.documents {
                if id == Feature::Pdf.id()
                    && let Some(pdf) = &mut doc.pdf
                {
                    pdf.invalidate();
                }
                if (id == Feature::Markdown.id() || id == Feature::Images.id())
                    && let Some(preview) = &mut doc.markdown
                {
                    preview.invalidate();
                }
            }
        }
    }
    pub(super) fn disabled_document_ui(&self, ui: &mut egui::Ui) -> bool {
        let Some(doc) = self.documents.get(self.active) else {
            return false;
        };
        let feature = if doc.notebook.is_some() {
            Some(Feature::Notebooks)
        } else if doc.hex.is_some() {
            Some(Feature::Binary)
        } else if doc.image.is_some() {
            Some(Feature::Images)
        } else if doc.pdf.is_some() {
            Some(Feature::Pdf)
        } else {
            None
        };
        if let Some(feature) = feature
            && !self.feature(feature)
        {
            ui.label("This document's extension is disabled. Enable it in View → Extensions.");
            ui.label(
                "The open document and unsaved edits are preserved. Saving is still available.",
            );
            return true;
        }
        false
    }
    pub(super) fn extensions_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Extensions");
        ui.label("Bundled native features and API v1 project-host tool adapters.");
        ui.small(format!("Manifests: {}", api::directory().display()));
        let entries = self.extensions.registry.entries.clone();
        let mut toggle = None;
        let mut settings = None;
        egui::ScrollArea::vertical()
            .id_salt("extension-list")
            .max_height(
                (ctx.content_rect().height()
                    - if self.extensions.settings_id.is_empty() {
                        250.
                    } else {
                        400.
                    })
                .clamp(100., 360.),
            )
            .show(ui, |ui| {
                for entry in entries {
                    ui.push_id(&entry.id, |ui| {
                        let mut enabled = self.extensions.registry.enabled(&entry.id);
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut enabled, &entry.name).changed() {
                                toggle = Some((entry.id.clone(), enabled));
                            }
                            ui.small(if entry.feature.is_some() {
                                "BUNDLED"
                            } else {
                                "INSTALLED TOOL"
                            });
                            if entry.manifest.is_some() && ui.small_button("Settings").clicked() {
                                settings = Some(entry.id.clone());
                            }
                        });
                        ui.label(&entry.description);
                        ui.small(format!("{} · {}", entry.id, entry.activation));
                        if let Some(manifest) = entry.manifest {
                            ui.monospace(format!(
                                "{} {}",
                                manifest.tool.program,
                                manifest.tool.args.join(" ")
                            ));
                        }
                        ui.separator();
                    });
                }
            });
        if let Some((id, enabled)) = toggle {
            self.set_extension(&id, enabled);
        }
        if let Some(id) = settings {
            self.extensions.settings_text = serde_json::to_string_pretty(
                &self
                    .extensions
                    .registry
                    .preferences
                    .get(&id)
                    .map(|p| p.settings.clone())
                    .unwrap_or_default(),
            )
            .unwrap_or_default();
            self.extensions.settings_id = id;
        }
        if !self.extensions.settings_id.is_empty() {
            ui.label(format!(
                "{} settings (JSON object)",
                self.extensions.settings_id
            ));
            ui.add(
                egui::TextEdit::multiline(&mut self.extensions.settings_text)
                    .code_editor()
                    .desired_rows(4)
                    .char_limit(32 * 1024),
            );
            if ui.button("Save extension settings").clicked() {
                match serde_json::from_str::<std::collections::BTreeMap<String, serde_json::Value>>(
                    &self.extensions.settings_text,
                ) {
                    Ok(settings) => {
                        self.extensions
                            .registry
                            .preferences
                            .entry(self.extensions.settings_id.clone())
                            .or_default()
                            .settings = settings;
                        self.cancel_extensions();
                        self.extensions.registry.pending = true;
                        self.extensions.registry.revision =
                            self.extensions.registry.revision.wrapping_add(1);
                        self.workbench.invalidate_tools();
                        self.extensions.settings_id.clear();
                    }
                    Err(error) => self.notify(format!("Invalid settings: {error}"), true),
                }
            }
        }
        ui.small("Enabling an installed adapter permits it to run on the project host with the project's current sandbox/access. Programs must already be installed there. No automatic upload or tool installation.");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.extensions.install_path).hint_text("Desktop path to extension.json").desired_width(320.));
            if ui.button("Install manifest").clicked() {
                match api::install(&files::expand_home(&self.extensions.install_path), &api::directory()) {
                    Ok(()) => { self.reload_extensions(); self.notify("Extension manifest installed, initially disabled. Review it before enabling.",false); }
                    Err(error) => self.notify(format!("{error:#}"),true),
                }
            }
            if ui.button("Reload").clicked() { self.reload_extensions(); }
        });
        for warning in &self.extensions.registry.warnings {
            ui.colored_label(self.themes.active().palette.error, warning);
        }
        if ui.button("Close").clicked() {
            self.dialog = Dialog::None;
            self.focus_editor(ctx);
        }
    }
    fn reload_extensions(&mut self) {
        self.cancel_extensions();
        let old = &mut self.extensions.registry;
        let mut registry = api::Registry::load(&api::directory(), old.preferences.clone());
        registry.revision = old.revision.wrapping_add(1);
        registry.pending = old.pending;
        self.extensions.registry = registry;
        self.workbench.invalidate_tools();
    }
    pub(super) fn extension_command(
        &mut self,
        extension: &str,
        command: &str,
        ctx: &egui::Context,
    ) {
        if self.extensions.busy {
            self.notify("An extension command is still running.", false);
            return;
        }
        if !self.project_ready() {
            self.notify("Open a project to run an extension on its host.", false);
            return;
        }
        let registry = &self.extensions.registry;
        let Some(manifest) = registry
            .entries
            .iter()
            .find(|e| e.id == extension && registry.enabled(extension))
            .and_then(|e| e.manifest.as_ref())
        else {
            return;
        };
        let doc = self.documents.get(self.active).filter(|d| !d.is_preview());
        let invocation = registry.invocation(
            manifest,
            command.into(),
            doc.and_then(|d| d.path.clone()),
            doc.map(|d| d.text.clone()).unwrap_or_default(),
        );
        self.extensions.busy = true;
        self.dispatch_tool(
            crate::project::Request::Extension {
                root: self.root(),
                invocation,
            },
            ToolJob::Extension {
                revision: registry.revision,
                operation: command.into(),
            },
            ctx,
        );
    }
    pub(super) fn extension_windows(&mut self, ctx: &egui::Context) {
        if self.extensions.output_open {
            egui::Window::new("Extension output")
                .default_size([700., 400.])
                .open(&mut self.extensions.output_open)
                .show(ctx, |ui| {
                    egui::ScrollArea::both().show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.extensions.output.as_str())
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        );
                    });
                });
        }
    }
}
