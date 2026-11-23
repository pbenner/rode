//! Shared native widgets for every executable panel provider.
use super::*;
impl EditorApp {
    pub(in crate::app) fn extension_panel_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let id = self.extension_panels.selected.clone();
        self.extension_panel_ui_for(&id, ui, ctx);
    }
    pub(in crate::app) fn extension_panel_ui_for(
        &mut self,
        id: &str,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) {
        let palette = self.themes.active().palette;
        let Some(state) = self.extension_panels.instances.get_mut(id) else {
            return;
        };
        let title = if state.view.title.is_empty() {
            state
                .provider
                .as_ref()
                .map(|p| p.name.as_str())
                .unwrap_or("Extension panel")
        } else {
            &state.view.title
        };
        layout::heading(ui, title, &state.view.status, palette);
        if !state.error.is_empty() {
            ui.colored_label(palette.error, &state.error);
        }
        if state.busy.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Working…");
            });
        }
        let mut chosen = None;
        ui.horizontal_wrapped(|ui| {
            render_fields(ui, state, true, &mut chosen, None);
            if state.view.fields.iter().any(|f| {
                !f.inline
                    && !state
                        .view
                        .sections
                        .iter()
                        .any(|s| s.fields.contains(&f.key))
            }) {
                let settings = keyboard::button(ui, "Settings");
                ui_probe::button(ui, "extension-settings", &settings);
                if settings.clicked() {
                    state.settings_open = !state.settings_open;
                }
            }
            for action in state.view.actions.iter().filter(|a| {
                !state
                    .view
                    .sections
                    .iter()
                    .any(|s| s.actions.contains(&a.id))
            }) {
                action_button(ui, action, state.ready(), palette, &mut chosen);
            }
        });
        if !state.view.message.is_empty() {
            ui.add(
                egui::Label::new(
                    RichText::new(&state.view.message).color(if state.view.error {
                        palette.error
                    } else {
                        palette.muted
                    }),
                )
                .wrap(),
            );
        }
        let available_height = ui.available_height();
        egui::ScrollArea::vertical()
            .id_salt(("extension-panel", &state.id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let content_top = ui.cursor().top();
                let chart_metrics = !state.view.charts.is_empty();
                let columns = ((ui.available_width() / 260.).floor() as usize)
                    .clamp(1, state.view.metrics.len().clamp(1, 4));
                let width = ((ui.available_width()
                    - ui.spacing().item_spacing.x * (columns - 1) as f32)
                    / columns as f32)
                    .clamp(1., 360.);
                for row in state.view.metrics.chunks(columns) {
                    ui.horizontal_top(|ui| {
                        for metric in row {
                            ui.allocate_ui_with_layout(
                                egui::vec2(width, 0.),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    layout::card_frame(palette).show(ui, |ui| {
                                        ui.set_min_width((width - 32.).max(0.));
                                        if chart_metrics {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.weak(&metric.label);
                                                ui.label(
                                                    RichText::new(&metric.value).size(20.).strong(),
                                                );
                                            });
                                        } else {
                                            ui.set_min_height(72.);
                                            ui.weak(&metric.label);
                                            ui.label(
                                                RichText::new(&metric.value).size(24.).strong(),
                                            );
                                        }
                                        if let Some(progress) = metric.progress {
                                            ui.add(
                                                egui::ProgressBar::new(progress).show_percentage(),
                                            );
                                        }
                                    });
                                },
                            );
                        }
                    });
                }
                if state.settings_open
                    || state.view.metrics.is_empty() && state.view.charts.is_empty()
                {
                    render_fields(ui, state, false, &mut chosen, None);
                }
                for section in state.view.sections.clone() {
                    ui.add_space(12.);
                    layout::card_frame(palette).show(ui, |ui| {
                        ui.set_width((ui.available_width() - 1.).max(0.));
                        ui.label(RichText::new(&section.title).size(18.).strong());
                        if !section.description.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&section.description).color(palette.muted),
                                )
                                .wrap(),
                            );
                        }
                        for index in &section.tables {
                            render_table(ui, (&state.id, *index), &state.view.tables[*index]);
                        }
                        render_fields(ui, state, false, &mut chosen, Some(&section.fields));
                        ui.horizontal_wrapped(|ui| {
                            for id in &section.actions {
                                if let Some(action) =
                                    state.view.actions.iter().find(|a| &a.id == id)
                                {
                                    action_button(ui, action, state.ready(), palette, &mut chosen);
                                }
                            }
                        });
                    });
                }
                for group in &state.view.action_groups {
                    ui.add_space(6.);
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(&group.title);
                        for action in &group.actions {
                            action_button(ui, action, state.ready(), palette, &mut chosen);
                        }
                    });
                }
                for (index, table) in state.view.tables.iter().enumerate() {
                    if !state
                        .view
                        .sections
                        .iter()
                        .any(|s| s.tables.contains(&index))
                    {
                        render_table(ui, (&state.id, index), table);
                    }
                }
                for plot in &state.view.charts {
                    ui.add_space(12.);
                    ui.strong(&plot.title);
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(&plot.subtitle);
                        if let Some(last) = plot.series.bars.last() {
                            ui.weak(format!("· Last bar: {} UTC", chart::date(last.at, true)));
                        }
                    });
                    let view = state.chart_views.entry(plot.key.clone()).or_default();
                    ui.horizontal_wrapped(|ui| {
                        for (name, label) in [
                            ("out", "−"),
                            ("in", "+"),
                            ("left", "←"),
                            ("right", "→"),
                            ("reset", "Reset view"),
                        ] {
                            let button = keyboard::button(ui, label);
                            ui_probe::button(
                                ui,
                                &format!("extension-chart:{}:{name}", plot.key),
                                &button,
                            );
                            if button.clicked() {
                                match name {
                                    "out" => view.zoom(1.4, 0.5, plot.series.bars.len()),
                                    "in" => view.zoom(1. / 1.4, 0.5, plot.series.bars.len()),
                                    "left" => view.pan(-0.2),
                                    "right" => view.pan(0.2),
                                    _ => *view = Default::default(),
                                }
                            }
                        }
                        ui.weak("Wheel: zoom · Drag: pan · Focus chart: + / −, ← / →, Home");
                    });
                    let height = (available_height - (ui.cursor().top() - content_top) - 8.)
                        .clamp(180., 520.);
                    chart::show(ui, plot, view, palette, height);
                }
                for link in &state.view.links {
                    ui.hyperlink_to(&link.label, &link.url);
                }
            });
        state.sync_settings(
            self.extensions
                .registry
                .preferences
                .get(&state.id)
                .map(|p| &p.settings),
        );
        if let Some(id) = chosen.take() {
            if let Err(error) = state.activate(&id) {
                state.error = error.to_string();
            }
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        let mut confirmed = false;
        let was_confirming = state.confirm.is_some();
        if let Some(action) = state.confirm.as_ref().map(|c| c.action.clone()) {
            let response = egui::Modal::new(egui::Id::new("extension-confirm")).show(ctx, |ui| {
                ui.set_max_width(480.);
                ui.heading(&action.label);
                ui.add(egui::Label::new(action.confirm.as_deref().unwrap_or_default()).wrap());
                ui.horizontal(|ui| {
                    let cancel = keyboard::button(ui, "Cancel");
                    ui_probe::button(ui, "extension-confirm-cancel", &cancel);
                    if cancel.clicked() {
                        state.confirm = None;
                    }
                    let confirm = keyboard::button(ui, "Confirm");
                    ui_probe::button(ui, "extension-confirm-submit", &confirm);
                    if confirm.clicked() {
                        keyboard::consume_activation(&confirm);
                        confirmed = true;
                    }
                });
            });
            if response.should_close() {
                state.confirm = None;
            }
        }
        if confirmed && let Err(error) = state.confirm_action() {
            state.error = error.to_string();
        }
        if confirmed || was_confirming != state.confirm.is_some() {
            // A detached Cancel/Escape must also resume root-owned scheduling.
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }
}

