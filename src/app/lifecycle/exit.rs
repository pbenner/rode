//! Window exit state is independent of project discovery and session caches.
//! Domain owners retain their cleanup resources; this controller only aggregates
//! acknowledged readiness and decides whether the guarded exit must wait.
use super::super::documents::ExitSnapshot;

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
    native_close: bool,
}

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
    pub(super) fn request(&mut self, readiness: Readiness, documents: ExitSnapshot) -> Decision {
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
    pub(super) fn advance(&mut self, readiness: Readiness, documents: ExitSnapshot) -> Progress {
        if !self.waiting() || readiness.pending() || documents.saving {
            return Progress::Idle;
        }
        if documents
            .dirty_versions
            .iter()
            .any(|version| !self.approved_discard.contains(version))
        {
            self.cancel();
            Progress::RecheckDocuments
        } else {
            Progress::RetryCleanup
        }
    }
    /// Native close waits outside modal guards until admitted keyboard input has
    /// crossed its native editor boundary. A single close event retains its intent.
    pub fn native_request(&mut self, requested: bool, input_pending: bool) -> bool {
        self.native_close |= requested;
        !input_pending && std::mem::take(&mut self.native_close)
    }
    /// Failure cancels automatic exit. A later clean snapshot cannot restart it.
    pub fn cancel(&mut self) -> bool {
        if !self.waiting() {
            return false;
        }
        self.phase = Phase::Running;
        self.approved_discard.clear();
        self.native_close = false;
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
            exit.request(readiness(true, true), ExitSnapshot::default()),
            Decision::Wait
        );
        assert_eq!(
            exit.request(readiness(false, true), ExitSnapshot::default()),
            Decision::Wait
        );
        assert!(exit.waiting());
        assert_eq!(
            exit.request(readiness(false, false), ExitSnapshot::default()),
            Decision::Close
        );
        assert!(exit.closing());
    }
    #[test]
    fn failure_cancels_automatic_exit_without_undoing_an_already_accepted_close() {
        let mut exit = Exit::default();
        assert_eq!(
            exit.request(readiness(false, true), ExitSnapshot::default()),
            Decision::Wait
        );
        assert!(exit.cancel());
        assert!(!exit.waiting());
        assert!(!exit.closing());
        assert_eq!(
            exit.request(readiness(false, false), ExitSnapshot::default()),
            Decision::Close
        );
        assert!(!exit.cancel());
        assert_eq!(
            exit.request(readiness(true, true), ExitSnapshot::default()),
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
                ExitSnapshot {
                    saving: true,
                    ..Default::default()
                }
            ),
            Decision::Wait
        );
        assert_eq!(
            exit.advance(
                readiness(false, false),
                ExitSnapshot {
                    saving: true,
                    ..Default::default()
                }
            ),
            Progress::Idle
        );
        assert_eq!(
            exit.advance(readiness(false, false), ExitSnapshot::default()),
            Progress::RetryCleanup
        );
        assert!(
            !exit.closing(),
            "Ready cleanup must be retried by its owner"
        );
    }

    #[test]
    fn cleanup_retries_cannot_approve_newer_discard_versions() {
        let documents = |version| ExitSnapshot {
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
            exit.advance(readiness(false, false), ExitSnapshot::default()),
            Progress::Idle
        );
    }
}
