//! Project-owned recipes and worker acknowledgements, separate from transfer
//! browser state while reusing its native picker and transport helpers. Saved
//! preferences are inert; explicit Mount and bookmark reopening can start services.
use super::*;
use crate::shared_folders::{Config, Control, Event as MountEvent};
mod picker;
mod view;

struct Row {
    id: u64,
    config: Config,
    running: Option<Arc<Control>>,
    ready: bool,
    unmounting: bool,
    message: String,
    restoring: bool,
}
enum Reply {
    Listed {
        generation: u64,
        picker: u64,
        token: u64,
        path: String,
        result: Result<Box<files::Response>, String>,
    },
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
    view: view::View,
    picker: Option<picker::Picker>,
    picker_serial: u64,
    browsing: Arc<AtomicBool>,
    rows: Vec<Row>,
    serial: u64,
    generation: u64,
    pending: Arc<AtomicUsize>,
    tx: mpsc::Sender<Reply>,
    rx: mpsc::Receiver<Reply>,
    stopping: bool,
}
impl Default for Sharing {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            view: Default::default(),
            picker: None,
            picker_serial: 0,
            browsing: Arc::default(),
            rows: vec![],
            serial: 0,
            generation: 0,
            pending: Arc::default(),
            tx,
            rx,
            stopping: false,
        }
    }
}
impl Sharing {
    /// Reopening the current bookmark is inert when its exact recipes are already mounted.
    pub(in crate::app) fn matches_mounted(&self, configs: &[Config]) -> bool {
        self.rows.len() == configs.len()
            && self.rows.iter().zip(configs).all(|(row, config)| {
                row.config == *config && row.running.is_some() && row.ready && !row.unmounting
            })
    }
    /// Request normal unmount once per transition. Worker and reply ownership remain live until
    /// cleanup is acknowledged; cancelling the transition does not undo an already requested stop.
    pub(in crate::app) fn stop_for_transition(&mut self) {
        if self.stopping {
            return;
        }
        self.stopping = true;
        self.picker = None;
        for id in self.rows.iter().map(|row| row.id).collect::<Vec<_>>() {
            self.stop(id);
        }
    }
    pub(in crate::app) fn finish_stop_wait(&mut self) {
        self.stopping = false;
    }
    fn stop(&mut self, id: u64) {
        let Some(row) = self
            .rows
            .iter_mut()
            .find(|row| row.id == id && !row.unmounting)
        else {
            return;
        };
        let Some(control) = &row.running else {
            return;
        };
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

    pub(in crate::app) fn pending(&self) -> bool {
        self.pending.load(Ordering::Acquire) > 0
            || self.rows.iter().any(|row| row.running.is_some())
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
        self.view = Default::default();
        self.picker = None;
        self.stopping = false;
    }
    pub(in crate::app) fn restore(&mut self, configs: Vec<Config>) {
        self.reset();
        for config in configs {
            self.insert(config);
        }
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
            restoring: false,
        });
    }
}

impl Drop for Sharing {
    fn drop(&mut self) {
        self.reset();
    }
}

impl EditorApp {
    /// Shares always use the active SSH project's policy, independently of File transfers.
    pub(in crate::app) fn project_shared_folder_endpoint(&self) -> Option<Endpoint> {
        if !self.project_ready() {
            return None;
        }
        let workspace = self.workspace.as_ref()?;
        Some(Endpoint::ssh(
            workspace.remote.clone()?,
            workspace.plan.clone(),
        ))
    }

    fn shared_folder_endpoint(&self, side: Side) -> Option<Endpoint> {
        self.project_shared_folder_endpoint()
            .map(|remote| match side {
                Side::Local => Endpoint::local(),
                Side::Remote => remote,
            })
    }

    /// Poll even while the project tab is hidden or detached; presentation owns no service.
    pub(in crate::app) fn shared_folders_tick(&mut self, ctx: &egui::Context) {
        self.poll_shared_folders();
        self.restore_bookmark_shares(ctx);
        if self.dialog != Dialog::SharedFolder {
            self.shared_folders.cancel_picker();
        }
    }

