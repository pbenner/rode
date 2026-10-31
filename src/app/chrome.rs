//! Compact editor chrome. Geometry is explicit so long labels and open panels
//! cannot change row heights or push essential controls out of the viewport.
//!
//! Tab indices are presentation order; document IDs own cursor/undo/worker identity. Keep the
//! active tab visible during overflow and preserve independent click targets for selecting,
//! closing and dragging a tab.
use super::*;

const TAB_HEIGHT: f32 = 36.0;
const TAB_MIN: f32 = 140.0;
const TAB_MAX: f32 = 260.0;

#[derive(Default)]
pub(super) struct Tabs {
    active: Option<u64>,
    reveal: Option<u64>,
    offset: f32,
    focus: Option<u64>,
    width: f32,
}
impl Tabs {
    pub(in crate::app) fn cancel_focus(&mut self) {
        self.focus = None;
    }
}
#[derive(Clone, Copy)]
struct TabDrag(u64);

impl EditorApp {
    pub(super) fn statusbar(&mut self, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let roomy = ctx.content_rect().width() >= 1100.;
        egui::TopBottomPanel::bottom("statusbar")
            .exact_height(27.)
            .frame(
                egui::Frame::new()
                    .fill(palette.raised)
                    .inner_margin(egui::Margin::symmetric(12, 4)),
            )
            .show(ctx, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let full_theme = format!("Theme: {}", self.themes.active().name);
                    let label = elide(&full_theme, if roomy { 26 } else { 18 });
                    ui.menu_button(palette.tag(label), |ui| self.appearance_menu(ui, ctx))
                        .response
                        .on_hover_text(format!(
                            "{full_theme}\nSyntax: {}\nChange interface or syntax theme · saved automatically", self.themes.active_syntax().name
                        ));
                    if roomy {
                        ui.label(palette.tag("RODE 0.1"));
                    }
                    if ui.small_button(palette.tag(if roomy { "Environment" } else { "Env" }))
                        .on_hover_text(format!("Project runtimes and variables\n{}", self.workbench_python_label()))
                        .clicked() {
                        self.toggle_panel(Panel::Environment, ctx);
                    }
                    let count = self.current_issue_count();
                    if ui
                        .small_button(
                            RichText::new(format!("! {count}"))
                                .monospace()
                                .size(11.)
                                .color(if count > 0 {
                                    palette.error
                                } else {
                                    palette.muted
                                }),
                        )
                        .on_hover_text("Problems")
                        .clicked()
                    {
                        self.toggle_panel(Panel::Problems, ctx);
                    }
                    if let Some(doc) = self.documents.get(self.active) {
                        if let Some(book)=&doc.notebook {ui.label(palette.tag(format!("Notebook · {} cells",book.book.cells.len())));}
                        else if let Some(hex)=&doc.hex {ui.label(palette.tag(format!("Binary · {} bytes",hex.buffer.bytes.len())));}
                        else if let Some(pdf)=&doc.pdf {ui.label(palette.tag(format!("PDF · page {} / {}",pdf.page,pdf.source.pages)));}
                        else if let Some(image) = &doc.image {
                            ui.label(palette.tag(format!(
                                "{} · {} × {}",
                                image.format, image.original[0], image.original[1]
                            )));
                        } else {
                            if roomy {
                                ui.label(palette.tag(language(doc.display_path())));
                                ui.label(palette.tag("UTF-8"));
                            }
                            ui.label(
                                palette.tag(format!("Ln {}, Col {}", doc.cursor.0, doc.cursor.1)),
                            );
                        }
                        if doc.saving {
                            ui.spinner();
                            if roomy {
                                ui.label(palette.tag("SAVING"));
                            }
                        }
                    }
                    ui.separator();
                    let width = ui.available_width().max(1.);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, 19.),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_min_size(egui::vec2(width, 19.));
                            let (connection, color) = match &self.workspace {
                                Some(w) if w.remote.is_some() => (
                                    format!(
                                        "↗ SSH {}{}",
                                        w.remote.as_ref().unwrap().profile.endpoint(),
                                        if w.connected { "" } else { " · offline" }
                                    ),
                                    if w.connected {
                                        palette.success
                                    } else {
                                        palette.error
                                    },
                                ),
                                _ => (">_ LOCAL".into(), palette.success),
                            };
                            let natural = ui
                                .painter()
                                .layout_no_wrap(connection.clone(), FontId::monospace(11.), color)
                                .size()
                                .x
                                + 8.;
                            let host_width = natural.min(190.).min(width * 0.5);
                            if ui
                                .add_sized(
                                    [host_width, 19.],
                                    egui::Button::new(
                                        RichText::new(&connection)
                                            .monospace()
                                            .size(11.)
                                            .color(color),
                                    )
                                    .frame(false)
                                    .truncate(),
                                )
                                .on_hover_text(&connection)
                                .clicked()
                            {
                                self.set_dialog(Dialog::Connect);
                            }
                            ui.separator();
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&self.status).size(11.).color(palette.muted),
                                )
                                .truncate()
                                .halign(Align::Min),
                            )
                            .on_hover_text(&self.status);
                        },
                    );
                });
            });
    }

    pub(super) fn terminal_header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let wide = ui.available_width() >= 700.;
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let native = self.terminal.as_ref().is_some_and(|t| t.native && t.is_alive());
                let persistent = self.terminal.as_ref().is_some_and(Terminal::persistent);
                let persistence_supported = self.terminal_persistence_supported();
                let mut persist_button = egui::Button::new(
                    RichText::new(if persistent { "Persist: on" } else { "Persist: off" })
                        .color(if persistent { palette.background } else { palette.text }),
                ).selected(persistent).small().min_size(egui::vec2(84., 0.));
                if persistent {
                    persist_button = persist_button.fill(palette.accent).stroke(egui::Stroke::new(1.0_f32, palette.accent));
                }
                // This row flows right to left: persistence is always the
                // rightmost action, with a stable width in both states.
                if ui.add_enabled(native && persistence_supported && !self.session_busy(), persist_button)
                    .on_hover_text("Keep running after exit. On: this terminal survives its last view closing. Off: it ends when its last view closes. Shared across attached RODE windows; changing this does not restart the shell.")
                    .on_disabled_hover_text(if native && !persistence_supported {
                        rode_session::LEGACY_PERSISTENCE_HELP
                    } else if native {
                        "Waiting for the terminal service to finish the current request."
                    } else {
                        "Persistence is available for native terminals. Temporary terminals and SSH login prompts cannot be made persistent."
                    }).clicked() {
                    self.toggle_terminal_persistence(ctx);
                }
                self.slurm_terminal_button(ui, ctx);
                if icons::small(ui, Icon::Close, "Hide terminal; keep session running").clicked() {
                    self.show_terminal = false;
                    self.focus_editor(ctx);
                }
                if wide && ui
                    .small_button("Copy")
                    .on_hover_text("Copy visible terminal contents")
                    .clicked()
                    && let Some(t) = self.terminal.as_ref().or(self.ssh_master.as_ref())
                {
                    ctx.copy_text(t.contents());
                }
                if wide && ui
                    .small_button("Sessions")
                    .on_hover_text("Reattach native terminals / Slurm allocations")
                    .clicked()
                {
                    self.open_sessions(ctx);
                }
                if wide && ui
                    .add_enabled(!self.session_busy(), egui::Button::new("New").small())
                    .on_hover_text("New terminal; use Persist to keep it running after exit")
                    .clicked()
                {
                    self.new_terminal(ctx);
                }
                if !wide {
                    egui::containers::menu::MenuButton::from_button(egui::Button::new("More").small()).ui(ui, |ui| {
                        if ui.add_enabled(!self.session_busy(), egui::Button::new("New terminal")).clicked() {
                            self.new_terminal(ctx); ui.close();
                        }
                        if ui.button("Sessions and Slurm allocations").clicked() {
                            self.open_sessions(ctx); ui.close();
                        }
                        if ui.button("Copy terminal output").clicked() {
                            if let Some(terminal) = self.terminal.as_ref().or(self.ssh_master.as_ref()) {
                                ctx.copy_text(terminal.contents());
                            }
                            ui.close();
                        }
                        if ui.add_enabled(self.terminal.as_ref().is_some_and(Terminal::persistent), egui::Button::new("Detach session"))
                            .on_hover_text("Enable Persist before detaching a terminal.").clicked() {
                            self.detach_terminal(ctx);
                            ui.close();
                        }
                    });
                }
                if wide && self
                    .terminal
                    .as_ref()
                    .is_some_and(|terminal| terminal.native)
                    && ui.add_enabled(persistent, egui::Button::new("Detach session").small())
                        .on_hover_text("Enable Persist to detach and keep this terminal running.").clicked()
                {
                    self.detach_terminal(ctx);
                }
                if wide && self.terminal.as_ref().is_some_and(|t| !t.is_alive()) {
                    ui.label(
                        RichText::new(
                            if self
                                .terminal
                                .as_ref()
                                .is_some_and(|terminal| terminal.native)
                            {
                                "VIEW ENDED"
                            } else {
                                "SESSION ENDED"
                            },
                        )
                        .size(10.)
                        .color(palette.error),
                    );
                }
                let remaining = ui.available_width().max(1.);
                if remaining < 80. { return; }
                ui.allocate_ui_with_layout(
                    egui::vec2(remaining, 20.),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_min_size(egui::vec2(remaining, 20.));
                        ui.label(
                            RichText::new("TERMINAL")
                                .monospace()
                                .size(11.)
                                .color(palette.accent),
                        );
                        if let Some(terminal) = self.terminal.as_ref().or(self.ssh_master.as_ref())
                        {
                            ui.add(egui::Label::new(palette.tag(&terminal.title)).truncate())
                                .on_hover_text(&terminal.title);
                        }
                    },
                );
            });
        });
    }

    pub(super) fn reveal_tab(&mut self, id: u64) {
        self.tabs.reveal = Some(id);
    }

    pub(super) fn cycle_tab(&mut self, direction: isize, ctx: &egui::Context) {
        if self.documents.is_empty() {
            return;
        }
        self.active =
            (self.active as isize + direction).rem_euclid(self.documents.len() as isize) as usize;
        self.tabs.reveal = Some(self.documents[self.active].id);
        self.focus_editor(ctx);
    }
    pub(super) fn focus_editor(&mut self, ctx: &egui::Context) {
        if self.dialog != Dialog::None || self.pending.is_some() || self.workbench_modal() {
            return;
        }
        self.editor_intent();
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::new("terminal-input")));
        if ctx.viewport_id() != egui::ViewportId::ROOT {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
        self.navigation.zoom = self
            .navigation
            .zoom
            .filter(|p| *p == navigation::Pane::Editor);
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("terminal-wants-focus"), false));
        if let Some(doc) = self.documents.get(self.active) {
            self.tabs.focus = Some(doc.id);
            ctx.request_repaint();
        }
    }
    pub(super) fn take_editor_focus(&mut self, ui: &egui::Ui, ctx: &egui::Context) -> Option<u64> {
        self.tabs.focus?;
        if self.dialog != Dialog::None
            || self.pending.is_some()
            || self.workbench_modal()
            || egui::Popup::is_any_open(ctx)
        {
            return None;
        }
        // A closed modal still blocks its background for one egui pass.
        if ctx.memory(|m| m.allows_interaction(ui.layer_id())) {
            self.tabs.focus.take()
        } else {
            ctx.request_repaint();
            None
        }
    }

    pub(super) fn reorder_tab(&mut self, source: u64, target: u64, after: bool) {
        if source == target {
            return;
        }
        let active = self.documents.get(self.active).map(|d| d.id);
        let Some(from) = self.documents.iter().position(|d| d.id == source) else {
            return;
        };
        if !self.documents.iter().any(|d| d.id == target) {
            return;
        }
        let document = self.documents.remove(from);
        let target = self.documents.iter().position(|d| d.id == target).unwrap();
        self.documents.insert(target + usize::from(after), document);
        if let Some(id) = active {
            self.active = self.documents.iter().position(|d| d.id == id).unwrap_or(0);
        }
        self.tabs.reveal = active;
    }

    pub(super) fn tab_strip(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let active = self.documents.get(self.active).map(|d| d.id);
        if active != self.tabs.active {
            self.tabs.reveal = active;
            self.tabs.active = active;
            if self.navigation.accepts_editor_focus() {
                self.focus_editor(ctx);
            }
        }
        let labels = tab_labels(&self.documents);
        let font = FontId::proportional(13.0);
        let mut widths: Vec<f32> = labels
            .iter()
            .map(|label| {
                let width = ui
                    .painter()
                    .layout_no_wrap(label.clone(), font.clone(), palette.text)
                    .size()
                    .x;
                (width + 94.0).clamp(TAB_MIN, TAB_MAX)
            })
            .collect();
        let natural: f32 = widths.iter().sum();
        let width = ui.available_width();
        let overflow = natural > width - 34.0;
        let tools_width = if overflow { 90.0 } else { 32.0 };
        let strip_width = (width - tools_width).max(32.0);
        if (self.tabs.width - strip_width).abs() > 0.5 {
            self.tabs.width = strip_width;
            self.tabs.reveal = active;
        }
        for width in &mut widths {
            *width = width.min(strip_width);
        }
        let total: f32 = widths.iter().sum();
        let (row, _) = ui.allocate_exact_size(egui::vec2(width, TAB_HEIGHT), Sense::hover());
        ui.painter().rect_filled(row, 0, palette.panel);
        let strip = egui::Rect::from_min_size(row.min, egui::vec2(strip_width, TAB_HEIGHT));
        let tools = egui::Rect::from_min_max(egui::pos2(strip.right(), row.top()), row.max);
        let mut close = None;
        let mut move_tab = None;
        let mut selected = None;
        let reveal = self.tabs.reveal.take();
        let old_offset = self.tabs.offset;
        let output = ui
            .scope_builder(egui::UiBuilder::new().max_rect(strip), |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                egui::ScrollArea::horizontal()
                    .id_salt("editor-tabs")
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .horizontal_scroll_offset(old_offset)
                    .max_height(TAB_HEIGHT)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (index, doc) in self.documents.iter().enumerate() {
                                let is_active = index == self.active;
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(widths[index], TAB_HEIGHT),
                                    Sense::hover(),
                                );
                                let close_rect = egui::Rect::from_center_size(
                                    egui::pos2(rect.right() - 17.0, rect.center().y),
                                    egui::vec2(24., 24.),
                                );
                                let body = egui::Rect::from_min_max(
                                    rect.min,
                                    egui::pos2(close_rect.left() - 2., rect.bottom()),
                                );
                                let response = ui.interact(
                                    body,
                                    ui.id().with(("tab", doc.id)),
                                    Sense::click_and_drag(),
                                );
                                let hovered = rect.contains(
                                    ui.input(|i| i.pointer.hover_pos().unwrap_or_default()),
                                );
                                let fill = if is_active {
                                    palette.background
                                } else if hovered {
                                    palette.hover
                                } else {
                                    palette.panel
                                };
                                let painter =
                                    ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
                                painter.rect_filled(rect, 0, fill);
                                painter.line_segment(
                                    [rect.right_top(), rect.right_bottom()],
                                    Stroke::new(1.0_f32, palette.border),
                                );
                                if is_active {
                                    painter.rect_filled(
                                        egui::Rect::from_min_size(
                                            rect.min,
                                            egui::vec2(rect.width(), 2.),
                                        ),
                                        0,
                                        palette.accent,
                                    );
                                } else {
                                    painter.line_segment(
                                        [rect.left_bottom(), rect.right_bottom()],
                                        Stroke::new(1.0_f32, palette.border),
                                    );
                                }
                                painter.text(
                                    egui::pos2(rect.left() + 12., rect.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    file_icon(&doc.name()),
                                    FontId::monospace(11.),
                                    if is_active {
                                        palette.accent
                                    } else {
                                        palette.muted
                                    },
                                );
                                let label_rect = egui::Rect::from_min_max(
                                    egui::pos2(rect.left() + 34., rect.top()),
                                    egui::pos2(rect.right() - 50., rect.bottom()),
                                );
                                let mut job = egui::text::LayoutJob::simple(
                                    labels[index].clone(),
                                    font.clone(),
                                    if is_active {
                                        palette.text
                                    } else {
                                        palette.muted
                                    },
                                    label_rect.width(),
                                );
                                job.wrap.max_rows = 1;
                                job.wrap.break_anywhere = true;
                                let galley = ui.fonts_mut(|f| f.layout_job(job));
                                painter.galley(
                                    egui::pos2(
                                        label_rect.left(),
                                        rect.center().y - galley.size().y / 2.,
                                    ),
                                    galley,
                                    palette.text,
                                );
                                if doc.dirty() {
                                    painter.circle_filled(
                                        egui::pos2(rect.right() - 42., rect.center().y),
                                        3.,
                                        palette.accent,
                                    );
                                }
                                let close_response = ui
                                    .interact(
                                        close_rect,
                                        ui.id().with(("close-tab", doc.id)),
                                        Sense::click(),
                                    )
                                    .on_hover_text(format!(
                                        "Close {} · {} / middle click\n{}",
                                        labels[index],
                                        self.editing_hint("Ctrl+W"),
                                        doc.display_path().unwrap_or("Unsaved file")
                                    ));
                                if close_response.hovered() {
                                    painter.rect_filled(close_rect, 3, palette.raised);
                                }
                                // The close target always occupies the same space, even
                                // when the dirty marker appears or hover changes.
                                let color = if close_response.hovered() {
                                    palette.text
                                } else {
                                    palette.muted
                                };
                                let center = close_rect.center();
                                painter.line_segment(
                                    [center + egui::vec2(-3., -3.), center + egui::vec2(3., 3.)],
                                    Stroke::new(1.0_f32, color),
                                );
                                painter.line_segment(
                                    [center + egui::vec2(-3., 3.), center + egui::vec2(3., -3.)],
                                    Stroke::new(1.0_f32, color),
                                );
                                if response.clicked() {
                                    selected = Some(index);
                                }
                                if response.clicked_by(egui::PointerButton::Middle)
                                    || close_response.clicked_by(egui::PointerButton::Middle)
                                    || close_response.clicked()
                                {
                                    close = Some(doc.id);
                                }
                                let response = response.on_hover_text(format!(
                                    "{}{}\nDrag to reorder · middle click to close",
                                    doc.display_path().unwrap_or("Unsaved file"),
                                    if doc.dirty() { "\nUnsaved changes" } else { "" }
                                ));
                                response.dnd_set_drag_payload(TabDrag(doc.id));
                                if let Some(payload) = response.dnd_hover_payload::<TabDrag>()
                                    && payload.0 != doc.id
                                {
                                    let after = ui.input(|i| {
                                        i.pointer.hover_pos().is_some_and(|p| p.x > rect.center().x)
                                    });
                                    let x = if after { rect.right() } else { rect.left() };
                                    painter.line_segment(
                                        [
                                            egui::pos2(x, rect.top() + 3.),
                                            egui::pos2(x, rect.bottom() - 3.),
                                        ],
                                        Stroke::new(2.0_f32, palette.accent),
                                    );
                                    if let Some(payload) = response.dnd_release_payload::<TabDrag>()
                                    {
                                        move_tab = Some((payload.0, doc.id, after));
                                    }
                                }
                                response.context_menu(|ui| {
                                    if ui.add(IconButton::new(Icon::Close, "Close tab")).clicked() {
                                        close = Some(doc.id);
                                        ui.close();
                                    }
                                    if let Some(path) = &doc.path
                                        && ui
                                            .add(IconButton::new(Icon::Copy, "Copy file path"))
                                            .clicked()
                                    {
                                        ctx.copy_text(path.clone());
                                        ui.close();
                                    }
                                });
                                if reveal == Some(doc.id) {
                                    ui.scroll_to_rect(rect, Some(Align::Center));
                                }
                            }
                        });
                    })
            })
            .inner;
        self.tabs.offset = output.state.offset.x;
        let max_offset = (total - strip_width).max(0.0);
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(tools)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 0.;
                ui.spacing_mut().button_padding = egui::vec2(2., 2.);
                if overflow {
                    if ui
                        .add_enabled(
                            self.tabs.offset > 0.5,
                            IconButton::icon(Icon::ChevronLeft, "Scroll tabs left")
                                .icon_size(14.)
                                .min_size(egui::vec2(28., TAB_HEIGHT)),
                        )
                        .clicked()
                    {
                        self.tabs.offset = (self.tabs.offset - strip_width * 0.7).max(0.);
                        ctx.request_repaint();
                    }
                    if ui
                        .add_enabled(
                            self.tabs.offset < max_offset - 0.5,
                            IconButton::icon(Icon::ChevronRight, "Scroll tabs right")
                                .icon_size(14.)
                                .min_size(egui::vec2(28., TAB_HEIGHT)),
                        )
                        .clicked()
                    {
                        self.tabs.offset = (self.tabs.offset + strip_width * 0.7).min(max_offset);
                        ctx.request_repaint();
                    }
                }
                icons::overflow(ui, |ui| {
                    ui.set_max_width(420.);
                    egui::ScrollArea::vertical()
                        .max_height(320.)
                        .show(ui, |ui| {
                            for (index, doc) in self.documents.iter().enumerate() {
                                if ui
                                    .selectable_label(
                                        index == self.active,
                                        format!(
                                            "{}{}",
                                            labels[index],
                                            if doc.dirty() { "  •" } else { "" }
                                        ),
                                    )
                                    .on_hover_text(doc.path.as_deref().unwrap_or("Unsaved file"))
                                    .clicked()
                                {
                                    selected = Some(index);
                                    self.tabs.reveal = Some(doc.id);
                                    ui.close();
                                }
                            }
                        });
                })
                .on_hover_text("All open editors · Ctrl+PageUp / Ctrl+PageDown");
            },
        );
        if let Some(index) = selected {
            self.active = index;
            self.tabs.reveal = self.documents.get(index).map(|d| d.id);
            self.focus_editor(ctx);
        }
        if let Some((source, target, after)) = move_tab {
            self.reorder_tab(source, target, after);
            ctx.request_repaint();
        }
        if let Some(id) = close {
            self.guarded(Action::CloseTab(id), ctx);
            ctx.request_repaint();
        }
    }
}

