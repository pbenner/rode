//! Preparation admission and connection cancellation belong to one window owner.
//!
//! Candidate identities are independent of the inbox workspace generation: a
//! candidate can be prepared while the current project's replies remain valid.
//! Prepared data awaiting an unsaved-buffer decision stays here, not in a UI
//! action that could outlive a superseding request.
use crate::app::Workspace;
use crate::{files::Response, sandbox};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct PreparationId(u64);

impl PreparationId {
    #[cfg(test)]
    pub(in crate::app) fn fixture(value: u64) -> Self {
        Self(value)
    }
}

pub(in crate::app) struct Prepared {
    pub response: Response,
    pub remote: bool,
    pub options: sandbox::Options,
    pub open_file: Option<String>,
    pub keep_buffers: bool,
    pub approved_documents: Vec<(u64, u64)>,
}

#[derive(Default)]
enum Candidate {
    #[default]
    Idle,
    Loading(PreparationId),
    Ready(PreparationId, Box<Prepared>),
}

#[derive(Default)]
pub(in crate::app) struct Controller {
    active: Option<Workspace>,
    serial: u64,
    candidate: Candidate,
    connection_cancel: Arc<AtomicBool>,
}

impl Controller {
    /// Borrow the committed/connecting context without exposing mutation rights.
    pub fn as_ref(&self) -> Option<&Workspace> {
        self.active.as_ref()
    }
    pub fn is_none(&self) -> bool {
        self.active.is_none()
    }
    pub fn is_some(&self) -> bool {
        self.active.is_some()
    }

    /// Publish only an already validated project; UI defaults never authorize access.
    pub fn activate(
        &mut self,
        root: String,
        remote: Option<crate::ssh::Remote>,
        plan: sandbox::Plan,
        options: sandbox::Options,
    ) {
        self.active = Some(Workspace {
            root,
            remote,
            connected: true,
            ready: true,
            plan,
            sandbox_options: options,
        });
    }

    /// Authentication has a context for presentation, but no prepared file grants.
    pub fn connecting(
        &mut self,
        remote: crate::ssh::Remote,
        root: String,
        options: sandbox::Options,
    ) {
        self.active = Some(Workspace {
            plan: sandbox::Plan {
                root: root.clone(),
                ..Default::default()
            },
            root,
            remote: Some(remote),
            connected: false,
            ready: false,
            sandbox_options: options,
        });
    }

    /// Successful authentication still requires project preparation before I/O.
    pub fn connected(&mut self) -> Option<(String, sandbox::Options)> {
        let workspace = self.active.as_mut()?;
        workspace.connected = true;
        Some((workspace.root.clone(), workspace.sandbox_options.clone()))
    }

    pub fn suspend_project(&mut self) {
        if let Some(workspace) = &mut self.active {
            workspace.ready = false;
        }
    }

    /// Drop project authority after a vault closes, retaining only the remote transport.
    pub fn clear_project_context(&mut self) {
        if let Some(workspace) = &mut self.active {
            if let Some(remote) = &mut workspace.remote {
                workspace.root = "~".into();
                workspace.ready = false;
                workspace.plan = Default::default();
                remote.profile.root = "~".into();
            } else {
                self.active = None;
            }
        }
    }

    /// Apply a checked canonical list result only to the matching current root.
    pub fn canonicalize_root(&mut self, requested: &str, canonical: &str) -> bool {
        if let Some(workspace) = &mut self.active
            && workspace.root == requested
        {
            workspace.root = canonical.into();
            return true;
        }
        false
    }

    pub fn set_environment(&mut self, settings: crate::environment::Settings) {
        if let Some(workspace) = &mut self.active {
            workspace.plan.environment = settings;
        }
    }

    /// Domain views must be released before the caller cleans this captured transport.
    pub fn release_transport(&mut self) -> Option<crate::ssh::Remote> {
        self.cancel_connection();
        self.active.take().and_then(|workspace| workspace.remote)
    }

    #[cfg(test)]
    pub(in crate::app) fn set_fixture(&mut self, workspace: Option<Workspace>) {
        self.active = workspace;
    }
    #[cfg(test)]
    pub(in crate::app) fn as_mut(&mut self) -> Option<&mut Workspace> {
        self.active.as_mut()
    }
    #[cfg(test)]
    pub(in crate::app) fn take_fixture(&mut self) -> Option<Workspace> {
        self.active.take()
    }

    fn advance(&mut self) -> PreparationId {
        self.serial = self
            .serial
            .checked_add(1)
            .expect("Preparation identity exhausted");
        PreparationId(self.serial)
    }

    /// Supersede only the candidate; the active workspace is unaffected.
    pub fn begin_preparation(&mut self) -> PreparationId {
        let id = self.advance();
        self.candidate = Candidate::Loading(id);
        id
    }

    pub fn preparing(&self) -> bool {
        matches!(self.candidate, Candidate::Loading(_))
    }

    /// Admit a matching result exactly once, before clearing any UI markers.
    pub fn finish_preparation(&mut self, id: PreparationId) -> bool {
        if !matches!(self.candidate, Candidate::Loading(current) if current == id) {
            return false;
        }
        self.candidate = Candidate::Idle;
        true
    }

