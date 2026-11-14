//! Explicit execution choices for user-operated Git and terminal surfaces.
//! These plans never replace the workspace policy used by agents and file tools.
use super::*;
use crate::sandbox::PlanExt as _;
use std::sync::{Arc, Mutex};

/// Authority requested by a UI adapter before starting background work.
/// Host scope drops project policy only for explicit discovery or host operations.
#[derive(Clone, Copy)]
pub(super) enum Scope {
    /// Use the current project host and its prepared policy, when present.
    Project,
    /// Use the selected host without a project context.
    Host,
    /// Use the desktop host without borrowing a selected SSH connection.
    Local,
}

/// Execution destination captured on the UI thread. A worker never
/// consults a later workspace selection or reconstructs policy from display labels.
pub(super) struct Target {
    remote: Option<Remote>,
    plan: Option<sandbox::Plan>,
}

impl Target {
    /// Capture only the authority explicitly requested by the domain adapter.
    pub(super) fn capture(workspace: Option<&Workspace>, scope: Scope) -> Self {
        Self {
            remote: match scope {
                Scope::Local => None,
                Scope::Project | Scope::Host => workspace.and_then(|w| w.remote.clone()),
            },
            plan: match scope {
                Scope::Project => workspace.map(|w| w.plan.clone()),
                Scope::Host | Scope::Local => None,
            },
        }
    }

    /// Captured transport, never a connection looked up after dispatch.
    pub(super) fn remote(&self) -> Option<&Remote> {
        self.remote.as_ref()
    }

    /// Captured project policy; absence means the adapter explicitly chose host access
    /// or no project existed. Readiness remains an adapter admission requirement.
    pub(super) fn plan(&self) -> Option<&sandbox::Plan> {
        self.plan.as_ref()
    }

    /// Execute on the captured host and policy. Call only from a worker.
    pub(super) fn request(&self, request: &Request) -> anyhow::Result<Response> {
        match &self.remote {
            Some(remote) => remote.request_in(request, self.plan.as_ref()),
            None => match &self.plan {
                Some(plan) => plan.local_request(request),
                None => files::local(request),
            },
        }
    }

    /// Execute an installed adapter with the same captured host and policy.
    pub(super) fn extension_request(
        &self,
        request: &crate::project::Request,
        cancel: &AtomicBool,
    ) -> anyhow::Result<crate::project::Response> {
        let reply = match &self.remote {
            Some(remote) => remote.extension_request(request, self.plan.as_ref(), cancel),
            None => self
                .plan
                .clone()
                .unwrap_or_default()
                .extension_request(request, cancel),
        }?;
        Ok(crate::project::Response::Extension(reply))
    }

    /// Prepare a separate normal-mode context for an explicit user Git choice.
    /// The captured project plan remains the source of root/environment authority.
    pub(super) fn prepare_host_git(&mut self, host: &HostPlan) -> anyhow::Result<()> {
        let project = self
            .plan
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Git requires a prepared project"))?;
        self.plan = Some(host.prepare(project, self.remote.as_ref())?);
        Ok(())
    }
}

/// One worker's captured execution target, generation-bound reply endpoint and
/// encrypted-mount work guard. Keep this value alive through reply publication.
pub(super) struct Work {
    pub target: Target,
    replies: workers::WorkspaceSender,
    _vault_work: vaults::Work,
}

impl Work {
    /// Publish only through the generation captured with the execution target.
    pub(super) fn send(&self, event: workers::WorkspaceEvent) {
        let _ = self.replies.send(event);
    }
}

impl EditorApp {
    /// Capture transport, policy, reply generation and mount-work ownership together.
    /// Domain-specific readiness checks must run before calling this composition boundary.
    pub(super) fn capture_work(&self, scope: Scope, ctx: &egui::Context) -> Work {
        Work {
            target: Target::capture(self.workspace.as_ref(), scope),
            replies: self.workers.workspace(ctx),
            _vault_work: self.vaults.work(),
        }
    }
}

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
