//! Interface scale, theme menus and immediate appearance preference updates.
//!
//! Interface and syntax choices remain separate; their writes share the normal
//! preference merge path and do not alter document contents or editor font size.

use super::*;

impl EditorApp {
    /// Apply a finite 50–200% interface zoom rounded to one percentage point;
    /// invalid input resets to 100%. Mark the new value for persistence.
    pub(super) fn set_ui_scale(&mut self, scale: f32, ctx: &egui::Context) {
        self.ui_scale = if scale.is_finite() {
            (scale.clamp(0.5, 2.0) * 100.).round() / 100.
        } else {
            1.0
        };
        ctx.set_zoom_factor(self.ui_scale);
        self.ui_scale_save_pending = true;
    }

    /// Queue a changed interface scale in preference storage; report unavailable storage
    /// without undoing the already applied zoom.
    pub(super) fn persist_ui_scale(
        &mut self,
        storage: Option<&mut (dyn eframe::Storage + 'static)>,
    ) {
        if !std::mem::take(&mut self.ui_scale_save_pending) {
            return;
        }
        if let Some(storage) = storage {
            storage.set_string("ui_scale", self.ui_scale.to_string());
            storage.flush();
        } else {
            self.notify("Interface scale changed, but could not be saved.", true);
        }
    }

    /// Render discrete zoom actions and presets; completed clicks close the menu
    /// before scaling moves its controls on the next frame.
    pub(super) fn ui_scale_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let percent = (self.ui_scale * 100.).round() as u16;
        // Scaling moves every widget. Use completed clicks and close the menu
        // before the next frame applies the zoom, rather than dragging a
        // control whose geometry changes underneath the pointer.
        icons::submenu(
            ui,
            Icon::Settings,
            &format!("Interface scale: {percent}%"),
            |ui| {
                ui.small("Relative to system display scaling");
                for (icon, label, step, enabled) in [
                    (
                        Icon::ChevronLeft,
                        "Smaller (5%)",
                        -0.05,
                        self.ui_scale > 0.5,
                    ),
                    (Icon::ChevronRight, "Larger (5%)", 0.05, self.ui_scale < 2.0),
                ] {
                    if ui
                        .add_enabled(enabled, IconButton::new(icon, label))
                        .clicked()
                    {
                        self.set_ui_scale(self.ui_scale + step, ctx);
                        ui.close();
                    }
                }
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(260.)
                    .show(ui, |ui| {
                        for preset in [50, 75, 90, 100, 110, 125, 150, 175, 200] {
                            let label = if preset == 100 {
                                "100% (system default)".into()
                            } else {
                                format!("{preset}%")
                            };
                            if ui.selectable_label(percent == preset, label).clicked() {
                                self.set_ui_scale(f32::from(preset) / 100., ctx);
                                ui.close();
                            }
                        }
                    });
            },
        );
        if ui
            .add(IconButton::new(Icon::Refresh, "Reset interface scale (100%)").shortcut("Ctrl+0"))
            .clicked()
        {
            self.set_ui_scale(1., ctx);
            ui.close();
        }
    }

    /// Render interface-theme choices and apply only an explicit changed selection.
    pub(super) fn theme_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut selected = self.themes.active().id.clone();
        egui::ScrollArea::vertical()
            .max_height(280.0)
            .show(ui, |ui| {
                for entry in &self.themes.entries {
                    ui.radio_value(&mut selected, entry.id.clone(), &entry.name);
                }
            });
        if selected != self.themes.active().id {
            self.select_theme(&selected, ctx);
            ui.close();
        }
    }

    /// Render the independent syntax-theme selector without changing interface colors.
    pub(super) fn syntax_theme_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut selected = self.themes.active_syntax().id.clone();
        ui.label(RichText::new("Syntax colors and styles").small());
        egui::ScrollArea::vertical()
            .id_salt("syntax-theme-choices")
            .max_height(280.)
            .show(ui, |ui| {
                for entry in &self.themes.syntax_entries {
                    ui.radio_value(&mut selected, entry.id.clone(), &entry.name);
                }
            });
        if selected != self.themes.active_syntax().id {
            self.select_syntax_theme(&selected, ctx);
            ui.close();
        }
    }

    /// Share interface and syntax choices between the footer and other appearance menus.
    pub(super) fn appearance_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(RichText::new("Interface theme").small());
        self.theme_menu(ui, ctx);
        ui.separator();
        icons::submenu(ui, Icon::Palette, "Syntax highlighting", |ui| {
            self.syntax_theme_menu(ui, ctx)
        });
    }

    /// Apply a known interface theme and mark its preference for saving; expose errors as notices.
    pub(super) fn select_theme(&mut self, id: &str, ctx: &egui::Context) {
        match self.themes.select(id, ctx) {
            Ok(()) => {
                self.status = format!("Theme: {}", self.themes.active().name);
                self.theme_save_pending = true;
            }
            Err(error) => self.notify(error.to_string(), true),
        }
    }

    /// Apply a known syntax theme and queue its independent saved selection.
    pub(super) fn select_syntax_theme(&mut self, id: &str, ctx: &egui::Context) {
        match self.themes.select_syntax(id, ctx) {
            Ok(()) => {
                self.status = format!("Syntax theme: {}", self.themes.active_syntax().name);
                self.syntax_save_pending = true;
            }
            Err(error) => self.notify(error.to_string(), true),
        }
    }

    /// Queue only changed interface/syntax identifiers and request a storage flush.
    /// Unavailable storage is reported while the in-memory theme remains applied.
    pub(super) fn persist_theme_choice(
        &mut self,
        storage: Option<&mut (dyn eframe::Storage + 'static)>,
    ) {
        if !self.theme_save_pending && !self.syntax_save_pending {
            return;
        }
        let save_ui = std::mem::take(&mut self.theme_save_pending);
        let save_syntax = std::mem::take(&mut self.syntax_save_pending);
        if let Some(storage) = storage {
            if save_ui {
                storage.set_string(theme::STORAGE_KEY, self.themes.active().id.clone());
            }
            if save_syntax {
                storage.set_string(
                    theme::syntax::STORAGE_KEY,
                    self.themes.active_syntax().id.clone(),
                );
            }
            storage.flush();
        } else {
            self.notify("Theme applied, but local preference storage is unavailable. The selection could not be saved.", true);
        }
    }
}
