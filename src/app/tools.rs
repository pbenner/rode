//! Stable tool identities and shared presentation metadata.
//!
//! Menus, command search, feature gates, project navigation and detached windows
//! consume these definitions. Their controllers and native window mechanics stay
//! in their respective domain modules.
use super::{Command, Icon, areas::Scope, commands::Definition, navigation};
use crate::extensions::Feature;
use eframe::egui;

/// Tool identity independent of its docked or detached presentation owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(in crate::app) enum Panel {
    Transfers,
    SharedFolders,
    Git,
    Monitor,
    Extension,
    Problems,
    Tasks,
    Settings,
    Agent,
    Debug,
}

/// Context-specific text shares one definition without forcing tab labels to
/// use the longer menu names. Scope and feature enablement have one source.
pub(in crate::app) struct PanelDefinition {
    pub tab_label: &'static str,
    pub title: &'static str,
    pub command: Definition,
    pub scope: Scope,
    pub feature: Option<Feature>,
}

impl Panel {
    pub fn definition(self) -> PanelDefinition {
        match self {
            Self::Transfers => PanelDefinition {
                tab_label: "Transfers",
                title: "FILE TRANSFERS",
                command: Definition {
                    label: "File transfers",
                    palette_label: "Remote: File transfers",
                    icon: Icon::Files,
                },
                scope: Scope::Host,
                feature: Some(Feature::Transfers),
            },
            Self::SharedFolders => PanelDefinition {
                tab_label: "Shared folders",
                title: "SHARED FOLDERS",
                command: Definition {
                    label: "Shared folders",
                    palette_label: "Project: Shared folders",
                    icon: Icon::Folder,
                },
                scope: Scope::Project,
                feature: Some(Feature::Transfers),
            },
            Self::Git => PanelDefinition {
                tab_label: "Git",
                title: "SOURCE CONTROL",
                command: Definition {
                    label: "Source control",
                    palette_label: "Git: Source control",
                    icon: Icon::Git,
                },
                scope: Scope::Project,
                feature: Some(Feature::Git),
            },
            Self::Monitor => PanelDefinition {
                tab_label: "Monitor",
                title: "SYSTEM MONITOR",
                command: Definition {
                    label: "System monitor",
                    palette_label: "System: Monitor",
                    icon: Icon::Monitor,
                },
                scope: Scope::Host,
                feature: Some(Feature::Monitor),
            },
            Self::Extension => PanelDefinition {
                tab_label: "Extension",
                title: "EXTENSION PANEL",
                command: Definition {
                    label: "Extension panel",
                    palette_label: "Extensions: Open panel",
                    icon: Icon::Extensions,
                },
                scope: Scope::Application,
                feature: None,
            },
            Self::Problems => PanelDefinition {
                tab_label: "Problems",
                title: "PROBLEMS",
                command: Definition {
                    label: "Problems",
                    palette_label: "Code: Problems",
                    icon: Icon::Problems,
                },
                scope: Scope::Project,
                feature: None,
            },
            Self::Tasks => PanelDefinition {
                tab_label: "Tasks",
                title: "TASKS",
                command: Definition {
                    label: "Tasks",
                    palette_label: "Tasks: Run, build and test",
                    icon: Icon::Play,
                },
                scope: Scope::Project,
                feature: None,
            },
            Self::Settings => PanelDefinition {
                tab_label: "Settings",
                title: "PROJECT SETTINGS",
                command: Definition {
                    label: "Project settings",
                    palette_label: "Project: Settings and sandbox access",
                    icon: Icon::Settings,
                },
                scope: Scope::Project,
                feature: None,
            },
            Self::Agent => PanelDefinition {
                tab_label: "Chat",
                title: "AGENT CHAT",
                command: Definition {
                    label: "Agent chat",
                    palette_label: "Agent: Chat",
                    icon: Icon::Chat,
                },
                scope: Scope::Project,
                feature: Some(Feature::Agents),
            },
            Self::Debug => PanelDefinition {
                tab_label: "Debugger",
                title: "DEBUGGER",
                command: Definition {
                    label: "Debugger",
                    palette_label: "Debug: Show debugger",
                    icon: Icon::Debug,
                },
                scope: Scope::Project,
                feature: Some(Feature::Debugger),
            },
        }
    }
    pub fn scope(self) -> Scope {
        self.definition().scope
    }
    pub fn title(self) -> &'static str {
        self.definition().title
    }
    pub fn feature(self) -> Option<Feature> {
        self.definition().feature
    }
}

/// Detachable view identity; selecting a view never creates a second controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(in crate::app) enum ToolTab {
    Terminal,
    Panel(Panel),
    Connections,
    AppSettings,
    Extension(u64),
}

impl ToolTab {
    pub(in crate::app) const ALL: [Self; 12] = [
        Self::Panel(Panel::Agent),
        Self::Terminal,
        Self::Panel(Panel::Git),
        Self::Panel(Panel::Monitor),
        Self::Panel(Panel::Problems),
        Self::Panel(Panel::Tasks),
        Self::Panel(Panel::SharedFolders),
        Self::Panel(Panel::Debug),
        Self::Panel(Panel::Settings),
        Self::Panel(Panel::Transfers),
        Self::Connections,
        Self::AppSettings,
    ];

    pub(in crate::app) fn command(self) -> Command {
        match self {
            Self::Terminal => Command::Terminal,
            Self::Panel(panel) => Command::Panel(panel),
            Self::Connections => Command::Connections,
            Self::AppSettings => Command::AppSettings,
            Self::Extension(_) => Command::Panel(Panel::Extension),
        }
    }

    pub(in crate::app) fn for_command(command: Command) -> Option<Self> {
        match command {
            Command::Terminal => Some(Self::Terminal),
            Command::Panel(panel) => Some(Self::Panel(panel)),
            Command::Connections => Some(Self::Connections),
            Command::AppSettings => Some(Self::AppSettings),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Connections | Self::AppSettings => self.command().definition().label,
            Self::Terminal => "Terminal",
            Self::Extension(_) => "Extension",
            Self::Panel(panel) => panel.definition().tab_label,
        }
    }
    pub fn scope(self) -> Scope {
        match self {
            Self::Terminal => Scope::Project,
            Self::Panel(panel) => panel.scope(),
            Self::Connections | Self::AppSettings | Self::Extension(_) => Scope::Application,
        }
    }
    /// Ordered project navigation and fallback share the same tool inventory.
    pub fn project() -> impl Iterator<Item = Self> {
        Self::ALL
            .into_iter()
            .filter(|tab| tab.scope() == Scope::Project)
    }
    pub fn pane(self) -> navigation::Pane {
        match self {
            Self::Terminal => navigation::Pane::Terminal,
            Self::Connections => navigation::Pane::Connections,
            Self::Panel(_) | Self::AppSettings | Self::Extension(_) => navigation::Pane::Tools,
        }
    }
    pub fn viewport(self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("rode-tool-tab", self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn tool_inventory_keeps_independent_native_window_owners() {
        let identities: HashSet<_> = ToolTab::ALL.into_iter().collect();
        let viewports: HashSet<_> = ToolTab::ALL.into_iter().map(ToolTab::viewport).collect();
        assert_eq!(
            identities.len(),
            ToolTab::ALL.len(),
            "A tool must appear only once"
        );
        assert_eq!(
            viewports.len(),
            ToolTab::ALL.len(),
            "Independent tools must not share a native viewport"
        );
    }
}
