//! Execution in one explicitly selected, owned allocation.
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Captured scheduler identity and installed site programs. Targets are runtime
/// state, not notebook metadata; the host rechecks ownership before every launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// Exact non-array, non-heterogeneous allocation ID.
    pub id: String,
    /// Captured name guards against selecting a different job after ID reuse.
    pub name: String,
    /// Installed squeue executable or site wrapper.
    pub queue: String,
    /// Installed srun executable or site wrapper.
    pub runner: String,
}
impl Target {
    /// Build a stdio launch; neither the scheduler nor the compute process uses a PTY.
    /// Both bridge and kernel run inside the compute-node sandbox, when active.
    pub fn notebook_argv(&self, plan: &crate::sandbox::Plan, python: &str) -> Result<Vec<String>> {
        super::native::argv(self, plan, python, false)
    }
    /// Launch the complete debugger coordinator inside the selected allocation.
    pub fn debugger_argv(&self, plan: &crate::sandbox::Plan, python: &str) -> Result<Vec<String>> {
        super::native::argv(self, plan, python, true)
    }
}
