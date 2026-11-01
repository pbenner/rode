//! Bounded Vim motions/operators translated into native source-editor edits.
//!
//! Normal/visual commands reuse the shared character-indexed text model and register.
//! Insert/undo events remain native TextEdit input; supported Ex actions route through
//! guarded editor commands rather than a shell or embedded Vim runtime.

use super::*;
impl State {
    fn reset_command(&mut self) {
        self.count = 0;
        self.operator = None;
        self.g = false;
    }
    fn point(&self, range: CCursorRange) -> usize {
        if matches!(self.mode, Mode::Visual | Mode::Line) && range.primary.index > self.anchor {
            range.primary.index.saturating_sub(1)
        } else {
            range.primary.index
        }
    }
    fn move_to(&mut self, text: &Text<'_>, range: &mut CCursorRange, point: usize) {
        let point = text.normal(point);
        *range = match self.mode {
            Mode::Visual => {
                if point >= self.anchor {
                    Text::range(self.anchor, (point + 1).min(text.len))
                } else {
                    Text::range((self.anchor + 1).min(text.len), point)
                }
            }
            Mode::Line => {
                let (a, _) = text.line(self.anchor);
                let (b, end) = text.line(point);
                if b >= a {
                    Text::range(a, (end + 1).min(text.len))
                } else {
                    let (_, end) = text.line(self.anchor);
                    Text::range((end + 1).min(text.len), b)
                }
            }
            _ => Text::range(point, point),
        };
        self.scroll = true;
    }
    #[allow(clippy::too_many_arguments)] // Explicit operands keep operator state local to one key sequence.
    fn operate(
        &mut self,
        ctx: &egui::Context,
        text: &Text<'_>,
        range: &mut CCursorRange,
        op: char,
        selected: std::ops::Range<usize>,
        linewise: bool,
        native: &mut Vec<egui::Event>,
    ) -> bool {
        self.register(ctx, text, selected.clone(), linewise);
        self.reset_command();
        if op == 'y' {
            self.mode = Mode::Normal;
            *range = Text::range(selected.start, selected.start);
            self.normalize = true;
            return false;
        }
        self.mode = if op == 'c' {
            Mode::Insert
        } else {
            Mode::Normal
        };
        self.normalize = op != 'c';
        if selected.is_empty() {
            return false;
        }
        self.replace(text, range, selected, String::new(), native)
    }
    /// Interpret a Vim text command within the bound source editor, retaining incomplete operator/count state.
    pub(super) fn vim_char(
        &mut self,
        ctx: &egui::Context,
        text: &Text<'_>,
        range: &mut CCursorRange,
        c: char,
        native: &mut Vec<egui::Event>,
        effects: &mut Vec<NavAction>,
    ) -> bool {
        if let Some(command) = &mut self.command {
            if command.len() + c.len_utf8() <= 128 {
                command.push(c);
            }
            return false;
        }
        let point = self.point(*range).min(text.len);
        let count = self.count.max(1);
        let (start, end) = text.line(point);
        if c.is_ascii_digit() && (c != '0' || self.count > 0) {
            self.count = (self.count * 10 + c.to_digit(10).unwrap() as usize).min(999);
            return false;
        }
        if c == 'g' && !self.g {
            self.g = true;
            return false;
        }
        if let Some((op, op_count)) = self.operator
            && c == op
        {
            let selected = text.lines(point, count.saturating_mul(op_count).min(999));
            let selected = if op == 'c' {
                selected.start
                    ..selected.end.saturating_sub(usize::from(
                        selected.end > selected.start
                            && text.slice(selected.end - 1..selected.end) == "\n",
                    ))
            } else {
                selected
            };
            return self.operate(ctx, text, range, op, selected, true, native);
        }
        let total = count
            .saturating_mul(self.operator.map_or(1, |(_, n)| n))
            .min(999);
        let motion = match c {
            'h' => Some(point.saturating_sub(total).max(start)),
            'l' => Some((point + total).min(end)),
            'j' => Some(text.vertical(point, total as isize)),
            'k' => Some(text.vertical(point, -(total as isize))),
            'w' => Some(text.word(point, total, false, false)),
            'b' => Some(text.word(point, total, true, false)),
            'e' => Some(text.word(point, total, false, true)),
            '0' => Some(start),
            '^' => Some(text.first(point)),
            '$' => Some(end),
            'g' if self.g => Some(text.line_at(count - 1)),
            'G' => Some(if self.count == 0 {
                text.line_at(usize::MAX)
            } else {
                text.line_at(count - 1)
            }),
            _ => None,
        };
        if let Some(target) = motion {
            if let Some((op, _)) = self.operator {
                let linewise = matches!(c, 'j' | 'k' | 'g' | 'G');
                let mut selected = point.min(target)..point.max(target);
                if matches!(c, 'e') {
                    selected.end = (selected.end + 1).min(text.len);
                }
                if linewise {
                    selected.start = text.line(selected.start).0;
                    selected.end = (text.line(selected.end).1 + 1).min(text.len);
                }
                return self.operate(ctx, text, range, op, selected, linewise, native);
            }
            self.move_to(text, range, target);
            self.reset_command();
            return false;
        }
        if self.operator.is_some() || self.g {
            self.reset_command();
            return false;
        }
        if matches!(c, 'd' | 'c' | 'y') {
            if matches!(self.mode, Mode::Visual | Mode::Line) {
                return self.operate(
                    ctx,
                    text,
                    range,
                    c,
                    range.as_sorted_char_range(),
                    self.mode == Mode::Line,
                    native,
                );
            }
            self.operator = Some((c, count));
            self.count = 0;
            return false;
        }
        match c {
            'i' | 'a' | 'I' | 'A' => {
                let point = match c {
                    'a' => (point + 1).min(end),
                    'I' => text.first(point),
                    'A' => end,
                    _ => point,
                };
                *range = Text::range(point, point);
                self.mode = Mode::Insert;
                self.checkpoint = true;
            }
            'o' | 'O' => {
                let position = if c == 'o' { end } else { start };
                *range = Text::range(position, position);
                self.mode = Mode::Insert;
                if c == 'O' {
                    self.after = Some(start);
                }
                self.reset_command();
                return self.replace(text, range, position..position, "\n".into(), native);
            }
            'v' | 'V' => {
                if matches!(self.mode, Mode::Visual | Mode::Line) {
                    self.mode = Mode::Normal;
                    *range = Text::range(point, point);
                } else {
                    self.anchor = point;
                    self.mode = if c == 'v' { Mode::Visual } else { Mode::Line };
                    self.move_to(text, range, point);
                }
            }
            'x' | 'X' | 's' => {
                let selected = if matches!(self.mode, Mode::Visual | Mode::Line) {
                    range.as_sorted_char_range()
                } else if c == 'X' {
                    point.saturating_sub(count).max(start)..point
                } else {
                    point..(point + count).min(end)
                };
                return self.operate(
                    ctx,
                    text,
                    range,
                    if c == 's' { 'c' } else { 'd' },
                    selected,
                    self.mode == Mode::Line,
                    native,
                );
            }
            'D' | 'C' => {
                return self.operate(
                    ctx,
                    text,
                    range,
                    if c == 'D' { 'd' } else { 'c' },
                    point..end,
                    false,
                    native,
                );
            }
            'Y' => {
                return self.operate(
                    ctx,
                    text,
                    range,
                    'y',
                    text.lines(point, count),
                    true,
                    native,
                );
            }
            'p' | 'P' => {
                if self.register.is_empty() {
                    self.reset_command();
                    return false;
                }
                let Some(_bytes) = self
                    .register
                    .len()
                    .checked_mul(count)
                    .filter(|n| *n < LIMIT)
                else {
                    self.reset_command();
                    return false;
                };
                let mut value = self.register.repeat(count);
                let selected = if matches!(self.mode, Mode::Visual | Mode::Line) {
                    range.as_sorted_char_range()
                } else {
                    let at = if self.linewise {
                        if c == 'P' {
                            start
                        } else {
                            (end + 1).min(text.len)
                        }
                    } else if c == 'P' {
                        point
                    } else {
                        (point + 1).min(end)
                    };
                    at..at
                };
                if self.linewise
                    && selected.start == text.len
                    && !text.text.is_empty()
                    && !text.text.ends_with('\n')
                {
                    value.insert(0, '\n');
                    self.after = Some(selected.start + 1);
                } else {
                    self.after = Some(if self.linewise {
                        selected.start
                    } else {
                        selected.start + value.chars().count().saturating_sub(1)
                    });
                }
                self.mode = Mode::Normal;
                self.normalize = true;
                self.reset_command();
                return self.replace(text, range, selected, value, native);
            }
            'u' => {
                native.push(key_event_undo(false));
                self.reset_command();
                self.normalize = true;
                return true;
            }
            'n' | 'N' => {
                effects.push(if c == 'n' {
                    NavAction::NextMatch
                } else {
                    NavAction::PreviousMatch
                });
                self.reset_command();
                return true;
            }
            '/' => {
                effects.push(NavAction::Find);
                self.reset_command();
                return true;
            }
            ':' => {
                self.command = Some(String::new());
            }
            _ => {}
        }
        self.reset_command();
        false
    }
    #[allow(clippy::too_many_arguments)] // Keyboard context and outputs are shared with the character dispatcher.
    /// Interpret non-text Vim keys using the same native selection and edit-event machinery.
    pub(super) fn vim_key(
        &mut self,
        ctx: &egui::Context,
        text: &Text<'_>,
        range: &mut CCursorRange,
        key_code: Key,
        modifiers: egui::Modifiers,
        native: &mut Vec<egui::Event>,
        effects: &mut Vec<NavAction>,
    ) -> bool {
        if key_code == Key::Escape
            || (modifiers.ctrl && matches!(key_code, Key::OpenBracket | Key::C))
        {
            if self.mode == Mode::Insert {
                self.checkpoint_after = true;
                self.after = Some(range.primary.index.saturating_sub(1));
            } else {
                let point = self.point(*range);
                *range = Text::range(point, point);
            }
            self.mode = Mode::Normal;
            self.mark = false;
            self.command = None;
            self.reset_command();
            self.normalize = true;
            return false;
        }
        if let Some(command) = &mut self.command {
            if key_code == Key::Backspace {
                command.pop();
            }
            if key_code == Key::Enter {
                let command = self.command.take().unwrap();
                match command.trim() {
                    "w" | "write" => effects.push(NavAction::Save),
                    "q" | "quit" => effects.push(NavAction::CloseFile),
                    "e" | "edit" => effects.push(NavAction::OpenFile),
                    "help" => effects.push(NavAction::Help),
                    _ => {
                        self.notice = Some(format!(
                            "Unsupported Vim command :{command}. Use :w, :q, :e or :help."
                        ))
                    }
                }
                self.reset_command();
                return true;
            }
            return false;
        }
        if modifiers.is_none()
            && let Some(c) = match key_code {
                Key::ArrowLeft | Key::Backspace => Some('h'),
                Key::ArrowRight => Some('l'),
                Key::ArrowUp => Some('k'),
                Key::ArrowDown => Some('j'),
                Key::Home => Some('0'),
                Key::End => Some('$'),
                Key::Delete => Some('x'),
                _ => None,
            }
        {
            return self.vim_char(ctx, text, range, c, native, effects);
        }
        if modifiers.ctrl && key_code == Key::R {
            native.push(key_event_undo(true));
            self.normalize = true;
            return true;
        }
        let point = self.point(*range);
        let count = self.count.max(1);
        let next = match key_code {
            Key::ArrowLeft => Some(point.saturating_sub(count).max(text.line(point).0)),
            Key::ArrowRight => Some((point + count).min(text.line(point).1)),
            Key::ArrowUp => Some(text.vertical(point, -(count as isize))),
            Key::ArrowDown => Some(text.vertical(point, count as isize)),
            Key::Home => Some(text.line(point).0),
            Key::End => Some(text.line(point).1),
            Key::D | Key::F if modifiers.ctrl => Some(text.vertical(point, 12)),
            Key::U | Key::B if modifiers.ctrl => Some(text.vertical(point, -12)),
            Key::PageDown => Some(text.vertical(point, 20)),
            Key::PageUp => Some(text.vertical(point, -20)),
            _ => None,
        };
        if let Some(next) = next {
            self.move_to(text, range, next);
            self.reset_command();
        }
        false
    }
}
