//! Transport contracts shared by the desktop and headless host helper.
#![warn(missing_docs)]
pub mod files;
pub mod framing;

/// Handshake is exchanged before any operation, including preparation.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Hello {
    /// Wire major version, independent of the session protocol.
    pub version: u32,
    /// Executable architecture reported by the running helper.
    pub architecture: String,
    /// SHA-256 of the selected helper executable.
    pub digest: String,
    /// Operations implemented by this helper build.
    pub capabilities: Vec<String>,
}

/// Correlation identity is echoed on success and failure.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Request<T> {
    /// Connection-local request correlation identifier.
    pub id: u64,
    /// Typed operation to execute.
    pub operation: T,
}

/// Errors remain data; a transport failure is distinct from an acknowledged operation failure.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Reply<T> {
    /// Connection-local request correlation identifier.
    pub id: u64,
    /// Acknowledged result or bounded host-side diagnostic.
    pub result: Result<T, String>,
}

/// Operations exposed by the connection-owned gateway. Project contexts are opaque.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Operation<F, S, E> {
    /// Canonicalise and probe a project policy on the authenticated host.
    Prepare {
        /// Requested root on this host.
        path: String,
        /// User-requested sandbox settings.
        options: S,
    },
    /// Capture new environment settings without changing the prepared grants.
    Derive {
        /// Existing opaque project context.
        context: String,
        /// Literal environment configuration; resolved secrets are not returned.
        environment: E,
    },
    /// Execute inside the captured context, or a narrowly allowed host operation.
    Run {
        /// Required for ordinary project operations; absent only for host administration.
        context: Option<String>,
        /// Typed operation whose paths belong to this connection.
        request: F,
    },
}
