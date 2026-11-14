//! Single owner of deferred transitions and versioned discard decisions.
//! Domain adapters supply facts; this state machine performs no I/O or GUI mutation.
use super::Action;
use crate::app::{documents::Changes, vaults::Barrier};

/// Opaque identity for the current confirmation or wait presentation. A callback must present this
/// identity before it can approve or cancel an intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct DecisionId(u64);

/// Acknowledgement barrier that currently delays an ownership change. Input waits remain nonmodal
/// so the source widget can finish its work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Wait {
    /// Native source input must cross its widget boundary before ownership changes.
    Input,
    /// A submitted explorer mutation must release the current project.
    Files,
    /// Runtime or lifetime work must acknowledge completion.
    Runtime,
    /// Submitted file contents must receive their save acknowledgement.
    Save,
}
impl Wait {
    /// User-facing explanation of the current acknowledgement barrier.
    pub fn message(self) -> &'static str {
        match self {
            Self::Input => "Applying queued editor input…",
            Self::Files => "Finishing the submitted file operation…",
            Self::Runtime => "Finishing submitted work and runtime lifetime changes…",
            Self::Save => "Waiting for file saves to be acknowledged…",
        }
    }
}

#[derive(PartialEq, Eq)]
enum Phase {
    Wait(Wait),
    Confirm(Vec<(u64, u64)>),
}
struct Pending {
    action: Action,
    decision: DecisionId,
    phase: Phase,
    approved: Vec<(u64, u64)>,
}

/// A current-host snapshot; no resource ownership crosses this interface.
pub(super) struct Policy {
    /// Whether the captured action still names an admissible target.
    pub valid: bool,
    /// Native input collection or replay has not finished.
    pub source_pending: bool,
    /// An explorer mutation still owns the current host context.
    pub files_pending: bool,
    /// Runtime or cleanup work still awaits acknowledgement.
    pub runtime_pending: bool,
    /// Current encrypted-workspace admission or lock barrier.
    pub vault: Barrier,
    /// The action retains buffers, so dirty text does not require discard approval.
    pub preserve_buffers: bool,
}

/// Pure state-machine result for the application adapter. Only `Execute` grants permission to
/// perform the captured ownership change.
pub(super) enum Effect {
    /// No matching action or callback remains to process.
    None,
    /// Keep the intent and schedule another policy evaluation.
    Held,
    /// Consume the admitted intent and perform its ownership change.
    Execute(Action),
    /// Release resources retained only by the cancelled intent.
    Cancelled(Action),
    /// Release the refused intent and present its admission barrier.
    Rejected(Action, Barrier),
}

/// Single owner of a deferred action and exact discard approvals. Policy/resource snapshots are
/// rechecked whenever the decision advances.
#[derive(Default)]
pub(in crate::app) struct Controller {
    serial: u64,
    pending: Option<Pending>,
}
impl Controller {
    fn next(&mut self) -> DecisionId {
        self.serial = self
            .serial
            .checked_add(1)
            .expect("Transition identity exhausted");
        DecisionId(self.serial)
    }
    /// Borrow the deferred intent without consuming or authorizing it.
    pub fn as_ref(&self) -> Option<&Action> {
        self.pending.as_ref().map(|p| &p.action)
    }
    /// Whether a test controller currently retains an intent.
    #[cfg(test)]
    pub fn is_some(&self) -> bool {
        self.pending.is_some()
    }
    /// Whether a test controller has no deferred intent.
    #[cfg(test)]
    pub fn is_none(&self) -> bool {
        self.pending.is_none()
    }
    /// Input waits remain nonmodal so the native source widget can acknowledge them.
    pub fn modal(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.phase != Phase::Wait(Wait::Input))
    }
    /// Modal decision identity and optional wait reason; input-only waits produce no modal
    /// presentation.
    pub fn presentation(&self) -> Option<(DecisionId, Option<Wait>)> {
        let pending = self.pending.as_ref().filter(|_| self.modal())?;
        Some((
            pending.decision,
            match pending.phase {
                Phase::Wait(reason) => Some(reason),
                Phase::Confirm(_) => None,
            },
        ))
    }
    /// Whether a test intent is specifically waiting for save acknowledgements.
    #[cfg(test)]
    pub fn waiting_for_save(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.phase == Phase::Wait(Wait::Save))
    }
    /// Whether file, runtime or save acknowledgements are delaying an intent. Input collection and
    /// discard confirmation are separate phases.
    pub fn waiting_for_ack(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| {
            matches!(
                p.phase,
                Phase::Wait(Wait::Files | Wait::Runtime | Wait::Save)
            )
        })
    }
    /// Cancel the retained intent and return its action so the adapter can release any associated
    /// candidate resources.
    pub(super) fn clear(&mut self) -> Option<Action> {
        self.pending.take().map(|p| p.action)
    }
    /// An old modal callback cannot cancel a replacement intent.
    pub(super) fn cancel(&mut self, id: DecisionId) -> Option<Action> {
        self.pending.as_ref().filter(|p| p.decision == id)?;
        self.clear()
    }
    /// Replace the pending intent, allocate a fresh decision identity and immediately evaluate
    /// current policy and document versions.
    pub(super) fn request(
        &mut self,
        action: Action,
        policy: Policy,
        changes: Changes,
        approved: Vec<(u64, u64)>,
    ) -> Effect {
        let decision = self.next();
        self.pending = Some(Pending {
            action,
            decision,
            phase: Phase::Wait(Wait::Input),
            approved,
        });
        self.advance(policy, changes)
    }
    /// Recheck both resource barriers and exact document versions on approval.
    pub(super) fn discard(&mut self, id: DecisionId, policy: Policy, changes: Changes) -> Effect {
        let Some(pending) = self.pending.as_mut().filter(|p| p.decision == id) else {
            return Effect::None;
        };
        if matches!(&pending.phase, Phase::Confirm(versions) if *versions == changes.dirty_versions)
        {
            pending.approved.clone_from(&changes.dirty_versions);
        }
        self.advance(policy, changes)
    }
    /// Re-evaluate validity, input, file, vault, runtime, save and dirty-version barriers in that
    /// order. Execution consumes the intent only after every required acknowledgement.
    pub(super) fn advance(&mut self, policy: Policy, changes: Changes) -> Effect {
        let Some(pending) = self.pending.as_ref() else {
            return Effect::None;
        };
        if !policy.valid {
            return Effect::Cancelled(self.clear().unwrap());
        }
        let phase = if policy.source_pending {
            Phase::Wait(Wait::Input)
        } else if policy.files_pending {
            Phase::Wait(Wait::Files)
        } else if policy.vault != Barrier::None {
            return Effect::Rejected(self.clear().unwrap(), policy.vault);
        } else if policy.runtime_pending {
            Phase::Wait(Wait::Runtime)
        } else if changes.saving {
            Phase::Wait(Wait::Save)
        } else if !policy.preserve_buffers && changes.unapproved(&pending.approved) {
            Phase::Confirm(changes.dirty_versions)
        } else {
            return Effect::Execute(self.clear().unwrap());
        };
        if self.pending.as_ref().unwrap().phase != phase {
            let decision = self.next();
            let pending = self.pending.as_mut().unwrap();
            pending.phase = phase;
            pending.decision = decision;
        }
        Effect::Held
    }
}
