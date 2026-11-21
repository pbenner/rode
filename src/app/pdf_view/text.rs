//! Window-local PDF selection and find presentation. The worker owns indexing;
//! snapshots contain immutable Unicode cells, never source edits or host paths.
use super::*;
mod worker;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Hit {
    page: usize,
    range: [usize; 2],
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    page: usize,
    byte: usize,
}

#[derive(Default)]
pub(super) struct State {
    worker: Option<worker::Worker>,
    open: bool,
    focus: bool,
    composing: bool,
    query: String,
    case_sensitive: bool,
    revision: u64,
    submitted: Option<(String, bool)>,
    active: Option<Hit>,
    reveal: Option<Hit>,
    jump: Option<usize>,
    selection: Option<(Position, Position)>,
    dragging: bool,
}
impl State {
    pub fn open(&mut self) {
        self.open = true;
        self.focus = true;
    }
    pub fn close_on_escape(&mut self, ctx: &egui::Context) -> bool {
        if self.open && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape))
        {
            self.open = false;
            self.focus = false;
            true
        } else {
            false
        }
    }
    pub fn search_button(&mut self, ui: &mut egui::Ui) {
        let response = ui.add(IconButton::new(Icon::Search, "Find").shortcut("Ctrl+F"));
        ui_probe::button(ui, "pdf-find", &response);
        if response.clicked() {
            self.open();
        }
    }
    pub fn search_ui(&mut self, ui: &mut egui::Ui) {
        if !self.open {
            return;
        }
        ui.add_space(4.);
        ui.horizontal_wrapped(|ui| {
            let id = ui.make_persistent_id("pdf-find-input");
            let step =
                if crate::keyboard::text_shortcuts_allowed(ui, id, false, &mut self.composing) {
                    ui.input_mut(|input| {
                        if input.consume_key(egui::Modifiers::SHIFT, Key::Enter) {
                            -1
                        } else if input.consume_key(egui::Modifiers::NONE, Key::Enter) {
                            1
                        } else {
                            0
                        }
                    })
                } else {
                    0
                };
            let input = ui.add(crate::keyboard::named(
                "Find in PDF",
                egui::TextEdit::singleline(&mut self.query)
                    .id(id)
                    // Match navigation handles Enter; IME confirmation must
                    // retain focus rather than taking TextEdit's default exit.
                    .return_key(None)
                    .hint_text("Find in PDF…")
                    .desired_width((ui.available_width() - 320.).clamp(120., 320.))
                    .char_limit(1024),
            ));
            ui_probe::button(ui, "pdf-find-input", &input);
            ui_probe::value(ui, "pdf-find-input-value", &input, &self.query);
            if self.focus {
                input.request_focus();
                self.focus = false;
            }
            let sensitive = crate::keyboard::checkbox(ui, &mut self.case_sensitive, "Match case");
            ui_probe::button(ui, "pdf-find-case", &sensitive);
            for (direction, label, name) in [
                (-1, "Previous match", "pdf-find-previous"),
                (1, "Next match", "pdf-find-next"),
            ] {
                let button = ui.add(IconButton::toolbar(
                    if direction < 0 {
                        Icon::ChevronUp
                    } else {
                        Icon::ChevronDown
                    },
                    label,
                ));
                ui_probe::button(ui, name, &button);
                if button.clicked() || step == direction {
                    self.step(direction);
                }
            }
            let (count, searching, indexed, error, limited) = self
                .worker
                .as_ref()
                .map(|worker| {
                    let snapshot = worker.output.lock().unwrap();
                    let current = snapshot.revision == self.revision
                        && self.submitted.as_ref()
                            == Some(&(self.query.clone(), self.case_sensitive));
                    (
                        if current { snapshot.matches.len() } else { 0 },
                        snapshot.searching || !current,
                        snapshot.indexed,
                        snapshot.error.clone(),
                        snapshot.limited,
                    )
                })
                .unwrap_or_default();
            let current = self
                .worker
                .as_ref()
                .and_then(|worker| {
                    worker
                        .output
                        .lock()
                        .unwrap()
                        .matches
                        .iter()
                        .position(|hit| Some(hit) == self.active.as_ref())
                })
                .map_or(0, |i| i + 1);
            let status = ui.label(if self.query.trim().is_empty() {
                "Type to search".into()
            } else {
                format!("{current} / {count}{}", if limited { "+" } else { "" })
            });
            ui_probe::button(ui, &format!("pdf-find-count:{count}"), &status);
            if searching {
                ui.spinner()
                    .on_hover_text(format!("Indexed {indexed} pages"));
            }
            if let Some(error) = error {
                ui.label("Partial results").on_hover_text(error);
            }
            let close = ui.add(IconButton::toolbar(Icon::Close, "Close search · Escape"));
            ui_probe::button(ui, "pdf-find-close", &close);
            if close.clicked() {
                self.open = false;
            }
        });
    }
    fn step(&mut self, direction: i32) {
        if self.submitted.as_ref() != Some(&(self.query.clone(), self.case_sensitive)) {
            return;
        }
        let hit = self.worker.as_ref().and_then(|worker| {
            let snapshot = worker.output.lock().unwrap();
            if snapshot.revision != self.revision || snapshot.matches.is_empty() {
                return None;
            }
            let current = snapshot
                .matches
                .iter()
                .position(|hit| Some(hit) == self.active.as_ref());
            let index = current.map_or(
                if direction < 0 {
                    snapshot.matches.len() - 1
                } else {
                    0
                },
                |index| {
                    (index as i32 + direction).rem_euclid(snapshot.matches.len() as i32) as usize
                },
            );
            Some(snapshot.matches[index].clone())
        });
        if let Some(hit) = hit {
            self.select_match(hit);
        }
    }
    fn select_match(&mut self, hit: Hit) {
        self.jump = Some(hit.page);
        self.selection = None;
        self.reveal = Some(hit.clone());
        self.active = Some(hit);
    }
    pub fn update(
        &mut self,
        source: &Arc<pdf::Source>,
        ctx: &egui::Context,
        visible: Vec<usize>,
        keyboard: bool,
        id: u64,
    ) -> Option<usize> {
        let worker = self
            .worker
            .get_or_insert_with(|| worker::Worker::new(source.clone(), ctx.clone()));
        let current_page = visible.first().copied().unwrap_or(1);
        let query = (self.query.clone(), self.case_sensitive);
        if self.submitted.as_ref() != Some(&query) {
            self.revision = self.revision.wrapping_add(1);
            self.submitted = Some(query);
            self.active = None;
            self.reveal = None;
            self.jump = None;
        }
        worker.request(worker::Request {
            visible,
            query: self.query.clone(),
            case_sensitive: self.case_sensitive,
            revision: self.revision,
            search: self.open,
        });
        let first = {
            let snapshot = worker.output.lock().unwrap();
            if self.open && self.active.is_none() && snapshot.revision == self.revision {
                snapshot.matches.first().cloned()
            } else {
                None
            }
        };
        if let Some(hit) = first {
            self.select_match(hit);
        }
        if keyboard {
            self.close_on_escape(ctx);
            let step = ctx.input_mut(|input| {
                if input.consume_key(egui::Modifiers::SHIFT, Key::F3) {
                    -1
                } else if input.consume_key(egui::Modifiers::NONE, Key::F3) {
                    1
                } else {
                    0
                }
            });
            if step != 0 {
                self.step(step);
            }
            if ctx.memory(|m| m.has_focus(image_view::ImageView::focus_id(id))) {
                if ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, Key::A)) {
                    self.select_page(current_page);
                }
                if ctx.input(|input| {
                    input
                        .events
                        .iter()
                        .any(|event| matches!(event, egui::Event::Copy))
                }) && let Some(text) = self.selected_text()
                {
                    ctx.copy_text(text);
                }
            }
        }
        self.jump.take()
    }
    fn select_page(&mut self, page: usize) {
        if let Some(text) = self.page(page) {
            self.selection = Some((
                Position { page, byte: 0 },
                Position {
                    page,
                    byte: text.text.len(),
                },
            ));
        }
    }
    fn page(&self, page: usize) -> Option<Arc<pdf::PageText>> {
        self.worker
            .as_ref()?
            .output
            .lock()
            .unwrap()
            .pages
            .get(&page)
            .cloned()
    }
    fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        if start == end {
            return None;
        }
        let mut result = String::new();
        for page in start.page..=end.page {
            let text = self.page(page)?;
            let first = if page == start.page { start.byte } else { 0 };
            let last = if page == end.page {
                end.byte
            } else {
                text.text.len()
            };
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(text.text.get(first..last)?);
        }
        Some(result)
    }
    /// Attach interaction to the existing canvas response, preserving one focus
    /// identity and preventing the scroll container from stealing selection drags.
    pub fn paint(
        &mut self,
        ui: &mut egui::Ui,
        page: usize,
        rect: egui::Rect,
        response: &egui::Response,
    ) {
        let Some(text) = self.page(page) else {
            return;
        };
        let cell_rect = |cell: &pdf::TextCell| {
            egui::Rect::from_min_max(
                rect.min + egui::vec2(cell.rect[0] * rect.width(), cell.rect[1] * rect.height()),
                rect.min + egui::vec2(cell.rect[2] * rect.width(), cell.rect[3] * rect.height()),
            )
        };
        let nearest = |point: egui::Pos2| {
            text.cells
                .iter()
                .min_by(|a, b| {
                    cell_rect(a)
                        .distance_sq_to_pos(point)
                        .total_cmp(&cell_rect(b).distance_sq_to_pos(point))
                })
                .map(|cell| {
                    let offset = point - cell_rect(cell).center();
                    let direction = egui::vec2(
                        cell.direction[0] * rect.width(),
                        cell.direction[1] * rect.height(),
                    );
                    if offset.dot(direction) < 0. {
                        cell.range[0]
                    } else {
                        cell.range[1]
                    }
                })
                .unwrap_or(0)
        };
        let pointer = ui.ctx().input(|input| input.pointer.clone());
        if ui.is_enabled()
            && (response.clicked() || response.drag_started_by(egui::PointerButton::Primary))
            && let Some(origin) = pointer.press_origin().or(response.interact_pointer_pos())
            && rect.contains(origin)
        {
            let byte = nearest(origin);
            self.selection = Some((Position { page, byte }, Position { page, byte }));
            self.dragging = response.drag_started_by(egui::PointerButton::Primary);
        }
        if ui.is_enabled()
            && self.dragging
            && let Some(point) = pointer.interact_pos()
            && rect.contains(point)
            && let Some((_, end)) = &mut self.selection
        {
            *end = Position {
                page,
                byte: nearest(point),
            };
        }
        if response.double_clicked()
            && let Some(point) = pointer.interact_pos()
            && rect.contains(point)
        {
            let byte = nearest(point);
            let start = text.text[..byte]
                .char_indices()
                .rev()
                .find(|(_, ch)| ch.is_whitespace())
                .map_or(0, |(i, ch)| i + ch.len_utf8());
            let end = text.text[byte..]
                .find(char::is_whitespace)
                .map_or(text.text.len(), |i| byte + i);
            self.selection = Some((Position { page, byte: start }, Position { page, byte: end }));
        }
        if rect
            .intersect(ui.clip_rect())
            .contains(pointer.hover_pos().unwrap_or(egui::Pos2::ZERO))
            && !text.cells.is_empty()
        {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        let hits = self
            .worker
            .as_ref()
            .map(|worker| {
                let snapshot = worker.output.lock().unwrap();
                if self.open && snapshot.revision == self.revision {
                    snapshot
                        .matches
                        .iter()
                        .filter(|hit| hit.page == page)
                        .cloned()
                        .collect::<Vec<_>>()
                } else {
                    vec![]
                }
            })
            .unwrap_or_default();
        for hit in hits {
            let active = Some(&hit) == self.active.as_ref();
            paint_range(
                ui,
                &text,
                hit.range,
                &cell_rect,
                if active {
                    egui::Color32::from_rgba_unmultiplied(255, 155, 0, 115)
                } else {
                    egui::Color32::from_rgba_unmultiplied(255, 215, 0, 75)
                },
            );
            if self.reveal.as_ref() == Some(&hit)
                && let Some(cell) = text.cells.iter().find(|cell| cell.range[1] > hit.range[0])
            {
                ui.scroll_to_rect(cell_rect(cell).expand(20.), None);
                self.reveal = None;
            }
        }
        if let Some((a, b)) = self.selection {
            let (start, end) = if a <= b { (a, b) } else { (b, a) };
            if (start.page..=end.page).contains(&page) {
                let range = [
                    if page == start.page { start.byte } else { 0 },
                    if page == end.page {
                        end.byte
                    } else {
                        text.text.len()
                    },
                ];
                paint_range(
                    ui,
                    &text,
                    range,
                    &cell_rect,
                    egui::Color32::from_rgba_unmultiplied(40, 120, 245, 85),
                );
            }
        }
        ui_probe::button(ui, &format!("pdf-text-ready:{page}"), response);
        ui_probe::region(ui, &format!("pdf-text-bounds:{page}"), response, || {
            text.cells.iter().map(&cell_rect).reduce(|a, b| a.union(b))
        });
        // Only bounded, borrowed values reach the opt-in hashing probe; never
        // reconstruct a multi-page selection during ordinary frame rendering.
        if let Some((a, b)) = self.selection
            && a.page == page
            && b.page == page
            && a.byte.abs_diff(b.byte) <= 64 * 1024
        {
            ui_probe::value(
                ui,
                "pdf-selection",
                response,
                &text.text[a.byte.min(b.byte)..a.byte.max(b.byte)],
            );
        }
        response.context_menu(|ui| {
            let selected = self.selected_text();
            if ui
                .add_enabled(
                    selected.is_some(),
                    crate::keyboard::Button::new("Copy").shortcut_text("Ctrl+C"),
                )
                .clicked()
            {
                if let Some(text) = selected {
                    ui.ctx().copy_text(text);
                }
                ui.close();
            }
            if crate::keyboard::button(ui, "Select page text").clicked() {
                self.select_page(page);
                ui.close();
            }
        });
    }
    pub fn end_frame(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.pointer.primary_down()) {
            self.dragging = false;
        }
    }
}

