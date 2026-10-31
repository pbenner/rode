//! Execution in one explicitly selected, owned allocation.
use anyhow::{Result, ensure};
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
        self.python_argv(plan, python, include_str!("../notebook_kernel.py"))
    }
    /// Launch the complete debugger coordinator inside the selected allocation.
    pub fn debugger_argv(&self, plan: &crate::sandbox::Plan, python: &str) -> Result<Vec<String>> {
        self.python_argv(plan, python, include_str!("../remote_debugger.py"))
    }
    fn python_argv(
        &self,
        plan: &crate::sandbox::Plan,
        python: &str,
        source: &str,
    ) -> Result<Vec<String>> {
        ensure!(
            !self.id.is_empty()
                && self.id.len() <= 20
                && self.id.bytes().all(|b| b.is_ascii_digit()),
            "Notebook targets require one ordinary Slurm job ID; arrays and heterogeneous jobs are not supported"
        );
        for program in [&self.queue, &self.runner] {
            ensure!(
                !program.trim().is_empty()
                    && !program.trim().starts_with('-')
                    && program.len() <= 1024
                    && !program.chars().any(char::is_control),
                "Choose an installed squeue/srun executable or wrapper"
            );
        }
        ensure!(
            !plan.active || !plan.prefix.is_empty(),
            "Missing compute sandbox plan"
        );
        let payload = serde_json::json!({
            "target": self,
            "root": plan.root,
            "prefix": if plan.active { plan.prefix.clone() } else { vec![] },
            "python": python,
            "source": source,
        });
        Ok(vec![
            "python3".into(),
            "-u".into(),
            "-c".into(),
            include_str!("step.py").into(),
            payload.to_string(),
        ])
    }
}
