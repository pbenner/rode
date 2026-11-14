//! Bounded project-location metadata; loading it never connects or starts a service.
use super::*;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(super) const MAX_PROJECTS: usize = 128;
const MAX_BYTES: usize = 192 * 1024;

/// Saved project location and requested policy, independent of live connection/session ownership.
/// SSH entries retain profile metadata rather than credentials or sockets.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Project {
    Local {
        root: String,
        sandbox: sandbox::Options,
    },
    Ssh {
        profile: Box<Profile>,
    },
}

/// Display name plus a bounded, validated project location. The map key is derived from location
/// identity, not this label.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Bookmark {
    pub name: String,
    pub project: Project,
}

/// Authentication configuration is metadata: no key contents, tokens or live sockets.
fn connection(profile: &Profile) -> serde_json::Value {
    serde_json::json!([
        profile.backend,
        profile.host,
        profile.user,
        profile.port,
        profile.identity,
        profile.config,
        profile.jump,
        profile.tunnel,
        profile.remote_helper,
        profile.x11_forwarding,
        profile.x11_trust
    ])
}

impl Project {
    /// Project path on its selected host; remote paths are not desktop filesystem paths.
    pub fn root(&self) -> &str {
        match self {
            Self::Local { root, .. } => root,
            Self::Ssh { profile } => &profile.root,
        }
    }
    /// Human-readable local/SSH host label without opening a connection.
    pub fn host(&self) -> String {
        match self {
            Self::Local { .. } => "Local".into(),
            Self::Ssh { profile } => format!("SSH · {}", profile.endpoint()),
        }
    }
    /// Display labels and requested sandbox settings do not create duplicate locations.
    pub fn id(&self) -> String {
        let identity = match self {
            Self::Local { root, .. } => serde_json::json!(["local", root]),
            Self::Ssh { profile } => serde_json::json!(["ssh", connection(profile), profile.root]),
        };
        format!("{:x}", Sha256::digest(identity.to_string().as_bytes()))
    }
    /// Compare the authentication/transport identity relevant to reopening a saved host,
    /// independent of label, root and sandbox draft.
    pub fn same_connection(a: &Profile, b: &Profile) -> bool {
        connection(a) == connection(b)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.root().is_empty() && self.root().len() <= 4096 && !self.root().contains('\0'),
            "Invalid bookmarked project path"
        );
        match self {
            Self::Local { root, sandbox } => {
                ensure!(
                    std::path::Path::new(root).is_absolute(),
                    "Local bookmarks need an absolute project path"
                );
                sandbox.validate()?;
            }
            Self::Ssh { profile } => {
                profile.validate()?;
                profile.sandbox.validate()?;
            }
        }
        Ok(())
    }
}

/// Validate a nonblank bookmark label bounded to 120 UTF-8 bytes without control characters.
pub(super) fn validate_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty() && name.len() <= 120 && !name.chars().any(char::is_control),
        "Use a project name of 1–120 bytes without control characters"
    );
    Ok(())
}

/// Decode bounded saved locations and validate every label, project policy and derived map key.
/// Missing settings produce an empty catalogue; malformed data returns an error.
pub(super) fn decode(raw: Option<&serde_json::Value>) -> Result<BTreeMap<String, Bookmark>> {
    let Some(raw) = raw else {
        return Ok(BTreeMap::new());
    };
    ensure!(
        raw.to_string().len() <= MAX_BYTES,
        "Project bookmarks exceed 192 KiB"
    );
    let projects: BTreeMap<String, Bookmark> =
        serde_json::from_value(raw.clone()).context("Invalid project bookmarks")?;
    ensure!(
        projects.len() <= MAX_PROJECTS,
        "At most 128 project bookmarks are supported"
    );
    for (id, bookmark) in &projects {
        validate_name(&bookmark.name)?;
        bookmark.project.validate()?;
        ensure!(
            *id == bookmark.project.id(),
            "Bookmark identity does not match its project location"
        );
    }
    Ok(projects)
}

/// Window-owned decoded bookmark cache and rename draft. It tracks stored raw data so unchanged
/// frames perform no repeated parsing or connection work.
#[derive(Default)]
pub(in crate::app) struct State {
    loaded: bool,
    raw: Option<serde_json::Value>,
    pub(super) projects: BTreeMap<String, Bookmark>,
    pub(super) error: Option<String>,
    pub(super) rename: Option<(String, String)>,
    pub(super) rename_focus: bool,
}
impl State {
    /// Reload only changed raw settings, clearing invalid catalogue data and obsolete rename drafts
    /// while retaining a visible decoding error.
    pub(super) fn sync(&mut self, raw: Option<&serde_json::Value>) {
        if self.loaded && self.raw.as_ref() == raw {
            return;
        }
        self.loaded = true;
        self.raw = raw.cloned();
        match decode(raw) {
            Ok(projects) => {
                self.projects = projects;
                self.error = None;
            }
            Err(error) => {
                self.projects.clear();
                self.error = Some(format!("{error:#}"));
            }
        }
        if self
            .rename
            .as_ref()
            .is_some_and(|(id, _)| !self.projects.contains_key(id))
        {
            self.rename = None;
        }
    }
}
