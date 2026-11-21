//! Folder selection reuses transfer endpoints and native directory rows. A
//! picker owns only a draft path; it cannot open a project or start a mount.
use super::{view::Field, *};

pub(super) struct Picker {
    pub serial: u64,
    field: Field,
    side: Side,
    pane: Pane,
    cancel: Arc<AtomicBool>,
    writable: bool,
}
impl Drop for Picker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
impl Picker {
    /// Late or cancelled listings cannot replace a reopened picker's path.
    pub(super) fn loaded(
        &mut self,
        token: u64,
        path: &str,
        result: Result<Box<files::Response>, String>,
    ) {
        let browser = &mut self.pane.browser;
        if browser.token != token || browser.requested != path {
            return;
        }
        browser.loading = false;
        if browser.location != path {
            return;
        }
        match result {
            Ok(response) => {
                browser.loaded(*response);
                self.pane.selected.clear();
                self.pane.list = Default::default();
                self.pane.query.clear();
                self.pane.new_folder.clear();
                self.pane.creating = false;
            }
            Err(error) => browser.error = Some(error),
        }
    }

    fn selected(&self) -> Option<String> {
        if !self.pane.ready() {
            return None;
        }
        self.pane
            .selected
            .iter()
            .next()
            .cloned()
            .filter(|path| {
                self.pane
                    .browser
                    .entries
                    .iter()
                    .any(|entry| entry.dir && entry.path == *path)
            })
            .or_else(|| Some(self.pane.browser.directory.clone()))
    }
}

impl Sharing {
    pub(in crate::app::transfers) fn cancel_picker(&mut self) {
        self.picker = None;
    }
}

impl EditorApp {
    /// Capture the field's host once. Paths with the same spelling on the two
    /// machines must never cause a remote selection to fall back to local I/O.
    pub(super) fn open_shared_folder_picker(&mut self, field: Field, ctx: &egui::Context) {
        let (side, draft) = self.transfers.sharing.view.folder(field);
        let Some(endpoint) = self.transfer_endpoint(side) else {
            return;
        };
        let path = if !draft.trim().is_empty() {
            draft.into()
        } else if self.transfers.panes[side].ready() {
            self.transfers.panes[side].browser.directory.clone()
        } else if side == Side::Local {
            files::expand_home("~").to_string_lossy().into()
        } else {
            self.transfer_host_root().unwrap_or_else(|| "~".into())
        };
        let state = &mut self.transfers.sharing;
        state.picker_serial = state.picker_serial.wrapping_add(1);
        let mut pane = Pane::default();
        pane.browser.location = path;
        pane.browser.needs_load = true;
        state.picker = Some(Picker {
            serial: state.picker_serial,
            field,
            side,
            pane,
            cancel: Arc::default(),
            writable: !endpoint
                .plan()
                .is_some_and(|p| p.active && p.project_read_only),
        });
        self.set_dialog(Dialog::SharedFolder);
        ctx.request_repaint();
    }

