//! Preparation admission and connection cancellation belong to one window owner.
//!
//! Candidate identities are independent of the inbox workspace generation: a
//! candidate can be prepared while the current project's replies remain valid.
//! Prepared data awaiting an unsaved-buffer decision stays here, not in a UI
//! action that could outlive a superseding request.
use super::ConnectionId;
use crate::app::Workspace;
use crate::ssh::{Profile, Remote};
use crate::{files::Response, sandbox};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Opaque candidate identity, separate from the workspace reply generation. Authentication and
/// candidate preparation do not by themselves replace the active project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct PreparationId(u64);

/// Preparation intent travels with the candidate, never inferred from its path
/// or from whichever view happens to be selected when the worker replies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum PreparationPolicy {
    /// Explicit picker/bookmark/settings choices are already resolved.
    Given,
    /// Explicit CLI/SSH startup restores this project's saved grants; the flag
    /// forces sandboxing when the user supplied `--sandbox`.
    Saved(bool),
    /// The implicit local CWD opens in Normal mode without restoring or
    /// replacing sandbox preferences. It must not depend on sandbox capability.
    CurrentDirectory,
    /// Restore the desktop CWD after SSH teardown without replacing retained buffers or leases.
    LocalFallback,
}

impl PreparationPolicy {
    /// Implicit desktop contexts never restore or persist sandbox grants from saved projects.
    pub fn implicit_local(self) -> bool {
        matches!(self, Self::CurrentDirectory | Self::LocalFallback)
    }
}

impl PreparationId {
    /// Construct an arbitrary candidate identity for stale-confirmation tests.
    #[cfg(test)]
    pub(in crate::app) fn fixture(value: u64) -> Self {
        Self(value)
    }
}

/// Prepared project result retained while guard approval is pending. Captured dirty versions and
/// file-open intent travel with the candidate.
pub(in crate::app) struct Prepared {
    /// Checked preparation result, not yet committed as active project authority.
    pub response: Response,
    /// Whether this candidate uses the authenticated remote host.
    pub remote: bool,
    /// Sandbox options captured with the preparation request.
    pub options: sandbox::Options,
    /// Captured origin controls whether opening this candidate saves its policy.
    pub policy: PreparationPolicy,
    /// Optional file to open only after the candidate is committed.
    pub open_file: Option<String>,
    /// Preserve existing buffers when reconfiguring the current project.
    pub keep_buffers: bool,
    /// Document/version pairs captured at preparation start to detect edits made while waiting.
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
enum LocalFallback {
    #[default]
    None,
    Needed,
    Active,
}

/// Owner of active workspace authority, one candidate and connection cancellation. Dropping it
/// cancels the owned connection attempt.
#[derive(Default)]
pub(in crate::app) struct Controller {
    active: Option<Workspace>,
    serial: u64,
    candidate: Candidate,
    connection_cancel: Arc<AtomicBool>,
    previous_connection: Option<Profile>,
    local_fallback: LocalFallback,
}

impl Controller {
    /// Identify the current candidate without exposing its mutable preparation state.
    pub fn preparation_id(&self) -> Option<PreparationId> {
        match self.candidate {
            Candidate::Loading(id) | Candidate::Ready(id, _) => Some(id),
            Candidate::Idle => None,
        }
    }
    /// Borrow the committed/connecting context without exposing mutation rights.
    pub fn as_ref(&self) -> Option<&Workspace> {
        self.active.as_ref()
    }
    /// Whether no active or connecting workspace context is retained.
    pub fn is_none(&self) -> bool {
        self.active.is_none()
    }
    /// Whether an active or connecting context exists; readiness must still be checked separately.
    pub fn is_some(&self) -> bool {
        self.active.is_some()
    }

    /// Explicit project ownership differs from the neutral desktop CWD, even
    /// when both resolve to the same path. Presentation cannot infer this intent.
    pub fn project_open(&self) -> bool {
        self.active
            .as_ref()
            .is_some_and(|workspace| workspace.ready)
            && !matches!(self.local_fallback, LocalFallback::Active)
    }

    /// Resume only the remembered host/project after every document has been unbound. A neutral
    /// local return is allowed; an explicitly opened local project still requires replacement guards.
    pub fn can_resume(&self, profile: &Profile, all_unbound: bool) -> bool {
        (self.active.is_none() || matches!(self.local_fallback, LocalFallback::Active))
            && all_unbound
            && self.previous_connection.as_ref().is_some_and(|last| {
                ConnectionId::for_profile(last) == ConnectionId::for_profile(profile)
            })
    }

