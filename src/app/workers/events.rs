//! Domain job descriptions and result payloads. The inbox owns lifecycle
//! envelopes and admission; feature producers never choose envelope generations.
use super::*;

/// Captured completion intent for file workers. Prepare/project-folder discovery uses raw
/// host scope; ordinary lists, file pickers and source operations use the active plan.
pub(in crate::app) enum Job {
    Prepare {
        remote: bool,
        options: sandbox::Options,
        keep_buffers: bool,
        token: u64,
        documents_at_start: Vec<(u64, u64)>,
        /// Startup restores saved host/project grants, optionally forcing sandboxing on.
        restore_saved: Option<bool>,
        /// Local startup file to open only after its containing project is accepted.
        open_file: Option<String>,
    },
    /// Local CLI file opened without inferring an unsafe whole-home project.
    StandaloneFile {
        path: String,
        token: u64,
    },
    Browse {
        remote: bool,
        token: u64,
        path: String,
        options: sandbox::Options,
    },
    BrowseFiles {
        token: u64,
        path: String,
        local: bool,
    },
    AgentAttachment {
        token: u64,
        local: bool,
    },
    List(String, u64),
    Explorer(explorer::Change),
    Open(String),
    SaveBinary {
        id: u64,
        bytes: Arc<Vec<u8>>,
        _lease: Arc<crate::images::Lease>,
    },
    Save {
        id: u64,
        text: String,
    },
    Reload {
        id: u64,
        version: u64,
    },
}

/// Domain payloads cannot bypass the envelope generation check.
pub(in crate::app) enum WorkspaceEvent {
    GitGutter(super::super::git_gutter::Completed),
    Latex {
        token: u64,
        preview: bool,
        versions: Vec<(u64, u64)>,
        result: Box<Result<crate::latex::Reply, String>>,
    },
    Marp {
        id: u64,
        version: u64,
        token: u64,
        exporting: bool,
        result: Box<Result<marp_view::Completed, String>>,
    },
    MarpPage {
        id: u64,
        version: u64,
        token: u64,
        page: usize,
        result: Result<crate::images::Decoded, String>,
    },
    NotebookParsed {
        id: u64,
        version: u64,
        result: Result<crate::notebook::Notebook, String>,
    },
    PdfPage {
        id: u64,
        version: u64,
        token: u64,
        page: usize,
        result: Result<crate::images::Decoded, String>,
    },
    MarkdownImage {
        id: u64,
        token: u64,
        uri: String,
        result: Result<crate::images::Decoded, String>,
    },
    Tool {
        job: Box<ToolJob>,
        result: Box<Result<crate::project::Response, String>>,
    },
    Done {
        job: Job,
        result: Box<Result<Response, String>>,
    },
    Session {
        job: sessions::Job,
        result: Result<crate::project::Response, String>,
    },
    Connected {
        result: Result<(), String>,
    },
}