fn paint_range(
    ui: &egui::Ui,
    text: &pdf::PageText,
    range: [usize; 2],
    rect: &impl Fn(&pdf::TextCell) -> egui::Rect,
    color: egui::Color32,
) {
    if range[0] == range[1] {
        return;
    }
    let first = text.cells.partition_point(|cell| cell.range[1] <= range[0]);
    for cell in text.cells[first..]
        .iter()
        .take_while(|cell| cell.range[0] < range[1])
    {
        let rect = rect(cell).intersect(ui.clip_rect());
        if rect.is_positive() {
            ui.painter().rect_filled(rect, 1, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_enter_confirms_ime_before_navigating_and_disabled_fields_keep_their_keys() {
        let ctx = egui::Context::default();
        let first = Hit {
            page: 1,
            range: [0, 1],
        };
        let second = Hit {
            page: 2,
            range: [0, 1],
        };
        let mut state = State {
            open: true,
            focus: true,
            query: "字".into(),
            revision: 1,
            submitted: Some(("字".into(), false)),
            active: Some(first.clone()),
            worker: Some(worker::Worker::fixture(worker::Snapshot {
                revision: 1,
                matches: vec![first.clone(), second.clone()],
                ..Default::default()
            })),
            ..Default::default()
        };
        let mut frame = |events, enabled| {
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.add_enabled_ui(enabled, |ui| state.search_ui(ui));
                    });
                },
            );
            state.submitted = Some((state.query.clone(), state.case_sensitive));
            state.active.clone()
        };
        let enter = |modifiers| {
            [true, false]
                .into_iter()
                .map(|pressed| egui::Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers,
                })
                .collect::<Vec<_>>()
        };
        frame(vec![], true);
        frame(vec![egui::Event::Ime(egui::ImeEvent::Enabled)], true);
        frame(
            vec![egui::Event::Ime(egui::ImeEvent::Preedit("字".into()))],
            true,
        );
        let mut confirming = vec![egui::Event::Ime(egui::ImeEvent::Commit("字".into()))];
        confirming.extend(enter(egui::Modifiers::NONE));
        assert_eq!(
            frame(confirming, true),
            Some(first.clone()),
            "IME confirmation changed the active match"
        );
        assert_eq!(frame(enter(egui::Modifiers::NONE), true), Some(second));
        assert_eq!(
            frame(enter(egui::Modifiers::SHIFT), true),
            Some(first.clone())
        );
        assert_eq!(frame(enter(egui::Modifiers::NONE), false), Some(first));
    }
}