/// Add the shortest distinct parent suffix only when basenames collide.
fn tab_labels(documents: &[Document]) -> Vec<String> {
    let names: Vec<_> = documents.iter().map(Document::name).collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for name in &names {
        *counts.entry(name).or_default() += 1;
    }
    documents
        .iter()
        .enumerate()
        .map(|(index, doc)| {
            let name = &names[index];
            if counts[name.as_str()] == 1 {
                return name.clone();
            }
            let peers: Vec<_> = documents
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index && names[*other] == *name)
                .map(|(_, d)| d)
                .collect();
            if peers.is_empty() {
                return name.clone();
            }
            let parent = |path: Option<&str>| {
                path.and_then(|p| std::path::Path::new(p).parent())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            let path = parent(doc.display_path());
            let suffix = |path: &str, count: usize| {
                path.rsplit('/')
                    .take(count)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("/")
            };
            for depth in 1..=path.split('/').count() {
                let candidate = suffix(&path, depth);
                if peers
                    .iter()
                    .all(|peer| suffix(&parent(peer.display_path()), depth) != candidate)
                {
                    return format!("{name} · {candidate}");
                }
            }
            format!("{name} · {path}")
        })
        .collect()
}

fn elide(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.into()
    } else {
        text.chars()
            .take(limit.saturating_sub(1))
            .chain(std::iter::once('…'))
            .collect()
    }
}

