//! Single owner of deferred transitions and versioned discard decisions.
//! Domain adapters supply facts; this state machine performs no I/O or GUI mutation.
use super::Action;
use crate::app::{documents::Changes, vaults::Barrier};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct DecisionId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Wait {
    Input,
    Files,
    Runtime,
    Save,
}
impl Wait {
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
    pub valid: bool,
    pub source_pending: bool,
    pub files_pending: bool,
    pub runtime_pending: bool,
    pub vault: Barrier,
    pub preserve_buffers: bool,
}

pub(super) enum Effect {
    None,
    Held,
    Execute(Action),
    Cancelled(Action),
    Rejected(Action, Barrier),
}

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
    pub fn as_ref(&self) -> Option<&Action> {
        self.pending.as_ref().map(|p| &p.action)
    }
    #[cfg(test)]
    pub fn is_some(&self) -> bool {
        self.pending.is_some()
    }
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
    #[cfg(test)]
    pub fn waiting_for_save(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|p| p.phase == Phase::Wait(Wait::Save))
    }
    pub fn waiting_for_ack(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| {
            matches!(
                p.phase,
                Phase::Wait(Wait::Files | Wait::Runtime | Wait::Save)
            )
        })
    }
    pub(super) fn clear(&mut self) -> Option<Action> {
        self.pending.take().map(|p| p.action)
    }
    /// An old modal callback cannot cancel a replacement intent.
    pub(super) fn cancel(&mut self, id: DecisionId) -> Option<Action> {
        self.pending.as_ref().filter(|p| p.decision == id)?;
        self.clear()
    }
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