    /// Reconnect an active matching host or its explicit disconnected recovery record.
    pub fn can_reconnect(&self, profile: &Profile, all_unbound: bool) -> bool {
        self.active
            .as_ref()
            .and_then(|workspace| workspace.remote.as_ref())
            .is_some_and(|remote| {
                ConnectionId::for_profile(&remote.profile) == ConnectionId::for_profile(profile)
            })
            || self.can_resume(profile, all_unbound)
    }

    /// Profile for an active remote host, falling back to the remembered disconnected host for
    /// explicit recovery.
    pub fn reconnect_profile(&self) -> Option<&Profile> {
        self.active
            .as_ref()
            .and_then(|workspace| workspace.remote.as_ref())
            .map(|remote| &remote.profile)
            .or(self.previous_connection.as_ref())
    }

    /// Capture recovery identity before views and the owned transport are released.
    pub fn remember_disconnect(&mut self) -> Option<ConnectionId> {
        let profile = &self.active.as_ref()?.remote.as_ref()?.profile;
        let identity = ConnectionId::for_profile(profile);
        self.previous_connection = Some(profile.clone());
        Some(identity)
    }

    /// Only a connected, prepared host supplies identity for restoring file bindings.
    pub fn restoration_binding(&self) -> Option<(&Remote, ConnectionId)> {
        let workspace = self
            .active
            .as_ref()
            .filter(|workspace| workspace.connected && workspace.ready)?;
        let remote = workspace.remote.as_ref()?;
        Some((remote, ConnectionId::for_profile(&remote.profile)))
    }

    /// Publish only an already validated project; UI defaults never authorize access.
    pub fn activate(
        &mut self,
        root: String,
        remote: Option<crate::ssh::Remote>,
        plan: sandbox::Plan,
        options: sandbox::Options,
    ) {
        self.local_fallback = LocalFallback::None;
        self.active = Some(Workspace {
            root,
            remote,
            connected: true,
            ready: true,
            plan,
            sandbox_options: options,
        });
    }

    /// The implicit desktop CWD is neutral and keeps a disconnected SSH identity
    /// eligible for recovery. It is not an explicitly opened project.
    pub fn activate_local_fallback(&mut self, root: String, plan: sandbox::Plan) {
        self.activate(root, None, plan, Default::default());
        self.local_fallback = LocalFallback::Active;
    }

    /// Queue one local return after teardown. A later explicit project/connection takes precedence.
    pub fn request_local_fallback(&mut self) {
        self.local_fallback = LocalFallback::Needed;
    }

    /// Consume only when no project or candidate owns the window. Failed probes do not retry forever.
    pub fn take_local_fallback(&mut self) -> bool {
        if self.active.is_none()
            && matches!(self.candidate, Candidate::Idle)
            && matches!(self.local_fallback, LocalFallback::Needed)
        {
            self.local_fallback = LocalFallback::None;
            true
        } else {
            false
        }
    }

