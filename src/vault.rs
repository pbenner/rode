//! Desktop client for the native encrypted-project lease coordinator.
use anyhow::Result;
pub use rode_host_core::vault::*;
use std::path::Path;
/// A host-level lease uses the same bounded asynchronous transport as project
/// tools, with an explicit unsandboxed launch limited to filesystem management.
pub struct Client {
    /// Owned asynchronous lease-coordinator transport, independent of project tools.
    pub session: crate::process::Session,
    /// Mount identity once verified by the coordinator.
    pub info: Option<Info>,
    /// SSH identity owning this host lease, or local when absent.
    pub remote: Option<crate::ssh::Remote>,
    /// Whether the lease close lifecycle completed.
    pub finished: bool,
}
impl Client {
    /// Start local/SSH filesystem management with transient stdin credentials; launch no
    /// project process before verified mount information arrives.
    pub fn start(
        request: Open,
        remote: Option<crate::ssh::Remote>,
        wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        request.settings.validate()?;
        let argv = if remote.is_some() {
            vec!["@rode/vault".into()]
        } else {
            vec![
                format!("/proc/{}/exe", std::process::id()),
                "--vault-helper".into(),
            ]
        };
        let session = crate::process::Session::start(
            crate::process::Launch {
                native_debug: false,
                argv,
                plan: crate::sandbox::Plan {
                    root: "/".into(),
                    ..Default::default()
                },
                remote: remote.clone(),
                python: String::new(),
                python_program: false,
                line_limit: 64 * 1024,
            },
            wake,
        )?;
        session.send(&serde_json::to_value(request)?)?;
        Ok(Self {
            session,
            info: None,
            remote,
            finished: false,
        })
    }
    /// Request release of this caller's shared lease; does not force-unmount another window's
    /// project.
    pub fn release(&self) {
        let _ = self.session.send(&serde_json::json!({"action":"release"}));
    }
    /// Check whether a root is within this verified mount and uses the same local/SSH
    /// connection identity.
    pub fn matches(&self, root: &str, remote: Option<&crate::ssh::Remote>) -> bool {
        self.info
            .as_ref()
            .is_some_and(|info| Path::new(root).starts_with(&info.mountpoint))
            && self.remote.as_ref().map(|r| &r.socket) == remote.map(|r| &r.socket)
    }
}
