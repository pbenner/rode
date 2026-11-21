//! Project/host-bound shared-folder recipes and worker acknowledgements. Saved
//! recipes are inert: only Mount starts a service, and a bookmark restores settings.
use super::*;
use crate::shared_folders::{Config, Control, Direction, Event as MountEvent};

struct Row {
    id: u64,
    config: Config,
    running: Option<Arc<Control>>,
    ready: bool,
    unmounting: bool,
    message: String,
}
enum Reply {
    Status {
        generation: u64,
        id: u64,
        event: MountEvent,
    },
    Finished {
        generation: u64,
        id: u64,
        result: Result<(), String>,
    },
}
pub(in crate::app) struct Sharing {
    pub open: bool,
    draft: Config,
    adding: bool,
    rows: Vec<Row>,
    serial: u64,
    generation: u64,
    pending: Arc<AtomicUsize>,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
}
impl Default for Sharing {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            open: false,
            draft: Default::default(),
            adding: false,
            rows: vec![],
            serial: 0,
            generation: 0,
            pending: Arc::default(),
            tx,
            rx,
        }
    }
}
impl Sharing {
    pub(in crate::app) fn pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0
    }
    pub(in crate::app) fn configs(&self) -> Vec<Config> {
        self.rows.iter().map(|r| r.config.clone()).collect()
    }
    pub(in crate::app) fn reset(&mut self) {
        for row in &self.rows {
            if let Some(control) = &row.running {
                control.cancel.store(true, Ordering::Release);
            }
        }
        self.generation = self.generation.wrapping_add(1);
        self.rows.clear();
        self.adding = false;
        self.draft = Default::default();
    }
    pub(in crate::app) fn restore(&mut self, configs: Vec<Config>) {
        self.reset();
        for config in configs {
            self.insert(config);
        }
        self.open = !self.rows.is_empty();
    }
    fn insert(&mut self, config: Config) {
        self.serial = self.serial.wrapping_add(1);
        self.rows.push(Row {
            id: self.serial,
            config,
            running: None,
            ready: false,
            unmounting: false,
            message: "Not mounted".into(),
        });
    }
}

impl Drop for Sharing {
    fn drop(&mut self) {
        self.reset();
    }
}

