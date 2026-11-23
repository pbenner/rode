//! Shared installation metadata and execution-scoped contributions.
//!
//! Legacy API-1 `tool` / `panel` manifests retain their serialized shape. New
//! manifests declare each contribution's host explicitly; adding a desktop panel
//! never gives it project authority or uploads it to an SSH workspace.
use crate::tool::API_VERSION;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One installed adapter command contributed to the palette.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCommand {
    /// Stable command identifier within its extension.
    pub id: String,
    /// User-visible command title.
    pub title: String,
}
/// Installed executable capabilities declared by a validated manifest.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    /// Literal executable name or absolute path; its contribution determines the host.
    pub program: String,
    /// Literal executable arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Language identifiers that activate this tool.
    #[serde(default)]
    pub languages: Vec<String>,
    /// Whether the adapter accepts buffer formatting operations.
    #[serde(default)]
    pub format: bool,
    /// Whether the adapter accepts buffer diagnostic operations.
    #[serde(default)]
    pub check: bool,
    /// Explicit commands contributed to the command palette.
    #[serde(default)]
    pub commands: Vec<ToolCommand>,
}
/// Desktop-installed metadata for an opt-in project-host adapter; repository files are not
/// automatically executed as manifests.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Required protocol version, currently [`API_VERSION`].
    pub api: u32,
    /// Stable namespaced extension identifier.
    pub id: String,
    /// User-visible extension name.
    pub name: String,
    /// Extension version label.
    pub version: String,
    /// Bounded description shown in the extension manager.
    pub description: String,
    /// Declared executable and activation capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<Tool>,
    /// Opt-in persistent desktop panel protocol. Panel providers are installed
    /// executables, never project-host tools or libraries loaded into the GUI.
    #[serde(default, skip_serializing_if = "is_false")]
    pub panel: bool,
    /// Panel wire version. Version 1 remains compatible with existing providers.
    #[serde(default = "panel_version", skip_serializing_if = "is_panel_v1")]
    pub panel_version: u32,
    /// Theme-owned icon name or absolute path to a local PNG image.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub icon: String,
    /// Explicit execution-scoped contributions. Mutually exclusive with legacy fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contributions: Vec<Contribution>,
}

