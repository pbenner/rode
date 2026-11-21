//! Project/host-bound shared-folder recipes and worker acknowledgements. Saved
//! recipes are inert: only Mount starts a service, and a bookmark restores settings.
use super::*;
use crate::shared_folders::{Config, Control, Event as MountEvent};
mod picker;
mod view;

/// Changes that retarget or release the authority retained by a live mount.
#[derive(Clone, Copy)]
pub(in crate::app) enum ContextChange {
    Project,
    Host,
    Access,
    Disable,
    Quit,
}
impl ContextChange {
    fn blocked(self) -> &'static str {
        match self {
            Self::Project => {
                "Unmount shared folders before changing projects, connections or sandbox policy."
            }
            Self::Host => "Unmount shared folders before changing the host.",
            Self::Access => "Unmount shared folders before changing file access permissions.",
            Self::Disable => "Unmount shared folders before disabling File transfers.",
            Self::Quit => "Unmount shared folders in File transfers before closing RODE.",
        }
    }
}

struct Row {
    id: u64,
    config: Config,
    running: Option<Arc<Control>>,
    ready: bool,
    unmounting: bool,
    message: String,
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
    pub open: bool,
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
}
impl Default for Sharing {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            open: false,
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
        }
    }
}
impl Sharing {
    /// One admission rule for UI availability and every context-changing adapter.
    /// The worker count includes cancelled rows until cleanup has finished.
    pub(in crate::app) fn admit(&self, change: ContextChange) -> Result<(), &'static str> {
        if self.pending() {
            Err(change.blocked())
        } else {
            Ok(())
        }
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
    /// Use the same domain admission for direct calls, shortcuts and native actions.
    pub(in crate::app) fn allow_shared_folder_change(&mut self, change: ContextChange) -> bool {
        match self.transfers.sharing.admit(change) {
            Ok(()) => true,
            Err(message) => {
                self.notify(message, false);
                false
            }
        }
    }

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
    /// Render facts and route the view's intents through controller admission.
    pub(in crate::app) fn shared_folders_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let available = self.transfer_host_endpoint().is_some();
        let state = &mut self.transfers.sharing;
        let intent = state.view.show(ui, &state.rows, palette, available);
        match intent {
            Some(view::Intent::Browse(field)) => self.open_shared_folder_picker(field, ctx),
            Some(view::Intent::Mount(id)) => self.start_shared_folder(id, ctx),
            Some(view::Intent::Stop(id)) => self.transfers.sharing.stop(id),
            Some(view::Intent::Remove(id)) => self
                .transfers
                .sharing
                .rows
                .retain(|row| row.id != id || row.running.is_some()),
            Some(view::Intent::Add(config)) => {
                let state = &mut self.transfers.sharing;
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