    /// Retain a completed candidate while its exact unsaved-buffer decision is pending.
    pub fn hold_prepared(&mut self, id: PreparationId, prepared: Prepared) {
        assert_eq!(
            id,
            PreparationId(self.serial),
            "Cannot retain an obsolete candidate"
        );
        self.candidate = Candidate::Ready(id, Box::new(prepared));
    }

    pub fn prepared(&self, id: PreparationId) -> Option<&Prepared> {
        match &self.candidate {
            Candidate::Ready(current, prepared) if *current == id => Some(prepared),
            _ => None,
        }
    }

    /// A stale confirmation cannot consume or replace the current candidate.
    pub fn take_prepared(&mut self, id: PreparationId) -> Option<Prepared> {
        self.prepared(id)?;
        let Candidate::Ready(_, prepared) = std::mem::take(&mut self.candidate) else {
            unreachable!()
        };
        Some(*prepared)
    }

    pub fn cancel_prepared(&mut self, id: PreparationId) {
        if self.prepared(id).is_some() {
            self.cancel_preparation();
        }
    }

    pub fn cancel_preparation(&mut self) {
        self.advance();
        self.candidate = Candidate::Idle;
    }

    /// Replace the connection attempt without leaving its predecessor running.
    pub fn begin_connection(&mut self) -> Arc<AtomicBool> {
        self.cancel_connection();
        self.connection_cancel = Arc::new(AtomicBool::new(false));
        self.connection_cancel.clone()
    }

    pub fn cancel_connection(&self) {
        self.connection_cancel.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(in crate::app) fn preparation_identity(&self) -> PreparationId {
        PreparationId(self.serial)
    }

    #[cfg(test)]
    pub(in crate::app) fn connection_cancellation(&self) -> Arc<AtomicBool> {
        self.connection_cancel.clone()
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.cancel_connection();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(path: &str) -> Prepared {
        Prepared {
            response: Response {
                path: path.into(),
                ..Default::default()
            },
            remote: false,
            options: Default::default(),
            open_file: None,
            keep_buffers: false,
            approved_documents: Vec::new(),
        }
    }

    #[test]
    fn superseded_and_duplicate_replies_cannot_finish_the_current_preparation() {
        let mut state = Controller::default();
        let old = state.begin_preparation();
        let current = state.begin_preparation();
        assert!(!state.finish_preparation(old));
        assert!(state.preparing());
        assert!(state.finish_preparation(current));
        assert!(!state.finish_preparation(current));
        assert!(!state.preparing());
    }

    #[test]
    fn a_stale_unsaved_buffer_decision_cannot_consume_or_cancel_a_new_candidate() {
        let mut state = Controller::default();
        let old = state.begin_preparation();
        assert!(state.finish_preparation(old));
        state.hold_prepared(old, prepared("old"));
        let current = state.begin_preparation();
        assert!(state.finish_preparation(current));
        state.hold_prepared(current, prepared("current"));
        state.cancel_prepared(old);
        assert!(state.take_prepared(old).is_none());
        assert_eq!(
            state.take_prepared(current).unwrap().response.path,
            "current"
        );
    }

    #[test]
    fn cancellation_rejects_late_results_and_releases_pending_decisions() {
        let mut state = Controller::default();
        let id = state.begin_preparation();
        state.cancel_preparation();
        assert!(!state.finish_preparation(id));
        let id = state.begin_preparation();
        assert!(state.finish_preparation(id));
        state.hold_prepared(id, prepared("pending"));
        state.cancel_prepared(id);
        assert!(state.take_prepared(id).is_none());
    }

    #[test]
    fn connection_replacement_and_drop_cancel_only_the_owned_attempt() {
        let mut state = Controller::default();
        let other = Controller::default();
        let first = state.begin_connection();
        let second = state.begin_connection();
        assert!(first.load(Ordering::Acquire));
        assert!(!second.load(Ordering::Acquire));
        drop(state);
        assert!(second.load(Ordering::Acquire));
        assert!(!other.connection_cancel.load(Ordering::Acquire));
    }

    #[test]
    fn preparing_or_cancelling_a_candidate_preserves_the_active_project_authority() {
        let mut state = Controller::default();
        let plan = sandbox::Plan {
            root: "/active".into(),
            ..Default::default()
        };
        state.activate("/active".into(), None, plan, Default::default());
        let candidate = state.begin_preparation();
        assert!(state.as_ref().unwrap().ready);
        assert_eq!(state.as_ref().unwrap().plan.root, "/active");
        state.cancel_preparation();
        assert!(!state.finish_preparation(candidate));
        assert!(state.as_ref().unwrap().ready);
        assert_eq!(state.as_ref().unwrap().root, "/active");
    }

    #[test]
    fn clearing_project_authority_retains_only_a_remote_connection() {
        let mut state = Controller::default();
        state.set_fixture(Some(crate::app::tests::remote_workspace()));
        let socket = state
            .as_ref()
            .unwrap()
            .remote
            .as_ref()
            .unwrap()
            .socket
            .clone();
        state.clear_project_context();
        let active = state.as_ref().unwrap();
        assert!(!active.ready);
        assert!(active.connected);
        assert_eq!(active.root, "~");
        assert_eq!(active.remote.as_ref().unwrap().socket, socket);
        assert!(!active.plan.active);
        state.activate(
            "/local".into(),
            None,
            Default::default(),
            Default::default(),
        );
        state.clear_project_context();
        assert!(state.is_none());
    }
}
