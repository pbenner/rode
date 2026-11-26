//! Saved workspace ownership, Welcome catalogue and explicit restoration.
//! Host execution remains with the existing agent, terminal and runtime owners.
use super::*;
mod capture;
pub(in crate::app) mod model;
mod restore;
mod store;
mod view;
mod worker;
pub(in crate::app) use model::Location;
use model::{Key, Record, Row, Snapshot};
use worker::{Command as Request, Event};

#[derive(Clone)]
pub(in crate::app) struct Resume {
    key: Key,
    location: Location,
    takeover: bool,
}
struct Stamp {
    hash: String,
    prompt: String,
    dirty: Vec<(u64, u64)>,
    eligible: bool,
}
#[derive(Clone)]
struct Owned {
    key: Key,
    location: String,
    policy: String,
}
pub(super) struct State {
    worker: worker::Worker,
    rows: Vec<Row>,
    own: Option<Owned>,
    paused: Option<Owned>,
    serial: u64,
    submitted: Option<(u64, Stamp)>,
    acknowledged: Option<Stamp>,
    enabling: bool,
    frozen: bool,
    listing: bool,
    last_list: Option<Instant>,
    catalogue_warning: String,
    error: String,
    failed: Option<String>,
    confirm: Option<Confirmation>,
    rename: Option<(Key, String)>,
    restore: Option<restore::Restoring>,
}
enum Confirmation {
    TakeOver(Row),
    Remove(Row),
}
impl State {
    pub(in crate::app) fn freeze_for_transition(&mut self) {
        self.frozen = self.own.is_some();
    }
    pub(in crate::app) fn barrier(&self) -> Arc<worker::Barrier> {
        self.worker.barrier.clone()
    }
    fn new(worker: worker::Worker) -> Self {
        Self {
            worker,
            rows: Vec::new(),
            own: None,
            paused: None,
            serial: 0,
            submitted: None,
            acknowledged: None,
            enabling: false,
            frozen: false,
            listing: false,
            last_list: None,
            catalogue_warning: String::new(),
            error: String::new(),
            failed: None,
            confirm: None,
            rename: None,
            restore: None,
        }
    }
    fn request(&mut self, command: Request) {
        if let Err(error) = self.worker.command(command) {
            self.error = error.to_string();
        }
    }
    /// A queued write supersedes the acknowledged record, even if the user
    /// subsequently returns to an older draft or layout.
    fn checkpoint_matches(&self, hash: &str) -> bool {
        self.submitted.as_ref().map_or_else(
            || self.acknowledged.as_ref().is_some_and(|s| s.hash == hash),
            |(_, stamp)| stamp.hash == hash,
        )
    }
    fn save_failed(&mut self, key: &Key, serial: u64) {
        if self.own.as_ref().is_some_and(|own| &own.key == key) {
            // Publication may have reached rename before a durability error.
            // An older acknowledgement no longer proves the file's contents.
            self.acknowledged = None;
            if self
                .submitted
                .as_ref()
                .is_some_and(|(seq, _)| *seq == serial)
            {
                self.failed = self.submitted.take().map(|(_, stamp)| stamp.hash);
            }
        }
    }
    fn refresh(&mut self) {
        if !self.listing {
            match self.worker.command(Request::List) {
                Ok(()) => {
                    self.listing = true;
                    self.last_list = Some(Instant::now());
                }
                Err(e) => self.error = e.to_string(),
            }
        }
    }
    /// Pausing stops writes and releases the live lease, while retaining this
    /// window's entry for a later explicit enable in the same project/policy.
    fn pause(&mut self) {
        let previous = self.own.clone();
        self.release();
        if self.own.is_none() {
            self.paused = previous;
        }
    }
    fn release(&mut self) {
        if let Some(own) = &self.own
            && let Err(error) = self.worker.command(Request::Release(own.key.clone()))
        {
            self.error = error.to_string();
            return;
        }
        self.own = None;
        self.paused = None;
        self.acknowledged = None;
        self.submitted = None;
        self.enabling = false;
        self.frozen = false;
        self.failed = None;
    }
}
impl EditorApp {
    pub(super) fn init_workspace_sessions(&mut self, ctx: &egui::Context) {
        let wake = ctx.clone();
        let result = eframe::storage_dir("RODE")
            .ok_or_else(|| anyhow::anyhow!("Cannot locate workspace session storage"))
            .and_then(|root| {
                worker::Worker::start(
                    root.join("workspace-sessions"),
                    Arc::new(move || wake.request_repaint()),
                )
            });
        match result {
            Ok(worker) => {
                let mut state = State::new(worker);
                state.refresh();
                self.workspace_sessions = Some(state);
            }
            Err(e) => self.notify(format!("Workspace sessions unavailable: {e:#}"), true),
        }
    }
    pub(super) fn poll_workspace_sessions(&mut self, ctx: &egui::Context) {
        loop {
            let event = self
                .workspace_sessions
                .as_ref()
                .and_then(|s| s.worker.events.try_recv().ok());
            let Some(event) = event else { break };
            let state = self.workspace_sessions.as_mut().unwrap();
            match event {
                Event::Listed { rows, warnings } => {
                    state.catalogue_warning = warnings.join("\n");
                    state.rows = rows;
                    state.listing = false;
                }
                Event::Kept { record, serial } => {
                    state.paused = None;
                    state.enabling = false;
                    state.failed = None;
                    state.error.clear();
                    state.own = Some(Owned {
                        key: record.key(),
                        location: record.snapshot.location.identity(),
                        policy: record.snapshot.policy,
                    });
                    if state
                        .submitted
                        .as_ref()
                        .is_some_and(|(seq, _)| *seq == serial)
                    {
                        state.acknowledged = state.submitted.take().map(|(_, stamp)| stamp);
                    }
                    state.refresh();
                }
                Event::Saved { key, serial } => {
                    if state.own.as_ref().is_some_and(|o| o.key == key) {
                        state.failed = None;
                        state.error.clear();
                    }
                    if state.own.as_ref().is_some_and(|o| o.key == key)
                        && state
                            .submitted
                            .as_ref()
                            .is_some_and(|(seq, _)| *seq == serial)
                    {
                        state.acknowledged = state.submitted.take().map(|(_, stamp)| stamp);
                    }
                }
                Event::Claimed { record, takeover } => {
                    self.workspace_session_claimed(*record, takeover, ctx);
                }
                Event::Revoked(key) => {
                    if state.own.as_ref().is_some_and(|o| o.key == key) {
                        state.own = None;
                        state.acknowledged = None;
                        state.submitted = None;
                        state.restore = None;
                        state.error = "This workspace session moved to another window. Your current buffers remain open.".into();
                    }
                    state.refresh();
                }
                Event::Focus => {
                    ctx.send_viewport_cmd_to(
                        egui::ViewportId::ROOT,
                        egui::ViewportCommand::Minimized(false),
                    );
                    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                }
                Event::Changed => state.refresh(),
                Event::Error { operation, message } => {
                    let checkpoint = matches!(
                        operation,
                        worker::Operation::Save(..) | worker::Operation::Keep(..)
                    );
                    match operation {
                        worker::Operation::List => state.listing = false,
                        worker::Operation::Keep(serial) => {
                            state.enabling = false;
                            if state
                                .submitted
                                .as_ref()
                                .is_some_and(|(seq, _)| *seq == serial)
                            {
                                state.failed = state.submitted.take().map(|(_, stamp)| stamp.hash);
                            }
                        }
                        worker::Operation::Claim => state.restore = None,
                        worker::Operation::Save(key, serial) => state.save_failed(&key, serial),
                        worker::Operation::Change | worker::Operation::Verify => {}
                    }
                    state.error = message.clone();
                    if checkpoint {
                        self.notify(format!("Workspace checkpoint failed: {message}. Open Welcome to retry saving or turn off Keep workspace before leaving."), true);
                    }
                }
            }
        }
        self.advance_workspace_resume(ctx);
        if let Some(state) = &mut self.workspace_sessions {
            if self.presentation.area == areas::Area::Welcome
                && state
                    .last_list
                    .is_none_or(|t| t.elapsed() >= Duration::from_secs(3))
            {
                state.refresh();
            }
            if state.own.is_some()
                || state.restore.is_some()
                || state.listing
                || self.presentation.area == areas::Area::Welcome
            {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    }
    pub(in crate::app) fn start_workspace_resume(
        &mut self,
        selection: Resume,
        ctx: &egui::Context,
    ) {
        let approved = self.documents.changes().dirty_versions;
        if let Some(state) = &mut self.workspace_sessions {
            state.restore = Some(restore::Restoring::claiming(
                selection.location.clone(),
                approved,
            ));
            if let Err(error) = state.worker.command(Request::Claim {
                key: selection.key,
                takeover: selection.takeover,
            }) {
                state.restore = None;
                state.frozen = false;
                state.error = error.to_string();
            }
        }
        ctx.request_repaint();
    }
}