impl EditorApp {
    fn start_shared_folder(&mut self, id: u64, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Transfers)
            || self.vaults.locking()
            || self.exit_pending()
            || self.pending.as_ref().is_some()
            || self.dialog != Dialog::None
            || self.foreign_modal(ctx)
            || self.workspace.preparing()
            || self.host_tools.preparing
        {
            return;
        }
        let Some(endpoint) = self.transfer_host_endpoint() else {
            self.notify(
                "Connect to an SSH host and choose its file access before mounting.",
                true,
            );
            return;
        };
        let state = &mut self.transfers.sharing;
        let Some(row) = state
            .rows
            .iter_mut()
            .find(|row| row.id == id && row.running.is_none())
        else {
            return;
        };
        let control = Arc::new(Control::default());
        row.running = Some(control.clone());
        row.ready = false;
        row.unmounting = false;
        row.message = "Mounting…".into();
        let config = row.config.clone();
        let generation = state.generation;
        let tx = state.tx.clone();
        let progress = tx.clone();
        let ctx = ctx.clone();
        let repaint = ctx.clone();
        let pending = Pending::new(&state.pending);
        let work = self.vaults.work();
        std::thread::spawn(move || {
            let result = crate::shared_folders::run(endpoint, config, control, |event| {
                let _ = progress.send(Reply::Status {
                    generation,
                    id,
                    event,
                });
                repaint.request_repaint();
            })
            .map_err(|error| format!("{error:#}"));
            drop(work);
            drop(pending);
            let _ = tx.send(Reply::Finished {
                generation,
                id,
                result,
            });
            ctx.request_repaint();
        });
    }
    pub(in crate::app) fn poll_shared_folders(&mut self) {
        let state = &mut self.transfers.sharing;
        while let Ok(reply) = state.rx.try_recv() {
            let (generation, id) = match &reply {
                Reply::Status { generation, id, .. } | Reply::Finished { generation, id, .. } => {
                    (*generation, *id)
                }
            };
            if generation != state.generation {
                continue;
            }
            let Some(row) = state.rows.iter_mut().find(|row| row.id == id) else {
                continue;
            };
            match reply {
                Reply::Status {
                    event: MountEvent::Ready,
                    ..
                } => {
                    row.ready = true;
                    row.message = "Mounted".into();
                }
                Reply::Status {
                    event: MountEvent::Error(error),
                    ..
                } => {
                    row.unmounting = false;
                    row.message = error;
                }
                Reply::Finished { result, .. } => {
                    row.running = None;
                    row.ready = false;
                    row.unmounting = false;
                    row.message = result.err().unwrap_or_else(|| "Not mounted".into());
                }
            }
        }
    }
    pub(in crate::app) fn shared_folders_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let available = self.transfer_host_endpoint().is_some();
        let mut start = None;
        let mut remove = None;
        super::ui::surface(palette).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Shared folders");
                let add = ui.add_enabled(
                    self.transfers.sharing.rows.len() < 8,
                    crate::keyboard::Button::new("Add shared folder"),
                );
                ui_probe::button(ui, "share-add", &add);
                if add.clicked() {
                    self.transfers.sharing.adding = true;
                }
            });
            ui.weak("Live access · SSHFS and FUSE are needed on the receiving host");
            for row in &mut self.transfers.sharing.rows {
                ui.push_id(row.id, |ui| {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(match row.config.direction {
                            Direction::LocalToRemote => "Local → SSH host",
                            Direction::RemoteToLocal => "SSH host → Local",
                        });
                        ui.weak(if row.config.read_only {
                            "Read only"
                        } else {
                            "Read and write"
                        });
                        if let Some(control) = &row.running {
                            if !row.ready || row.unmounting {
                                ui.spinner();
                            }
                            let button = ui.add_enabled(
                                !row.unmounting,
                                crate::keyboard::Button::new(if row.ready {
                                    "Unmount"
                                } else {
                                    "Cancel mount"
                                }),
                            );
                            ui_probe::button(ui, &format!("share-unmount:{}", row.id), &button);
                            if button.clicked() {
                                row.unmounting = true;
                                row.message = if row.ready {
                                    "Unmounting…"
                                } else {
                                    "Cancelling mount…"
                                }
                                .into();
                                if row.ready {
                                    control.unmount.store(true, Ordering::Release);
                                } else {
                                    control.cancel.store(true, Ordering::Release);
                                }
                            }
                        } else {
                            let button =
                                ui.add_enabled(available, crate::keyboard::Button::new("Mount"));
                            ui_probe::button(ui, &format!("share-mount:{}", row.id), &button);
                            if button.clicked() {
                                start = Some(row.id);
                            }
                            let button = crate::keyboard::button(ui, "Remove");
                            ui_probe::button(ui, &format!("share-remove:{}", row.id), &button);
                            if button.clicked() {
                                remove = Some(row.id);
                            }
                        }
                    });
                    ui.add(
                        egui::Label::new(format!(
                            "{} → {}",
                            row.config.source, row.config.destination
                        ))
                        .wrap(),
                    );
                    let color = if row.ready && row.message == "Mounted" {
                        palette.success
                    } else if !row.unmounting
                        && row.message != "Not mounted"
                        && (row.running.is_none() || row.ready)
                    {
                        palette.error
                    } else {
                        palette.muted
                    };
                    let message =
                        ui.add(egui::Label::new(RichText::new(&row.message).color(color)).wrap());
                    ui_probe::value(
                        ui,
                        &format!("share-status:{}", row.id),
                        &message,
                        &row.message,
                    );
                    if row.ready {
                        ui_probe::button(ui, &format!("share-ready:{}", row.id), &message);
                        if !row.unmounting && row.message != "Mounted" {
                            ui_probe::button(
                                ui,
                                &format!("share-unmount-error:{}", row.id),
                                &message,
                            );
                        }
                    } else if row.running.is_none() && row.message != "Not mounted" {
                        ui_probe::button(ui, &format!("share-error:{}", row.id), &message);
                    }
                });
            }
            if self.transfers.sharing.adding {
                self.transfers.sharing.form(ui);
            }
        });
        if let Some(id) = remove {
            self.transfers.sharing.rows.retain(|row| row.id != id);
        }
        if let Some(id) = start {
            self.start_shared_folder(id, ctx);
        }
    }
}

