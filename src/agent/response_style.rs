//! Compare the selected response style with the native host's acknowledged setting.
use super::Personality;

/// A selected tone has not yet been confirmed by the process that writes its hint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// A settings request is still awaiting its correlated host acknowledgement.
    Pending(Personality),
    /// The host retained a different tone.
    Mismatch {
        /// The user's selection, retained until they explicitly change it.
        selected: Personality,
        /// The actual tone reported by the native controller.
        applied: Personality,
    },
}
impl Status {
    /// Explain the observed state without claiming an unverified helper version.
    pub fn message(self) -> String {
        match self {
            Self::Pending(selected) => format!(
                "Waiting for the agent host to confirm {}. New messages are held.",
                selected.label()
            ),
            Self::Mismatch { selected, applied } => format!(
                "Tone not applied: {} is selected, but the agent host reports {}. Review the host settings or select {}. New messages are held; existing work is not stopped.",
                selected.label(),
                applied.label(),
                applied.label()
            ),
        }
    }
}

/// These operations can submit work or release automatic dispatch.
pub(crate) fn requires_confirmation(operation: &rode_agent_core::queue::Action) -> bool {
    matches!(
        operation,
        rode_agent_core::queue::Action::Push { .. }
            | rode_agent_core::queue::Action::Dispatch { .. }
            | rode_agent_core::queue::Action::Pause { paused: false }
    )
}
