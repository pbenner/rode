//! Window exit state is independent of project discovery and session caches.
//! Domain owners retain their cleanup resources; this controller only aggregates
//! acknowledged readiness and decides whether the guarded exit must wait.

#[derive(Default)]
enum Phase {
    #[default]
    Running,
    Waiting,
    Closing,
}

#[derive(Default)]
pub(in crate::app) struct Exit {
    phase: Phase,
    approved_discard: Vec<(u64, u64)>,
}

/// A GUI-thread snapshot. No flag transfers ownership of a process or lease.
pub(in crate::app) struct Readiness {
    pub terminals: bool,
    pub runtimes: bool,
    pub notebooks: bool,
    pub debugger: bool,
    pub installation: bool,
}
impl Readiness {
    pub fn runtime_pending(&self) -> bool {
        self.terminals || self.runtimes || self.notebooks || self.debugger
    }
    pub fn pending(&self) -> bool {
        self.runtime_pending() || self.installation
    }
}

impl Exit {
    pub fn waiting(&self) -> bool {
        matches!(self.phase, Phase::Waiting)
    }
    pub fn closing(&self) -> bool {
        matches!(self.phase, Phase::Closing)
    }
    /// Called only after dirty-buffer and encrypted-project guards accept Quit.
    pub fn request(
        &mut self,
        readiness: Readiness,
        dirty_versions: impl IntoIterator<Item = (u64, u64)>,
    ) -> bool {
        if matches!(self.phase, Phase::Running) {
            self.approved_discard = dirty_versions.into_iter().collect();
        }
        if !self.closing() {
            self.phase = if readiness.pending() {
                Phase::Waiting
            } else {
                Phase::Closing
            };
        }
        self.closing()
    }
    /// A discard decision applies only to the source versions accepted by the quit guard.
    pub fn discard_approved(&self, id: u64, version: u64) -> bool {
        self.approved_discard.contains(&(id, version))
    }
    /// Failure cancels automatic exit. A later clean snapshot cannot restart it.
    pub fn cancel(&mut self) -> bool {
        if !self.waiting() {
            return false;
        }
        self.phase = Phase::Running;
        self.approved_discard.clear();
        true
    }
    /// Explicit close after guards, or the opt-in bounded automation hook.
    pub fn close(&mut self) {
        self.phase = Phase::Closing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn readiness(installation: bool, notebooks: bool) -> Readiness {
        Readiness {
            terminals: false,
            runtimes: false,
            notebooks,
            debugger: false,
            installation,
        }
    }
    #[test]
    fn every_outstanding_domain_acknowledgement_keeps_exit_waiting() {
        let mut exit = Exit::default();
        assert!(!exit.request(readiness(true, true), []));
        assert!(!exit.request(readiness(false, true), []));
        assert!(exit.waiting());
        assert!(exit.request(readiness(false, false), []));
        assert!(exit.closing());
    }
    #[test]
    fn failure_cancels_automatic_exit_without_undoing_an_already_accepted_close() {
        let mut exit = Exit::default();
        assert!(!exit.request(readiness(false, true), []));
        assert!(exit.cancel());
        assert!(!exit.waiting());
        assert!(!exit.closing());
        assert!(exit.request(readiness(false, false), []));
        assert!(!exit.cancel());
        assert!(exit.request(readiness(true, true), []));
        assert!(exit.closing());
    }
}
