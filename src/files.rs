//! Bounded file operations shared by the local and remote workspace backends.
//!
//! # Design decisions
//!
//! Content digests identify the bytes a buffer was based on; timestamp/size
//! checks alone can miss an external edit with unchanged metadata. Cooperating
//! writers lock and verify the current inode before publishing a replacement.
//! Source directories need no permanent lock files, which avoids leaving editor
//! artifacts in repositories; configuration uses a different stable-lock scheme
//! because merging shared preferences must also cover destination creation.
//!
//! Saves acknowledge their submitted snapshot, not the buffer at completion, so
//! typing during I/O remains unsaved rather than being incorrectly marked clean.
//! Symlink targets are resolved before replacement to preserve the link itself.
//! These rules coordinate RODE's local and remote writers; external programs remain
//! subject to best-effort conflict detection rather than a universal file lock.
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Shared file request with the editor's project-service and policy payloads.
pub type Request =
    rode_host_protocol::files::Request<crate::project::Request, crate::sandbox::Options>;
pub use rode_host_core::files::{digest, expand_home};
pub use rode_host_protocol::files::{Entry, MAX_ENTRIES, MAX_FILE_BYTES};

/// File-channel reply plus desktop-only decoded attachments. Unused wire fields retain
/// defaults; fields marked `serde(skip)` never cross SSH.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Response {
    /// Desktop-only normalized prompt contents, never accepted from the file peer.
    #[serde(skip)]
    pub attachment: Option<rode_agent_core::attachment::Attachment>,
    /// Canonical host path associated with the operation.
    pub path: String,
    /// Sorted immediate children for a directory listing.
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// UTF-8 source contents for a text read.
    #[serde(default)]
    pub text: String,
    /// SHA-256 of the bytes read or successfully saved.
    #[serde(default)]
    pub revision: String,
    /// Actual sandbox plan returned by project preparation.
    #[serde(default)]
    pub project: Option<crate::sandbox::Plan>,
    /// Project-service result when the request was a tool operation.
    #[serde(default)]
    pub tool: Option<crate::project::Response>,
    /// Standard base64-encoded raster bytes, before desktop decoding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Standard base64-encoded binary/preview bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary: Option<String>,
    /// Desktop-only rendered PDF and retained source snapshot.
    #[serde(skip)]
    pub pdf: Option<crate::pdf::Loaded>,
    /// Desktop-only decoded raster pixels and budget lease.
    #[serde(skip)]
    pub preview: Option<crate::images::Decoded>,
    /// Desktop-only decoded binary snapshot and static report.
    #[serde(skip)]
    pub hex: Option<crate::binary::Loaded>,
    /// Desktop-only notebook parse result; failure preserves access to source JSON.
    #[serde(skip)]
    pub notebook: Option<Result<crate::notebook::Notebook, String>>,
}

impl From<rode_host_protocol::files::Response> for Response {
    fn from(reply: rode_host_protocol::files::Response) -> Self {
        Self {
            path: reply.path,
            entries: reply.entries,
            text: reply.text,
            revision: reply.revision,
            image: reply.image,
            binary: reply.binary,
            ..Default::default()
        }
    }
}

/// Execute on the current host. The caller owns the sandbox boundary and worker lifetime.
pub fn local(request: &Request) -> Result<Response> {
    match request {
        Request::Tool { request } => Ok(Response {
            tool: Some(crate::project::local(request)?),
            ..Default::default()
        }),
        Request::Prepare { path, sandbox } => {
            let plan = crate::sandbox::prepare(path, sandbox)?;
            let mut response = local(&Request::List {
                path: plan.root.clone(),
            })?;
            response.project = Some(plan);
            Ok(response)
        }
        _ => rode_host_core::files::dispatch(request).map(Into::into),
    }
}
