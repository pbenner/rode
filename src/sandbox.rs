//! Desktop file/adapter dispatch through the shared host policy.
use anyhow::{Context, Result};
pub use rode_host_core::sandbox::*;
use std::{path::Path, process::Command, time::Duration};

pub(crate) mod presets;

/// Desktop reply decoding and service dispatch, outside the headless policy library.
pub trait PlanExt {
    /// Run an installed adapter under the captured project policy.
    fn extension_request(
        &self,
        request: &crate::project::Request,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<crate::extensions::Reply>;
    /// Dispatch a desktop file/service request through this policy.
    fn local_request(&self, request: &crate::files::Request) -> Result<crate::files::Response>;
    /// Dispatch using an explicitly retained executable lease.
    fn local_request_with_helper(
        &self,
        request: &crate::files::Request,
        executable: &Path,
    ) -> Result<crate::files::Response>;
    /// Whether this operation should resolve the configured environment provider.
    fn environment_applies(&self, request: &crate::files::Request) -> bool;
}
impl PlanExt for Plan {
    /// Run an adapter coordinator under this captured active plan with cancellation;
    /// a failure never retries the request outside its sandbox.
    fn extension_request(
        &self,
        request: &crate::project::Request,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<crate::extensions::Reply> {
        let (command, _helper) = self.coordinator_command("--extension-helper")?;
        crate::extensions::execute(command, request, cancel)
    }
    /// Run a blocking file request under this plan; active-sandbox failures never retry outside
    /// it.
    fn local_request(&self, request: &crate::files::Request) -> Result<crate::files::Response> {
        if !self.active && !self.environment_applies(request) {
            return crate::files::local(request);
        }
        if let Some(executable) = &self.native_helper {
            return self.local_request_with_helper(request, Path::new(executable));
        }
        let helper = match &self.helper {
            Some(helper) => helper.clone(),
            None => HelperBinary::new()?,
        };
        self.local_request_with_helper(request, &helper.path)
    }

    /// Run a file request using an explicitly supplied helper executable. Active plans bind
    /// only that helper into the sandbox; callers retain the helper for the operation's
    /// lifetime.
    fn local_request_with_helper(
        &self,
        request: &crate::files::Request,
        executable: &Path,
    ) -> Result<crate::files::Response> {
        if !self.active && !self.environment_applies(request) {
            return crate::files::local(request);
        }
        let mut args = if self.active {
            self.prefix[1..].to_vec()
        } else {
            vec![]
        };
        let destination = self.helper_destination(executable, "/rode-file-helper");
        if self.active {
            anyhow::ensure!(
                args.pop().as_deref() == Some("--"),
                "Invalid sandbox launch plan"
            );
            args.extend([
                "--ro-bind".into(),
                executable.to_string_lossy().into(),
                destination.clone(),
                "--".into(),
            ]);
        }
        if self.environment_applies(request) {
            args.extend(self.environment_prefix("")?);
        }
        args.extend([
            if self.active {
                destination
            } else {
                executable.to_string_lossy().into_owned()
            },
            "--file-helper".into(),
        ]);
        let command = if self.active {
            let mut command = Command::new(&self.prefix[0]);
            command.args(args);
            command
        } else {
            let mut command = Command::new(&args[0]);
            command.args(&args[1..]);
            command
        };
        let output = crate::ssh::run_bounded(
            command,
            serde_json::to_vec(request)?,
            Duration::from_secs(if matches!(request, crate::files::Request::Tool { .. }) {
                150
            } else {
                45
            }),
        )?;
        serde_json::from_slice(&output).context("Invalid sandbox file response")
    }

    fn environment_applies(&self, request: &crate::files::Request) -> bool {
        self.environment.mise
            && matches!(request,
            crate::files::Request::Tool { request } if !matches!(request,
                crate::project::Request::Mise { .. } | crate::project::Request::Monitor { .. }
                | crate::project::Request::Sessions { .. }))
    }
}
