//! Native extension controls; installed manifests never provide arbitrary UI code.
//!
//! Toggles update field-merged preferences and invalidate activation generations. Disabling
//! cancels owned executable work while preserving documents and already-submitted source
//! saves.
use super::*;
use crate::extensions::{self as api, Feature};
use std::collections::BTreeMap;

/// Desktop-installed manifest registry, explicit settings editor and cancellable adapter work state.
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
    fn cancel_work(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.cancel = Arc::new(AtomicBool::new(false));
    }

    /// Invalidate project-scoped extension UI and requests when their captured workspace authority
    /// is replaced.
    pub(super) fn reset_project(&mut self) {
        self.cancel_work();
        self.busy = false;
        self.registry.revision = self.registry.revision.wrapping_add(1);
    }

    /// Load bounded installed manifests and saved enablement; loading alone never runs adapter executables.
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
    /// User changes share the ownership guard so disabling transfers first unshares its folders.
    pub(in crate::app) fn request_extension(
        &mut self,
        id: &str,
        enabled: bool,
        ctx: &egui::Context,
    ) {
        if id == Feature::Transfers.id() && !enabled {
            self.guarded(Action::DisableTransfers, ctx);
        } else {
            self.set_extension(id, enabled);
        }
    }
    /// Cancel captured adapter jobs and rotate the flag so future work has a fresh lifetime.
    pub(super) fn cancel_extensions(&mut self) {
        self.extensions.cancel_work();
    }
    /// Read effective bundled-feature enablement from the current registry.
    pub(super) fn feature(&self, feature: Feature) -> bool {
        self.extensions.registry.feature(feature)
    }
    /// Allow core panels and only enabled optional feature panels.
    pub(super) fn panel_enabled(&self, panel: Panel) -> bool {
        if panel == Panel::Extension {
            return self.extensions.registry.panels().next().is_some();
        }
        panel.feature().is_none_or(|f| self.feature(f))
    }
    /// Persist explicitly changed extension preferences; retain pending state when storage is absent.
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
    /// Change feature enablement, invalidate source-tool epochs and cancel affected owned work
    /// without replacing source buffers or abandoning protected save acknowledgements.
    pub(super) fn set_extension(&mut self, id: &str, enabled: bool) {
        if self
            .extensions
            .registry
            .entries
            .iter()
            .any(|entry| entry.id == id && !entry.project_scoped())
        {
            self.extensions.registry.set_enabled(id, enabled);
            if !enabled {
                let selected = self.extension_panels.selected == id;
                self.remove_extension_panel(id);
                if selected && self.presentation.area == areas::Area::Extension {
                    self.presentation.enter_area(areas::Area::Projects);
                }
            }
            return;
        }
        if id == Feature::Transfers.id() && !enabled && self.shared_folders.pending() {
            return;
        }
        self.cancel_extensions();
        self.extensions.registry.set_enabled(id, enabled);
        self.sync_project_environment();
        // Invalidates all source-tool results, including jobs queued before disabling.
        self.workbench.invalidate_tools();
        if !enabled {
            if self.extension_panels.selected == id {
                self.remove_extension_panel(id);
                if self.presentation.area == areas::Area::Extension {
                    self.presentation.enter_area(areas::Area::Projects);
                }
            }
            if self
                .presentation
                .area
                .panel()
                .is_some_and(|panel| panel.feature().is_some_and(|feature| feature.id() == id))
            {
                self.presentation.enter_area(areas::Area::Projects);
            }
            if id == Feature::Transfers.id() {
                self.transfers.reset();
                self.shared_folders.reset();
            }
            if id == Feature::Debugger.id() {
                self.leave_debugger();
            }
            if id == Feature::Latex.id() {
                self.stop_latex();
            }
            if id == Feature::Agents.id() {
                self.stop_project_agent();
            }
            if id == Feature::Monitor.id() {
                // Drain an outstanding reply before a re-enabled collector can restart.
                self.host_tools.stop_monitor();
            }
            if self
                .presentation
                .panel
                .is_some_and(|panel| !self.panel_enabled(panel))
            {
                self.presentation.clear_panel();
            }
            self.remove_extension_panel(id);
            self.documents.disable_feature(id);
        }
    }
    /// Explain a disabled specialized viewer while preserving access to the underlying document state.
    pub(super) fn disabled_document_ui(&self, ui: &mut egui::Ui) -> bool {
        let Some(doc) = self.documents.active() else {
            return false;
        };
        let feature = doc.feature();
        if let Some(feature) = feature
            && !self.feature(feature)
        {
            ui.label("This document's extension is disabled. Enable it in View → Extensions.");
            ui.label(
                if matches!(
                    feature,
                    Feature::Images | Feature::Pdf | Feature::Pptx | Feature::Docx
                ) {
                    "The open preview is preserved. It remains read-only."
                } else {
                    "The open document and unsaved edits are preserved. Saving is still available."
                },
            );
            return true;
        }
        false
    }
    /// Render installed/native feature controls and explicit manifest/settings operations.
    pub(super) fn extensions_dialog(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        layout::heading(
            ui,
            "Extensions",
            "Native features, project tools and desktop panels",
            palette,
        );
        ui.small(format!("Manifests: {}", api::directory().display()));
        let filter = ui.add(crate::keyboard::named(
            "Filter extensions by name or ID",
            egui::TextEdit::singleline(&mut self.dialog_input)
                .id_salt("extension-filter")
                .hint_text("Filter extensions by name or ID")
                .desired_width(f32::INFINITY)
                .char_limit(256),
        ));
        ui_probe::button(ui, "extension-filter", &filter);
        ui_probe::value(ui, "extension-filter-value", &filter, &self.dialog_input);
        if self.dialog_focus {
            filter.request_focus();
            self.dialog_focus = false;
        }
        let query = self.dialog_input.trim().to_lowercase();
        let entries: Vec<_> = self
            .extensions
            .registry
            .entries
            .iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.id.to_lowercase().contains(&query)
                    || entry.name.to_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        if entries.is_empty() {
            ui.label("No matching extensions.");
        }
        let mut toggle = None;
        let mut settings = None;
        let mut open_panel = None;
        let scroll = egui::ScrollArea::vertical()
            .id_salt("extension-list")
            .max_height(
                (ctx.content_rect().height()
                    - if self.extensions.settings_id.is_empty() {
                        250.
                    } else {
                        400.
                    })
                .clamp(100., 360.),
            );
        let scroll = if filter.changed() {
            scroll.vertical_scroll_offset(0.)
        } else {
            scroll
        };
        scroll.show(ui, |ui| {
            for entry in entries {
                let included = entry.included();
                ui.push_id(&entry.id, |ui| {
                    layout::card(ui, palette, |ui| {
                        let mut enabled = self.extensions.registry.enabled(&entry.id);
                        ui.horizontal_wrapped(|ui| {
                            let checkbox = crate::keyboard::checkbox(ui, &mut enabled, &entry.name);
                            ui_probe::button(
                                ui,
                                &format!("extension-toggle:{}", entry.id),
                                &checkbox,
                            );
                            if checkbox.changed() {
                                toggle = Some((entry.id.clone(), enabled));
                            }
                            ui.small(if included { "INCLUDED" } else { "INSTALLED" });
                            if entry.panel().is_some()
                                && enabled
                                && crate::keyboard::small_button(ui, "Open panel").clicked()
                            {
                                open_panel = Some(entry.id.clone());
                            }
                            if entry.manifest.is_some() || matches!(entry.feature, Some(Feature::ProjectBookmarks)) {
                                let edit = crate::keyboard::small_button(ui, "Settings");
                                ui_probe::button(ui, &format!("extension-edit-settings:{}", entry.id), &edit);
                                if edit.clicked() { settings = Some(entry.id.clone()); }
                            }
                        });
                        ui.label(&entry.description);
                        ui.small(format!("{} · {}", entry.id, entry.activation));
                        if let Some(manifest) = &entry.manifest {
                            if let Some(panel) = manifest.panel() {
                                ui.small("Desktop · Ordinary access to this computer's files, network and credentials; independent of the project sandbox.");
                                ui.monospace(format!("{} {}", panel.tool.program, panel.tool.args.join(" ")));
                            }
                            if let Some(tool) = manifest.project_tool() {
                                ui.small("Project host · Runs locally or over SSH with the project's current sandbox and access policy. No automatic upload.");
                                ui.monospace(format!("{} {}", tool.program, tool.args.join(" ")));
                            }
                        }
                    });
                    ui.add_space(8.);
                });
            }
        });
        if let Some(id) = open_panel {
            self.set_dialog(Dialog::None);
            self.open_extension_panel(&id, ctx);
        }
        if let Some((id, enabled)) = toggle {
            self.request_extension(&id, enabled, ctx);
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
            let settings_input = ui.add(crate::keyboard::editor_named(
                "Extension settings JSON",
                egui::TextEdit::multiline(&mut self.extensions.settings_text)
                    .code_editor()
                    .desired_rows(4)
                    .char_limit(32 * 1024),
            ));
            ui_probe::button(ui, "extension-settings-json", &settings_input);
            ui_probe::value(
                ui,
                "extension-settings-json-value",
                &settings_input,
                &self.extensions.settings_text,
            );
            let save = crate::keyboard::button(ui, "Save extension settings");
            ui_probe::button(ui, "extension-save-settings", &save);
            if save.clicked() {
                match serde_json::from_str::<std::collections::BTreeMap<String, serde_json::Value>>(
                    &self.extensions.settings_text,
                ) {
                    Ok(settings) => {
                        let id = self.extensions.settings_id.clone();
                        match self.save_extension_settings(&id, settings) {
                            Ok(()) => self.extensions.settings_id.clear(),
                            Err(error) => self.notify(format!("Invalid settings: {error}"), true),
                        }
                    }
                    Err(error) => self.notify(format!("Invalid settings: {error}"), true),
                }
            }
        }
        ui.small("Review each contribution's execution location and access before enabling it. Executables must already be installed on their declared host.");
        ui.add_space(8.);
        ui.horizontal_wrapped(|ui| {
            ui.add(crate::keyboard::named("Desktop path to extension.json", egui::TextEdit::singleline(&mut self.extensions.install_path).hint_text("Desktop path to extension.json").desired_width(ui.available_width().min(320.))));
            if crate::keyboard::button(ui, "Install manifest").clicked() {
                match api::install(&files::expand_home(&self.extensions.install_path), &api::directory()) {
                    Ok(()) => { self.reload_extensions(); self.notify("Extension manifest installed, initially disabled. Review it before enabling.",false); }
                    Err(error) => self.notify(format!("{error:#}"),true),
                }
            }
            if crate::keyboard::button(ui, "Reload").clicked() { self.reload_extensions(); }
        });
        for warning in &self.extensions.registry.warnings {
            ui.colored_label(self.themes.active().palette.error, warning);
        }
        if crate::keyboard::button(ui, "Close").clicked() {
            self.dialog = Dialog::None;
            self.focus_editor(ctx);
        }
    }
    /// Desktop settings notify only their running provider. Project contributions
    /// additionally invalidate captured formatter/command work under the old settings.
    pub(super) fn save_extension_settings(
        &mut self,
        id: &str,
        settings: BTreeMap<String, serde_json::Value>,
    ) -> anyhow::Result<()> {
        let revision = self.extensions.registry.revision;
        self.extensions.registry.set_settings(id, settings)?;
        if self.extensions.registry.revision != revision {
            self.cancel_extensions();
            self.workbench.invalidate_tools();
        }
        Ok(())
    }
    pub(super) fn reload_extensions(&mut self) {
        self.reload_extensions_from(&api::directory());
    }
    pub(super) fn reload_extensions_from(&mut self, directory: &std::path::Path) {
        let old = &mut self.extensions.registry;
        let mut registry = api::Registry::load(directory, old.preferences.clone());
        let changed = !old.same_project_tools(&registry);
        registry.revision = old.revision.wrapping_add(u64::from(changed));
        registry.pending = old.pending;
        self.extensions.registry = registry;
        if changed {
            self.cancel_extensions();
            self.workbench.invalidate_tools();
        }
    }
    /// Validate and dispatch an explicit installed-adapter command using its captured registry revision.
    pub(super) fn extension_command(
        &mut self,
        extension: &str,
        command: &str,
        ctx: &egui::Context,
    ) {
        if command == "@panel.open" {
            self.open_extension_panel(extension, ctx);
            return;
        }
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
            .filter(|m| {
                m.project_tool()
                    .is_some_and(|tool| tool.commands.iter().any(|c| c.id == command))
            })
        else {
            return;
        };
        let doc = self.documents.active().filter(|d| !d.is_preview());
        let invocation = registry.invocation(
            manifest,
            command.into(),
            doc.and_then(|d| d.path.clone()),
            doc.map(|d| d.text.clone()).unwrap_or_default(),
        );
        let invocation = match invocation {
            Ok(invocation) => invocation,
            Err(error) => {
                self.notify(error.to_string(), true);
                return;
            }
        };
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
    /// Render retained adapter output independently of whether its configuration dialog is open.
    pub(super) fn extension_windows(&mut self, ctx: &egui::Context) {
        if self.extensions.output_open {
            egui::Window::new("Extension output")
                .default_size([700., 400.])
                .frame(layout::dialog_frame(
                    &ctx.style(),
                    self.themes.active().palette,
                ))
                .open(&mut self.extensions.output_open)
                .show(ctx, |ui| {
                    layout::heading(
                        ui,
                        "Extension output",
                        "Results from the project-host tool adapter",
                        self.themes.active().palette,
                    );
                    egui::ScrollArea::both().show(ui, |ui| {
                        let output = ui.add(crate::keyboard::editor_named(
                            "Extension output",
                            egui::TextEdit::multiline(&mut self.extensions.output.as_str())
                                .code_editor()
                                .desired_width(f32::INFINITY),
                        ));
                        ui_probe::value(
                            ui,
                            "extension-output-value",
                            &output,
                            &self.extensions.output,
                        );
                    });
                });
        }
    }
}
