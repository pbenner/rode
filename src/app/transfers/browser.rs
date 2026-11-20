//! Compact file panes. Shared directory rows retain keyboard navigation; secondary
//! actions live in one menu and the primary copy action stays in a fixed footer.
use super::*;

impl EditorApp {
    pub(super) fn transfer_pane_ui(&mut self, ui: &mut egui::Ui, side: usize, ctx: &egui::Context) {
        let palette = self.themes.active().palette;
        let generation = self.transfers.generation;
        let host_ready = self.transfer_host_endpoint().is_some();
        let can_transfer = host_ready && self.transfers.panes.iter().all(Pane::ready);
        if side == 1 && !host_ready {
            ui.disable();
        }
        let host = self.transfer_host_label();
        let mut browse = None;
        let mut create = None;
        let mut transfer = false;
        let mut folder_target = None;
        let mut list_rect = egui::Rect::NOTHING;
        ui.push_id(side, |ui| {
            let pane = &mut self.transfers.panes[side];
            let footer_height = layout::control_height(ui)
                * if ui.available_width() < 230. { 2. } else { 1. }
                + 10.;
            egui::TopBottomPanel::bottom(ui.id().with("copy-actions"))
                .frame(egui::Frame::new())
                .exact_height(footer_height)
                .show_inside(ui, |ui| {
                    ui.add_space(5.);
                    ui.horizontal_wrapped(|ui| {
                        let label = if side == 0 { "Upload" } else { "Download" };
                        let button = ui.add_enabled(
                            !pane.selected.is_empty() && can_transfer,
                            IconButton::new(
                                if side == 0 {
                                    Icon::ChevronRight
                                } else {
                                    Icon::ChevronLeft
                                },
                                label,
                            ),
                        );
                        ui_probe::button(
                            ui,
                            if side == 0 {
                                "transfer-upload"
                            } else {
                                "transfer-download"
                            },
                            &button,
                        );
                        transfer = button.clicked();
                        ui.weak(format!("{} selected", pane.selected.len()));
                    });
                });
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(20.), Sense::hover());
                (if side == 0 { Icon::Files } else { Icon::Remote }).paint(
                    ui.painter(),
                    rect,
                    palette.accent,
                );
                ui.strong(if side == 0 {
                    "Local files"
                } else {
                    "Remote files"
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let icon_id = ui.next_auto_id().with("folder-actions-icon");
                    let button =
                        egui::Button::new(egui::Atom::custom(icon_id, egui::Vec2::splat(16.)))
                            .min_size(egui::vec2(28., 26.));
                    let (response, _) = crate::keyboard::MenuButton::from_button(
                        button,
                        "Folder actions",
                    )
                    .ui(ui, |ui| {
                        if crate::keyboard::button(ui, "Home").clicked() {
                            browse = Some("~".into());
                            ui.close();
                        }
                        if crate::keyboard::button(ui, "Refresh").clicked() {
                            browse = Some(pane.browser.directory.clone());
                            ui.close();
                        }
                        pane_actions(ui, pane);
                    });
                    ui_probe::button(ui, &format!("transfer-actions:{side}"), &response);
                    Icon::More.paint(
                        ui.painter(),
                        egui::Rect::from_center_size(
                            response.rect.center(),
                            egui::Vec2::splat(16.),
                        ),
                        palette.muted,
                    );
                });
            });
            if ui.available_height() >= 270. {
                ui.add(
                    egui::Label::new(
                        RichText::new(if side == 0 { "This computer" } else { &host })
                            .small()
                            .color(palette.muted),
                    )
                    .truncate(),
                )
                .on_hover_text(&host);
            }
            ui.add_space(3.);
            ui.horizontal(|ui| {
                let up = ui.add_enabled(
                    !pane.browser.loading,
                    IconButton::toolbar(Icon::ChevronUp, "Parent folder"),
                );
                ui_probe::button(ui, &format!("transfer-Up:{side}"), &up);
                if up.clicked() {
                    browse = Some(parent(&pane.browser.directory));
                }
                let go_width = 28.;
                let path = ui.add_sized(
                    [
                        (ui.available_width() - go_width - ui.spacing().item_spacing.x).max(20.),
                        layout::control_height(ui),
                    ],
                    crate::keyboard::named(
                        "Folder path",
                        egui::TextEdit::singleline(&mut pane.browser.location)
                            .desired_width(f32::INFINITY),
                    ),
                );
                ui_probe::button(ui, &format!("transfer-path:{side}"), &path);
                ui_probe::value(
                    ui,
                    &format!("transfer-path:{side}-value"),
                    &path,
                    &pane.browser.location,
                );
                let go = ui.add_enabled(
                    !pane.browser.loading,
                    IconButton::toolbar(Icon::ChevronRight, "Go to folder"),
                );
                ui_probe::button(ui, &format!("transfer-Go:{side}"), &go);
                if go.clicked() || path.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    browse = Some(pane.browser.location.clone());
                }
            });
            let query = ui.add(crate::keyboard::named(
                "Filter files",
                egui::TextEdit::singleline(&mut pane.query)
                    .hint_text("Filter files…")
                    .desired_width(f32::INFINITY),
            ));
            ui_probe::button(ui, &format!("transfer-filter:{side}"), &query);
            ui_probe::value(
                ui,
                &format!("transfer-filter:{side}-value"),
                &query,
                &pane.query,
            );
            if pane.creating {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [
                            (ui.available_width() - 65.).max(20.),
                            layout::control_height(ui),
                        ],
                        crate::keyboard::named(
                            "New folder name",
                            egui::TextEdit::singleline(&mut pane.new_folder)
                                .hint_text("Folder name"),
                        ),
                    );
                    if ui
                        .add_enabled(
                            explorer::valid_name(&pane.new_folder) && pane.ready(),
                            crate::keyboard::Button::new("Create"),
                        )
                        .clicked()
                    {
                        create = Some(
                            std::path::Path::new(&pane.browser.directory)
                                .join(&pane.new_folder)
                                .to_string_lossy()
                                .into(),
                        );
                        pane.creating = false;
                        pane.new_folder.clear();
                    }
                });
            }
            if pane.browser.loading {
                ui.spinner();
            } else if pane.ready() {
                ui_probe::button(ui, &format!("transfer-ready:{side}"), &ui.response());
                ui_probe::button(
                    ui,
                    &format!("transfer-listing:{side}:{}", pane.browser.token),
                    &ui.response(),
                );
                ui_probe::value(
                    ui,
                    &format!("transfer-directory:{side}"),
                    &ui.response(),
                    &pane.browser.directory,
                );
            }
            if let Some(error) = &pane.browser.error {
                ui.colored_label(palette.error, error);
            }
            ui.add_space(3.);
            list_rect = ui.available_rect_before_wrap();
            let query = pane.query.to_lowercase();
            let entries: Vec<_> = pane
                .browser
                .entries
                .iter()
                .filter(|entry| entry.name.to_lowercase().contains(&query))
                .cloned()
                .collect();
            let ready = pane.ready();
            let selected = &pane.selected;
            let token = pane.browser.token;
            let directory = &pane.browser.directory;
            let mut on_row = |ui: &mut egui::Ui, entry: &Entry, response: &egui::Response| {
                if ready && can_transfer && response.drag_started() {
                    let mut paths = if selected.contains(&entry.path) {
                        selected.iter().cloned().collect::<Vec<_>>()
                    } else {
                        vec![entry.path.clone()]
                    };
                    paths.sort();
                    response.dnd_set_drag_payload(drag::Files {
                        side,
                        generation,
                        token,
                        directory: directory.clone(),
                        paths,
                    });
                }
                if entry.dir && response.contains_pointer() {
                    folder_target = Some(entry.path.clone());
                    if egui::DragAndDrop::has_payload_of_type::<drag::Files>(ctx)
                        || ui.input(|i| !i.raw.hovered_files.is_empty())
                    {
                        ui.painter().rect_stroke(
                            response.rect.shrink(1.),
                            5,
                            Stroke::new(1.5_f32, palette.accent),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
            };
            let choice = file_browser::rows_with_options(
                ui,
                &format!("transfer-rows:{side}"),
                &entries,
                pane.hidden,
                &mut pane.list,
                &pane.query,
                (0, false),
                None,
                ready,
                file_browser::RowOptions {
                    height: Some(list_rect.height().max(1.)),
                    checked: Some(&pane.selected),
                    icons: true,
                    on_row: Some(&mut on_row),
                },
            );
            match choice {
                Some(file_browser::Choice::Select(entry)) => {
                    if !pane.selected.insert(entry.path.clone()) {
                        pane.selected.remove(&entry.path);
                    }
                }
                Some(file_browser::Choice::Open(entry)) if entry.dir => browse = Some(entry.path),
                Some(file_browser::Choice::Open(entry)) => {
                    pane.selected.insert(entry.path);
                }
                Some(file_browser::Choice::Parent) => {
                    browse = Some(parent(&pane.browser.directory))
                }
                None => {}
            }
            if entries
                .iter()
                .all(|entry| !pane.hidden && entry.name.starts_with('.'))
                && ready
            {
                ui.painter().with_clip_rect(list_rect).text(
                    list_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    if pane.query.is_empty() {
                        "Drop files here"
                    } else {
                        "No matching files"
                    },
                    FontId::proportional(13.),
                    palette.muted,
                );
            }
        });
        self.transfer_drop_ui(ui, side, list_rect, folder_target, ctx);
        if let Some(path) = create {
            self.transfer_list(
                side,
                self.transfers.panes[side].browser.directory.clone(),
                Some(path),
                ctx,
            );
        }
        if let Some(path) = browse {
            self.transfer_list(side, path, None, ctx);
        }
        if transfer {
            self.queue_transfer(
                side,
                self.transfers.panes[side]
                    .selected
                    .iter()
                    .cloned()
                    .collect(),
                ctx,
            );
        }
    }
}

fn parent(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .unwrap_or(std::path::Path::new("/"))
        .to_string_lossy()
        .into()
}

fn pane_actions(ui: &mut egui::Ui, pane: &mut Pane) {
    crate::keyboard::checkbox(ui, &mut pane.hidden, "Show hidden files");
    ui.separator();
    for label in [
        "Select all",
        "Clear selection",
        "Select this folder",
        "New folder…",
    ] {
        if crate::keyboard::button(ui, label).clicked() {
            match label {
                "Select all" => {
                    pane.selected = pane
                        .browser
                        .entries
                        .iter()
                        .filter(|entry| {
                            (pane.hidden || !entry.name.starts_with('.'))
                                && entry
                                    .name
                                    .to_lowercase()
                                    .contains(&pane.query.to_lowercase())
                        })
                        .map(|entry| entry.path.clone())
                        .collect()
                }
                "Clear selection" => pane.selected.clear(),
                "Select this folder" if pane.ready() => {
                    pane.selected = HashSet::from([pane.browser.directory.clone()])
                }
                "New folder…" => pane.creating = !pane.creating,
                _ => {}
            }
            ui.close();
        }
    }
}
