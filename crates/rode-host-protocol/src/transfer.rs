//! Bounded file-transfer messages, independent of document buffers and JSON snapshots.
use serde::{Deserialize, Serialize};

/// Maximum decoded bytes in one transfer message.
pub const BLOCK: usize = 256 * 1024;

/// One entry without a repeated parent path. Symlinks are reported but never followed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    /// Literal leaf name.
    pub name: String,
    /// Directory entry, excluding symbolic links.
    pub directory: bool,
    /// Symbolic links and special files cannot be transferred.
    pub regular: bool,
    /// File length, or zero for directories.
    pub size: u64,
    /// Preserve the owner's execute bit when creating a new private file.
    pub executable: bool,
    /// Opaque inode/change identity used for optimistic conflict detection.
    pub version: String,
}

/// Requests use a dedicated, policy-scoped channel and at most one open read/write pair.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    /// Renew a stream while a user resolves a conflict.
    Ping,
    /// Read one bounded directory listing.
    List {
        /// Directory on this endpoint.
        path: String,
    },
    /// Inspect a leaf without following a symlink; absence is a successful result.
    Stat {
        /// Entry on this endpoint.
        path: String,
    },
    /// Create a directory, accepting an existing ordinary directory.
    Directory {
        /// New directory's literal path.
        path: String,
    },
    /// Pin a regular source inode after checking its listing identity.
    OpenRead {
        /// Regular source file.
        path: String,
        /// Identity returned by Stat.
        version: String,
    },
    /// Read the next block from the pinned source.
    Read,
    /// Stage a neighboring private file; None requires an absent destination.
    OpenWrite {
        /// Final destination, never the staging name.
        path: String,
        /// Approved identity to replace; None forbids replacement.
        expected: Option<String>,
    },
    /// Append one standard-base64 block to the stage.
    Write {
        /// At most BLOCK decoded bytes.
        data: String,
    },
    /// Publish only a complete, verified stream and unchanged destination.
    Commit {
        /// Total number of source bytes.
        size: u64,
        /// Source's complete SHA-256.
        digest: String,
        /// New files retain the source owner's execute bit.
        executable: bool,
    },
    /// Close handles and remove an unpublished stage.
    Abort,
}

/// Replies stay bounded; an empty data block ends a source stream and returns its digest.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    /// Canonical directory or destination.
    #[serde(default)]
    pub path: String,
    /// Immediate children for List.
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// Leaf metadata for Stat; None means absent.
    #[serde(default)]
    pub entry: Option<Entry>,
    /// One encoded block for Read.
    #[serde(default)]
    pub data: String,
    /// Stream SHA-256, only present at EOF or after Commit.
    #[serde(default)]
    pub digest: String,
}