    /// Authentication has a context for presentation, but no prepared file grants.
    pub fn connecting(
        &mut self,
        remote: crate::ssh::Remote,
        root: String,
        options: sandbox::Options,
    ) {
        self.local_fallback = LocalFallback::None;
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

    /// Withdraw project readiness while retaining its transport and other context for recovery or
    /// re-preparation.
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
                workspace.sandbox_options = Default::default();
                remote.profile.root = "~".into();
                remote.profile.sandbox = Default::default();
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

    /// Replace the active plan’s environment settings without changing its host or prepared project
    /// identity.
    pub fn set_environment(&mut self, settings: crate::environment::Settings) {
        if let Some(workspace) = &mut self.active {
            workspace.plan.environment = settings;
        }
    }

    /// Domain views must be released before the caller cleans this captured transport.
    pub fn release_transport(&mut self) -> Option<crate::ssh::Remote> {
        self.cancel_connection();
        self.local_fallback = LocalFallback::None;
        self.active.take().and_then(|workspace| workspace.remote)
    }

    /// Install synthetic workspace authority directly in coordinator tests.
    #[cfg(test)]
    pub(in crate::app) fn set_fixture(&mut self, workspace: Option<Workspace>) {
        self.local_fallback = LocalFallback::None;
        self.active = workspace;
    }
    /// Test-only mutable access to committed context; production uses typed controller operations.
    #[cfg(test)]
    pub(in crate::app) fn as_mut(&mut self) -> Option<&mut Workspace> {
        self.active.as_mut()
    }
    /// Remove synthetic context directly in teardown tests, bypassing production transport-release
    /// ordering.
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

    /// Whether a candidate read/preparation is still awaiting its matching completion.
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

    /// Borrow a completed candidate only when its identity matches the current held result.
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

    /// Cancel a held candidate only for its matching decision identity; obsolete callbacks do
    /// nothing.
    pub fn cancel_prepared(&mut self, id: PreparationId) {
        if self.prepared(id).is_some() {
            self.cancel_preparation();
        }
    }

    /// Invalidate all candidate identities and drop prepared data while retaining active workspace
    /// authority.
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

    /// Signal cancellation to the current connection worker; the worker retains responsibility for
    /// process cleanup.
    pub fn cancel_connection(&self) {
        self.connection_cancel.store(true, Ordering::Release);
    }

    /// Expose the current candidate serial for stale-result regression tests.
    #[cfg(test)]
    pub(in crate::app) fn preparation_identity(&self) -> PreparationId {
        PreparationId(self.serial)
    }

    /// Clone the current cancellation flag for connection-ownership regression tests.
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

    #[test]
    fn explicit_project_identity_is_independent_of_its_path() {
        let mut state = Controller::default();
        state.activate_local_fallback("/cwd".into(), Default::default());
        assert!(!state.project_open());
        state.activate("/cwd".into(), None, Default::default(), Default::default());
        assert!(state.project_open());
        state.clear_project_context();
        assert!(!state.project_open());
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
        assert!(!state.project_open());
        let workspace = state.as_ref().unwrap();
        assert!(workspace.connected && !workspace.ready);
        assert_eq!(workspace.remote.as_ref().unwrap().socket, socket);
        assert!(!workspace.sandbox_options.enabled);
    }

    #[test]
    fn local_return_is_once_only_and_only_the_fallback_preserves_ssh_recovery() {
        let mut state = Controller::default();
        state.set_fixture(Some(crate::app::tests::remote_workspace()));
        let profile = state.reconnect_profile().unwrap().clone();
        state.remember_disconnect();
        state.release_transport();
        state.request_local_fallback();
        assert!(state.take_local_fallback());
        assert!(!state.take_local_fallback());
        state.activate_local_fallback("/desktop".into(), Default::default());
        assert!(state.is_some() && !state.is_none());
        assert!(state.can_resume(&profile, true));
        assert!(!state.can_resume(&profile, false));
        state.activate(
            "/chosen-project".into(),
            None,
            Default::default(),
            Default::default(),
        );
        assert!(!state.can_resume(&profile, true));
        state.request_local_fallback();
        assert!(
            !state.take_local_fallback(),
            "An explicit project must not be replaced"
        );
    }

    #[test]
    fn recovery_identity_waits_for_unbinding_and_restoration_requires_preparation() {
        let mut state = Controller::default();
        state.set_fixture(Some(crate::app::tests::remote_workspace()));
        let original = state.reconnect_profile().unwrap().clone();
        assert!(state.can_reconnect(&original, false));
        let identity = state.remember_disconnect().unwrap();
        state.release_transport();
        assert!(!state.can_resume(&original, false));
        assert!(state.can_resume(&original, true));
        let mut renamed = original.clone();
        renamed.name = "Another display label".into();
        renamed.sandbox.enabled = !renamed.sandbox.enabled;
        assert!(state.can_resume(&renamed, true));
        for field in ["host", "user", "port", "identity", "config", "jump", "root"] {
            let mut changed = original.clone();
            match field {
                "host" => changed.host.push_str(".other"),
                "user" => changed.user.push_str("other"),
                "port" => changed.port.push('1'),
                "identity" => changed.identity.push_str("other"),
                "config" => changed.config.push_str("other"),
                "jump" => changed.jump.push_str("other"),
                "root" => changed.root.push_str("/other"),
                _ => unreachable!(),
            }
            assert!(!state.can_resume(&changed, true), "{field}");
        }
        let workspace = crate::app::tests::remote_workspace();
        let remote = workspace.remote.unwrap();
        state.connecting(
            remote.clone(),
            original.root.clone(),
            original.sandbox.clone(),
        );
        assert!(state.restoration_binding().is_none());
        state.connected();
        assert!(state.restoration_binding().is_none());
        state.activate(
            original.root.clone(),
            Some(remote),
            workspace.plan,
            original.sandbox,
        );
        assert_eq!(state.restoration_binding().unwrap().1, identity);
        state.suspend_project();
        assert!(state.restoration_binding().is_none());
    }

    fn prepared(path: &str) -> Prepared {
        Prepared {
            policy: PreparationPolicy::Given,
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
