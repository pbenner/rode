//! Host-owned column sizing and virtualized data tables for all panel providers.
use super::*;
use std::borrow::Cow;

#[cfg(test)]
mod tests;

const MIN_WIDTH: f32 = 72.;
const MAX_WIDTH: f32 = 4096.;
const GRIP_WIDTH: f32 = 10.;
const MIN_HEIGHT: f32 = 160.;
const MAX_HEIGHT: f32 = 1200.;

/// Only user overrides are persisted. Automatic widths continue following new
/// data and font/window changes. Provider, table identity and column schema keep
/// unrelated reports separate, even when a provider reuses a table key.
fn column_id(panel: &str, identity: &str, columns: &[String]) -> egui::Id {
    egui::Id::new(("extension-table-columns", panel, identity, columns))
}

#[derive(Clone)]
struct Measured {
    signature: egui::Id,
    widths: Vec<f32>,
}

/// Measure a bounded sample, cached independently of window width. Large tables
/// must not shape thousands of offscreen cells on every frame. Longer values
/// remain available in tooltips and through manual resizing.
fn automatic_widths(ui: &egui::Ui, id: egui::Id, table: &protocol::Table) -> Vec<f32> {
    let font = egui::FontSelection::Default.resolve(ui.style());
    let sample: Vec<Vec<String>> = std::iter::once(&table.columns)
        .chain(table.rows.iter().take(32))
        .map(|row| {
            row.iter()
                .map(|cell| {
                    cell.chars()
                        .take(160)
                        .map(|ch| if matches!(ch, '\r' | '\n') { ' ' } else { ch })
                        .collect()
                })
                .collect()
        })
        .collect();
    let signature = egui::Id::new((&font, &sample));
    let measured = ui.ctx().data(|data| data.get_temp::<Measured>(id));
    let mut widths = if let Some(measured) = measured.filter(|m| m.signature == signature) {
        measured.widths
    } else {
        let mut widths = vec![MIN_WIDTH; table.columns.len()];
        ui.fonts_mut(|fonts| {
            for row in sample {
                for (width, text) in widths.iter_mut().zip(row) {
                    let galley = fonts.layout_no_wrap(text, font.clone(), egui::Color32::WHITE);
                    *width = width.max((galley.size().x + GRIP_WIDTH + 8.).min(420.));
                }
            }
        });
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                id,
                Measured {
                    signature,
                    widths: widths.clone(),
                },
            );
        });
        widths
    };
    // Reserve the frame, column gaps and scrollbar before filling spare space.
    // Manual widths may deliberately exceed this; horizontal scrolling handles it.
    let available = ui.available_width()
        - 8.
        - ui.spacing().scroll.allocated_width()
        - ui.spacing().item_spacing.x * widths.len().saturating_sub(1) as f32;
    let spare = ((available - widths.iter().sum::<f32>()) / widths.len().max(1) as f32).max(0.);
    for width in &mut widths {
        *width = (*width + spare).min(MAX_WIDTH);
    }
    widths
}

/// Keep a fixed row stride even for multiline provider values; the tooltip keeps
/// the original text. Wrapping here would invalidate virtual scrolling geometry.
fn single_line(text: &str) -> Cow<'_, str> {
    if text.contains(['\r', '\n']) {
        Cow::Owned(text.replace(['\r', '\n'], " "))
    } else {
        Cow::Borrowed(text)
    }
}

