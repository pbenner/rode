//! Window exit state is independent of project discovery and session caches.
//! Domain owners retain their cleanup resources; this controller only aggregates
//! acknowledged readiness and decides whether the guarded exit must wait.
use super::super::documents::Changes;

#[derive(Default)]
enum Phase {
    #[default]
    Running,
    Waiting,
    Closing,
}

/// Quit state and exact dirty versions whose discard was approved. Resource owners retain their
/// processes and leases until cleanup is acknowledged.
#[derive(Default)]
pub(in crate::app) struct Exit {
    phase: Phase,
    approved_discard: Vec<(u64, u64)>,
}

/// Initial adapter effect after quit guards: wait for acknowledgements or accept native window
/// closure.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Decision {
    Wait,
    Close,
}

/// Cleanup may reveal another owned operation; the adapter must retry cleanup,
/// not close directly on a single ready snapshot.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Progress {
    Idle,
    RecheckDocuments,
    RetryCleanup,
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
    /// Whether any terminal, runtime, notebook or debugger owner still has pending lifetime work.
    pub fn runtime_pending(&self) -> bool {
        self.terminals || self.runtimes || self.notebooks || self.debugger
    }
    /// Whether runtime work or an installation acknowledgement still blocks normal window exit.
    pub fn pending(&self) -> bool {
        self.runtime_pending() || self.installation
    }
}

impl Exit {
    /// Whether quit has been accepted but cleanup/save acknowledgements still prevent native
    /// closure.
    pub fn waiting(&self) -> bool {
        matches!(self.phase, Phase::Waiting)
    }
    /// Whether the native close has already been accepted; later failures cannot reopen this state.
    pub fn closing(&self) -> bool {
        matches!(self.phase, Phase::Closing)
    }
    /// Called only after dirty-buffer and encrypted-project guards accept Quit.
    pub(super) fn request(&mut self, readiness: Readiness, documents: Changes) -> Decision {
        if matches!(self.phase, Phase::Running) {
            self.approved_discard = documents.dirty_versions;
        }
        if !self.closing() {
            self.phase = if readiness.pending() || documents.saving {
                Phase::Waiting
            } else {
                Phase::Closing
            };
        }
        if self.closing() {
            Decision::Close
        } else {
            Decision::Wait
        }
    }
    /// A discard decision applies only to versions accepted by the quit guard.
    pub(super) fn advance(&mut self, readiness: Readiness, documents: Changes) -> Progress {
        if !self.waiting() || readiness.pending() || documents.saving {
            return Progress::Idle;
        }
        if documents.unapproved(&self.approved_discard) {
            self.cancel();
            Progress::RecheckDocuments
        } else {
            Progress::RetryCleanup
        }
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
        assert_eq!(
            exit.request(readiness(true, true), Changes::default()),
            Decision::Wait
        );
        assert_eq!(
            exit.request(readiness(false, true), Changes::default()),
            Decision::Wait
        );
        assert!(exit.waiting());
        assert_eq!(
            exit.request(readiness(false, false), Changes::default()),
            Decision::Close
        );
        assert!(exit.closing());
    }
    #[test]
    fn failure_cancels_automatic_exit_without_undoing_an_already_accepted_close() {
        let mut exit = Exit::default();
        assert_eq!(
            exit.request(readiness(false, true), Changes::default()),
            Decision::Wait
        );
        assert!(exit.cancel());
        assert!(!exit.waiting());
        assert!(!exit.closing());
        assert_eq!(
            exit.request(readiness(false, false), Changes::default()),
            Decision::Close
        );
        assert!(!exit.cancel());
        assert_eq!(
            exit.request(readiness(true, true), Changes::default()),
            Decision::Close
        );
        assert!(exit.closing());
    }

    #[test]
    fn a_clean_save_in_flight_requires_its_acknowledgement_before_cleanup_retries() {
        let mut exit = Exit::default();
        assert_eq!(
            exit.request(
                readiness(false, false),
                Changes {
                    saving: true,
                    ..Default::default()
                }
            ),
            Decision::Wait
        );
        assert_eq!(
            exit.advance(
                readiness(false, false),
                Changes {
                    saving: true,
                    ..Default::default()
                }
            ),
            Progress::Idle
        );
        assert_eq!(
            exit.advance(readiness(false, false), Changes::default()),
            Progress::RetryCleanup
        );
        assert!(
            !exit.closing(),
            "Ready cleanup must be retried by its owner"
        );
    }

    #[test]
    fn cleanup_retries_cannot_approve_newer_discard_versions() {
        let documents = |version| Changes {
            dirty_versions: vec![(1, version)],
            saving: false,
        };
        let mut exit = Exit::default();
        assert_eq!(
            exit.request(readiness(false, true), documents(3)),
            Decision::Wait
        );
        assert_eq!(
            exit.advance(readiness(false, false), documents(3)),
            Progress::RetryCleanup
        );
        assert_eq!(
            exit.request(readiness(false, true), documents(4)),
            Decision::Wait
        );
        assert_eq!(
            exit.advance(readiness(false, false), documents(4)),
            Progress::RecheckDocuments
        );
        assert!(!exit.waiting());
        assert_eq!(
            exit.advance(readiness(false, false), Changes::default()),
            Progress::Idle
        );
    }
}
