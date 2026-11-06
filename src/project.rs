//! Shared project services, composed with persistent session control.
use anyhow::Result;
pub use rode_host_core::project::git_auth;
pub use rode_host_core::project::{
    Analysis, Cpu, Diagnostic, Environment, Environments, GitFile, Gpu, KernelSpec, Monitor,
    Output, Process, Runtime, StashAction, environment_variables, hunk_patches, parse_nvidia,
    parse_status, python_path, run, run_controlled, run_controlled_limited, run_controlled_stream,
};
/// Desktop/session composition of the shared request envelope.
pub type Request =
    rode_host_core::project::Request<crate::sessions::Operation, crate::sessions::Helper>;
/// Desktop/session composition of the shared result envelope.
pub type Response = rode_host_core::project::Response<crate::sessions::Reply>;
/// Run inside the captured host policy; session administration is handled separately.
pub fn local(request: &Request) -> Result<Response> {
    if let Request::Sessions { operation, .. } = request {
        return Ok(Response::Sessions(crate::sessions::local(operation)?));
    }
    rode_host_core::project::local(request)
}