    fn browse_shared_folder(&mut self, path: String, create: Option<String>, ctx: &egui::Context) {
        let Some(picker) = &self.transfers.sharing.picker else {
            return;
        };
        let Some(endpoint) = self.transfer_endpoint(picker.side) else {
            return;
        };
        let state = &mut self.transfers.sharing;
        let picker = state.picker.as_mut().unwrap();
        if picker.pane.browser.loading {
            return;
        }
        if let Some(folder) = &create {
            let destination = std::path::Path::new(folder);
            if picker.field != Field::Destination
                || !picker.writable
                || !picker.pane.ready()
                || folder != &path
                || destination.parent()
                    != Some(std::path::Path::new(&picker.pane.browser.directory))
                || !destination
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(explorer::valid_name)
            {
                return;
            }
        }
        picker.pane.browser.location = path.clone();
        if state.browsing.swap(true, Ordering::AcqRel) {
            // A cancelled picker may still be releasing its SSH channel. Keep
            // one listing worker, and retry after that worker requests repaint.
            if create.is_none() {
                picker.pane.browser.needs_load = true;
            } else {
                picker
                    .pane
                    .browser
                    .location
                    .clone_from(&picker.pane.browser.directory);
                picker.pane.browser.error = Some(
                    "The previous folder operation is still finishing. Try Create again.".into(),
                );
            }
            return;
        }
        picker.pane.browser.needs_load = false;
        picker.pane.browser.token = picker.pane.browser.token.wrapping_add(1);
        picker.pane.browser.requested = path.clone();
        picker.pane.browser.loading = true;
        picker.pane.browser.error = None;
        picker.pane.selected.clear();
        picker.cancel.store(true, Ordering::Release);
        picker.cancel = Arc::default();
        let cancel = picker.cancel.clone();
        let serial = picker.serial;
        let token = picker.pane.browser.token;
        let generation = state.generation;
        let tx = state.tx.clone();
        let active = Listing(state.browsing.clone());
        let pending = Pending::new(&self.transfers.pending);
        let work = self.vaults.work();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = list_directory(endpoint, &path, create, cancel)
                .map(Box::new)
                .map_err(|error| format!("{error:#}"));
            drop(work);
            drop(pending);
            drop(active);
            let _ = tx.send(Reply::Listed {
                generation,
                picker: serial,
                token,
                path,
                result,
            });
            ctx.request_repaint();
        });
    }

    /// The ordinary modal host supplies viewport ownership, Escape and keyboard
    /// barriers. Directory navigation shares rows with project/file pickers.
    pub(in crate::app) fn shared_folder_picker(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(picker) = &self.transfers.sharing.picker
            && picker.pane.browser.needs_load
            && !picker.pane.browser.loading
        {
            self.browse_shared_folder(picker.pane.browser.location.clone(), None, ctx);
        }
        let palette = self.themes.active().palette;
        let host = self.transfer_host_label();
        let Some(picker) = &mut self.transfers.sharing.picker else {
            self.dialog = Dialog::None;
            return;
        };
        let mut browse = None;
        let mut create = None;
        // Keep confirmation outside the scrolling body, even at large interface
        // scales. The budget uses themed control/text metrics rather than a row count.
        let footer_height =
            layout::control_height(ui) + ui.text_style_height(&egui::TextStyle::Body) + 24.;
        egui::ScrollArea::vertical()
            .id_salt("shared-folder-picker-body")
            .auto_shrink([false, true])
            .max_height((ctx.content_rect().height() - 80. - footer_height).max(20.))
            .show(ui, |ui| {
                let host = ui.label(palette.tag(if picker.side == Side::Local {
                    "This computer"
                } else {
                    &host
                }));
                ui_probe::value(
                    ui,
                    "share-picker-host",
                    &host,
                    if picker.side == Side::Local {
                        "local"
                    } else {
                        "remote"
                    },
                );
                ui.heading(if picker.field == Field::Source {
                    "Choose source folder"
                } else {
                    "Choose mount folder"
                });
                ui.weak(if picker.field == Field::Source {
                    "Choose the folder whose files you want to share."
                } else {
                    "Choose an empty folder on the receiving machine, or create one below."
                });
                let pane = &mut picker.pane;
                ui.add_space(8.);
                ui.horizontal_wrapped(|ui| {
                    for (label, path, name) in [
                        (
                            "Up",
                            file_browser::parent(&pane.browser.directory),
                            "share-picker-up",
                        ),
                        ("Home", "~".into(), "share-picker-home"),
                    ] {
                        let button = ui.add_enabled(
                            !pane.browser.loading,
                            crate::keyboard::Button::new(label),
                        );
                        ui_probe::button(ui, name, &button);
                        if button.clicked() {
                            browse = Some(path);
                        }
                    }
                    if crate::keyboard::checkbox(ui, &mut pane.hidden, "Hidden folders").changed() {
                        pane.selected.clear();
                    }
                });
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let go = ui
                            .add_enabled(!pane.browser.loading, crate::keyboard::Button::new("Go"));
                        ui_probe::button(ui, "share-picker-go", &go);
                        if go.clicked() {
                            browse = Some(pane.browser.location.clone());
                        }
                        let path = ui
                            .add_enabled_ui(!pane.browser.loading, |ui| {
                                ui.add_sized(
                                    [ui.available_width().max(1.), layout::control_height(ui)],
                                    crate::keyboard::named(
                                        "Folder path",
                                        egui::TextEdit::singleline(&mut pane.browser.location)
                                            .desired_width(f32::INFINITY),
                                    ),
                                )
                            })
                            .inner;
                        ui_probe::button(ui, "share-picker-path", &path);
                        ui_probe::value(
                            ui,
                            "share-picker-path-value",
                            &path,
                            &pane.browser.location,
                        );
                        if self.dialog_focus && path.enabled() {
                            path.request_focus();
                            self.dialog_focus = false;
                        }
                        if path.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            browse = Some(pane.browser.location.clone());
                        }
                    });
                });
                let query = ui.add_sized(
                    [ui.available_width().max(1.), layout::control_height(ui)],
                    crate::keyboard::named(
                        "Filter folders",
                        egui::TextEdit::singleline(&mut pane.query).hint_text("Filter folders…"),
                    ),
                );
                ui_probe::button(ui, "share-picker-filter", &query);
                ui_probe::value(ui, "share-picker-filter-value", &query, &pane.query);
                if query.changed() {
                    pane.selected.clear();
                }
                if pane.browser.loading {
                    ui.spinner();
                }
                if let Some(error) = &pane.browser.error {
                    ui.colored_label(palette.error, error);
                }
                let query = pane.query.to_lowercase();
                let entries: Vec<_> = pane
                    .browser
                    .entries
                    .iter()
                    .filter(|e| e.dir && e.name.to_lowercase().contains(&query))
                    .cloned()
                    .collect();
                let ready = pane.ready();
                let action = file_browser::rows_with_options(
                    ui,
                    "share-picker",
                    &entries,
                    pane.hidden,
                    &mut pane.list,
                    &pane.query,
                    (0, false),
                    pane.selected.iter().next().map(String::as_str),
                    ready,
                    file_browser::RowOptions {
                        height: Some((ctx.content_rect().height() - 370.).clamp(60., 320.)),
                        icons: true,
                        ..Default::default()
                    },
                );
                match action {
                    Some(file_browser::Choice::Select(entry)) => {
                        pane.selected.clear();
                        pane.selected.insert(entry.path);
                    }
                    Some(file_browser::Choice::Open(entry)) => browse = Some(entry.path),
                    Some(file_browser::Choice::Parent) => {
                        browse = Some(file_browser::parent(&pane.browser.directory))
                    }
                    None => {}
                }
                if pane.ready() {
                    ui_probe::value(
                        ui,
                        "share-picker-directory",
                        &ui.response(),
                        &pane.browser.directory,
                    );
                    if entries.is_empty() {
                        ui.weak("No matching folders. You can select the current folder.");
                    }
                }
                if picker.field == Field::Destination {
                    ui.horizontal_wrapped(|ui| {
                        let new = ui.add_enabled(
                            picker.writable && pane.ready(),
                            crate::keyboard::Button::new("New folder"),
                        );
                        ui_probe::button(ui, "share-picker-new", &new);
                        if new.clicked() {
                            pane.creating = true;
                        }
                        if pane.creating {
                            let name = ui.add(crate::keyboard::named(
                                "New folder name",
                                egui::TextEdit::singleline(&mut pane.new_folder)
                                    .desired_width(ui.available_width().clamp(40., 180.)),
                            ));
                            ui_probe::button(ui, "share-picker-name", &name);
                            ui_probe::value(ui, "share-picker-name-value", &name, &pane.new_folder);
                            let button = ui.add_enabled(
                                picker.writable
                                    && pane.ready()
                                    && explorer::valid_name(&pane.new_folder),
                                crate::keyboard::Button::new("Create"),
                            );
                            ui_probe::button(ui, "share-picker-create", &button);
                            if button.clicked() {
                                create = Some(
                                    std::path::Path::new(&pane.browser.directory)
                                        .join(&pane.new_folder)
                                        .to_string_lossy()
                                        .into_owned(),
                                );
                            }
                        }
                    });
                }
            });
        let selected = picker.selected();
        if let Some(path) = &selected {
            ui.add(egui::Label::new(path).truncate())
                .on_hover_text(path);
        }
        ui.separator();
        let mut accept = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            let choose = ui.add_enabled(
                selected.is_some(),
                crate::keyboard::Button::new("Select folder"),
            );
            ui_probe::button(ui, "share-picker-select", &choose);
            accept = choose.clicked();
            let close = crate::keyboard::button(ui, "Cancel");
            ui_probe::button(ui, "share-picker-cancel", &close);
            cancel = close.clicked();
        });
        let field = picker.field;
        if accept && let Some(path) = selected {
            self.transfers.sharing.view.select_folder(field, path);
            self.dialog = Dialog::None;
            self.transfers.sharing.cancel_picker();
        } else if cancel {
            self.dialog = Dialog::None;
            self.transfers.sharing.cancel_picker();
        } else if let Some(path) = create {
            self.browse_shared_folder(path.clone(), Some(path), ctx);
        } else if let Some(path) = browse {
            self.browse_shared_folder(path, None, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker() -> Picker {
        let mut pane = Pane::default();
        pane.browser.token = 7;
        pane.browser.requested = "/current".into();
        pane.browser.location = "/current".into();
        pane.browser.loading = true;
        Picker {
            serial: 1,
            field: Field::Source,
            side: Side::Local,
            pane,
            cancel: Arc::default(),
            writable: true,
        }
    }

    #[test]
    fn picker_rejects_old_results_and_cancels_owned_directory_work_on_close() {
        let mut picker = picker();
        let reply = || {
            Ok(Box::new(files::Response {
                path: "/canonical".into(),
                ..Default::default()
            }))
        };
        picker.loaded(6, "/current", reply());
        picker.loaded(7, "/other", reply());
        assert!(picker.pane.browser.loading);
        assert!(picker.selected().is_none());
        picker.pane.browser.location = "/typed".into();
        picker.loaded(7, "/current", reply());
        assert!(!picker.pane.browser.loading);
        assert_eq!(picker.pane.browser.location, "/typed");
        assert!(picker.selected().is_none());
        picker.pane.browser.location = "/current".into();
        picker.loaded(7, "/current", reply());
        assert_eq!(picker.selected().as_deref(), Some("/canonical"));
        let cancel = picker.cancel.clone();
        drop(picker);
        assert!(cancel.load(Ordering::Acquire));
    }

    #[test]
    fn closed_and_reopened_pickers_reject_previous_host_replies() {
        let ctx = egui::Context::default();
        let mut app = EditorApp::new(
            &eframe::CreationContext::_new_kittest(ctx),
            Startup::default(),
        );
        app.transfers.sharing.picker = Some(picker());
        let state = &mut app.transfers.sharing;
        for (generation, serial) in [(state.generation.wrapping_add(1), 1), (state.generation, 0)] {
            state
                .tx
                .send(Reply::Listed {
                    generation,
                    picker: serial,
                    token: 7,
                    path: "/current".into(),
                    result: Ok(Box::new(files::Response {
                        path: "/old-host".into(),
                        ..Default::default()
                    })),
                })
                .unwrap();
        }
        app.poll_shared_folders();
        assert!(
            app.transfers
                .sharing
                .picker
                .as_ref()
                .unwrap()
                .pane
                .browser
                .directory
                .is_empty()
        );
        app.transfers.sharing.reset();
        assert!(app.transfers.sharing.picker.is_none());
    }

    #[test]
    fn picker_confirmation_stays_visible_in_compact_scaled_windows() {
        for (size, scale) in [
            (egui::vec2(850., 560.), 1.),
            (egui::vec2(850., 560.), 1.5),
            (egui::vec2(640., 420.), 1.),
        ] {
            let ctx = egui::Context::default();
            let mut app = EditorApp::new(
                &eframe::CreationContext::_new_kittest(ctx.clone()),
                Startup::default(),
            );
            ctx.set_zoom_factor(scale);
            ctx.style_mut(|style| style.animation_time = 0.);
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("picker.json");
            ui_probe::install_at(&ctx, path.clone());
            let mut picker = picker();
            picker.field = Field::Destination;
            picker.loaded(
                7,
                "/current",
                Ok(Box::new(files::Response {
                    path: "/current".into(),
                    entries: (0..50)
                        .map(|index| Entry {
                            path: format!("/current/folder-{index}"),
                            name: format!("folder-{index}"),
                            dir: true,
                        })
                        .collect(),
                    ..Default::default()
                })),
            );
            app.transfers.sharing.picker = Some(picker);
            app.set_dialog(Dialog::SharedFolder);
            for _ in 0..4 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| app.dialogs(ctx),
                );
            }
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let rect = |name: &str| {
                let button = state["buttons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|button| button["name"] == name)
                    .unwrap_or_else(|| panic!("Missing {name}: {state}"));
                assert_eq!(button["enabled"], true, "{state}");
                let [left, top, right, bottom]: [f32; 4] =
                    serde_json::from_value(button["rect"].clone()).unwrap();
                egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
            };
            let select = rect("share-picker-select");
            let cancel = rect("share-picker-cancel");
            assert!(
                ctx.content_rect().contains_rect(select),
                "{size}/{scale}: {state}"
            );
            assert!(
                ctx.content_rect().contains_rect(cancel),
                "{size}/{scale}: {state}"
            );
            assert!(!select.intersects(cancel), "{size}/{scale}: {state}");
            if scale == 1. && size.y == 560. {
                let path = rect("share-picker-path");
                let go = rect("share-picker-go");
                let filter = rect("share-picker-filter");
                assert!((path.center().y - go.center().y).abs() < 1., "{state}");
                assert!(
                    path.bottom() <= filter.top() && filter.bottom() < select.top(),
                    "{state}"
                );
            }
        }
    }
}
