//! Ownership-changing intents are admitted from domain snapshots, then executed by adapters.
//! The private controller retains decisions across input/save/lifetime acknowledgements.
use super::*;
mod control;
pub(super) use control::Controller;
use control::{Effect, Policy};
#[cfg(test)]
mod tests;

#[derive(Clone)]
/// Operations routed through common dirty-buffer/encrypted-project guards before changing
/// ownership or discarding state.
pub(in crate::app) enum Action {
    PreparedProject {
        id: workspace::PreparationId,
    },
    Project {
        path: String,
        remote: bool,
        options: sandbox::Options,
    },
    Remote(Box<Profile>),
    Reconnect(Box<Profile>),
    Disconnect,
    CloseTab(u64),
    Quit,
    LockEncrypted,
}

impl EditorApp {
    fn transition_facts(
        &self,
        action: &Action,
        ctx: &egui::Context,
    ) -> (Policy, documents::Changes) {
        let mut preserve_buffers = matches!(action, Action::Disconnect);
        let mut valid = !self.exit.waiting() && !self.exit.closing();
        let replacing = matches!(
            action,
            Action::Project { .. }
                | Action::PreparedProject { .. }
                | Action::Remote(_)
                | Action::Reconnect(_)
        );
        let vault = match action {
            Action::Remote(profile) => {
                preserve_buffers = self.can_resume_ssh(profile);
                vaults::Transition::Connection
            }
            Action::Reconnect(profile) => {
                valid &= self.can_reconnect_ssh(profile);
                preserve_buffers = true;
                vaults::Transition::RecoveryConnection
            }
            Action::Disconnect => {
                valid &= self.workspace.as_ref().is_some_and(|w| w.remote.is_some());
                vaults::Transition::Connection
            }
            Action::Project { path, remote, .. } => vaults::Transition::Project(
                path,
                self.workspace
                    .as_ref()
                    .filter(|w| w.remote.is_some() == *remote)
                    .and_then(|w| w.remote.as_ref()),
            ),
            Action::PreparedProject { id } => {
                if let Some(prepared) = self.workspace.prepared(*id) {
                    preserve_buffers = prepared.keep_buffers;
                    vaults::Transition::Project(
                        &prepared.response.path,
                        self.workspace
                            .as_ref()
                            .filter(|w| w.remote.is_some() == prepared.remote)
                            .and_then(|w| w.remote.as_ref()),
                    )
                } else {
                    valid = false;
                    vaults::Transition::Stay
                }
            }
            _ => vaults::Transition::Stay,
        };
        let document = match action {
            Action::CloseTab(id) => Some(*id),
            _ => None,
        };
        let runtime_pending = if let Some(id) = document {
            valid &= self.documents.iter().any(|doc| doc.id == id);
            self.documents.notebook_pending(id)
        } else {
            (replacing || matches!(action, Action::Disconnect))
                && (self.runtime_work_pending() || self.agent_installation_pending())
        };
        (
            Policy {
                valid,
                source_pending: self.editor_operations.collecting()
                    || self.editor_input_pending(ctx),
                files_pending: self.explorer_state.busy,
                runtime_pending,
                vault: self.vaults.admission(vault),
                preserve_buffers,
            },
            self.documents.changes_for(document),
        )
    }
    pub(in crate::app) fn guarded(&mut self, action: Action, ctx: &egui::Context) {
        if let Action::PreparedProject { .. } = &action {
            let busy = self.pending.as_ref().is_some_and(|pending| match pending {
                Action::PreparedProject { id } => self.workspace.prepared(*id).is_some(),
                _ => true,
            });
            if busy {
                self.release_guarded_action(action);
                return;
            }
        } else if matches!(
            action,
            Action::Quit
                | Action::Disconnect
                | Action::Remote(_)
                | Action::Reconnect(_)
                | Action::LockEncrypted
        ) {
            self.invalidate_project_candidate(workspace::PickerReset::Invalidate);
        }
        self.request_guarded(action, ctx, false);
    }
    /// The input owner proves that earlier source edits already crossed their widget boundary.
    /// Events after this explicit Close command belong to its destination, not the old buffer.
    pub(in crate::app) fn guarded_source_close(
        &mut self,
        frame: &input::Frame,
        ctx: &egui::Context,
    ) {
        if let Some(id) = frame
            .document()
            .filter(|id| Some(*id) == self.documents.selected_id())
        {
            self.request_guarded(Action::CloseTab(id), ctx, true);
        }
    }
    fn request_guarded(&mut self, action: Action, ctx: &egui::Context, source_boundary: bool) {
        // Prepared replies are polled on the root viewport; keep the initiating dialog owner.
        let owner = if matches!(&action, Action::PreparedProject { .. }) {
            self.dialog_viewport
        } else {
            ctx.viewport_id()
        };
        self.cancel_guarded_action();
        let approved = match &action {
            Action::PreparedProject { id } => self
                .workspace
                .prepared(*id)
                .map(|p| p.approved_documents.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let (mut policy, changes) = self.transition_facts(&action, ctx);
        if source_boundary {
            policy.source_pending = false;
        }
        let effect = self.pending.request(action, policy, changes, approved);
        self.dialog_viewport = owner;
        self.transition_effect(effect, ctx);
    }
    pub(in crate::app) fn advance_guarded_action(&mut self, ctx: &egui::Context) {
        let Some(action) = self.pending.as_ref() else {
            return;
        };
        let (policy, changes) = self.transition_facts(action, ctx);
        let effect = self.pending.advance(policy, changes);
        self.transition_effect(effect, ctx);
    }
    pub(in crate::app) fn discard_guarded_action(
        &mut self,
        id: control::DecisionId,
        ctx: &egui::Context,
    ) {
        let Some(action) = self.pending.as_ref() else {
            return;
        };
        let (policy, changes) = self.transition_facts(action, ctx);
        let effect = self.pending.discard(id, policy, changes);
        self.transition_effect(effect, ctx);
    }
    fn transition_effect(&mut self, effect: Effect, ctx: &egui::Context) {
        match effect {
            Effect::Execute(action) => self.perform(action, ctx),
            Effect::Cancelled(action) => self.release_guarded_action(action),
            Effect::Rejected(action, barrier) => {
                self.release_guarded_action(action);
                self.notify(barrier.message(), true);
            }
            Effect::Held => ctx.request_repaint_after(Duration::from_millis(50)),
            Effect::None => {}
        }
    }
    fn release_guarded_action(&mut self, action: Action) {
        if let Action::PreparedProject { id } = action {
            self.workspace.cancel_prepared(id);
        }
    }
    pub(in crate::app) fn cancel_guarded_action(&mut self) {
        if let Some(action) = self.pending.clear() {
            self.release_guarded_action(action);
        }
    }
    pub(in crate::app) fn cancel_guarded_decision(&mut self, id: control::DecisionId) {
        if let Some(action) = self.pending.cancel(id) {
            self.release_guarded_action(action);
        }
    }
    /// Inject an accepted effect in low-level domain-lifetime tests only.
    #[cfg(test)]
    pub(in crate::app) fn perform_fixture(&mut self, action: Action, ctx: &egui::Context) {
        self.perform(action, ctx);
    }
    /// Execute only effects accepted by the shared transition controller.
    fn perform(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::PreparedProject { id } => {
                if let Some(prepared) = self.workspace.take_prepared(id) {
                    self.commit_project(
                        prepared.response,
                        prepared.remote,
                        prepared.options,
                        prepared.keep_buffers,
                        prepared.open_file,
                        ctx,
                    );
                }
            }
            Action::Project {
                path,
                remote,
                options,
            } => self.prepare_project(path, options, remote, false, None, ctx),
            Action::Remote(profile) => {
                let resume = self.can_resume_ssh(&profile);
                self.connect(*profile, ctx, resume);
            }
            Action::Reconnect(profile) => self.connect(*profile, ctx, true),
            Action::Disconnect => self.detach_ssh(ctx),
            Action::CloseTab(id) => {
                let owner = self.documents.owner(id);
                if self.documents.notebook_pending(id) {
                    self.notify("Wait for kernel startup or submitted work to be acknowledged before closing this notebook.",false);
                    return;
                }
                if self.documents.close(id).is_some() && let Some(viewport) = owner {
                    self.focus_editor_in(viewport, ctx);
                }
            }

            Action::Quit => match self.vaults.quit_policy() {
                vaults::QuitPolicy::ClearLostProject => {
                    self.clear_project(documents::Retention::Discard, workspace::LeaveReason::ProjectChanged);
                    self.request_exit(ctx);
                }
                vaults::QuitPolicy::Lock => self.lock_encrypted(true, ctx),
                vaults::QuitPolicy::RecoveryRequired => self.notify(
                    "Finish encrypted-project unlocking or locking before exiting. Use Open encrypted project → Lock existing mount to resolve an unlocked mount.", true),
                vaults::QuitPolicy::Close => self.request_exit(ctx),
            },
            Action::LockEncrypted => self.lock_encrypted(false, ctx),
        }
    }
}
