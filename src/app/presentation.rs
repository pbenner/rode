//! Pure ownership of the root area, sidebar and tool-window selection.
//!
//! Readers see an immutable projection. Transitions are the only writers, so a
//! selected tool always has content and a detached tab has exactly one owner.
//! Native focus, refreshes and running services stay in the application adapters.
use super::{
    Panel, ToolTab,
    areas::{Area, Scope},
    sidebar::View,
    tool_windows::Surface,
};

/// Read-only presentation projection; it owns no project or process state.
#[derive(Default)]
pub(super) struct Selection {
    /// Central root destination; visiting a global area retains project tools.
    pub area: Area,
    /// Explorer and Connections share one mutually exclusive sidebar slot.
    pub sidebar: Option<View>,
    /// Remembered project panel, including while the editor or terminal is shown.
    pub panel: Option<Panel>,
    /// Selected content of the combined project-tool host.
    pub surface: Surface,
    /// The combined host occupies its own native viewport.
    pub detached: bool,
    /// Presentation visibility only; hiding never cancels running work.
    pub tools_hidden: bool,
    /// Each tab has at most one independently detached presentation owner.
    pub floating_tabs: Vec<ToolTab>,
    /// Deliberately selected project destinations; automatic login/shell setup is excluded.
    pub last_tool: Option<ToolTab>,
}

/// The single writer for root navigation and tool placement. Deliberately has
/// no `DerefMut`: renderers cannot create incomplete tool selections.
#[derive(Default)]
pub(super) struct Controller(Selection);

impl std::ops::Deref for Controller {
    type Target = Selection;
    fn deref(&self) -> &Selection {
        &self.0
    }
}

impl Controller {
    /// Visiting an area preserves the selected project tool and sidebar.
    pub fn enter_area(&mut self, area: Area) {
        self.0.area = area;
    }

    pub fn show_connections(&mut self) {
        self.0.sidebar = Some(View::Connections);
    }
    pub fn hide_sidebar(&mut self) {
        self.0.sidebar = None;
    }
    pub fn explorer(&mut self, visible: bool) {
        if visible {
            self.0.area = Area::Projects;
            self.0.sidebar = Some(View::Explorer);
        } else if self.0.sidebar == Some(View::Explorer) {
            self.hide_sidebar();
        }
    }

    pub fn selected_tool(&self) -> Option<ToolTab> {
        match self.surface {
            Surface::Editor => None,
            Surface::Terminal => Some(ToolTab::Terminal),
            Surface::Tools => self.panel.map(ToolTab::Panel),
        }
    }

    /// Only project tools participate in the Project tools restoration history.
    pub fn remember(&mut self, tab: ToolTab) {
        if tab.scope() == Scope::Project {
            self.0.last_tool = Some(tab);
        }
    }
    pub fn select_editor(&mut self) {
        self.0.surface = Surface::Editor;
    }
    pub fn select_terminal(&mut self) {
        self.remember(ToolTab::Terminal);
        self.0.area = Area::Projects;
        if !self.floating_tabs.contains(&ToolTab::Terminal) {
            self.0.surface = Surface::Terminal;
            self.show_tools();
        }
    }
    /// A project panel and its surface are selected atomically.
    pub fn select_panel(&mut self, panel: Panel) {
        debug_assert_eq!(panel.scope(), Scope::Project);
        self.0.panel = Some(panel);
        self.0.surface = Surface::Tools;
        self.remember(ToolTab::Panel(panel));
        self.show_tools();
    }
    /// Removing a panel cannot leave an empty Tools surface selected.
    pub fn clear_panel(&mut self) {
        self.0.panel = None;
        if self.surface == Surface::Tools {
            self.0.surface = Surface::Editor;
        }
    }
    pub fn hide_tools(&mut self) {
        self.0.tools_hidden = true;
    }
    pub fn show_tools(&mut self) {
        self.0.tools_hidden = false;
    }
    pub fn detach_tools(&mut self) {
        self.0.detached = true;
    }
    pub fn dock_tools(&mut self) {
        self.0.detached = false;
        self.show_tools();
    }

    /// Publish the new owner before terminal startup or any focus side effect.
    /// Duplicate detach requests are inert and never duplicate a controller.
    pub fn detach_tab(&mut self, tab: ToolTab) {
        if self.floating_tabs.contains(&tab) {
            return;
        }
        self.0.floating_tabs.push(tab);
        if self.selected_tool() == Some(tab) {
            self.select_editor();
        }
        if self.area.tab() == Some(tab) {
            self.0.area = Area::Projects;
        }
    }
    /// Releasing a host does not select or restart its underlying service.
    pub fn reattach_tab(&mut self, tab: ToolTab) {
        self.0.floating_tabs.retain(|candidate| *candidate != tab);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_preserves_tools_and_sidebar_has_one_selection() {
        let mut state = Controller::default();
        assert_eq!(state.area, Area::Welcome);
        assert_eq!(state.sidebar, None);
        state.select_panel(Panel::Agent);
        state.explorer(true);
        state.show_connections();
        assert_eq!(state.sidebar, Some(View::Connections));
        state.enter_area(Area::Monitor);
        state.enter_area(Area::Projects);
        assert_eq!(state.selected_tool(), Some(ToolTab::Panel(Panel::Agent)));
        state.select_editor();
        assert_eq!(state.last_tool, Some(ToolTab::Panel(Panel::Agent)));
        state.clear_panel();
        assert_eq!(state.surface, Surface::Editor);
    }
    #[test]
    fn only_explicit_tool_selection_updates_restoration_history() {
        let mut state = Controller::default();
        state.enter_area(Area::Welcome);
        assert!(state.last_tool.is_none());
        state.select_panel(Panel::Settings);
        state.enter_area(Area::Monitor);
        assert_eq!(state.last_tool, Some(ToolTab::Panel(Panel::Settings)));
        state.select_terminal();
        state.select_editor();
        assert_eq!(state.last_tool, Some(ToolTab::Terminal));
    }
    #[test]
    fn placement_is_unique_and_hiding_never_discards_selection() {
        let mut state = Controller::default();
        let chat = ToolTab::Panel(Panel::Agent);
        state.select_panel(Panel::Agent);
        state.detach_tools();
        state.hide_tools();
        state.show_tools();
        assert_eq!(state.selected_tool(), Some(chat));
        state.detach_tab(chat);
        state.detach_tab(chat);
        assert_eq!(state.floating_tabs, vec![chat]);
        assert_eq!(state.surface, Surface::Editor);
        state.select_terminal();
        state.reattach_tab(chat);
        assert_eq!(state.selected_tool(), Some(ToolTab::Terminal));
        state.dock_tools();
        assert!(!state.detached);
        assert!(!state.tools_hidden);
        state.enter_area(Area::Monitor);
        state.detach_tab(ToolTab::Panel(Panel::Monitor));
        assert_eq!(state.area, Area::Projects);
    }
}
