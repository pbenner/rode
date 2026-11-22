//! Encrypted-storage folder discovery on an explicitly selected local/SSH host.
//! The shared native rows handle navigation; listing never mounts a filesystem.
use super::*;

#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct Picker {
    open: bool,
    browser: file_browser::State,
    list: navigation::List,
    filter: String,
}

impl State {
    /// Keep the token across picker lifetimes so a late reply cannot populate a
    /// reopened form, even when its folder has the same spelling.
    pub(in crate::app) fn cancel_folder_picker(&mut self) -> bool {
        let open = self.picker.open;
        self.picker.open = false;
        self.picker.browser.reset();
        self.picker.list = Default::default();
        self.picker.filter.clear();
        open
    }

    pub(in crate::app) fn folder_matches(&self, token: u64, path: &str, remote: bool) -> bool {
        self.picker.open && self.remote == remote && self.picker.browser.matches(token, path)
    }

    pub(in crate::app) fn folder_loaded(&mut self, mut response: Response) {
        response.entries.retain(|entry| entry.dir);
        self.picker.browser.loaded(response);
        self.picker.list = Default::default();
    }

    pub(in crate::app) fn folder_failed(&mut self, error: String) {
        self.picker.browser.loading = false;
        self.picker.browser.error = Some(error);
    }
}