/// The fixed execution boundary of a contribution, never a sandbox override.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Host {
    /// This computer with the desktop user's ordinary access.
    Desktop,
    /// The active local/SSH project host under its actual project policy.
    Project,
}
/// One executable contribution. A manifest may declare one of each kind.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Contribution {
    /// Persistent native dashboard on the desktop; never a project-host service.
    Panel {
        host: Host,
        tool: Tool,
        #[serde(default = "panel_version")]
        protocol: u32,
        #[serde(default)]
        icon: String,
    },
    /// One-shot commands, formatters and checkers on the active project host.
    ProjectTool { host: Host, tool: Tool },
}
/// Borrowed, normalized panel metadata shared by legacy and explicit manifests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Panel<'a> {
    /// The desktop executable and its literal arguments.
    pub tool: &'a Tool,
    /// Supported panel wire version, independent of the manifest API.
    pub protocol: u32,
    /// A theme-owned icon identifier or absolute PNG path; empty uses the generic icon.
    pub icon: &'a str,
}
impl Manifest {
    /// Normalize either manifest form without executing or allocating an adapter.
    pub fn panel(&self) -> Option<Panel<'_>> {
        self.contributions
            .iter()
            .find_map(|c| match c {
                Contribution::Panel {
                    tool,
                    protocol,
                    icon,
                    ..
                } => Some(Panel {
                    tool,
                    protocol: *protocol,
                    icon,
                }),
                _ => None,
            })
            .or_else(|| {
                self.panel
                    .then(|| {
                        self.tool.as_ref().map(|tool| Panel {
                            tool,
                            protocol: self.panel_version,
                            icon: &self.icon,
                        })
                    })
                    .flatten()
            })
    }
    /// Project authority belongs only to the declared project-tool contribution.
    pub fn project_tool(&self) -> Option<&Tool> {
        self.contributions
            .iter()
            .find_map(|c| match c {
                Contribution::ProjectTool { tool, .. } => Some(tool),
                _ => None,
            })
            .or_else(|| (!self.panel).then_some(self.tool.as_ref()).flatten())
    }
    /// Whether changing this extension can affect project-host work.
    pub fn project_scoped(&self) -> bool {
        self.project_tool().is_some()
    }
    /// Validate identities, literal executables, limits and execution boundaries.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.api == API_VERSION,
            "Unsupported extension API; RODE supports version 1."
        );
        ensure!(
            valid_id(&self.id) && !self.id.starts_with("rode."),
            "Use a unique publisher.name ID; rode.* is reserved."
        );
        ensure!(
            !self.name.trim().is_empty()
                && self.name.len() <= 80
                && self.version.len() <= 40
                && self.description.len() <= 1024,
            "Invalid extension description."
        );
        if self.contributions.is_empty() {
            let tool = self
                .tool
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Declare an executable contribution"))?;
            tool.validate()?;
            if self.panel {
                validate_panel(tool, self.panel_version, &self.icon)?;
            } else {
                ensure!(
                    self.panel_version == 1 && self.icon.is_empty(),
                    "Panel metadata requires a panel contribution"
                );
                validate_project(tool)?;
            }
        } else {
            ensure!(
                self.tool.is_none()
                    && !self.panel
                    && self.panel_version == 1
                    && self.icon.is_empty(),
                "Do not mix contributions with legacy tool/panel fields"
            );
            ensure!(
                self.contributions.len() <= 2,
                "At most one panel and one project tool may be declared"
            );
            let (mut panels, mut tools) = (0, 0);
            for contribution in &self.contributions {
                match contribution {
                    Contribution::Panel {
                        host,
                        tool,
                        protocol,
                        icon,
                    } => {
                        ensure!(*host == Host::Desktop, "Panels must run on the desktop");
                        panels += 1;
                        tool.validate()?;
                        validate_panel(tool, *protocol, icon)?;
                    }
                    Contribution::ProjectTool { host, tool } => {
                        ensure!(
                            *host == Host::Project,
                            "Project tools must run on the project host"
                        );
                        tools += 1;
                        tool.validate()?;
                        validate_project(tool)?;
                    }
                }
            }
            ensure!(panels <= 1 && tools <= 1, "Duplicate contribution kind");
        }
        Ok(())
    }
}
impl Tool {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.program.is_empty()
                && self.program.len() <= 4096
                && !self.program.contains('\0')
                && (!self.program.contains('/') || Path::new(&self.program).is_absolute()),
            "The program must be an absolute host path or a name on its PATH."
        );
        ensure!(
            self.args.len() <= 64
                && self.args.iter().map(String::len).sum::<usize>() <= 16 * 1024
                && self.args.iter().all(|s| !s.contains('\0')),
            "Invalid adapter arguments."
        );
        ensure!(
            self.languages.len() <= 64
                && self
                    .languages
                    .iter()
                    .all(|s| !s.is_empty() && s.len() <= 40),
            "Invalid language list."
        );
        ensure!(
            self.commands.len() <= 32
                && self
                    .commands
                    .iter()
                    .all(|c| valid_id(&c.id) && !c.title.trim().is_empty() && c.title.len() <= 120),
            "Invalid commands."
        );
        let unique: std::collections::BTreeSet<_> = self.commands.iter().map(|c| &c.id).collect();
        ensure!(
            unique.len() == self.commands.len(),
            "Duplicate command IDs."
        );
        Ok(())
    }
}
fn validate_panel(tool: &Tool, protocol: u32, icon: &str) -> Result<()> {
    // Persistent transport's 64-item argv budget includes the executable. Legacy
    // one-shot project tools retain their existing 64-argument allowance.
    ensure!(
        tool.args.len() < 64,
        "Panel executables accept at most 63 arguments"
    );
    ensure!(matches!(protocol, 1..=3), "Unsupported panel protocol");
    ensure!(
        ["", "chart", "monitor", "folder"].contains(&icon)
            || (icon.len() <= 4096
                && !icon.chars().any(char::is_control)
                && Path::new(icon).is_absolute()
                && Path::new(icon)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))),
        "Icon must be chart, monitor, folder, or an absolute path to a PNG file"
    );
    ensure!(
        !tool.format && !tool.check && tool.languages.is_empty() && tool.commands.is_empty(),
        "Declare project-host capabilities in a separate project-tool contribution"
    );
    Ok(())
}
fn validate_project(tool: &Tool) -> Result<()> {
    ensure!(
        tool.format || tool.check || !tool.commands.is_empty(),
        "Declare a formatter, checker or command"
    );
    Ok(())
}
fn panel_version() -> u32 {
    1
}
fn is_panel_v1(value: &u32) -> bool {
    *value == 1
}
fn is_false(value: &bool) -> bool {
    !*value
}
/// Validate a stable namespaced extension or command identifier.
pub fn valid_id(id: &str) -> bool {
    (3..=80).contains(&id.len())
        && id.contains('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_manifests_keep_their_wire_shape_and_host_boundary() {
        for panel in [false, true] {
            let mut value = json!({"api":1,"id":"test.legacy","name":"Legacy","version":"1","description":"Fixture","tool":{"program":"test-provider","args":[],"languages":[],"format":false,"check":false,"commands":[]}});
            if panel {
                value["panel"] = true.into();
            } else {
                value["tool"]["check"] = true.into();
            }
            let manifest: Manifest = serde_json::from_value(value.clone()).unwrap();
            manifest.validate().unwrap();
            assert_eq!(manifest.panel().is_some(), panel);
            assert_eq!(manifest.project_tool().is_some(), !panel);
            assert_eq!(serde_json::to_value(&manifest).unwrap(), value);
        }
    }
    #[test]
    fn custom_icons_accept_local_png_paths_in_both_manifest_forms() {
        let path = std::env::temp_dir()
            .join("extension icon.png")
            .to_string_lossy()
            .into_owned();
        for icon in ["", "chart", "monitor", "folder", &path] {
            let mut value = combined();
            value["contributions"][0]["icon"] = icon.into();
            serde_json::from_value::<Manifest>(value)
                .unwrap()
                .validate()
                .unwrap();
            let legacy = json!({"api":1,"id":"test.icon","name":"Icon","version":"1","description":"Fixture",
                "panel":true,"icon":icon,"tool":{"program":"provider"}});
            serde_json::from_value::<Manifest>(legacy)
                .unwrap()
                .validate()
                .unwrap();
        }
        for icon in [
            "unknown",
            "relative.png",
            "https://example.com/icon.png",
            "file:///tmp/icon.png",
            "/tmp/icon.svg",
            "/tmp/icon.png\n",
        ] {
            let mut value = combined();
            value["contributions"][0]["icon"] = icon.into();
            assert!(
                serde_json::from_value::<Manifest>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }
    fn combined() -> serde_json::Value {
        json!({"api":1,"id":"test.combined","name":"Combined","version":"1","description":"Fixture","contributions":[
            {"kind":"panel","host":"desktop","protocol":2,"tool":{"program":"local-dashboard"}},
            {"kind":"project-tool","host":"project","tool":{"program":"project-checker","languages":["rust"],"check":true,"commands":[{"id":"test.inspect","title":"Inspect"}]}}
        ]})
    }
    #[test]
    fn combined_extensions_validate_each_host_and_normalize_both_contributions() {
        let manifest: Manifest = serde_json::from_value(combined()).unwrap();
        manifest.validate().unwrap();
        assert_eq!(manifest.panel().unwrap().tool.program, "local-dashboard");
        assert_eq!(manifest.project_tool().unwrap().program, "project-checker");
        assert!(manifest.project_scoped());
        for index in [0, 1] {
            let mut bad = combined();
            bad["contributions"][index]["host"] =
                if index == 0 { "project" } else { "desktop" }.into();
            assert!(
                serde_json::from_value::<Manifest>(bad)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let mut bad = combined();
        bad["contributions"][1] = bad["contributions"][0].clone();
        assert!(
            serde_json::from_value::<Manifest>(bad)
                .unwrap()
                .validate()
                .is_err()
        );
        let mut bad = combined();
        bad["tool"] = json!({"program":"ambiguous"});
        assert!(
            serde_json::from_value::<Manifest>(bad)
                .unwrap()
                .validate()
                .is_err()
        );
    }
}
