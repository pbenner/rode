//! Shared native widgets for every executable panel provider.
use super::table::render_table;
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
        if state.view.content.is_empty() {
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
        }
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
                if !state.view.content.is_empty() {
                    render_content(
                        ui,
                        state,
                        &state.view.content.clone(),
                        palette,
                        &mut chosen,
                        available_height,
                    );
                    return;
                }
                let content_top = ui.cursor().top();
                let _ = render_metrics(ui, &state.id, &state.view, palette);
                if state.settings_open
                    || state.view.metrics.is_empty() && state.view.charts.is_empty()
                {
                    render_fields(ui, state, false, &mut chosen, None);
                }
                for section in state.view.sections.clone() {
                    let _ = render_section(ui, state, &section, palette, &mut chosen);
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
                    render_chart(
                        ui,
                        &state.id,
                        &mut state.chart_views,
                        plot,
                        palette,
                        available_height - (ui.cursor().top() - content_top),
                    );
                }
                for link in &state.view.links {
                    let _ = render_link(ui, &state.id, link);
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

fn render_chart(
    ui: &mut egui::Ui,
    panel_id: &str,
    chart_views: &mut BTreeMap<String, chart::View>,
    plot: &protocol::Chart,
    palette: theme::Palette,
    available_height: f32,
) {
    // The entire chart, including its toolbar, keeps its identity when moved
    // between sections. Provider identity isolates equal chart keys in panels.
    ui.scope_builder(
        egui::UiBuilder::new().id(("extension-chart-root", panel_id, &plot.key)),
        |ui| {
            ui.add_space(12.);
            ui.strong(&plot.title);
            ui.horizontal_wrapped(|ui| {
                ui.weak(&plot.subtitle);
                if let Some(last) = plot.series.bars.last() {
                    ui.weak(format!("· Last bar: {} UTC", chart::date(last.at, true)));
                }
            });
            let view = chart_views.entry(plot.key.clone()).or_default();
            ui.horizontal_wrapped(|ui| {
                for (name, label) in [
                    ("out", "−"),
                    ("in", "+"),
                    ("left", "←"),
                    ("right", "→"),
                    ("reset", "Reset view"),
                ] {
                    let button = keyboard::button(ui, label);
                    ui_probe::button(ui, &format!("extension-chart:{}:{name}", plot.key), &button);
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
            let height = available_height.clamp(180., 520.);
            chart::show(ui, plot, view, palette, height);
        },
    );
}

fn render_link(ui: &mut egui::Ui, panel_id: &str, link: &protocol::Link) -> egui::Response {
    if link.key.is_empty() {
        return ui.hyperlink_to(&link.label, &link.url);
    }
    ui.scope_builder(
        egui::UiBuilder::new().id(("extension-link", panel_id, &link.key)),
        |ui| ui.hyperlink_to(&link.label, &link.url),
    )
    .inner
}

fn render_content(
    ui: &mut egui::Ui,
    state: &mut Instance,
    content: &[protocol::Content],
    palette: theme::Palette,
    chosen: &mut Option<String>,
    height: f32,
) {
    use protocol::Content;
    for item in content {
        match item {
            Content::Fields { keys } => render_fields(ui, state, false, chosen, Some(keys)),
            Content::Actions { keys } => {
                ui.horizontal_wrapped(|ui| {
                    for key in keys {
                        if let Some(action) = state.view.actions.iter().find(|a| &a.id == key) {
                            ui.scope_builder(
                                egui::UiBuilder::new().id((
                                    "extension-action",
                                    &state.id,
                                    &action.id,
                                )),
                                |ui| action_button(ui, action, state.ready(), palette, chosen),
                            );
                        }
                    }
                });
            }
            Content::Metrics { keys } => {
                let view = View {
                    layout: state.view.layout,
                    metrics: keys
                        .iter()
                        .filter_map(|key| {
                            state.view.metrics.iter().find(|m| &m.key == key).cloned()
                        })
                        .collect(),
                    ..Default::default()
                };
                let _ = render_metrics(ui, &state.id, &view, palette);
            }
            Content::Table { key } => {
                if let Some((index, table)) = state
                    .view
                    .tables
                    .iter()
                    .enumerate()
                    .find(|(_, t)| &t.key == key)
                {
                    render_table(ui, (&state.id, index), table);
                }
            }
            Content::Chart { key } => {
                if let Some(plot) = state.view.charts.iter().find(|c| &c.key == key) {
                    render_chart(ui, &state.id, &mut state.chart_views, plot, palette, height);
                }
            }
            Content::Link { key } => {
                if let Some(link) = state.view.links.iter().find(|l| &l.key == key) {
                    let _ = render_link(ui, &state.id, link);
                }
            }
            Content::ActionGroup { key } => {
                if let Some(group) = state.view.action_groups.iter().find(|g| &g.key == key) {
                    ui.horizontal_wrapped(|ui| {
                        ui.weak(&group.title);
                        for action in &group.actions {
                            ui.scope_builder(
                                egui::UiBuilder::new().id((
                                    "extension-action",
                                    &state.id,
                                    &action.id,
                                )),
                                |ui| action_button(ui, action, state.ready(), palette, chosen),
                            );
                        }
                    });
                }
            }
            Content::Section { key } => {
                if let Some(section) = state.view.sections.iter().find(|s| &s.key == key).cloned() {
                    let _ = render_section(ui, state, &section, palette, chosen);
                }
            }
        }
    }
}

/// Preferences are upper bounds; narrow panels always wrap without overflow.
fn metric_columns(width: f32, count: usize, preferred: Option<u8>) -> usize {
    ((width / 260.).floor() as usize)
        .max(1)
        .min(usize::from(preferred.unwrap_or(4).clamp(1, 4)))
        .min(count.max(1))
}

fn render_metrics(
    ui: &mut egui::Ui,
    id: &str,
    view: &View,
    palette: theme::Palette,
) -> Vec<egui::Rect> {
    let layout = view.layout.unwrap_or_default();
    let compact = layout.metric_density == protocol::MetricDensity::Compact
        || (layout.metric_density == protocol::MetricDensity::Auto && !view.charts.is_empty());
    let has_progress = view.metrics.iter().any(|metric| metric.progress.is_some());
    let columns = metric_columns(
        ui.available_width(),
        view.metrics.len(),
        layout.metric_columns,
    );
    let width = ((ui.available_width() - ui.spacing().item_spacing.x * (columns - 1) as f32)
        / columns as f32)
        .clamp(
            1.,
            if layout.metric_columns.is_some() {
                f32::INFINITY
            } else {
                360.
            },
        );
    let text_width = (width - layout::card_frame(palette).total_margin().sum().x).max(1.);
    let value_font = egui::FontId::proportional(if compact { 20. } else { 24. });
    let label_height = view
        .metrics
        .iter()
        .map(|m| {
            egui::WidgetText::from(RichText::new(&m.label).color(palette.muted))
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Wrap),
                    text_width,
                    egui::TextStyle::Body,
                )
                .size()
                .y
        })
        .fold(0_f32, f32::max);
    // RichText::strong selects a potentially different font; measure that galley too.
    let value_height = view
        .metrics
        .iter()
        .map(|m| {
            egui::WidgetText::from(RichText::new(&m.value).font(value_font.clone()).strong())
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Wrap),
                    text_width,
                    egui::TextStyle::Body,
                )
                .size()
                .y
        })
        .fold(0_f32, f32::max);
    let mut cards = vec![];
    for row in view.metrics.chunks(columns) {
        ui.horizontal_top(|ui| {
            for metric in row {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.),
                    Layout::top_down(Align::Min),
                    |ui| {
                        let frame = layout::card_frame(palette).show(ui, |ui| {
                            ui.set_width(text_width);
                            if compact {
                                ui.spacing_mut().item_spacing.y =
                                    ui.spacing().item_spacing.y.min(4.);
                            }

                            ui.set_min_height(if compact { 48. } else { 72. });
                            ui.allocate_ui_with_layout(
                                egui::vec2(text_width, label_height),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    ui.set_min_height(label_height);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&metric.label).color(palette.muted),
                                        )
                                        .wrap(),
                                    );
                                },
                            );
                            ui.allocate_ui_with_layout(
                                egui::vec2(text_width, value_height),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    ui.set_min_height(value_height);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&metric.value)
                                                .font(value_font.clone())
                                                .strong(),
                                        )
                                        .wrap(),
                                    );
                                },
                            );
                            let progress_height = if compact {
                                (ui.spacing().interact_size.y * 0.6).max(16.)
                            } else {
                                ui.spacing().interact_size.y
                            };
                            if let Some(progress) = metric.progress {
                                ui.add(
                                    egui::ProgressBar::new(progress)
                                        .desired_height(progress_height)
                                        .show_percentage(),
                                );
                            } else if has_progress {
                                ui.allocate_space(egui::vec2(0., progress_height));
                            }
                        });
                        ui_probe::button(
                            ui,
                            &format!(
                                "extension-metric:{id}:{}",
                                if metric.key.is_empty() {
                                    cards.len().to_string()
                                } else {
                                    metric.key.clone()
                                }
                            ),
                            &frame.response,
                        );
                        cards.push(frame.response.rect);
                    },
                );
            }
        });
    }
    cards
}