/// Inputs and choice fields share focus registration, Enter submission and
/// value probes. They remain editable while a provider is doing I/O.
fn render_fields(
    ui: &mut egui::Ui,
    state: &mut Instance,
    inline: bool,
    chosen: &mut Option<String>,
    section: Option<&[String]>,
) {
    let ready = state.ready() && state.confirm.is_none() && ui.is_enabled();
    for field in state.view.fields.iter().filter(|f| {
        if let Some(keys) = section {
            keys.contains(&f.key)
        } else {
            f.inline == inline
                && !state
                    .view
                    .sections
                    .iter()
                    .any(|s| s.fields.contains(&f.key))
        }
    }) {
        ui.push_id((&state.id, &field.key), |ui| {
            if !inline {
                ui.label(&field.label);
            }
            let value = state
                .values
                .entry(field.key.clone())
                .or_insert_with(|| field.value.clone());
            let input_id = ui.id().with("input");
            let submit = field.submit.is_some()
                && !field.multiline
                && ready
                && ui.memory(|memory| memory.has_focus(input_id))
                && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
            let response = if field.choices.is_empty() {
                let input = if field.multiline {
                    egui::TextEdit::multiline(value)
                } else {
                    egui::TextEdit::singleline(value)
                };
                ui.add(keyboard::named(
                    &field.label,
                    input
                        .id(input_id)
                        .hint_text(&field.label)
                        .desired_width(if inline {
                            260_f32.min(ui.available_width())
                        } else {
                            f32::INFINITY
                        })
                        .char_limit(2048),
                ))
            } else {
                let selected = field
                    .choices
                    .iter()
                    .find(|c| c.value == *value)
                    .map(|c| c.label.as_str())
                    .unwrap_or(value);
                let response = egui::ComboBox::from_id_salt("choice")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for choice in &field.choices {
                            ui.selectable_value(value, choice.value.clone(), &choice.label);
                        }
                    })
                    .response;
                keyboard::field_target(ui, &field.label, &response);
                response
            };
            ui_probe::button(ui, &format!("extension-field:{}", field.key), &response);
            ui_probe::value(
                ui,
                &format!("extension-field:{}-value", field.key),
                &response,
                value,
            );
            if submit {
                ui.memory_mut(|m| {
                    m.move_focus(egui::FocusDirection::None);
                    m.request_focus(input_id);
                });
                *chosen = field.submit.clone();
            }
            ui.add_space(6.);
        });
    }
}
fn action_button(
    ui: &mut egui::Ui,
    action: &Action,
    ready: bool,
    palette: theme::Palette,
    chosen: &mut Option<String>,
) {
    let mut button = keyboard::Button::new(&action.label).selected(action.selected);
    if action.primary {
        button = keyboard::Button::new(
            RichText::new(&action.label)
                .color(palette.background)
                .strong(),
        )
        .fill(palette.accent);
    }
    let response = ui.add_enabled(action.enabled && ready, button.truncate());
    ui_probe::button(ui, &format!("extension-action:{}", action.id), &response);
    if response.clicked() {
        keyboard::consume_activation(&response);
        *chosen = Some(action.id.clone());
    }
}

