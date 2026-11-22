//! Compact encrypted-project form with a browsable storage directory. Tool
//! overrides stay under Advanced; the fixed footer remains visible at small sizes.
use super::*;

impl EditorApp {
    pub(in crate::app) fn encrypted_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.encrypted_folder_ui(ui, ctx) {
            return;
        }
        let palette = self.themes.active().palette;
        let top = ui.cursor().min.y;
        if self.vaults.locking {
            ui.heading("Locking encrypted project");
            ui.spinner();
            ui.label("Waiting for operations or the project host…");
            if !self.vaults.lock_sent && crate::keyboard::button(ui, "Keep unlocked").clicked() {
                self.vaults.locking = false;
                self.vaults.after_lock = AfterLock::Project;
                self.dialog = Dialog::None;
            }
            return;
        }
        if let Some(info) = self
            .vaults
            .active
            .as_ref()
            .and_then(|client| client.info.clone())
        {
            ui.heading("Encrypted project");
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "{} · {}",
                        self.vaults.lock_status(),
                        info.cipherdir
                    ))
                    .color(palette.accent),
                )
                .truncate(),
            )
            .on_hover_text(&info.cipherdir);
            ui.add(egui::Label::new(format!("Project view: {}", info.mountpoint)).truncate())
                .on_hover_text(&info.mountpoint);
            if !self.vaults.message.is_empty() {
                egui::ScrollArea::vertical()
                    .id_salt("vault-active-message")
                    .max_height((ctx.content_rect().height() / 6.).max(20.))
                    .show(ui, |ui| {
                        ui.label(&self.vaults.message);
                    });
            }
            ui.small("Stop terminal jobs before locking. Other RODE windows must release this project first. Locking closes tabs after unsaved-change checks.");
            if crate::keyboard::button(ui, "Stop project tools & lock").clicked() {
                self.guarded(Action::LockEncrypted, ctx);
            }
            if crate::keyboard::button(ui, "Close").clicked() {
                self.dialog = Dialog::None;
            }
            if !self.vaults.lost {
                return;
            }
            ui.separator();
        }
        let busy = self.vaults.pending.is_some() || self.vaults.staged.is_some();
        let connected = self
            .workspace
            .as_ref()
            .is_some_and(|w| w.connected && w.remote.is_some());
        let mut browse = false;
        let mut recover = false;
        let footer = layout::control_height(ui) + 28.;
        let header = ui.cursor().min.y - top;
        egui::ScrollArea::vertical().id_salt("vault-form-body")
            .auto_shrink([false, true])
            .max_height((ctx.content_rect().height() - 80. - footer - header).max(20.))
            .show(ui, |ui| {
                layout::heading(ui, "Open encrypted project", "Choose your encrypted storage, then unlock it with gocryptfs. Passwords are never saved.", palette);
                if !self.vaults.message.is_empty() {
                    let message = ui.label(&self.vaults.message);
                    if !busy && self.vaults.closing.is_empty() {
                        ui_probe::button(ui, "vault-operation-complete", &message);
                    }
                }
                ui.add_enabled_ui(!busy, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Location");
                        if crate::keyboard::selectable_label(ui, !self.vaults.remote, "This computer").clicked() {
                            self.vaults.remote = false;
                            self.vaults.cancel_folder_picker();
                        }
                        let remote = ui.add_enabled(connected, crate::keyboard::Button::selectable(self.vaults.remote, "SSH host"));
                        ui_probe::button(ui, "vault-remote", &remote);
                        if remote.clicked() {
                            self.vaults.remote = true;
                            self.vaults.cancel_folder_picker();
                        }
                        if self.vaults.remote {
                            let host = self.workspace.as_ref().and_then(|w| w.remote.as_ref())
                                .map(|remote| remote.profile.endpoint()).unwrap_or_else(|| "Disconnected".into());
                            ui.label(palette.tag(host));
                        }
                    });
                    ui.add_space(8.);
                    ui.label("Encrypted storage folder");
                    ui.horizontal(|ui| {
                        let button_width = layout::button_width(ui, "Browse…") + ui.spacing().item_spacing.x;
                        let folder = ui.add_sized([(ui.available_width() - button_width).max(1.), layout::control_height(ui)],
                            crate::keyboard::named("Encrypted storage folder", egui::TextEdit::singleline(&mut self.vaults.settings.cipherdir).hint_text("Choose a folder…")));
                        ui_probe::button(ui, "vault-folder", &folder);
                        ui_probe::value(ui, "vault-folder-value", &folder, &self.vaults.settings.cipherdir);
                        if self.dialog_focus {
                            folder.request_focus();
                            self.dialog_focus = false;
                        }
                        let button = crate::keyboard::button(ui, "Browse…");
                        ui_probe::button(ui, "vault-browse", &button);
                        browse = button.clicked();
                    });
                    let create = crate::keyboard::checkbox(ui, &mut self.vaults.create, "Create new encrypted storage in an empty folder");
                    ui_probe::button(ui, "vault-create", &create);
                    ui.add_space(8.);
                    ui.label("Password");
                    password_field(ui, &mut self.vaults.password, "vault-password", "Password");
                    if self.vaults.create {
                        ui.label("Confirm new password");
                        password_field(ui, &mut self.vaults.confirmation, "vault-confirmation", "Confirm new password");
                    } else {
                        ui.small("Leave blank to reuse an already-unlocked RODE mount.");
                    }
                    ui.add_space(8.);
                    crate::keyboard::checkbox(ui, &mut self.vaults.options.enabled, "Sandbox the unlocked project when available");
                    let advanced = crate::keyboard::CollapsingHeader::new("Advanced options").id_salt("vault-advanced").show(ui, |ui| {
                        for (label, value, name, hint) in [
                            ("gocryptfs executable", &mut self.vaults.settings.program, "vault-program", "gocryptfs"),
                            ("Unmount executable (blank: automatic)", &mut self.vaults.settings.unmount_program, "vault-unmount", "fusermount3"),
                        ] {
                            ui.label(label);
                            let field = ui.add(crate::keyboard::named(label, egui::TextEdit::singleline(value).desired_width(f32::INFINITY).hint_text(hint)));
                            ui_probe::button(ui, name, &field);
                            ui_probe::value(ui, &format!("{name}-value"), &field, value);
                        }
                        let lock = crate::keyboard::button(ui, "Lock existing mount");
                        ui_probe::button(ui, "vault-lock-existing", &lock);
                        recover = lock.clicked();
                    });
                    ui_probe::button(ui, "vault-advanced", &advanced.header_response);
                });
                if busy || !self.vaults.closing.is_empty() {
                    ui.horizontal(|ui| { ui.spinner(); ui.label(if busy { "Unlocking…" } else { "Finishing encrypted-project cleanup…" }); });
                }
            });
        ui.separator();
        let mut submit = false;
        let mut close = false;
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(
                !busy
                    && !self.vaults.settings.cipherdir.trim().is_empty()
                    && (!self.vaults.remote || connected),
                |ui| {
                    let unlock = palette.primary(
                        ui,
                        if self.vaults.create {
                            "Create & unlock"
                        } else {
                            "Unlock & open"
                        },
                    );
                    ui_probe::button(ui, "vault-unlock", &unlock);
                    submit = unlock.clicked();
                },
            );
            let cancel = crate::keyboard::button(ui, if busy { "Cancel unlock" } else { "Cancel" });
            ui_probe::button(ui, "vault-cancel", &cancel);
            close = cancel.clicked();
        });
        if close {
            self.cancel_encrypted_dialog();
            self.dialog = Dialog::None;
        } else if browse {
            self.open_encrypted_folder_picker(ctx);
        } else if recover || submit {
            self.open_encrypted(recover, ctx);
        }
    }
}
