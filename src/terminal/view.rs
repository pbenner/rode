//! Native terminal rendering, focus and input handling. This layer borrows an
//! existing terminal; it never creates a process or changes backend ownership.
use super::*;

const SHELL_KEYS: egui::EventFilter = egui::EventFilter {
    tab: true,
    horizontal_arrows: true,
    vertical_arrows: true,
    escape: true,
};

impl Terminal {
    /// Return whether the native window and terminal input widget have focus. Do not call
    /// inside an egui `input`/`memory` lock closure.
    pub fn focused(ctx: &egui::Context) -> bool {
        ctx.memory(|m| m.has_focus(egui::Id::new("terminal-input")))
    }

    /// Request terminal input focus after the current egui input barrier permits it.
    pub fn focus(ctx: &egui::Context) {
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("terminal-input")));
        // A command/manager modal blocks background focus for one additional
        // frame. Retry once the terminal layer can receive interaction.
        ctx.data_mut(|data| data.insert_temp(egui::Id::new("terminal-wants-focus"), true));
        ctx.request_repaint();
    }

    /// Prepare shell input before editor shortcuts and widgets handle it.
    /// The caller excludes hidden terminals, dialogs, and open menus.
    pub fn prepare_input(ctx: &egui::Context) {
        let id = egui::Id::new("terminal-input");
        let escape = ctx.input(|input| input.key_pressed(Key::Escape));
        ctx.memory_mut(|memory| {
            if memory.has_focus(id) || (escape && memory.had_focus_last_frame(id)) {
                // egui decides navigation before widgets run, but a fresh focus
                // request cannot install a filter until the following frame.
                // Cancel that pending navigation before it reaches any widget.
                // Escape may already have cleared the terminal's focus.
                if !memory.has_focus(id) {
                    memory.request_focus(id);
                }
                memory.move_focus(egui::FocusDirection::None);
                memory.set_focus_lock_filter(id, SHELL_KEYS);
            }
        });
    }

    /// Render selectable terminal cells and process focused shell input. Returns input
    /// errors; ordinary control keys remain shell input outside editor escape routes.
    pub fn ui(&mut self, ui: &mut egui::Ui, font_size: f32, palette: Palette) -> Option<String> {
        self.ui_with_response(ui, font_size, palette).error
    }

    /// Return the original interaction response for observation. Re-reading it
    /// through Context::read_response repeats hit testing and can surrender focus.
    pub(crate) fn ui_with_response(
        &mut self,
        ui: &mut egui::Ui,
        font_size: f32,
        palette: Palette,
    ) -> TerminalView {
        let font = FontId::monospace(font_size);
        let char_size =
            ui.fonts_mut(|fonts| Vec2::new(fonts.glyph_width(&font, 'M'), fonts.row_height(&font)));
        let available = ui.available_size();
        let (rect, _) = ui.allocate_exact_size(available.max(Vec2::splat(1.0)), Sense::hover());
        let response = ui.interact(
            rect,
            egui::Id::new("terminal-input"),
            Sense::click_and_drag(),
        );
        if ui.memory(|memory| memory.allows_interaction(ui.layer_id()))
            && ui
                .ctx()
                .data_mut(|data| data.remove_temp::<bool>(egui::Id::new("terminal-wants-focus")))
                .unwrap_or(false)
        {
            response.request_focus();
        }
        if response.clicked() || response.drag_started() {
            response.request_focus();
        }
        if response.has_focus() {
            ui.memory_mut(|memory| memory.set_focus_lock_filter(response.id, SHELL_KEYS));
        }
        response.clone().on_hover_cursor(egui::CursorIcon::Text);
        let rows = ((rect.height() - 8.0) / char_size.y)
            .floor()
            .clamp(2.0, 200.0) as u16;
        let cols = ((rect.width() - 12.0) / char_size.x)
            .floor()
            .clamp(10.0, 400.0) as u16;
        let resize_error = if self.size != (rows, cols) {
            self.resize(rows, cols).err().map(|error| error.to_string())
        } else {
            None
        };
        let mut bytes_to_send = Vec::new();
        let origin = rect.min + egui::vec2(6.0, 4.0);
        let mut copy_selection = None;
        if let Ok(mut parser) = self.parser.lock() {
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll.abs() > 0.1 {
                    let old = parser.screen().scrollback();
                    parser.screen_mut().set_scrollback(
                        (old as isize + (scroll / char_size.y).round() as isize).max(0) as usize,
                    );
                }
            }
            if self
                .selection
                .as_ref()
                .is_some_and(|selection| !selection.valid_for(parser.screen()))
            {
                self.selection = None;
            }
            // Capture the anchor on press. A move and release can arrive together,
            // before egui ever reports a drag or retains its press origin.
            if response.contains_pointer()
                && ui.is_enabled()
                && let Some(position) = ui.input(|input| {
                    input.events.iter().find_map(|event| match event {
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            ..
                        } => Some(*pos),
                        _ => None,
                    })
                })
                && rect.contains(position)
            {
                response.request_focus();
                self.selection = Some(selection::Selection::new(
                    parser.screen(),
                    selection::point(parser.screen(), origin, char_size, position, (rows, cols)),
                ));
            }
            if let Some(position) = ui.input(|input| input.pointer.interact_pos())
                && let Some(selection) = &mut self.selection
                && selection.dragging
            {
                selection.head =
                    selection::point(parser.screen(), origin, char_size, position, (rows, cols));
                selection.dragging = ui.input(|input| input.pointer.primary_down());
            }
            if response.clicked_by(egui::PointerButton::Primary) {
                self.selection = None;
            }
            if response.has_focus()
                && ui.is_enabled()
                && !response.context_menu_opened()
                && ui.memory(|memory| memory.allows_interaction(ui.layer_id()))
            {
                let events = ui.input(|input| input.events.clone());
                // Native clipboard shortcuts replace their key press. A release can
                // retain modifiers after they have already been released this frame.
                let copy_modifiers = events
                    .iter()
                    .find_map(|event| match event {
                        egui::Event::Key {
                            key: Key::C,
                            modifiers,
                            ..
                        } => Some(*modifiers),
                        _ => None,
                    })
                    .unwrap_or_else(|| ui.input(|input| input.modifiers));
                for event in events {
                    if !self.is_alive()
                        && !matches!(
                            event,
                            egui::Event::Copy
                                | egui::Event::Key {
                                    key: Key::C | Key::Escape,
                                    ..
                                }
                        )
                    {
                        continue;
                    }
                    match event {
                        egui::Event::Text(text) => {
                            if ui.input(|i| i.modifiers.alt && !i.modifiers.ctrl) {
                                bytes_to_send.push(0x1b);
                            }
                            bytes_to_send.extend(text.as_bytes());
                        }
                        egui::Event::Paste(text) => {
                            if parser.screen().bracketed_paste() {
                                bytes_to_send.extend(b"\x1b[200~");
                            }
                            if parser.screen().bracketed_paste() {
                                bytes_to_send.extend(text.as_bytes());
                            } else {
                                bytes_to_send.extend(
                                    text.replace("\r\n", "\n").replace('\n', "\r").as_bytes(),
                                );
                            }
                            if parser.screen().bracketed_paste() {
                                bytes_to_send.extend(b"\x1b[201~");
                            }
                        }
                        egui::Event::Copy => {
                            if let Some(text) = self
                                .selection
                                .as_ref()
                                .and_then(|selection| selection.text(parser.screen()))
                            {
                                ui.ctx().copy_text(text);
                            } else if copy_modifiers.shift
                                || copy_modifiers.mac_cmd
                                || cfg!(target_os = "macos")
                            {
                                ui.ctx().copy_text(parser.screen().contents());
                            } else if self.is_alive() {
                                bytes_to_send.push(0x03);
                            }
                        }
                        // On macOS Command+X is a clipboard action; Control+X
                        // arrives as an ordinary key event and is encoded below.
                        egui::Event::Cut if !cfg!(target_os = "macos") => bytes_to_send.push(0x18),
                        egui::Event::Key {
                            key,
                            pressed: true,
                            modifiers,
                            ..
                        } => {
                            if key == Key::C
                                && (modifiers.command || modifiers.mac_cmd || modifiers.ctrl)
                            {
                                if let Some(text) = self
                                    .selection
                                    .as_ref()
                                    .and_then(|selection| selection.text(parser.screen()))
                                {
                                    ui.ctx().copy_text(text);
                                } else if modifiers.shift || modifiers.mac_cmd {
                                    ui.ctx().copy_text(parser.screen().contents());
                                } else if self.is_alive()
                                    && let Some(bytes) = key_bytes(
                                        key,
                                        modifiers,
                                        parser.screen().application_cursor(),
                                    )
                                {
                                    bytes_to_send.extend(bytes);
                                }
                            } else if key == Key::Escape && self.selection.is_some() {
                                self.selection = None;
                            } else if self.is_alive()
                                && let Some(bytes) =
                                    key_bytes(key, modifiers, parser.screen().application_cursor())
                            {
                                bytes_to_send.extend(bytes);
                            }
                        }
                        _ => {}
                    }
                    // Later events in this same frame must see the selection cleared
                    // by typing, paste or a shell key (in particular, Control+C).
                    if !bytes_to_send.is_empty() {
                        self.selection = None;
                    }
                }
                if !bytes_to_send.is_empty() {
                    parser.screen_mut().set_scrollback(0);
                    self.selection = None;
                }
            }
            let painter = ui.painter().with_clip_rect(rect);
            painter.rect_filled(rect, 0, palette.terminal_background);
            let screen = parser.screen();
            let focused = response.has_focus();
            let cursor = if self.is_alive() && !screen.hide_cursor() && screen.scrollback() == 0 {
                let (row, col) = screen.cursor_position();
                let mut col = col.min(cols.saturating_sub(1));
                if screen
                    .cell(row, col)
                    .is_some_and(|cell| cell.is_wide_continuation())
                {
                    col = col.saturating_sub(1);
                }
                (row < rows).then_some((row, col))
            } else {
                None
            };
            for row in 0..rows {
                for col in 0..cols {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let pos =
                        origin + egui::vec2(col as f32 * char_size.x, row as f32 * char_size.y);
                    // Bash PS1 commonly uses SGR 1;30 / 1;34. Match xterm's
                    // default boldColors behavior: brighten only the basic
                    // foreground bank, never backgrounds, RGB or indices 8+.
                    let foreground = match cell.fgcolor() {
                        vt100::Color::Idx(index @ 0..=7) if cell.bold() => {
                            vt100::Color::Idx(index + 8)
                        }
                        color => color,
                    };
                    let mut fg = ansi_color(foreground, palette.terminal_foreground, &palette.ansi);
                    let mut bg =
                        ansi_color(cell.bgcolor(), palette.terminal_background, &palette.ansi);
                    if cell.inverse() {
                        std::mem::swap(&mut fg, &mut bg);
                    }
                    let selected = self
                        .selection
                        .as_ref()
                        .is_some_and(|selection| selection.contains(row, col));
                    if selected || (focused && cursor == Some((row, col))) {
                        bg = palette.accent;
                        fg = palette.terminal_background;
                    }
                    if bg != palette.terminal_background {
                        painter.rect_filled(
                            Rect::from_min_size(
                                pos,
                                Vec2::new(
                                    char_size.x * if cell.is_wide() { 2.0 } else { 1.0 },
                                    char_size.y,
                                ),
                            ),
                            0,
                            bg,
                        );
                    }
                    if cell.has_contents() {
                        fg = self.contrast.foreground(
                            fg,
                            bg,
                            cell.contents(),
                            palette.terminal_minimum_contrast,
                        );
                        painter.text(
                            pos,
                            egui::Align2::LEFT_TOP,
                            cell.contents(),
                            font.clone(),
                            fg,
                        );
                    }
                    if cell.underline() {
                        painter.line_segment(
                            [pos + egui::vec2(0.0, char_size.y - 1.0), pos + char_size],
                            Stroke::new(1.0_f32, fg),
                        );
                    }
                }
            }
            if !focused && let Some((row, col)) = cursor {
                let pos = origin + egui::vec2(col as f32 * char_size.x, row as f32 * char_size.y);
                let width = if screen.cell(row, col).is_some_and(|cell| cell.is_wide()) {
                    2.0
                } else {
                    1.0
                };
                painter.rect_stroke(
                    Rect::from_min_size(pos, egui::vec2(char_size.x * width, char_size.y)),
                    0,
                    Stroke::new(1.0_f32, palette.muted),
                    egui::StrokeKind::Inside,
                );
            }
            response.context_menu(|ui| {
                let selected = self
                    .selection
                    .as_ref()
                    .and_then(|selection| selection.text(screen));
                let copy = ui.add_enabled(selected.is_some(), egui::Button::new("Copy selection"));
                if copy.clicked() {
                    ui.ctx().copy_text(selected.unwrap_or_default());
                    ui.close();
                }
                copy_selection = Some(copy);
                if ui.button("Copy visible output").clicked() {
                    ui.ctx().copy_text(screen.contents());
                    ui.close();
                }
            });
        }
        let error = if !bytes_to_send.is_empty() {
            self.send(bytes_to_send)
                .err()
                .map(|e| e.to_string())
                .or(resize_error)
        } else {
            resize_error
        };
        TerminalView {
            response,
            error,
            first_cell: Rect::from_min_size(origin, char_size),
            copy_selection,
        }
    }
}