/// Virtual rows bound native layout work even for the largest accepted table.
/// Index-based identities also distinguish tables with identical display titles.
fn render_table(ui: &mut egui::Ui, id: (&str, usize), table: &protocol::Table) -> usize {
    ui.add_space(12.);
    ui.strong(&table.title);
    let row_height = ui.text_style_height(&egui::TextStyle::Body).max(24.);
    let width = (ui.available_width() / table.columns.len().max(1) as f32).max(96.);
    let frame = egui::Frame::new().inner_margin(4).show(ui, |ui| {
        egui::ScrollArea::both()
            .id_salt(("extension-table-scroll", id))
            .max_height(320.)
            .auto_shrink([false, true])
            .show_rows(ui, row_height, table.rows.len() + 1, |ui, range| {
                egui::Grid::new(("extension-table", id))
                    .striped(true)
                    .start_row(range.start)
                    .spacing(ui.spacing().item_spacing)
                    // Match the virtual stride even when the theme's default
                    // button/grid height is larger than a read-only data row.
                    .min_row_height(row_height)
                    .num_columns(table.columns.len())
                    .show(ui, |ui| {
                        for index in range.clone() {
                            let header = index == 0;
                            let row = if header {
                                &table.columns
                            } else {
                                &table.rows[index - 1]
                            };
                            for cell in row {
                                let text = if header {
                                    RichText::new(cell).strong()
                                } else {
                                    RichText::new(cell)
                                };
                                ui.add_sized(
                                    [width - 8., row_height],
                                    egui::Label::new(text).truncate(),
                                )
                                .on_hover_text(cell);
                            }
                            ui.end_row();
                        }
                    });
                range.len()
            })
    });
    let mut output = frame.inner;
    let response = ui.interact(
        frame.response.rect,
        ui.id().with(("extension-table-focus", id)),
        Sense::focusable_noninteractive(),
    );
    keyboard::field_target(ui, &table.title, &response);
    ui_probe::button(ui, &format!("extension-table:{}:{}", id.0, id.1), &response);
    if response.contains_pointer() && ui.input(|i| i.pointer.primary_pressed()) {
        response.request_focus();
    }
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                ..Default::default()
            },
        )
    });
    let max_offset = (output.content_size - output.inner_rect.size()).max(egui::Vec2::ZERO);
    if response.has_focus() {
        let mut next = output.state.offset;
        ui.input_mut(|input| {
            for (key, delta) in [
                (Key::ArrowUp, egui::vec2(0., -row_height)),
                (Key::ArrowDown, egui::vec2(0., row_height)),
                (Key::ArrowLeft, egui::vec2(-48., 0.)),
                (Key::ArrowRight, egui::vec2(48., 0.)),
                (Key::PageUp, egui::vec2(0., -output.inner_rect.height())),
                (Key::PageDown, egui::vec2(0., output.inner_rect.height())),
            ] {
                if input.consume_key(egui::Modifiers::NONE, key) {
                    next += delta;
                }
            }
            if input.consume_key(egui::Modifiers::NONE, Key::Home) {
                next.y = 0.;
            }
            if input.consume_key(egui::Modifiers::NONE, Key::End) {
                next.y = max_offset.y;
            }
        });
        next = next.max(egui::Vec2::ZERO).min(max_offset);
        if next != output.state.offset {
            output.state.offset = next;
            output.state.store(ui.ctx(), output.id);
            ui.ctx().request_repaint();
        }
        ui.painter().rect_stroke(
            frame.response.rect.shrink(0.5),
            4.,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    ui_probe::value(
        ui,
        &format!("extension-table-position:{}:{}", id.0, id.1),
        &response,
        if output.state.offset.y >= max_offset.y - 0.5 {
            "end"
        } else if output.state.offset.y <= 0.5 {
            "start"
        } else {
            "middle"
        },
    );
    response.on_hover_text("Focus table: arrow keys, Page Up/Down, Home/End");
    output.inner
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn section_fields_stay_out_of_the_toolbar_and_keep_unsent_edits() {
        let ctx = egui::Context::default();
        let mut state = Instance::default();
        state.view.fields.push(protocol::Field {
            key: "weight".into(),
            label: "Research weight".into(),
            value: "70".into(),
            inline: true,
            ..Default::default()
        });
        let keys = vec!["weight".to_owned()];
        state.view.sections.push(protocol::Section {
            title: "Projects".into(),
            fields: keys.clone(),
            ..Default::default()
        });
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut chosen = None;
                render_fields(ui, &mut state, true, &mut chosen, None);
                assert!(!state.values.contains_key("weight"));
                state.values.insert("weight".into(), "85".into());
                render_fields(ui, &mut state, false, &mut chosen, Some(&keys));
                assert_eq!(state.values["weight"], "85");
            });
        });
    }
    #[test]
    fn end_scroll_stays_at_the_last_row_with_large_interface_controls() {
        let ctx = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("table.json");
        ui_probe::install_at(&ctx, path.clone());
        let table = protocol::Table {
            title: "Data".into(),
            columns: vec!["Column".into()],
            rows: vec![vec!["Value".into()]; 512],
        };
        for frame in 0..5 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800., 600.),
                    )),
                    events: if frame == 1 {
                        vec![egui::Event::Key {
                            key: Key::End,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.spacing_mut().interact_size.y = 40.;
                        if frame == 0 {
                            ui.memory_mut(|m| {
                                m.request_focus(
                                    ui.id().with(("extension-table-focus", ("test", 0usize))),
                                )
                            });
                        }
                        render_table(ui, ("test", 0), &table);
                    });
                },
            );
            if frame >= 2 {
                let snapshot: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                let expected = format!(
                    "extension-table-position:test:0:{}",
                    crate::files::digest(b"end")
                );
                assert!(
                    snapshot["buttons"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|b| b["name"] == expected),
                    "End scroll moved after layout stabilized"
                );
            }
        }
    }
    #[test]
    fn maximum_size_tables_only_lay_out_visible_rows() {
        let ctx = egui::Context::default();
        let table = protocol::Table {
            title: "Large report".into(),
            columns: vec!["Column".into(); 16],
            rows: vec![vec!["Value".into(); 16]; 4096],
        };
        let mut rendered = 0;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 600.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    rendered = render_table(ui, ("test.panel", 0), &table);
                });
            },
        );
        assert!(
            rendered > 0 && rendered < 40,
            "Layout expanded to {rendered} rows"
        );
    }
}