fn section_body(
    ui: &mut egui::Ui,
    state: &mut Instance,
    section: &protocol::Section,
    palette: theme::Palette,
    chosen: &mut Option<String>,
) {
    if !section.description.is_empty() {
        ui.add(egui::Label::new(RichText::new(&section.description).color(palette.muted)).wrap());
    }
    if !section.content.is_empty() {
        render_content(ui, state, &section.content, palette, chosen, 300.);
        return;
    }
    for index in &section.tables {
        render_table(ui, (&state.id, *index), &state.view.tables[*index]);
    }
    render_fields(ui, state, false, chosen, Some(&section.fields));
    ui.horizontal_wrapped(|ui| {
        for id in &section.actions {
            if let Some(action) = state.view.actions.iter().find(|a| &a.id == id) {
                action_button(ui, action, state.ready(), palette, chosen);
            }
        }
    });
}

fn render_section(
    ui: &mut egui::Ui,
    state: &mut Instance,
    section: &protocol::Section,
    palette: theme::Palette,
    chosen: &mut Option<String>,
) -> Option<egui::CollapsingResponse<()>> {
    if !section.key.is_empty() {
        let builder =
            egui::UiBuilder::new().id(("extension-section-root", &state.id, &section.key));
        return ui
            .scope_builder(builder, |ui| {
                render_section_inner(ui, state, section, palette, chosen)
            })
            .inner;
    }
    render_section_inner(ui, state, section, palette, chosen)
}
fn render_section_inner(
    ui: &mut egui::Ui,
    state: &mut Instance,
    section: &protocol::Section,
    palette: theme::Palette,
    chosen: &mut Option<String>,
) -> Option<egui::CollapsingResponse<()>> {
    ui.add_space(12.);
    layout::card_frame(palette)
        .show(ui, |ui| {
            ui.set_width((ui.available_width() - 1.).max(0.));
            if section.collapsible {
                let header = keyboard::CollapsingHeader::new(
                    RichText::new(&section.title).size(18.).strong(),
                )
                .id_salt(("extension-section", &state.id, &section.key))
                .default_open(section.default_open.unwrap_or(true))
                .show_unindented(ui, |ui| section_body(ui, state, section, palette, chosen));
                ui_probe::button(
                    ui,
                    &format!("extension-section:{}", section.key),
                    &header.header_response,
                );
                Some(header)
            } else {
                ui.label(RichText::new(&section.title).size(18.).strong());
                section_body(ui, state, section, palette, chosen);
                None
            }
        })
        .inner
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
    let fields: Vec<_> = if let Some(keys) = section {
        keys.iter()
            .filter_map(|key| state.view.fields.iter().find(|field| &field.key == key))
            .collect()
    } else {
        state
            .view
            .fields
            .iter()
            .filter(|field| {
                field.inline == inline
                    && !state
                        .view
                        .sections
                        .iter()
                        .any(|section| section.fields.contains(&field.key))
            })
            .collect()
    };
    for field in fields {
        let builder = if state.view.content.is_empty() {
            egui::UiBuilder::new().id_salt((&state.id, &field.key))
        } else {
            egui::UiBuilder::new().id(("extension-field", &state.id, &field.key))
        };
        ui.scope_builder(builder, |ui| {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keyed_links_keep_focus_across_containers_and_do_not_alias_other_panels() {
        let ctx = egui::Context::default();
        let mut link = protocol::Link {
            key: "docs".into(),
            label: "Documentation".into(),
            url: "https://example.com".into(),
        };
        let mut first_id = None;
        for frame in 0..3 {
            link.label = format!("Documentation {frame}");
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui
                        .push_id(format!("container-{frame}"), |ui| {
                            render_link(ui, "first", &link)
                        })
                        .inner;
                    let other = render_link(ui, "second", &link);
                    assert_ne!(response.id, other.id);
                    if let Some(id) = first_id {
                        assert_eq!(response.id, id);
                        assert!(response.has_focus());
                        assert!(!other.has_focus());
                    } else {
                        first_id = Some(response.id);
                        response.request_focus();
                    }
                });
            });
        }
    }
    #[test]
    fn chart_focus_and_zoom_survive_reparenting_and_stay_isolated_between_panels() {
        use protocol::Content;
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chart-identity.json");
        ui_probe::install_at(&ctx, path.clone());
        let bars: Vec<_> = (0..128).map(|at| serde_json::json!({"at":at,"open":10,"high":10,"low":10,"close":10,"volume":0})).collect();
        let plot: protocol::Chart = serde_json::from_value(serde_json::json!({
            "key":"latency", "title":"Latency", "series":{"name":"Service", "currency":"ms", "exchange":"local", "bars":bars}
        })).unwrap();
        let mut first = Instance {
            id: "first-panel".into(),
            view: View {
                title: "Service".into(),
                charts: vec![plot.clone()],
                sections: vec![protocol::Section {
                    key: "details".into(),
                    title: "Details".into(),
                    collapsible: true,
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut second = Instance {
            id: "second-panel".into(),
            view: View {
                title: "Another service".into(),
                charts: vec![plot],
                content: vec![Content::Chart {
                    key: "latency".into(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let mut initial = chart::View::default();
        initial.zoom(0.5, 0.5, 128);
        first.chart_views.insert("latency".into(), initial);
        for frame in 0..4 {
            let chart = Content::Chart {
                key: "latency".into(),
            };
            let section = Content::Section {
                key: "details".into(),
            };
            first.view.sections[0].content.clear();
            if matches!(frame, 1 | 2) {
                first.view.content = vec![section];
                first.view.sections[0].content = vec![chart];
            } else {
                first.view.content = vec![chart, section];
            }
            if frame == 2 {
                first.view.charts[0].title = "Renamed latency".into();
                first.view.sections[0].title = "Renamed details".into();
            }
            first.view.validate_version(3).unwrap();
            second.view.validate_version(3).unwrap();
            let before = first.chart_views["latency"];
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 1400.),
                    )),
                    events: if frame == 1 {
                        vec![egui::Event::Key {
                            key: Key::Plus,
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
                        if frame == 0 {
                            ui.memory_mut(|memory| {
                                memory.request_focus(
                                    egui::Id::new((
                                        "extension-chart-root",
                                        "first-panel",
                                        "latency",
                                    ))
                                    .with(("extension-chart", "latency")),
                                )
                            });
                        }
                        let content = first.view.content.clone();
                        ui.push_id(format!("changed-container-{frame}"), |ui| {
                            render_content(
                                ui,
                                &mut first,
                                &content,
                                theme::Palette::default(),
                                &mut None,
                                300.,
                            )
                        });
                        let content = second.view.content.clone();
                        render_content(
                            ui,
                            &mut second,
                            &content,
                            theme::Palette::default(),
                            &mut None,
                            300.,
                        );
                    });
                },
            );
            if frame == 1 {
                assert_ne!(
                    first.chart_views["latency"], before,
                    "Keyboard zoom lost focus after moving into a section"
                );
            } else {
                assert_eq!(first.chart_views["latency"], before);
            }
            assert_eq!(second.chart_views["latency"], chart::View::default());
            let snapshot: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let plots: Vec<_> = snapshot["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|button| button["name"] == "extension-chart:latency")
                .collect();
            assert_eq!(plots.len(), 2);
            assert_eq!(
                plots[0]["focused"], true,
                "Chart lost focus in frame {frame}"
            );
            assert_eq!(
                plots[1]["focused"], false,
                "Another provider inherited chart focus"
            );
        }
    }
    #[test]
    fn ordered_content_interleaves_native_controls_and_preserves_drafts() {
        let ctx = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("composition.json");
        ui_probe::install_at(&ctx, path.clone());
        let mut state = Instance {
            id: "test".into(),
            ..Default::default()
        };
        state.view = serde_json::from_value(serde_json::json!({
            "title":"Service",
            "metrics":[{"key":"uptime","label":"Uptime","value":"99.9%"}],
            "fields":[{"key":"region","label":"Region","value":"EU"}],
            "tables":[{"key":"events","title":"Events","columns":["Event"],"rows":[]}],
            "actions":[{"id":"refresh-data","label":"Refresh","enabled":true}],
            "sections":[{"key":"details","title":"Details","content":[{"kind":"fields","keys":["region"]},{"kind":"table","key":"events"},{"kind":"actions","keys":["refresh-data"]}]}],
            "content":[{"kind":"metrics","keys":["uptime"]},{"kind":"section","key":"details"}]
        })).unwrap();
        state.view.validate_version(3).unwrap();
        state.values.insert("region".into(), "US".into());
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800., 1000.),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let content = state.view.content.clone();
                    render_content(
                        ui,
                        &mut state,
                        &content,
                        theme::Palette::default(),
                        &mut None,
                        800.,
                    );
                });
            },
        );
        assert_eq!(state.values["region"], "US");
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let controls = snapshot["buttons"].as_array().unwrap();
        let y = |key| {
            controls.iter().find(|c| c["name"] == key).unwrap()["rect"][1]
                .as_f64()
                .unwrap()
        };
        assert!(y("extension-metric:test:uptime") < y("extension-field:region"));
        assert!(y("extension-field:region") < y("extension-table:test:0"));
        assert!(y("extension-table:test:0") < y("extension-action:refresh-data"));
    }
    #[test]
    fn metric_preferences_wrap_responsively_and_preserve_equal_heights() {
        let mut view = View {
            metrics: (0..4)
                .map(|index| protocol::Metric {
                    key: String::new(),
                    label: format!("Metric {index}"),
                    value: "8.32 h".into(),
                    progress: (index == 1).then_some(0.4),
                })
                .collect(),
            ..Default::default()
        };
        let render = |view: &View, width, large_fonts| {
            let ctx = egui::Context::default();
            if large_fonts {
                ctx.style_mut(|style| {
                    style.override_font_id = Some(egui::FontId::proportional(28.))
                });
            }
            let mut cards = vec![];
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 700.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        cards = render_metrics(ui, "test", view, theme::Palette::default());
                    });
                },
            );
            assert_eq!(cards.len(), 4);
            assert!(
                cards
                    .iter()
                    .all(|r| (r.height() - cards[0].height()).abs() < 0.5),
                "Unequal metric cards at width {width}: {cards:?}"
            );
            assert!(cards.iter().all(|r| r.right() <= width));
            cards
        };
        view.layout = Some(protocol::PanelLayout {
            metric_columns: Some(4),
            metric_density: protocol::MetricDensity::Comfortable,
        });
        let comfortable = render(&view, 1300., false);
        view.layout.as_mut().unwrap().metric_density = protocol::MetricDensity::Compact;
        let compact = render(&view, 1300., false);
        assert!(compact[0].height() < comfortable[0].height());
        assert!(compact.iter().all(|r| r.top() == compact[0].top()));
        view.metrics[0].label =
            "A localized label with enough words to wrap onto several lines in a narrow panel"
                .into();
        view.metrics[2].value = "A long formatted value that also wraps".into();
        let narrow = render(&view, 780., false);
        let _ = render(&view, 780., true);
        assert_eq!(narrow[0].top(), narrow[1].top());
        assert!(narrow[2].top() > narrow[0].top());
        view.layout.as_mut().unwrap().metric_columns = Some(1);
        let single = render(&view, 1300., false);
        assert!(single.windows(2).all(|rows| rows[1].top() > rows[0].top()));
    }

    #[test]
    fn disclosure_state_survives_replies_and_title_changes_without_losing_drafts() {
        let ctx = egui::Context::default();
        ctx.style_mut(|style| style.animation_time = 0.);
        let mut state = Instance {
            id: "test".into(),
            ..Default::default()
        };
        state.view.fields.push(protocol::Field {
            key: "weight".into(),
            label: "Weight".into(),
            value: "70".into(),
            ..Default::default()
        });
        let mut section = protocol::Section {
            key: "details".into(),
            title: "Details".into(),
            collapsible: true,
            default_open: Some(false),
            fields: vec!["weight".into()],
            ..Default::default()
        };
        let mut rect = egui::Rect::NOTHING;
        for frame in 0..5 {
            if frame == 1 {
                section.default_open = Some(true);
            }
            if frame == 4 {
                section.title = "Updated details".into();
                section.default_open = Some(false);
                state.values.insert("weight".into(), "85".into());
            }
            let mut visible = false;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600., 400.),
                    )),
                    events: if matches!(frame, 2 | 3) {
                        vec![
                            egui::Event::PointerMoved(rect.center()),
                            egui::Event::PointerButton {
                                pos: rect.center(),
                                button: egui::PointerButton::Primary,
                                pressed: frame == 2,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let response = ui
                            .push_id(format!("new-container-{frame}"), |ui| {
                                render_section(
                                    ui,
                                    &mut state,
                                    &section,
                                    theme::Palette::default(),
                                    &mut None,
                                )
                            })
                            .inner
                            .unwrap();
                        rect = response.header_response.rect;
                        visible = response.body_response.is_some();
                    });
                },
            );
            assert_eq!(visible, frame >= 3, "frame {frame}");
            if frame == 4 {
                assert_eq!(state.values["weight"], "85");
            }
        }
    }
    #[test]
    fn section_inputs_follow_the_providers_declared_order() {
        let ctx = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fields.json");
        ui_probe::install_at(&ctx, path.clone());
        let mut state = Instance::default();
        for key in ["a", "b"] {
            state.view.fields.push(protocol::Field {
                key: key.into(),
                label: key.into(),
                ..Default::default()
            });
        }
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_fields(
                    ui,
                    &mut state,
                    false,
                    &mut None,
                    Some(&["b".into(), "a".into()]),
                );
            });
        });
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let controls = snapshot["buttons"].as_array().unwrap();
        let y = |key| {
            controls.iter().find(|c| c["name"] == key).unwrap()["rect"][1]
                .as_f64()
                .unwrap()
        };
        assert!(y("extension-field:b") < y("extension-field:a"));
    }
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
        assert_table_scroll_survives_layout_changes(false);
    }
    #[test]
    fn keyed_tables_keep_scroll_and_focus_when_storage_order_and_title_change() {
        assert_table_scroll_survives_layout_changes(true);
    }
    fn assert_table_scroll_survives_layout_changes(keyed: bool) {
        let ctx = egui::Context::default();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("table.json");
        ui_probe::install_at(&ctx, path.clone());
        let mut table = protocol::Table {
            key: if keyed {
                "events".into()
            } else {
                String::new()
            },
            title: "Data".into(),
            columns: vec!["Column".into()],
            rows: vec![vec!["Value".into()]; 512],
        };
        for frame in 0..5 {
            let index = usize::from(keyed && frame >= 3);
            if keyed && frame >= 3 {
                table.title = "Renamed events".into();
            }
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
                            let (scope, identity) = if keyed {
                                (
                                    egui::Id::new(("extension-table", "test", "events")),
                                    "key:events",
                                )
                            } else {
                                (ui.id(), "legacy:0")
                            };
                            ui.memory_mut(|m| {
                                m.request_focus(scope.with((
                                    "extension-table-focus",
                                    ("test", identity.to_string()),
                                )))
                            });
                        }
                        if keyed && frame >= 3 {
                            ui.push_id("moved-to-another-section", |ui| {
                                render_table(ui, ("test", index), &table)
                            });
                        } else {
                            render_table(ui, ("test", index), &table);
                        }
                    });
                },
            );
            if frame >= 2 {
                let snapshot: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                let expected = format!(
                    "extension-table-position:test:{index}:{}",
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
            key: String::new(),
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