impl EditorApp {
    pub(super) fn open_encrypted_folder_picker(&mut self, ctx: &egui::Context) {
        self.vaults.cancel_folder_picker();
        let path = if !self.vaults.settings.cipherdir.trim().is_empty() {
            self.vaults.settings.cipherdir.clone()
        } else if self.vaults.remote {
            self.root()
        } else {
            self.workspace
                .as_ref()
                .filter(|w| w.remote.is_none())
                .map(|w| w.root.clone())
                .unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into()
                })
        };
        self.vaults.picker.open = true;
        self.vaults.picker.browser.location = path;
        self.vaults.picker.browser.needs_load = true;
        self.dialog_focus = true;
        ctx.request_repaint();
    }

    fn browse_encrypted_folder(&mut self, path: String, ctx: &egui::Context) {
        // A missing/disconnected SSH host is an error, never a local fallback.
        if self.vaults.remote
            && !self
                .workspace
                .as_ref()
                .is_some_and(|w| w.connected && w.remote.is_some())
        {
            self.vaults
                .folder_failed("Connect SSH before browsing folders on that host.".into());
            self.vaults.picker.browser.needs_load = false;
            return;
        }
        let picker = &mut self.vaults.picker;
        let path = if picker.browser.directory.is_empty() {
            path
        } else {
            file_browser::resolve_path(&picker.browser.directory, &path)
        };
        let token = picker.browser.begin(&path);
        picker.list = Default::default();
        picker.filter.clear();
        self.dispatch(
            Request::List { path: path.clone() },
            Job::BrowseEncrypted {
                token,
                path,
                remote: self.vaults.remote,
            },
            ctx,
        );
    }

    /// Return to the existing encrypted-project draft on selection or Cancel;
    /// the password and access options never enter the browser or its worker.
    pub(super) fn encrypted_folder_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
        if !self.vaults.picker.open {
            return false;
        }
        if self.vaults.picker.browser.needs_load {
            self.browse_encrypted_folder(self.vaults.picker.browser.location.clone(), ctx);
        }
        let palette = self.themes.active().palette;
        let host = if self.vaults.remote {
            self.workspace
                .as_ref()
                .and_then(|w| w.remote.as_ref())
                .map_or_else(
                    || "SSH · disconnected".into(),
                    |remote| format!("SSH · {}", remote.profile.endpoint()),
                )
        } else {
            "This computer".into()
        };
        let picker = &mut self.vaults.picker;
        let mut browse = None;
        let mut chosen = None;
        let mut cancel = false;
        let footer = layout::control_height(ui) + 24.;
        egui::ScrollArea::vertical().id_salt("vault-picker-body")
            .max_height((ctx.content_rect().height() - 80. - footer).max(20.))
            .auto_shrink([false, true]).show(ui, |ui| {
                ui.label(palette.tag(&host));
                ui.heading("Choose encrypted storage folder");
                ui.label("Select the folder containing gocryptfs.conf, or an empty folder when creating a new encrypted project.");
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    let up = crate::keyboard::button(ui, "Up");
                    ui_probe::button(ui, "vault-picker-up", &up);
                    if up.clicked() { browse = Some(file_browser::parent(&picker.browser.location)); }
                    let go_width = layout::button_width(ui, "Browse") + ui.spacing().item_spacing.x;
                    let path = ui.add_sized([(ui.available_width() - go_width).max(1.), layout::control_height(ui)],
                        crate::keyboard::named("Folder path", egui::TextEdit::singleline(&mut picker.browser.location)));
                    ui_probe::button(ui, "vault-picker-path", &path);
                    ui_probe::value(ui, "vault-picker-directory", &path, &picker.browser.directory);
                    if path.changed() && picker.browser.loading {
                        picker.browser.token = picker.browser.token.wrapping_add(1);
                        picker.browser.loading = false;
                    }
                    if self.dialog_focus {
                        path.request_focus();
                        self.dialog_focus = false;
                    }
                    let go = crate::keyboard::button(ui, "Browse");
                    ui_probe::button(ui, "vault-picker-go", &go);
                    if go.clicked() || path.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        browse = Some(picker.browser.location.clone());
                    }
                });
                ui.horizontal(|ui| {
                    ui.add(crate::keyboard::named("Filter folders", egui::TextEdit::singleline(&mut picker.filter).hint_text("Filter folders…").desired_width((ui.available_width() - 90.).max(1.))));
                    if crate::keyboard::checkbox(ui, &mut self.show_hidden, "Hidden").changed() {
                        self.explorer_state.tree_dirty = true;
                    }
                });
                if picker.browser.loading { ui.spinner(); }
                if let Some(error) = &picker.browser.error { ui.colored_label(palette.error, error); }
                let query_text = picker.filter.to_lowercase();
                let entries: Vec<_> = picker.browser.entries.iter()
                    .filter(|entry| entry.name.to_lowercase().contains(&query_text))
                    .cloned().collect();
                let ready = !picker.browser.loading && picker.browser.error.is_none()
                    && picker.browser.directory == picker.browser.location && !picker.browser.directory.is_empty();
                let query = format!("{}:{}:{}", picker.browser.token, picker.filter, self.show_hidden);
                let action = file_browser::rows_with_options(ui, "vault-picker-rows", &entries,
                    self.show_hidden, &mut picker.list, &query, (0, false), None, ready,
                    file_browser::RowOptions { icons: true, height: Some((ctx.content_rect().height() - 320.).clamp(100., 320.)), ..Default::default() });
                if let Some(action) = action {
                    match action {
                        file_browser::Choice::Parent => browse = Some(file_browser::parent(&picker.browser.directory)),
                        file_browser::Choice::Open(entry) => browse = Some(entry.path),
                        file_browser::Choice::Select(_) => {}
                    }
                }
                // Use exactly the same filtered/hidden list as the row navigator.
                let selected = picker.list.navigated.then(|| entries.iter()
                    .filter(|entry| self.show_hidden || !entry.name.starts_with('.'))
                    .nth(picker.list.index)).flatten();
                if ready {
                    chosen = Some(selected.map_or_else(|| picker.browser.directory.clone(), |entry| entry.path.clone()));
                }
            });
        ui.separator();
        let mut accept = false;
        ui.horizontal(|ui| {
            let select = ui.add_enabled(
                chosen.is_some() && browse.is_none(),
                crate::keyboard::Button::new("Select folder"),
            );
            ui_probe::button(ui, "vault-picker-select", &select);
            accept = select.clicked();
            let back = crate::keyboard::button(ui, "Cancel");
            ui_probe::button(ui, "vault-picker-cancel", &back);
            cancel = back.clicked();
        });
        if cancel {
            self.vaults.cancel_folder_picker();
            self.dialog_focus = true;
        } else if let Some(path) = browse {
            self.browse_encrypted_folder(path, ctx);
        } else if accept && let Some(path) = chosen {
            self.vaults.settings.cipherdir = path;
            self.vaults.cancel_folder_picker();
            self.dialog_focus = true;
        }
        true
    }
}
