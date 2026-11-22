//! Root sidebar selection is independent of the central application area. One
//! selection prevents Explorer and Connections from competing for the same edge.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum View {
    Explorer,
    Connections,
}

#[derive(Default)]
pub(super) struct State {
    pub connections_width: Option<f32>,
}

pub(super) fn connections_panel_id(zoom: bool) -> egui::Id {
    egui::Id::new(if zoom {
        "connections-sidebar-zoom"
    } else {
        "connections-sidebar"
    })
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

    /// The shared sidebar content also renders in its existing detached host.
    /// Pin entry actions above one scrolling library, so long lists cannot hide
    /// Open project, Connect SSH or New tunnel.
    pub(super) fn connections_sidebar(&mut self, ctx: &egui::Context) {
        if ctx.viewport_id() != egui::ViewportId::ROOT
            || !self.connections_visible()
            || !self.pane_visible(navigation::Pane::Connections, egui::ViewportId::ROOT)
        {
            return;
        }
        let palette = self.themes.active().palette;
        let zoom = self.navigation.zoom == Some(navigation::Pane::Connections);
        let maximum = (ctx.available_rect().width() - 320.).clamp(220., 450.);
        let panel = egui::SidePanel::left(connections_panel_id(zoom))
            .default_width(290.)
            .width_range(220. ..=maximum)
            .frame(layout::pane_frame(palette.panel));
        let panel = if zoom {
            panel.exact_width(ctx.available_rect().width())
        } else if let Some(width) = self.sidebar.connections_width.take() {
            panel.exact_width(width.clamp(220., maximum))
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
            self.connections_content(ui, ctx);
        });
    }
}