pub(super) fn bounded_label(
    ui: &mut egui::Ui,
    text: RichText,
    width: f32,
    height: f32,
) -> egui::Response {
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_min_size(egui::vec2(width, height));
            ui.add(egui::Label::new(text).truncate())
        },
    )
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    fn editor() -> (EditorApp, egui::Context) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        (EditorApp::new(&cc, Startup::default()), ctx)
    }
    #[test]
    fn duplicate_filenames_use_the_shortest_unique_parent_suffix() {
        let (mut app, _) = editor();
        for path in [
            "/project/api/src/config.py",
            "/project/worker/src/config.py",
            "/project/data/config.py",
            "/project/🦀/main.rs",
        ] {
            app.new_file();
            app.documents.last_mut().unwrap().path = Some(path.into());
        }
        assert_eq!(
            tab_labels(&app.documents),
            [
                "config.py · api/src",
                "config.py · worker/src",
                "config.py · data",
                "main.rs"
            ]
        );
        app.documents[1].path = Some("/project/worker/src/settings.py".into());
        assert_eq!(
            tab_labels(&app.documents),
            [
                "config.py · src",
                "settings.py",
                "config.py · data",
                "main.rs"
            ]
        );
        assert_eq!(elide("🦀你好long.txt", 5), "🦀你好l…");
    }
    #[test]
    fn rearranging_and_closing_tabs_preserves_selection_and_unsaved_buffers() {
        let (mut app, ctx) = editor();
        for text in ["one", "two", "three"] {
            app.new_file();
            app.documents.last_mut().unwrap().text = text.into();
        }
        let ids: Vec<_> = app.documents.iter().map(|d| d.id).collect();
        app.reorder_tab(ids[0], ids[1], true);
        assert_eq!(
            app.documents
                .iter()
                .map(|d| d.text.as_str())
                .collect::<Vec<_>>(),
            ["two", "one", "three"]
        );
        assert_eq!(app.documents[app.active].id, ids[2]);
        app.reorder_tab(ids[2], ids[1], false);
        assert_eq!(app.documents[app.active].id, ids[2]);
        assert_eq!(app.active, 0);
        let before: Vec<_> = app.documents.iter().map(|d| d.id).collect();
        app.reorder_tab(ids[0], u64::MAX, false);
        assert_eq!(
            app.documents.iter().map(|d| d.id).collect::<Vec<_>>(),
            before
        );
        app.guarded(Action::CloseTab(ids[0]), &ctx);
        assert!(matches!(app.pending,Some(Action::CloseTab(id)) if id == ids[0]));
        assert_eq!(app.documents.len(), 3);
    }
    #[test]
    fn tab_navigation_wraps_and_strip_height_stays_fixed_with_long_names() {
        let (mut app, ctx) = editor();
        for index in 0..12 {
            app.new_file();
            app.documents.last_mut().unwrap().path =
                Some(format!("/project/a_very_long_file_name_{index}.txt"));
        }
        app.cycle_tab(1, &ctx);
        assert_eq!(app.active, 0);
        app.cycle_tab(-1, &ctx);
        assert_eq!(app.active, 11);
        for frame in 0..10 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(850., 560.),
                    )),
                    time: Some(frame as f64 * 0.1),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let before = ui.cursor().top();
                        app.tab_strip(ui, ctx);
                        assert!(
                            ui.cursor().top() - before
                                <= TAB_HEIGHT + ui.spacing().item_spacing.y + 1.
                        );
                    });
                },
            );
        }
        assert!(app.tabs.offset > 0., "The active last tab must be revealed");
    }
    #[test]
    fn editor_focus_survives_the_pass_after_a_modal_closes() {
        let (mut app, ctx) = editor();
        app.new_file();
        let frame = |time: f64| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(850., 560.),
            )),
            time: Some(time),
            ..Default::default()
        };
        let _ = ctx.run(frame(0.), |ctx| {
            egui::Modal::new(egui::Id::new("previous-dialog")).show(ctx, |ui| {
                ui.label("Open file");
            });
        });
        app.focus_editor(&ctx);
        let _ = ctx.run(frame(0.1), |ctx| app.editor(ctx));
        assert!(
            app.tabs.focus.is_some(),
            "Keep the focus request while the old modal layer blocks input"
        );
        let _ = ctx.run(frame(0.2), |ctx| app.editor(ctx));
        assert!(app.tabs.focus.is_none());
        let mut input = frame(0.3);
        input
            .events
            .push(egui::Event::Text("typing without an extra click".into()));
        let _ = ctx.run(input, |ctx| app.editor(ctx));
        assert_eq!(app.documents[0].text, "typing without an extra click");
    }
}
