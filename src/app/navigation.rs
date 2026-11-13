//! Keyboard navigation for the existing native panes. No global keyboard grabs.
//!
//! Focus intent is recorded separately from current widget focus so asynchronous opens can
//! restore the intended pane. Leaders/direct shortcuts honor modal barriers and native shell
//! input; no global window-manager keys are grabbed.
use super::*;
mod bindings;
pub(super) mod editing;
pub(super) use bindings::Style;
#[cfg(test)]
mod tests;
pub(super) use bindings::Action as NavAction;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Logical focus destination shared by keyboard routing, zoom and detached viewports.
pub(super) enum Pane {
    Explorer,
    Editor,
    Terminal,
    Tools,
    Menus,
}
impl Pane {
    fn index(self) -> usize {
        self as usize
    }
    pub fn id(self) -> egui::Id {
        egui::Id::new(("pane-focus", self.index()))
    }
}
/// Per-window focus intent, measured pane rectangles and shortcut-style state.
/// Layout measurements are rebuilt each frame; remembered focus survives asynchronous opens.
pub(super) struct State {
    pub bindings: bindings::Bindings,
    pub editing: editing::State,
    pub active: Pane,
    pub used: bool,
    pending: Option<Pane>,
    /// Focus requests target a native host so another tool window cannot consume them.
    pending_viewport: egui::ViewportId,
    leader: Option<Instant>,
    // Commands following source input must join the source replay queue after
    // the editor has translated that input, preserving order across frames.
    source_tail: Vec<egui::Event>,
    pub zoom: Option<Pane>,
    pub widths: [Option<f32>; 2],
    rects: [Option<egui::Rect>; 5],
    viewports: [Option<egui::ViewportId>; 5],
    remembered: std::collections::HashMap<egui::ViewportId, [Option<egui::Id>; 5]>,
    pub list: List,
    modal: bool,
}
impl State {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        Self {
            bindings: bindings::Bindings::load(storage),
            editing: Default::default(),
            active: Pane::Editor,
            used: false,
            pending: None,
            pending_viewport: egui::ViewportId::ROOT,
            leader: None,
            source_tail: Vec::new(),
            zoom: None,
            widths: [None; 2],
            rects: [None; 5],
            viewports: [None; 5],
            remembered: Default::default(),
            list: Default::default(),
            modal: false,
        }
    }
    pub fn begin_layout(&mut self) {
        self.rects = [None; 5];
        self.viewports = [None; 5];
    }
    pub fn rect(&self, pane: Pane) -> Option<egui::Rect> {
        self.rects[pane.index()]
    }
    pub fn visible(&self, pane: Pane) -> bool {
        self.zoom.is_none_or(|zoom| zoom == pane)
    }
    pub fn editor_intent(&mut self) {
        self.active = Pane::Editor;
        self.pending = Some(Pane::Editor);
        self.pending_viewport = egui::ViewportId::ROOT;
        self.zoom = self.zoom.filter(|p| *p == Pane::Editor);
    }
    pub fn terminal_intent(&mut self) {
        self.active = Pane::Terminal;
        self.pending = Some(Pane::Terminal);
        self.zoom = self.zoom.filter(|p| *p == Pane::Terminal);
    }
    pub fn request_focus(&mut self, pane: Pane, viewport: egui::ViewportId) {
        self.active = pane;
        self.pending = Some(pane);
        self.pending_viewport = viewport;
    }
    pub fn accepts_editor_focus(&self) -> bool {
        !self.used || self.active == Pane::Editor
    }
    /// Consume only command events, before TextEdit and the PTY see them. In
    /// particular, a prefix is not a terminal mode that remaps ordinary typing.
    fn input(&mut self, ctx: &egui::Context) -> Vec<NavAction> {
        if self
            .leader
            .is_some_and(|at| at.elapsed() > Duration::from_secs(5))
        {
            self.leader = None;
        }
        let mut actions = vec![];
        let terminal_focus = Terminal::focused(ctx);
        let prefix = self.bindings.prefix(terminal_focus);
        let source_focus = self.editing.has_focus(ctx);
        self.source_tail.clear();
        ctx.input_mut(|input| {
            let cut_prefix=prefix.key==Key::X && (input.modifiers.matches_exact(prefix.modifiers)||input.events.iter().any(|event|matches!(event,egui::Event::Key {key:Key::X,modifiers,..} if modifiers.matches_exact(prefix.modifiers))));
            let mut swallow_text = self.leader.is_some();
            let mut source_input = false;
            let mut defer_tail = false;
            input.events.retain(|event| {
                let command = match event {
                    egui::Event::Key { key, modifiers, pressed: true, .. } =>
                        prefix.matches(*key, *modifiers, false)
                            || self.leader.is_some()
                            || self.bindings.action(*key, *modifiers, false).is_some(),
                    egui::Event::Cut => cut_prefix && !terminal_focus,
                    _ => false,
                };
                if defer_tail || (source_focus && source_input && command) {
                    defer_tail = true;
                    // Cut carries no modifiers. Preserve the already recognized
                    // prefix even if Ctrl is released before this tail replays.
                    self.source_tail.push(if command && matches!(event, egui::Event::Cut) {
                        egui::Event::Key {
                            key: prefix.key, physical_key: None, pressed: true,
                            repeat: false, modifiers: prefix.modifiers,
                        }
                    } else { event.clone() });
                    return false;
                }
                let retain = match event {
                egui::Event::Key {
                    key,
                    modifiers,
                    pressed: true,
                    repeat,
                    ..
                } => {
                    if prefix.matches(*key, *modifiers, false) {
                        if !repeat {
                            self.leader = Some(Instant::now());
                        }
                        swallow_text = true;
                        false
                    } else if self.leader.is_some() {
                        if !repeat {
                            if *key != Key::Escape
                                && let Some(action) = self.bindings.action(*key, *modifiers, true)
                            {
                                actions.push(action);
                            }
                            self.leader = None;
                        }
                        swallow_text = true;
                        false
                    } else if let Some(action) = self.bindings.action(*key, *modifiers, false) {
                        if !repeat {
                            actions.push(action);
                        }
                        swallow_text = true;
                        false
                    } else {
                        true
                    }
                }
                egui::Event::Cut
                    if cut_prefix && !terminal_focus =>
                {
                    self.leader = Some(Instant::now());
                    swallow_text = true;
                    false
                }
                egui::Event::Text(_)
                | egui::Event::Copy
                | egui::Event::Cut
                | egui::Event::Paste(_) => !swallow_text,
                _ => true,
                };
                source_input |= retain && matches!(event,
                    egui::Event::Key { pressed: true, .. }
                    | egui::Event::Text(_)
                    | egui::Event::Paste(_)
                    | egui::Event::Cut
                    | egui::Event::Copy
                    | egui::Event::Ime(_));
                retain
            });
        });
        if self.leader.is_some() {
            ctx.request_repaint_after(Duration::from_secs(5));
        }
        actions
    }
}
/// Shared list cursor for the palette and loaded-file suggestions. No directory scan.
#[derive(Default)]
pub(super) struct List {
    query: String,
    pub index: usize,
    pub navigated: bool,
}
impl List {
    pub fn keys(ctx: &egui::Context) -> (isize, bool) {
        ctx.input_mut(|i| {
            let delta = isize::from(i.consume_key(egui::Modifiers::NONE, Key::ArrowDown))
                - isize::from(i.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
            (delta, i.consume_key(egui::Modifiers::NONE, Key::Enter))
        })
    }
    pub fn update(&mut self, query: &str, len: usize, delta: isize) -> bool {
        let changed = self.query != query;
        if changed {
            self.query = query.into();
            self.index = 0;
            self.navigated = false;
        }
        self.navigated |= delta != 0;
        self.index = self
            .index
            .saturating_add_signed(delta)
            .min(len.saturating_sub(1));
        changed || delta != 0
    }
}

impl EditorApp {
    /// Route pane/leader commands only when modal barriers and native terminal input permit them.
    pub(super) fn navigation_shortcuts(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.focused)
            || self.dialog != Dialog::None
            || self.pending.is_some()
            || self.workbench_modal()
            || self.session_exit_pending()
            || self.vaults.locking
            || egui::Popup::is_any_open(ctx)
        {
            self.navigation.leader = None;
            return;
        }
        for action in self.navigation.input(ctx) {
            self.navigate(action, ctx);
        }
    }
    /// Apply one typed navigation action through existing app/domain operations.
    pub(super) fn navigate(&mut self, action: NavAction, ctx: &egui::Context) {
        use NavAction::*;
        self.navigation.used = true;
        self.navigation.editing.cancel_sequence();
        match action {
            Left | Right | Up | Down => {
                let current = self.navigation.active;
                self.navigation.zoom = None;
                if let Some(rect) = self.navigation.rects[current.index()] {
                    let axis = if matches!(action, Left | Right) { 0 } else { 1 };
                    let sign = if matches!(action, Right | Down) {
                        1.
                    } else {
                        -1.
                    };
                    let next = [Pane::Explorer, Pane::Editor, Pane::Terminal, Pane::Tools]
                        .into_iter()
                        .filter(|p| *p != current && self.pane_exists(*p))
                        .filter(|p| {
                            self.navigation.viewports[p.index()]
                                .is_none_or(|viewport| viewport == ctx.viewport_id())
                        })
                        .filter_map(|p| self.navigation.rects[p.index()].map(|other| (p, other)))
                        .filter(|(_, other)| {
                            (other.center()[axis] - rect.center()[axis]) * sign > 1.
                                && other.min[1 - axis] < rect.max[1 - axis]
                                && other.max[1 - axis] > rect.min[1 - axis]
                        })
                        .min_by(|(_, a), (_, b)| {
                            let cost = |r: egui::Rect| {
                                (r.center()[axis] - rect.center()[axis]).abs()
                                    + (r.center()[1 - axis] - rect.center()[1 - axis]).abs() * 0.1
                            };
                            cost(*a).total_cmp(&cost(*b))
                        })
                        .map(|(p, _)| p);
                    if let Some(pane) = next {
                        self.focus_pane(pane, ctx);
                    }
                }
            }
            Next | Previous => {
                let panes: Vec<_> = [Pane::Explorer, Pane::Editor, Pane::Terminal, Pane::Tools]
                    .into_iter()
                    .filter(|p| self.pane_exists(*p))
                    .collect();
                let index = panes
                    .iter()
                    .position(|p| *p == self.navigation.active)
                    .unwrap_or(0);
                let step = if action == Next { 1 } else { panes.len() - 1 };
                self.focus_pane(panes[(index + step) % panes.len()], ctx);
            }
            Editor => self.focus_pane(Pane::Editor, ctx),
            Explorer => self.focus_pane(Pane::Explorer, ctx),
            Terminal => self.focus_pane(Pane::Terminal, ctx),
            Tools => {
                if let Some(tab) = self
                    .floating_tab(ctx.viewport_id())
                    .filter(|tab| tab.pane() == Pane::Tools)
                {
                    self.focus_tool_tab(tab, ctx);
                } else {
                    self.focus_pane(Pane::Tools, ctx);
                }
            }
            Menus => self.focus_pane(Pane::Menus, ctx),
            Narrower | Wider | Shorter | Taller => {
                self.navigation.zoom = None;
                let viewport = self.navigation.viewports[self.navigation.active.index()]
                    .unwrap_or(ctx.viewport_id());
                if viewport != egui::ViewportId::ROOT
                    && matches!(self.navigation.active, Pane::Tools | Pane::Terminal)
                {
                    let mut size = ctx
                        .input(|input| {
                            input
                                .raw
                                .viewports
                                .get(&viewport)
                                .and_then(|viewport| viewport.inner_rect)
                                .map(|rect| rect.size())
                        })
                        .unwrap_or(egui::vec2(900., 760.));
                    let axis = usize::from(matches!(action, Shorter | Taller));
                    size[axis] = (size[axis]
                        + if matches!(action, Wider | Taller) {
                            32.
                        } else {
                            -32.
                        })
                    .max(if axis == 0 { 640. } else { 420. });
                    ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::InnerSize(size));
                } else if matches!(action, Narrower | Wider)
                    && self.navigation.active == Pane::Explorer
                {
                    let width = self
                        .navigation
                        .rect(Pane::Explorer)
                        .map_or(250., |rect| rect.width());
                    self.navigation.widths[0] =
                        Some(width + if action == Wider { 32. } else { -32. });
                }
            }
            Zoom => {
                let viewport = self.navigation.viewports[self.navigation.active.index()]
                    .unwrap_or(ctx.viewport_id());
                if viewport != egui::ViewportId::ROOT
                    && matches!(self.navigation.active, Pane::Tools | Pane::Terminal)
                {
                    let maximized = ctx
                        .input(|input| {
                            input
                                .raw
                                .viewports
                                .get(&viewport)
                                .and_then(|viewport| viewport.maximized)
                        })
                        .unwrap_or(false);
                    ctx.send_viewport_cmd_to(
                        viewport,
                        egui::ViewportCommand::Maximized(!maximized),
                    );
                    return;
                }
                let pane = self.navigation.active;
                if pane != Pane::Menus {
                    self.navigation.zoom = if self.navigation.zoom == Some(pane) {
                        None
                    } else {
                        Some(pane)
                    };
                }
            }
            Hide => {
                if let Some(tab) = self.floating_tab(ctx.viewport_id()) {
                    self.reattach_tool_tab(tab, ctx);
                    return;
                }
                self.remember_tool_tab();
                match self.navigation.active {
                    Pane::Explorer => self.show_explorer = false,
                    Pane::Terminal => self.show_terminal = false,
                    Pane::Tools => self.workbench.panel = None,
                    _ => {}
                }
                self.navigation.zoom = None;
                self.focus_pane(Pane::Editor, ctx);
            }
            NewTerminal => {
                self.new_terminal(ctx);
                self.focus_pane(Pane::Terminal, ctx);
            }
            PreviousTerminal | NextTerminal => {
                self.focus_pane(Pane::Terminal, ctx);
                self.cycle_terminal(if action == NextTerminal { 1 } else { -1 }, ctx);
            }
            Persistence => {
                if self.terminal.is_some() {
                    self.focus_pane(Pane::Terminal, ctx);
                }
                self.toggle_terminal_persistence(ctx);
            }
            OpenFile => self.set_dialog(Dialog::Open),
            OpenFolder => self.set_dialog(Dialog::Folder),
            Commands => self.set_dialog(Dialog::Commands),
            Help => self.set_dialog(Dialog::Shortcuts),
            Save => self.save_active(ctx, false),
            SaveAs => self.save_active(ctx, true),
            CloseFile => {
                if let Some(doc) = self.documents.active() {
                    self.guarded(super::Action::CloseTab(doc.id), ctx);
                }
            }
            NextFile => self.cycle_tab(1, ctx),
            PreviousFile => self.cycle_tab(-1, ctx),
            NextMatch | PreviousMatch => self.style_find_match(action == PreviousMatch, ctx),
            Find => {
                self.markdown_source();
                self.show_find = true;
                self.find_focus = true;
            }
        }
        ctx.request_repaint();
    }
    fn pane_exists(&self, pane: Pane) -> bool {
        match pane {
            Pane::Explorer => self.show_explorer,
            Pane::Editor => true,
            Pane::Terminal => self.terminal_visible(),
            Pane::Tools => self.tools_visible(),
            Pane::Menus => true,
        }
    }
    /// Record focus intent and reveal the requested surface before focusing its native widget.
    pub(super) fn focus_pane(&mut self, pane: Pane, ctx: &egui::Context) {
        let tab = match pane {
            Pane::Terminal => Some(workbench::ToolTab::Terminal),
            Pane::Tools => Some(workbench::ToolTab::Panel(
                self.workbench.panel.unwrap_or(Panel::Monitor),
            )),
            _ => None,
        };
        if let Some(tab) = tab.filter(|tab| self.tab_detached(*tab)) {
            self.focus_tool_tab(tab, ctx);
            return;
        }
        let viewport = tab.map_or(egui::ViewportId::ROOT, |tab| self.tool_viewport(tab));
        if viewport == Self::tools_viewport() {
            self.workbench.tools_hidden = false;
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Visible(true));
        }
        if viewport != ctx.viewport_id() {
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
        }
        self.navigation.request_focus(pane, viewport);
        if self.navigation.zoom.is_some_and(|p| p != pane) {
            self.navigation.zoom = None;
        }
        self.tabs.cancel_focus();
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("terminal-wants-focus"), false));
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("terminal-input")));
        match pane {
            Pane::Explorer => self.show_explorer = true,
            Pane::Terminal => {
                self.terminal_intent();
                self.show_terminal = true;
                if self.terminal.is_none() && self.ssh_master.is_none() && !self.session_busy() {
                    self.new_terminal(ctx);
                }
            }
            Pane::Tools => {
                self.workbench.surface = workbench::Surface::Tools;
                self.workbench.tools_hidden = false;
                self.remember_tool_tab();
                if self.workbench.panel.is_none() {
                    self.toggle_panel(Panel::Monitor, ctx);
                }
            }
            Pane::Editor => {
                self.focus_editor(ctx);
            }
            Pane::Menus => {}
        }
        ctx.request_repaint();
    }
    /// A stable fallback focus target, plus remembered child controls for tools.
    pub(super) fn pane_ui(&mut self, ui: &mut egui::Ui, pane: Pane) {
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        if self.navigation.zoom.is_none()
            && (self.navigation.viewports[pane.index()].is_none()
                || ctx.input(|input| input.focused)
                || (self.navigation.pending == Some(pane)
                    && self.navigation.pending_viewport == ctx.viewport_id()))
        {
            self.navigation.rects[pane.index()] = Some(rect);
            self.navigation.viewports[pane.index()] = Some(ctx.viewport_id());
        }
        let host_id = if ctx.viewport_id() == egui::ViewportId::ROOT {
            pane.id()
        } else {
            pane.id().with(ctx.viewport_id())
        };
        let host = ui.interact(rect, host_id, Sense::focusable_noninteractive());
        ui_probe::button(ui, &format!("pane-{pane:?}"), &host);
        if self.navigation.zoom == Some(pane) {
            ui_probe::button(ui, &format!("pane-maximized-{pane:?}"), &host);
        }
        let blocked = self.dialog != Dialog::None
            || self.pending.is_some()
            || self.workbench_modal()
            || egui::Popup::is_any_open(&ctx);
        if !blocked
            && self.navigation.pending == Some(pane)
            && self.navigation.pending_viewport == ctx.viewport_id()
            && ctx.memory(|m| m.allows_interaction(ui.layer_id()))
        {
            if ctx.viewport_id() != egui::ViewportId::ROOT {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            match pane {
                Pane::Terminal => Terminal::focus(&ctx),
                Pane::Editor if !self.documents.is_empty() => self.focus_editor(&ctx),
                Pane::Explorer if self.workspace.is_some() => host.request_focus(),
                _ => {
                    let remembered = self
                        .navigation
                        .remembered
                        .get(&ctx.viewport_id())
                        .and_then(|panes| panes[pane.index()]);
                    ctx.memory_mut(|m| m.request_focus(remembered.unwrap_or(host.id)));
                }
            }
            self.navigation.pending = None;
        }
        if !blocked && self.navigation.pending.is_none() {
            let focused = ctx.memory(|m| m.focused());
            let inside = focused
                .and_then(|id| ctx.read_response(id))
                .is_some_and(|r| rect.contains(r.interact_rect.center()));
            let clicked = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer.interact_pos().is_some_and(|p| rect.contains(p))
            });
            if ctx.input(|input| input.focused)
                && (inside || (pane == Pane::Terminal && Terminal::focused(&ctx)) || clicked)
            {
                self.navigation.active = pane;
                if inside {
                    self.navigation
                        .remembered
                        .entry(ctx.viewport_id())
                        .or_insert([None; 5])[pane.index()] = focused;
                }
            }
        }
        if self.navigation.used && self.navigation.active == pane && !blocked {
            ui.painter().rect_stroke(
                rect.shrink(1.),
                0,
                Stroke::new(
                    1.0_f32,
                    self.themes.active().palette.accent.gamma_multiply(0.6),
                ),
                egui::StrokeKind::Inside,
            );
        }
    }
    /// Render the same focus/zoom commands exposed by keyboard navigation.
    pub(super) fn navigation_menu(&mut self, response: &egui::Response) {
        self.navigation
            .remembered
            .entry(response.ctx.viewport_id())
            .or_insert([None; 5])[Pane::Menus.index()] = Some(response.id);
        if self.navigation.pending == Some(Pane::Menus)
            && self.navigation.pending_viewport == response.ctx.viewport_id()
        {
            response.request_focus();
            self.navigation.pending = None;
        }
    }
    /// Resolve deferred focus using this frame's measured panes and owning viewport.
    pub(super) fn navigation_finish(&mut self, ctx: &egui::Context) {
        let modal = self.dialog != Dialog::None
            || self.pending.is_some()
            || self.workbench_modal()
            || self.session_exit_pending();
        if self.navigation.modal && !modal {
            self.navigation.pending = Some(self.navigation.active);
            self.navigation.pending_viewport = ctx.viewport_id();
            ctx.request_repaint();
        }
        self.navigation.modal = modal;
        if self.navigation.leader.is_some() && ctx.input(|input| input.focused) {
            let palette = self.themes.active().palette;
            egui::Area::new(egui::Id::new("navigation-hint"))
                .anchor(egui::Align2::CENTER_BOTTOM, [0., -40.])
                .order(egui::Order::Tooltip)
                .interactable(false)
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(palette.raised)
                        .stroke(Stroke::new(1.0_f32, palette.accent))
                        .inner_margin(10)
                        .show(ui, |ui| {
                            let hint = ui.label(format!(
                                "{} · navigation · Escape cancels",
                                self.navigation
                                    .bindings
                                    .prefix_label(Terminal::focused(ctx))
                            ));
                            ui_probe::button(ui, "navigation-leader", &hint);
                            ui.set_max_width(680.);
                            for (action, label, keys) in self.navigation.bindings.commands() {
                                if matches!(
                                    action,
                                    NavAction::Editor
                                        | NavAction::Explorer
                                        | NavAction::Terminal
                                        | NavAction::Tools
                                        | NavAction::Help
                                ) {
                                    ui.small(format!("{keys} · {label}"));
                                }
                            }
                        });
                });
        }
    }
}