impl Sharing {
    fn form(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        let direction = crate::keyboard::ComboBox::new("share-direction", "Share direction")
            .selected_text(match self.draft.direction {
                Direction::LocalToRemote => "Local folder on SSH host",
                Direction::RemoteToLocal => "SSH folder on this computer",
            })
            .show_ui(ui, |ui| {
                for (direction, label, name) in [
                    (
                        Direction::LocalToRemote,
                        "Local folder on SSH host",
                        "share-local-to-remote",
                    ),
                    (
                        Direction::RemoteToLocal,
                        "SSH folder on this computer",
                        "share-remote-to-local",
                    ),
                ] {
                    let choice = crate::keyboard::selectable_value(
                        ui,
                        &mut self.draft.direction,
                        direction,
                        label,
                    );
                    ui_probe::button(ui, name, &choice);
                }
            });
        ui_probe::button(ui, "share-direction", &direction.response);
        for (name, label, value) in [
            ("share-source", "Source folder", &mut self.draft.source),
            (
                "share-destination",
                "Empty mount folder",
                &mut self.draft.destination,
            ),
        ] {
            ui.label(label);
            let field = ui.add(crate::keyboard::named(
                label,
                egui::TextEdit::singleline(value)
                    .desired_width(ui.available_width())
                    .hint_text("/absolute/path or ~/folder"),
            ));
            ui_probe::button(ui, name, &field);
            ui_probe::value(ui, &format!("{name}-value"), &field, value);
        }
        let readonly = crate::keyboard::checkbox(ui, &mut self.draft.read_only, "Read only");
        ui_probe::button(ui, "share-read-only", &readonly);
        ui.weak("Create an empty mount folder first. Only the selected source is shared. Sandboxed programs still need permission to access the mount path.");
        let valid = self.draft.validate().is_ok()
            && !self.rows.iter().any(|row| {
                row.config.direction == self.draft.direction
                    && row.config.destination == self.draft.destination
            });
        ui.horizontal(|ui| {
            let add = ui.add_enabled(valid, crate::keyboard::Button::new("Save shared folder"));
            ui_probe::button(ui, "share-save", &add);
            if add.clicked() {
                self.insert(self.draft.clone());
                self.draft = Default::default();
                self.adding = false;
            }
            let cancel = crate::keyboard::button(ui, "Cancel");
            ui_probe::button(ui, "share-cancel", &cancel);
            if cancel.clicked() {
                self.adding = false;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_shares_guard_transitions_and_forced_reset_cancels_owned_work() {
        let ctx = egui::Context::default();
        let mut app = EditorApp::new(
            &eframe::CreationContext::_new_kittest(ctx.clone()),
            Startup::default(),
        );
        app.workspace
            .set_fixture(Some(crate::app::tests::remote_workspace()));
        app.bind_project_transfers();
        let config = Config {
            source: "/chosen/source".into(),
            destination: "/chosen/mount".into(),
            ..Default::default()
        };
        app.transfers.sharing.restore(vec![config]);
        let control = Arc::new(Control::default());
        app.transfers.sharing.rows[0].running = Some(control.clone());
        let work = Pending::new(&app.transfers.sharing.pending);
        app.guarded(Action::Disconnect, &ctx);
        assert!(app.workspace.as_ref().unwrap().connected);
        app.guarded(Action::Quit, &ctx);
        assert!(!app.exit_pending());
        app.prepare_project(
            "/different/project".into(),
            Default::default(),
            true,
            false,
            None,
            &ctx,
        );
        assert!(!app.workspace.preparing());
        let candidate = app.workspace.begin_preparation();
        app.workspace.hold_prepared(
            candidate,
            workspace::Prepared {
                response: Response::default(),
                remote: true,
                options: Default::default(),
                open_file: None,
                keep_buffers: false,
                approved_documents: vec![],
            },
        );
        app.guarded(Action::PreparedProject { id: candidate }, &ctx);
        assert!(
            app.workspace.prepared(candidate).is_none(),
            "Rejected candidates must release their resources"
        );
        app.set_extension(crate::extensions::Feature::Transfers.id(), false);
        assert!(app.feature(crate::extensions::Feature::Transfers));
        app.transfers.reset();
        assert!(control.cancel.load(Ordering::Acquire));
        assert!(app.transfers.sharing.configs().is_empty());
        assert!(
            app.transfers.pending(),
            "Cleanup must retain its lifetime barrier"
        );
        drop(work);
        assert!(!app.transfers.pending());
    }
}
