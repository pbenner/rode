//! Root sidebar selection is independent of the central application area. One
//! selection prevents Explorer and Connections from competing for the same edge.
use super::*;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    Explorer,
    Connections,
}

#[derive(Default)]
pub(super) struct State {
    /// Requested width, consumed by the next unzoomed layout of this native host.
    pub width: Option<f32>,
}

const DEFAULT_WIDTH: f32 = 250.;
const MIN_WIDTH: f32 = 180.;

/// Content-independent width/resize identity for one native host. Keep Explorer's
/// stored IDs so existing widths survive; zoom has its own temporary geometry.
fn panel_id(viewport: egui::ViewportId, zoom: bool) -> egui::Id {
    if viewport == egui::ViewportId::ROOT {
        egui::Id::new(if zoom { "explorer-zoom" } else { "explorer" })
    } else {
        egui::Id::new(("explorer", viewport, zoom))
    }
}

/// Sidebar libraries share a native, keyboard-discoverable disclosure without
/// nested-content indentation or vertical guide lines. Scope this treatment to
/// sidebar sections rather than changing tree and form styling globally.
pub(super) fn section<R>(
    ui: &mut egui::Ui,
    label: &str,
    id: impl std::hash::Hash,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::CollapsingResponse<R> {
    let response = keyboard::CollapsingHeader::new(label)
        .id_salt(id)
        .default_open(true)
        .show_unindented(ui, add);
    ui_probe::button(
        ui,
        &format!("connections-section:{label}"),
        &response.header_response,
    );
    response
}

impl EditorApp {
    fn sidebar_state_mut(&mut self, viewport: egui::ViewportId) -> Option<&mut State> {
        self.editor_view_mut(viewport).map(|view| &mut view.sidebar)
    }

    /// Mouse and keyboard resizing use the same panel state for both components.
    /// Accumulate commands delivered before layout rather than losing earlier steps.
    pub(super) fn resize_sidebar(&mut self, ctx: &egui::Context, delta: f32) {
        let viewport = ctx.viewport_id();
        let stored = egui::containers::panel::PanelState::load(ctx, panel_id(viewport, false))
            .map_or(DEFAULT_WIDTH, |state| state.rect.width());
        if let Some(state) = self.sidebar_state_mut(viewport) {
            state.width = Some(state.width.unwrap_or(stored) + delta);
        }
    }

    pub(super) fn connections_visible(&self) -> bool {
        self.presentation.sidebar == Some(View::Connections)
            && !self.tab_detached(ToolTab::Connections)
    }

    /// Reveal the sole existing owner without replacing the current central
    /// view, reconnecting a host, or starting a project service.
    pub(super) fn show_connections(&mut self, ctx: &egui::Context) {
        let tab = ToolTab::Connections;
        if self.tab_detached(tab) {
            self.focus_tool_tab(tab, ctx);
        } else {
            if let Some(editor) = self.editor_view_mut(egui::ViewportId::ROOT) {
                editor.tabs.cancel_focus();
            }
            self.agents.focus = false;
            ctx.data_mut(|data| data.insert_temp(egui::Id::new("terminal-wants-focus"), false));
            ctx.memory_mut(|memory| memory.surrender_focus(egui::Id::new("terminal-input")));
            self.presentation.show_connections();
            self.navigation.zoom = None;
            self.navigation
                .request_focus(navigation::Pane::Connections, egui::ViewportId::ROOT);
            if ctx.viewport_id() != egui::ViewportId::ROOT {
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
            }
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    pub(super) fn toggle_connections(&mut self, ctx: &egui::Context) {
        if self.connections_visible() {
            self.presentation.hide_sidebar();
            self.open_area(self.presentation.area, ctx);
        } else {
            self.show_connections(ctx);
        }
    }

    /// One resizable themed container hosts Explorer or Connections. Switching
    /// components changes content and focus, retaining the host's panel width.
    pub(super) fn workspace_sidebar(&mut self, ctx: &egui::Context) {
        let viewport = ctx.viewport_id();
        let view = if self.explorer_visible(viewport) {
            View::Explorer
        } else if viewport == egui::ViewportId::ROOT && self.connections_visible() {
            View::Connections
        } else {
            return;
        };
        let pane = match view {
            View::Explorer => navigation::Pane::Explorer,
            View::Connections => navigation::Pane::Connections,
        };
        if !self.pane_visible(pane, viewport) {
            return;
        }
        let palette = self.themes.active().palette;
        let zoom = self.pane_zoom(viewport) == Some(pane);
        let minimum_editor = if viewport == egui::ViewportId::ROOT {
            320.
        } else {
            240.
        };
        let maximum = (ctx.available_rect().width() - minimum_editor).clamp(MIN_WIDTH, 450.);
        let panel = egui::SidePanel::left(panel_id(viewport, zoom))
            .default_width(DEFAULT_WIDTH)
            .width_range(MIN_WIDTH..=maximum)
            .frame(layout::pane_frame(palette.panel));
        let panel = if zoom {
            panel.exact_width(ctx.available_rect().width())
        } else if let Some(width) = self
            .sidebar_state_mut(viewport)
            .and_then(|state| state.width.take())
        {
            panel.exact_width(width.clamp(MIN_WIDTH, maximum))
        } else {
            panel
        };
        panel.show(ctx, |ui| {
            palette.paint_panel(ui);
            if self.foreign_modal(ctx)
                || self.dialog != Dialog::None
                || self.pending.modal()
                || self.workbench_modal()
            {
                ui.disable();
            }
            match view {
                View::Explorer => self.explorer_content(ui, ctx),
                View::Connections => self.connections_content(ui, ctx),
            }
        });
    }
}
