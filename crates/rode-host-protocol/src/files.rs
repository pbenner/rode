//! Filesystem wire values; decoded images, notebooks and UI state never cross this boundary.
use serde::{Deserialize, Serialize};

/// Maximum text/hex file size in bytes; image/PDF transport has its own limit.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Maximum retained entries in one nonrecursive directory listing.
pub const MAX_ENTRIES: usize = 10_000;

/// Explicit explorer operations executed on the captured project host.
/// Sources name directory entries: a final symlink is operated on, not followed.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Mutation {
    /// Create one directory without replacing an existing entry.
    CreateDirectory {
        /// New directory path; its parent must exist.
        path: String,
    },
    /// Move an entry without replacing an existing destination.
    Rename {
        /// Existing entry path.
        path: String,
        /// New path on the same filesystem.
        destination: String,
    },
    /// Duplicate an entry; directory contents are bounded and symlinks retained.
    Copy {
        /// Existing entry path.
        path: String,
        /// New path, which must not exist.
        destination: String,
    },
    /// Remove an entry, including a directory's children, after explicit confirmation.
    Remove {
        /// Entry to remove; a symlink's target is preserved.
        path: String,
    },
}

/// One directory entry on the operation's host.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Entry {
    /// Host-side path to the entry.
    pub path: String,
    /// Filename used for display, without a file-type prefix.
    pub name: String,
    /// Whether following this entry resolves to a directory.
    pub dir: bool,
}

/// Transport-only file and service envelope. The dispatcher supplies its typed service
/// and preparation payloads. Paths belong to the captured host context.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request<T = serde_json::Value, S = serde_json::Value> {
    /// Perform an explicit explorer operation within the prepared filesystem policy.
    Mutate {
        /// Typed operation with literal project-host paths.
        mutation: Mutation,
    },
    /// Run a project-host service through the file channel.
    Tool {
        /// Project service request payload.
        request: T,
    },
    /// Canonicalize a project root and probe the requested sandbox policy.
    Prepare {
        /// Project directory on the chosen host.
        path: String,
        /// Requested access policy; the returned plan records actual availability.
        sandbox: S,
    },
    /// Read one directory without recursive traversal.
    List {
        /// Directory path on the chosen host.
        path: String,
    },
    /// Read bounded bytes, returning UTF-8 text or base64 binary according to content.
    ReadAuto {
        /// Source file path on the chosen host.
        path: String,
    },
    /// Read bounded bytes explicitly as hex-editor data.
    ReadHex {
        /// Source file path on the chosen host.
        path: String,
    },
    /// Atomically save binary bytes, preserving revision and permission protections.
    WriteBinary {
        /// Destination path on the chosen host.
        path: String,
        /// Standard base64-encoded replacement bytes.
        data: String,
        /// SHA-256 revision required for replacement; `None` means create without overwriting.
        expected: Option<String>,
    },
    /// Read a UTF-8 text file, rejecting NUL-containing data.
    Read {
        /// Source file path on the chosen host.
        path: String,
    },
    /// Transfer encoded raster bytes without remote decoding.
    ReadImage {
        /// Image path on the chosen host.
        path: String,
    },
    /// Transfer preview bytes, including PDFs, without remote rendering.
    ReadBinary {
        /// Preview source path on the chosen host.
        path: String,
    },
    /// Atomically save UTF-8 source using the same conflict rules as binary saves.
    Write {
        /// Destination path on the chosen host.
        path: String,
        /// Replacement source text.
        text: String,
        /// SHA-256 revision required for replacement; `None` means create without overwriting.
        expected: Option<String>,
    },
}

/// Bounded encoded preview limit, independent of desktop decoders.
pub const MAX_PREVIEW_BYTES: usize = 32 * 1024 * 1024;

/// Transport-only result of a filesystem operation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Response {
    /// Canonical operation path on this host.
    pub path: String,
    #[serde(default)]
    /// Sorted direct children of a listed directory.
    pub entries: Vec<Entry>,
    #[serde(default)]
    /// UTF-8 source snapshot.
    pub text: String,
    #[serde(default)]
    /// SHA-256 revision of the bytes read or written.
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Base64 encoded raster bytes, without desktop decoding.
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Base64 encoded binary or preview bytes.
    pub binary: Option<String>,
}
