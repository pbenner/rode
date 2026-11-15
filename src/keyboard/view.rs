//! Noninteractive hint painting and native control-search presentation.
use super::*;

/// Paint hint labels without adding pointer targets. Search owns a separate top
/// modal; its keyboard events are supplied only while its native controls render.
pub(crate) fn finish(ctx: &egui::Context, background: egui::Color32, foreground: egui::Color32) {
    let Some(catalog) = catalog(ctx) else {
        return;
    };
    let mode = {
        let mut windows = catalog.0.lock().unwrap();
        std::mem::take(&mut windows.entry(ctx.viewport_id()).or_default().mode)
    };
    let next = match mode {
        Mode::Hints { choices, prefix } => {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                Id::new("keyboard-hints"),
            ));
            for choice in choices.iter().filter(|c| c.code.starts_with(&prefix)) {
                let galley = painter.layout_no_wrap(
                    choice.code.to_uppercase(),
                    FontId::monospace(13.),
                    background,
                );
                let rect = Rect::from_min_size(
                    choice.target.rect.left_top(),
                    galley.size() + egui::vec2(10., 6.),
                );
                let bounds = ctx.content_rect().shrink(2.);
                let rect = rect.translate(egui::vec2(
                    (bounds.right() - rect.right()).min(0.),
                    (bounds.bottom() - rect.bottom()).min(0.),
                ));
                painter.rect_filled(rect, 3., foreground);
                painter.galley(rect.min + egui::vec2(5., 3.), galley, background);
            }
            painter.text(
                ctx.content_rect().center_bottom() - egui::vec2(0., 12.),
                egui::Align2::CENTER_BOTTOM,
                "Type a hint to focus · Enter/Space activates · Escape cancels",
                FontId::proportional(12.),
                foreground,
            );
            Mode::Hints { choices, prefix }
        }
        Mode::Search {
            targets,
            mut query,
            mut selected,
        } => {
            let events = {
                let mut windows = catalog.0.lock().unwrap();
                std::mem::take(&mut windows.entry(ctx.viewport_id()).or_default().held)
            };
            ctx.input_mut(|i| i.events.extend(events));
            let mut chosen = None;
            let mut tail = Vec::new();
            let mut canceled = false;
            let response = egui::Modal::new(Id::new("keyboard-control-search"))
                .area(
                    egui::Modal::default_area(Id::new("keyboard-control-search"))
                        .order(egui::Order::Tooltip),
                )
                .show(ctx, |ui| {
                    if ui.is_sizing_pass() {
                        let events = ctx.input_mut(|i| std::mem::take(&mut i.events));
                        let mut windows = catalog.0.lock().unwrap();
                        let window = windows.entry(ctx.viewport_id()).or_default();
                        for event in events {
                            window.hold(event);
                        }
                        ctx.request_repaint();
                        return;
                    }
                    ui.set_width(440_f32.min(ctx.content_rect().width() - 48.));
                    ui.heading("Find control in this window");
                    let confirm = ctx.input_mut(|i| {
                        if let Some(index) = i.events.iter().position(|e| {
                            matches!(
                                e,
                                egui::Event::Key {
                                    key: Key::Enter | Key::Escape,
                                    pressed: true,
                                    ..
                                }
                            )
                        }) {
                            tail = i.events.split_off(index + 1);
                            canceled = matches!(
                                i.events.pop(),
                                Some(egui::Event::Key {
                                    key: Key::Escape,
                                    ..
                                })
                            );
                            !canceled
                        } else {
                            false
                        }
                    });
                    let mut step = 0;
                    for (key, direction) in [(Key::ArrowUp, -1_isize), (Key::ArrowDown, 1)] {
                        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                            step = direction;
                        }
                    }
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut query)
                            .id(Id::new("keyboard-control-query"))
                            .hint_text("Search visible controls…")
                            .char_limit(256),
                    );
                    field.request_focus();
                    let mut filtered: Vec<_> = targets
                        .iter()
                        .filter_map(|t| rank(&query, &t.label).map(|score| (score, t)))
                        .collect();
                    filtered.sort_by_key(|(score, _)| *score);
                    let filtered: Vec<_> = filtered.into_iter().map(|(_, t)| t).collect();
                    if field.changed() {
                        selected = 0;
                    }
                    selected = selected
                        .saturating_add_signed(step)
                        .min(filtered.len().saturating_sub(1));
                    if confirm {
                        chosen = filtered.get(selected).map(|t| (*t).clone());
                    }
                    egui::ScrollArea::vertical()
                        .max_height(300.)
                        .show(ui, |ui| {
                            for (index, target) in filtered.iter().enumerate() {
                                let row = ui.selectable_label(index == selected, &target.label);
                                if index == selected {
                                    row.scroll_to_me(Some(Align::Center));
                                }
                                if row.clicked() {
                                    chosen = Some((*target).clone());
                                }
                            }
                        });
                    if filtered.is_empty() {
                        ui.label("No matching visible controls");
                    }
                });
            let close = canceled || response.should_close();
            ctx.input_mut(|i| i.events.retain(|e| !keyboard_event(e)));
            if let Some(target) = chosen {
                let mut windows = catalog.0.lock().unwrap();
                let window = windows.entry(ctx.viewport_id()).or_default();
                for event in tail {
                    window.hold(event);
                }
                Mode::Focus {
                    target,
                    applied: false,
                    grace: 2,
                }
            } else if close {
                let mut windows = catalog.0.lock().unwrap();
                let window = windows.entry(ctx.viewport_id()).or_default();
                for event in tail {
                    window.hold(event);
                }
                targets
                    .iter()
                    .find(|target| Some(target.widget) == window.origin)
                    .map(|target| Mode::Focus {
                        target: target.clone(),
                        applied: false,
                        grace: 2,
                    })
                    .unwrap_or(Mode::Idle)
            } else {
                Mode::Search {
                    targets,
                    query,
                    selected,
                }
            }
        }
        other => other,
    };
    let mut windows = catalog.0.lock().unwrap();
    let window = windows.entry(ctx.viewport_id()).or_default();
    window.mode = if window.notice.is_some() {
        Mode::Idle
    } else {
        next
    };
    if matches!(window.mode, Mode::Idle) {
        window.held.clear();
    }
}