/// Virtual rows bound native layout work even for the largest accepted table.
/// Keyed identities survive insertions and renames; legacy tables use their index.
pub(super) fn render_table(ui: &mut egui::Ui, id: (&str, usize), table: &protocol::Table) -> usize {
    if !table.key.is_empty() {
        return ui
            .scope_builder(
                egui::UiBuilder::new().id(("extension-table", id.0, &table.key)),
                |ui| render_table_inner(ui, id, table),
            )
            .inner;
    }
    render_table_inner(ui, id, table)
}
fn render_table_inner(ui: &mut egui::Ui, id: (&str, usize), table: &protocol::Table) -> usize {
    let stable_id = (
        id.0,
        if table.key.is_empty() {
            format!("legacy:{}", id.1)
        } else {
            format!("key:{}", table.key)
        },
    );
    ui.add_space(12.);
    ui.strong(&table.title);
    let columns_id = column_id(id.0, &stable_id.1, &table.columns);
    let defaults = automatic_widths(ui, columns_id.with("measure"), table);
    let mut overrides = ui.ctx().data_mut(|data| {
        data.get_persisted::<Vec<Option<f32>>>(columns_id)
            .unwrap_or_default()
    });
    overrides.resize(table.columns.len(), None);
    for width in &mut overrides {
        *width = width
            .filter(|w| w.is_finite())
            .map(|w| w.clamp(MIN_WIDTH, MAX_WIDTH));
    }
    let widths: Vec<f32> = overrides
        .iter()
        .zip(&defaults)
        .map(|(width, default)| width.unwrap_or(*default))
        .collect();
    let font = egui::FontSelection::Default.resolve(ui.style());
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font)).max(24.);
    let height_id = egui::Id::new(("extension-table-height", &stable_id));
    let default_height = (ui.ctx().content_rect().height() * 0.65).clamp(320., 720.);
    let height_override = ui.ctx().data_mut(|data| {
        data.get_persisted::<f32>(height_id)
            .filter(|height| height.is_finite())
            .map(|height| height.clamp(MIN_HEIGHT, MAX_HEIGHT))
    });
    let height = height_override.unwrap_or(default_height);
    let content_height = (table.rows.len() + 1) as f32 * (row_height + ui.spacing().item_spacing.y);
    // The outer panel scrolls the full page. Do not shrink a nested table to
    // the few remaining pixels below preceding cards, leaving a tiny viewport.
    let min_height = if height_override.is_some() {
        height
    } else {
        height.min(content_height)
    };
    let spacing = ui.spacing().item_spacing.x;
    let total_width = widths.iter().sum::<f32>() + spacing * widths.len().saturating_sub(1) as f32;
    let mut headers = Vec::new();
    let frame = egui::Frame::new().inner_margin(4).show(ui, |ui| {
        egui::ScrollArea::both()
            .id_salt(("extension-table-scroll", &stable_id))
            .max_height(height)
            .min_scrolled_height(min_height)
            .auto_shrink([false, height_override.is_none()])
            .show_rows(ui, row_height, table.rows.len() + 1, |ui, range| {
                ui.set_min_width(total_width);
                for index in range.clone() {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(total_width, row_height), Sense::hover());
                    if index % 2 == 1 {
                        ui.painter()
                            .rect_filled(rect, 0., ui.visuals().faint_bg_color);
                    }
                    let mut left = rect.left();
                    let header = index == 0;
                    let row = if header {
                        &table.columns
                    } else {
                        &table.rows[index - 1]
                    };
                    for (column, (cell, width)) in row.iter().zip(&widths).enumerate() {
                        let cell_rect = egui::Rect::from_min_size(
                            egui::pos2(left, rect.top()),
                            egui::vec2(*width, row_height),
                        );
                        let label_rect = egui::Rect::from_min_max(
                            cell_rect.min + egui::vec2(4., 0.),
                            cell_rect.max - egui::vec2(GRIP_WIDTH, 0.),
                        );
                        let text = if header {
                            RichText::new(single_line(cell)).strong()
                        } else {
                            RichText::new(single_line(cell))
                        };
                        let response = ui
                            .scope_builder(
                                egui::UiBuilder::new()
                                    .max_rect(label_rect)
                                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                |ui| ui.add(egui::Label::new(text).truncate()),
                            )
                            .inner
                            .on_hover_text(cell);
                        if header {
                            headers.push((column, cell_rect));
                            ui_probe::region(
                                ui,
                                &format!("extension-table-column:{}:{}:{column}", id.0, id.1),
                                &response,
                                || Some(cell_rect),
                            );
                        }
                        left += width + spacing;
                    }
                }
                range.len()
            })
    });
    let mut output = frame.inner;
    if content_height > default_height || height_override.is_some() {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(frame.response.rect.width(), GRIP_WIDTH),
            Sense::hover(),
        );
        let drag_id = height_id.with("drag");
        let mut start = ui
            .ctx()
            .data(|data| data.get_temp::<Option<f32>>(drag_id).flatten());
        let resized = layout::resize::Handle {
            rect,
            id: height_id.with("grip"),
            label: &format!("Resize {} height", table.title),
            edge: layout::resize::Edge::Bottom,
            size: height,
            default: default_height,
            range: MIN_HEIGHT..=MAX_HEIGHT,
            enabled: true,
        }
        .show(ui, &mut start);
        ui_probe::button(
            ui,
            &format!("extension-table-height-resize:{}:{}", id.0, id.1),
            &resized.response,
        );
        ui.ctx().data_mut(|data| {
            data.insert_temp(drag_id, start);
            if resized.response.changed() {
                if resized.reset {
                    data.remove::<f32>(height_id);
                } else {
                    data.insert_persisted(height_id, resized.size);
                }
            }
        });
    }
    let response = ui.interact(
        frame.response.rect,
        ui.id().with(("extension-table-focus", &stable_id)),
        Sense::focusable_noninteractive(),
    );
    keyboard::field_target(ui, &table.title, &response);
    ui_probe::button(ui, &format!("extension-table:{}:{}", id.0, id.1), &response);
    let grip_rect = |rect: egui::Rect| {
        egui::Rect::from_min_max(egui::pos2(rect.right() - GRIP_WIDTH, rect.top()), rect.max)
    };
    let on_grip = ui.input(|i| i.pointer.interact_pos()).is_some_and(|pos| {
        headers
            .iter()
            .any(|(_, rect)| grip_rect(*rect).contains(pos))
    });
    if !on_grip && response.contains_pointer() && ui.input(|i| i.pointer.primary_pressed()) {
        response.request_focus();
    }
    // Register after scroll/cell interactions, clipped to the actual viewport.
    // These grips share the app's keyboard support and non-compounding drag math.
    let mut starts = ui.ctx().data(|data| {
        data.get_temp::<Vec<Option<f32>>>(columns_id.with("drag"))
            .unwrap_or_default()
    });
    starts.resize(widths.len(), None);
    let mut changed = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(output.inner_rect), |ui| {
        ui.set_clip_rect(ui.clip_rect().intersect(output.inner_rect));
        for (column, rect) in headers {
            let resized = layout::resize::Handle {
                rect: grip_rect(rect).intersect(output.inner_rect),
                id: columns_id.with(column),
                label: &format!("Resize {} column", table.columns[column]),
                edge: layout::resize::Edge::Right,
                size: widths[column],
                default: defaults[column],
                range: MIN_WIDTH..=MAX_WIDTH,
                enabled: true,
            }
            .show(ui, &mut starts[column]);
            ui_probe::button(
                ui,
                &format!("extension-table-column-resize:{}:{}:{column}", id.0, id.1),
                &resized.response,
            );
            if resized.response.changed() {
                overrides[column] = (!resized.reset).then_some(resized.size);
                changed = true;
            }
        }
    });
    ui.ctx().data_mut(|data| {
        data.insert_temp(columns_id.with("drag"), starts);
        if changed {
            data.insert_persisted(columns_id, overrides);
        }
    });
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
