//! Explicit execution choices for user-operated Git and terminal surfaces.
//! These plans never replace the workspace policy used by agents and file tools.
use super::*;
use std::sync::{Arc, Mutex};

/// Retain the project host, environment and executable lease, without its sandbox.
/// SSH requests need a separately prepared host context; the restricted context
/// must not be reused with an altered desktop-side label.
pub(super) fn outside_sandbox(project: &sandbox::Plan) -> sandbox::Plan {
    sandbox::Plan {
        root: project.root.clone(),
        environment: project.environment.clone(),
        native_helper: project.native_helper.clone(),
        helper: project.helper.clone(),
        ..Default::default()
    }
}

/// One separate normal-mode SSH context per open project, prepared on a worker.
#[derive(Clone, Default)]
pub(super) struct HostPlan(Arc<Mutex<Option<sandbox::Plan>>>);

impl HostPlan {
    /// Prepare/cache normal-mode Git execution for an explicitly selected host mode.
    /// May block on SSH preparation; callers must use a worker and retain the separate project policy.
    pub(super) fn prepare(
        &self,
        project: &sandbox::Plan,
        remote: Option<&Remote>,
    ) -> anyhow::Result<sandbox::Plan> {
        let Some(remote) = remote else {
            return Ok(outside_sandbox(project));
        };
        let mut cached = self
            .0
            .lock()
            .map_err(|_| anyhow::anyhow!("Host Git context lock failed"))?;
        if cached.as_ref().is_none_or(|plan| plan.root != project.root) {
            let reply = remote.request_in(
                &Request::Prepare {
                    path: project.root.clone(),
                    sandbox: Default::default(),
                },
                None,
            )?;
            let plan = reply
                .project
                .ok_or_else(|| anyhow::anyhow!("Host returned no normal-mode Git context"))?;
            anyhow::ensure!(
                !plan.active && !plan.requested && plan.prefix.is_empty(),
                "Host did not prepare normal-mode Git execution"
            );
            *cached = Some(plan);
        }
        let mut plan = cached.as_ref().unwrap().clone();
        plan.environment = project.environment.clone();
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_choice_clears_isolation_without_changing_the_project_policy() {
        let mut project = sandbox::Plan {
            root: "/project".into(),
            requested: true,
            active: true,
            project_read_only: true,
            host_context: Some("restricted-context".into()),
            native_helper: Some("/host/helper".into()),
            prefix: vec!["sandbox".into(), "--".into()],
            ..Default::default()
        };
        project.environment.mise = true;
        let host = HostPlan::default().prepare(&project, None).unwrap();
        assert!(!host.active && !host.requested && !host.project_read_only);
        assert!(host.prefix.is_empty() && host.host_context.is_none());
        assert_eq!(host.root, project.root);
        assert_eq!(host.environment, project.environment);
        assert_eq!(host.native_helper, project.native_helper);
        assert!(project.active && project.requested && project.project_read_only);
        assert_eq!(project.host_context.as_deref(), Some("restricted-context"));
        assert_eq!(project.prefix, ["sandbox", "--"]);
    }
}