    fn start_shared_folder(&mut self, id: u64, restoring: bool, ctx: &egui::Context) {
        if !self.feature(crate::extensions::Feature::Transfers)
            || self.vaults.locking()
            || self.exit_pending()
            || self.pending.as_ref().is_some()
            || self.dialog != Dialog::None
            || self.foreign_modal(ctx)
            || self.workspace.preparing()
            || self.project_bookmarks.restore_shares.is_some()
        {
            return;
        }
        let Some(endpoint) = self.project_shared_folder_endpoint() else {
            self.notify(
                "Open a project on an SSH host before mounting shared folders.",
                true,
            );
            return;
        };
        let state = &mut self.shared_folders;
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
        row.restoring = restoring;
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
    /// Restore an explicitly reopened bookmark, using only the accepted project's transfer policy.
    pub(in crate::app) fn mount_bookmark_shares(
        &mut self,
        configs: Vec<Config>,
        ctx: &egui::Context,
    ) {
        self.shared_folders.restore(configs);
        let ids = self
            .shared_folders
            .rows
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>();
        for id in ids {
            self.start_shared_folder(id, true, ctx);
        }
    }
    pub(in crate::app) fn poll_shared_folders(&mut self) {
        let mut failure = None;
        let mut stop_failed = false;
        let state = &mut self.shared_folders;
        while let Ok(reply) = state.rx.try_recv() {
            if let Reply::Listed {
                generation,
                picker,
                token,
                path,
                result,
            } = reply
            {
                if generation == state.generation
                    && let Some(current) = state.picker.as_mut().filter(|p| p.serial == picker)
                {
                    current.loaded(token, &path, result);
                }
                continue;
            }
            let (generation, id) = match &reply {
                Reply::Status { generation, id, .. } | Reply::Finished { generation, id, .. } => {
                    (*generation, *id)
                }
                Reply::Listed { .. } => unreachable!(),
            };
            if generation != state.generation {
                continue;
            }
            let Some(row) = state.rows.iter_mut().find(|row| row.id == id) else {
                continue;
            };
            match reply {
                Reply::Listed { .. } => unreachable!(),
                Reply::Status {
                    event: MountEvent::Ready,
                    ..
                } => {
                    row.ready = true;
                    row.restoring = false;
                    row.message = "Mounted".into();
                }
                Reply::Status {
                    event: MountEvent::Error(error),
                    ..
                } => {
                    if state.stopping {
                        stop_failed = true;
                        failure = Some(format!(
                            "Could not unshare {}: {error}",
                            row.config.destination
                        ));
                    }
                    row.unmounting = false;
                    row.message = error;
                }
                Reply::Finished { result, .. } => {
                    if let Err(error) = &result {
                        if state.stopping {
                            stop_failed = true;
                            failure = Some(format!("Shared-folder cleanup failed: {error}"));
                        } else if row.restoring {
                            failure = Some(format!(
                                "Could not restore shared folder {}: {error}",
                                row.config.destination
                            ));
                        }
                    }
                    row.running = None;
                    row.ready = false;
                    row.unmounting = false;
                    row.restoring = false;
                    row.message = result.err().unwrap_or_else(|| "Not mounted".into());
                }
            }
        }
        // A save or source-input barrier can temporarily replace Wait::Shares after we have
        // requested unmount. Its failure still cancels that intent instead of leaving it stuck.
        if stop_failed {
            self.cancel_guarded_action();
        }
        if let Some(error) = failure {
            self.notify(error, true);
        }
    }
    /// Render facts and route the view's intents through controller admission.
    pub(in crate::app) fn shared_folders_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let available = self.project_shared_folder_endpoint().is_some();
        let state = &mut self.shared_folders;
        let intent = state.view.show(ui, &state.rows, palette, available);
        match intent {
            Some(view::Intent::Browse(field)) => self.open_shared_folder_picker(field, ctx),
            Some(view::Intent::Mount(id)) => self.start_shared_folder(id, false, ctx),
            Some(view::Intent::Stop(id)) => self.shared_folders.stop(id),
            Some(view::Intent::Remove(id)) => self
                .shared_folders
                .rows
                .retain(|row| row.id != id || row.running.is_some()),
            Some(view::Intent::Add(config)) => {
                let state = &mut self.shared_folders;
                if state.rows.len() < 8
                    && config.validate().is_ok()
                    && !state.rows.iter().any(|row| {
                        row.config.direction == config.direction
                            && row.config.destination == config.destination
                    })
                {
                    state.insert(config);
                    state.view.saved();
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests;
