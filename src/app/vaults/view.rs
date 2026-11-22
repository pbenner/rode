//! Credentials in the shared project picker; existing-mount management stays
//! separate from opening and cannot change project sandbox options.
use super::*;

/// Narrow form access: the project picker can edit credentials and tool paths,
/// but cannot mutate mount leases or their lifecycle.
pub(in crate::app) struct Opening<'a> {
    state: &'a mut State,
}

impl State {
    pub(in crate::app) fn opening(&mut self) -> Opening<'_> {
        Opening { state: self }
    }
}

impl Opening<'_> {
    pub(in crate::app) fn busy(&self) -> bool {
        self.state.pending.is_some()
            || self.state.staged.is_some()
            || !self.state.closing.is_empty()
    }

    pub(in crate::app) fn message(&self) -> &str {
        &self.state.message
    }

    pub(in crate::app) fn create(&self) -> bool {
        self.state.create
    }

    pub(in crate::app) fn clear_credentials(&mut self) {
        self.state.password.clear();
        self.state.confirmation.clear();
    }

    /// Render only encryption-specific inputs. Folder, host, sandbox selection
    /// and submission belong to the standard project picker.
    pub(in crate::app) fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let state = &mut self.state;
        ui.small("Select gocryptfs storage. It is unlocked on the selected host before the project sandbox is prepared. Passwords are never saved.");
        let create = crate::keyboard::checkbox(
            ui,
            &mut state.create,
            "Create new encrypted storage in an empty folder",
        );
        ui_probe::button(ui, "vault-create", &create);
        ui.label("Password");
        password_field(ui, &mut state.password, "vault-password", "Password");
        if state.create {
            ui.label("Confirm new password");
            password_field(
                ui,
                &mut state.confirmation,
                "vault-confirmation",
                "Confirm new password",
            );
        } else {
            ui.small("Leave blank to reuse an already-unlocked RODE mount.");
        }
        let mut recover = false;
        let advanced = crate::keyboard::CollapsingHeader::new("Encryption tools and recovery")
            .id_salt("vault-advanced")
            .show(ui, |ui| {
                for (label, value, name, hint) in [
                    (
                        "gocryptfs executable",
                        &mut state.settings.program,
                        "vault-program",
                        "gocryptfs",
                    ),
                    (
                        "Unmount executable (blank: automatic)",
                        &mut state.settings.unmount_program,
                        "vault-unmount",
                        "fusermount3",
                    ),
                ] {
                    ui.label(label);
                    let field = ui.add(crate::keyboard::named(
                        label,
                        egui::TextEdit::singleline(value)
                            .desired_width(f32::INFINITY)
                            .hint_text(hint),
                    ));
                    ui_probe::button(ui, name, &field);
                    ui_probe::value(ui, &format!("{name}-value"), &field, value);
                }
                let lock = crate::keyboard::button(ui, "Lock existing mount");
                ui_probe::button(ui, "vault-lock-existing", &lock);
                recover = lock.clicked();
            });
        ui_probe::button(ui, "vault-advanced", &advanced.header_response);
        recover
    }
}

impl EditorApp {
    /// Manage an active or interrupted mount. Opening always uses Dialog::Folder.
    pub(in crate::app) fn encrypted_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.vaults.locking {
            ui.heading("Locking encrypted project");
            ui.spinner();
            ui.label("Waiting for operations or the project host…");
            let cancel = if self.vaults.lost {
                "Cancel verification"
            } else {
                "Keep unlocked"
            };
            if !self.vaults.lock_sent && crate::keyboard::button(ui, cancel).clicked() {
                self.vaults.locking = false;
                self.vaults.after_lock = AfterLock::Project;
                self.dialog = Dialog::None;
            }
            return;
        }
        let palette = self.themes.active().palette;
        ui.heading("Encrypted project");
        if let Some(info) = self
            .vaults
            .active
            .as_ref()
            .and_then(|client| client.info.clone())
        {
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
            ui.small("Locking closes temporary terminals and document readers. End persistent or shared jobs first, and release other RODE views of this project. Unsaved changes are checked before tabs close.");
            if self.vaults.lost {
                ui.label("The lock state is unknown. Verify and lock the existing mount without reopening the project. For SSH storage, reconnect to its host first.");
                if crate::keyboard::button(ui, "Verify & lock").clicked() {
                    self.guarded(Action::LockEncrypted, ctx);
                }
                if crate::keyboard::button(ui, "Unlock again…").clicked() {
                    self.encrypted_open_dialog();
                }
            } else if crate::keyboard::button(ui, "Stop project tools & lock").clicked() {
                self.guarded(Action::LockEncrypted, ctx);
            }
        } else if self.vaults.needs_lock {
            ui.label(format!(
                "Encrypted storage: {}",
                self.vaults.settings.cipherdir
            ));
            let lock = crate::keyboard::button(ui, "Lock existing mount");
            ui_probe::button(ui, "vault-lock-existing", &lock);
            if lock.clicked() {
                self.open_encrypted(true, ctx);
            }
        }
        if !self.vaults.message.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("vault-active-message")
                .max_height((ctx.content_rect().height() / 3.).max(20.))
                .show(ui, |ui| {
                    ui.label(&self.vaults.message);
                });
        }
        if self.vaults.pending.is_some() || !self.vaults.closing.is_empty() {
            ui.spinner();
            ui.label("Finishing encrypted-project cleanup…");
        }
        if crate::keyboard::button(ui, "Close").clicked() {
            self.dialog = Dialog::None;
        }
    }
}
