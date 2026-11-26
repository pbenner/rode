//! Versioned workspace descriptions. Paths and service IDs are restoration
//! intents; the normal host, sandbox and revision checks grant access later.
use super::super::*;
use crate::app::crash_recovery::Entry;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub(super) const FORMAT: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(in crate::app) enum Location {
    Project {
        project: project_bookmarks::Project,
    },
    Encrypted {
        cipherdir: String,
        remote: Option<Box<Profile>>,
        sandbox: sandbox::Options,
    },
}
impl Location {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Project { project } => project.validate(),
            Self::Encrypted {
                cipherdir,
                remote,
                sandbox,
            } => {
                ensure!(
                    !cipherdir.is_empty() && cipherdir.len() <= 4096 && !cipherdir.contains('\0'),
                    "Invalid encrypted workspace path"
                );
                if let Some(profile) = remote {
                    profile.validate()?;
                }
                sandbox.validate()
            }
        }
    }
    pub fn encrypted(&self) -> bool {
        matches!(self, Self::Encrypted { .. })
    }
    pub fn path(&self) -> &str {
        match self {
            Self::Project { project } => project.root(),
            Self::Encrypted { cipherdir, .. } => cipherdir,
        }
    }
    pub fn host(&self) -> String {
        match self {
            Self::Project { project } => project.host(),
            Self::Encrypted { remote, .. } => remote
                .as_ref()
                .map_or_else(|| "Local".into(), |p| p.endpoint()),
        }
    }
    pub fn remote(&self) -> bool {
        match self {
            Self::Project { project } => matches!(project, project_bookmarks::Project::Ssh { .. }),
            Self::Encrypted { remote, .. } => remote.is_some(),
        }
    }
    pub fn identity(&self) -> String {
        serde_json::to_string(self).expect("workspace locations serialize")
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::app) struct Document {
    pub key: u64,
    pub path: Option<String>,
    pub name: String,
    pub relative: bool,
    pub revision: Option<String>,
    pub cursor: (usize, usize),
    pub group: u64,
    pub selected: bool,
    pub private: bool,
    pub notebook_source: bool,
    pub draft: Option<Entry>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub(in crate::app) struct Layout {
    pub sidebar: Option<sidebar::View>,
    pub panel: Option<Panel>,
    pub terminal: bool,
    pub tools: bool,
    pub detached: bool,
    pub tools_hidden: bool,
    pub last_tool: Option<ToolTab>,
    pub floating: Vec<ToolTab>,
    pub floating_extensions: Vec<String>,
    pub explorer_width: Option<f32>,
    pub connections_width: Option<f32>,
    pub window_size: Option<[f32; 2]>,
    pub editor_windows: Vec<EditorWindow>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::app) struct EditorWindow {
    pub group: u64,
    pub size: [f32; 2],
}

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::app) struct Agent {
    pub id: String,
    pub policy: String,
    pub controller: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(in crate::app) struct Runtime {
    pub id: String,
    pub document: Option<u64>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(in crate::app) struct Snapshot {
    pub location: Location,
    pub policy: String,
    pub documents: Vec<Document>,
    pub layout: Layout,
    pub draft: String,
    pub agent: Option<Agent>,
    pub terminals: Vec<String>,
    pub selected_terminal: Option<String>,
    pub runtimes: Vec<Runtime>,
}
impl Snapshot {
    pub fn validate(&self) -> Result<()> {
        self.location.validate()?;
        ensure!(
            self.documents.len() <= 256
                && self.terminals.len() <= 16
                && self.runtimes.len() <= 64
                && self.layout.floating.len() <= 32
                && self.layout.editor_windows.len() <= 32
                && self.layout.floating_extensions.len() <= 32,
            "Workspace session exceeds its item limit"
        );
        ensure!(
            self.policy.len() == 64 && self.policy.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid workspace policy fingerprint"
        );
        ensure!(
            self.terminals.iter().all(|id| valid_id(id))
                && self.runtimes.iter().all(|r| valid_id(&r.id))
                && self.agent.as_ref().is_none_or(|a| valid_id(&a.id)
                    && a.policy == self.policy
                    && a.controller.as_ref().is_none_or(|id| valid_id(id)))
                && self
                    .selected_terminal
                    .as_ref()
                    .is_none_or(|id| self.terminals.contains(id)),
            "Invalid workspace service reference"
        );
        ensure!(
            self.layout
                .floating
                .iter()
                .all(|tab| !matches!(tab, ToolTab::Extension(_)))
                && self
                    .layout
                    .floating_extensions
                    .iter()
                    .all(|id| !id.is_empty()
                        && id.len() <= 256
                        && !id.chars().any(char::is_control)),
            "Invalid saved tool identity"
        );
        let mut keys = HashSet::new();
        for document in &self.documents {
            ensure!(
                keys.insert(document.key),
                "Duplicate workspace document identity"
            );
            ensure!(
                document.name.len() <= 4096
                    && document
                        .path
                        .as_ref()
                        .is_none_or(|p| p.len() <= 4096 && !p.contains('\0')),
                "Invalid workspace file path"
            );
            if document.relative
                && let Some(path) = &document.path
            {
                ensure!(
                    std::path::Path::new(path)
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_))),
                    "Invalid relative encrypted file path"
                );
            }
            ensure!(
                !(document.private || self.location.encrypted()) || document.draft.is_none(),
                "Encrypted buffers cannot enter workspace storage"
            );
        }
        ensure!(
            !self.location.encrypted() || self.draft.is_empty(),
            "Encrypted prompt drafts cannot enter workspace storage"
        );
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Key {
    pub id: String,
    pub generation: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Owner {
    pub generation: String,
    pub desktop: rode_agent_core::service::ViewOwner,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Record {
    pub format: u32,
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub updated: u64,
    pub owner: Owner,
    pub snapshot: Snapshot,
}
impl Record {
    pub fn key(&self) -> Key {
        Key {
            id: self.id.clone(),
            generation: self.owner.generation.clone(),
        }
    }
}

/// Catalogue decoding skips buffers; opening Welcome never restores files or contacts SSH.
#[derive(Clone, Deserialize)]
pub(super) struct Summary {
    pub format: u32,
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub updated: u64,
    pub owner: Owner,
    pub snapshot: LocationOnly,
}
#[derive(Clone, Deserialize)]
pub(super) struct LocationOnly {
    pub location: Location,
}
impl Summary {
    pub fn key(&self) -> Key {
        Key {
            id: self.id.clone(),
            generation: self.owner.generation.clone(),
        }
    }
}
#[derive(Clone)]
pub(super) struct Row {
    pub summary: Summary,
    pub active: bool,
}

pub(super) fn valid_id(id: &str) -> bool {
    id.len() == 34 && id.starts_with("s-") && id[2..].bytes().all(|b| b.is_ascii_hexdigit())
}
pub(super) fn valid_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty() && name.len() <= 120 && !name.chars().any(char::is_control),
        "Use a session name of 1–120 bytes without control characters"
    );
    Ok(())
}
pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
