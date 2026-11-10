//! Standard source commands use one native replacement and undo checkpoint per
//! operation. Selection endpoints are character offsets, including reversed ranges.
use super::*;
use std::ops::Range;

/// Native source-edit command selected by the standard keyboard preset.
#[derive(Clone, Copy)]
pub(super) enum Action {
    Indent,
    Outdent,
    Tab,
    Comment,
    Duplicate,
    Delete,
    Select,
    Above,
    Below,
    Redo,
}

/// Map an exact pressed-key/modifier combination to a standard edit command.
/// Unrelated input returns `None` so native text handling can retain it.
pub(super) fn action(event: &egui::Event) -> Option<Action> {
    let egui::Event::Key {
        key,
        modifiers,
        pressed: true,
        ..
    } = event
    else {
        return None;
    };
    if modifiers.matches_exact(egui::Modifiers::COMMAND) {
        match key {
            Key::CloseBracket => Some(Action::Indent),
            Key::OpenBracket => Some(Action::Outdent),
            Key::Slash => Some(Action::Comment),
            Key::L => Some(Action::Select),
            Key::Enter => Some(Action::Below),
            Key::Y => Some(Action::Redo),
            _ => None,
        }
    } else if modifiers.matches_exact(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT) {
        match key {
            Key::D => Some(Action::Duplicate),
            Key::K => Some(Action::Delete),
            Key::Enter => Some(Action::Above),
            _ => None,
        }
    } else if *key == Key::Tab && modifiers.matches_exact(egui::Modifiers::NONE) {
        Some(Action::Tab)
    } else if *key == Key::Tab && modifiers.matches_exact(egui::Modifiers::SHIFT) {
        Some(Action::Outdent)
    } else {
        None
    }
}

/// A selection ending at column zero does not include that following line.
fn lines(text: &Text<'_>, selection: CCursorRange) -> Range<usize> {
    let range = selection.as_sorted_char_range();
    let last = if range.is_empty() {
        range.end
    } else {
        range.end - 1
    };
    let start = text.line(range.start).0;
    let end = text.line(last).1;
    start..(end + usize::from(end < text.len))
}

/// Follow existing leading whitespace using a bounded sample; empty source uses
/// four spaces, except languages whose conventional indentation is a literal tab.
pub(super) fn indentation(source: &str, path: Option<&str>) -> &'static str {
    let mut width = 4;
    let mut spaces = false;
    for line in source.lines().take(200) {
        if line.trim().is_empty() {
            continue;
        }
        if line.starts_with('\t') && !spaces {
            return "\t";
        }
        let count = line.bytes().take_while(|b| *b == b' ').count();
        if count > 0 {
            spaces = true;
            width = width.min(count);
        }
    }
    if !spaces
        && crate::languages::detect(path)
            .is_some_and(|l| matches!(l.id.as_str(), "go" | "makefile"))
    {
        "\t"
    } else {
        &"    "[..width]
    }
}

fn comment_prefix(path: Option<&str>) -> Option<&'static str> {
    match crate::languages::detect(path)?.id.as_str() {
        "rust" | "go" | "c" | "cpp" | "javascript" | "jsx" | "typescript" | "tsx" | "java"
        | "csharp" | "php" | "swift" | "kotlin" | "dart" | "scala" | "scss" | "sass" => Some("//"),
        "python" | "bash" | "zsh" | "fish" | "ruby" | "julia" | "r" | "perl" | "elixir"
        | "toml" | "yaml" | "dockerfile" | "makefile" | "cmake" | "gitignore" | "dotenv" => {
            Some("#")
        }
        "lua" | "haskell" | "sql" => Some("--"),
        "fortran" => Some("!"),
        "latex" => Some("%"),
        _ => None,
    }
}

impl State {
    fn standard_replace(
        &mut self,
        text: &Text<'_>,
        range: &mut CCursorRange,
        target: Range<usize>,
        value: String,
        after: CCursorRange,
        native: &mut Vec<egui::Event>,
    ) -> bool {
        if text.slice(target.clone()) == value {
            return false;
        }
        if self.replace(text, range, target, value, native) {
            self.after_range = Some(after);
            true
        } else {
            self.notice = Some("This edit would exceed the source buffer limit.".into());
            false
        }
    }

    /// Translate a source-edit command into selection changes and native undoable events.
    /// Ranges use character indices. Return `true` to defer remaining input until
    /// queued native text/undo events have been applied.
    pub(super) fn standard(
        &mut self,
        text: &Text<'_>,
        range: &mut CCursorRange,
        action: Action,
        path: Option<&str>,
        indent: &str,
        native: &mut Vec<egui::Event>,
    ) -> bool {
        if matches!(action, Action::Redo) {
            native.push(key_event_undo(true));
            return true;
        }
        let block = lines(text, *range);
        let source = text.slice(block.clone());
        let newline = if text.text.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let collapsed = |point| Text::range(point, point);
        match action {
            Action::Select => {
                let end = if range.as_sorted_char_range() == block
                    && !block.is_empty()
                    && block.end < text.len
                {
                    lines(text, collapsed(block.end)).end
                } else {
                    block.end
                };
                *range = Text::range(block.start, end);
                false
            }
            Action::Duplicate => {
                let value = if source.ends_with('\n') {
                    source.to_owned()
                } else {
                    format!("{newline}{source}")
                };
                let shift = value.chars().count();
                let mut after = *range;
                after.primary.index += shift;
                after.secondary.index += shift;
                self.standard_replace(text, range, block.end..block.end, value, after, native)
            }
            Action::Delete => {
                let mut target = block;
                if target.end == text.len && !source.ends_with('\n') && target.start > 0 {
                    target.start -= 1;
                    if target.start > 0 && text.slice(target.start - 1..target.start) == "\r" {
                        target.start -= 1;
                    }
                }
                let after = collapsed(target.start);
                self.standard_replace(text, range, target, String::new(), after, native)
            }
            Action::Above | Action::Below => {
                let above = matches!(action, Action::Above);
                let line = if above {
                    block.start
                } else {
                    block.end.saturating_sub(usize::from(!block.is_empty()))
                };
                let start = text.line(line).0;
                let leading: String = text
                    .slice(start..text.line(line).1)
                    .chars()
                    .take_while(|c| matches!(c, ' ' | '\t'))
                    .collect();
                let at = if above { block.start } else { block.end };
                let (value, point) = if above || source.ends_with('\n') {
                    (format!("{leading}{newline}"), at + leading.len())
                } else {
                    (
                        format!("{newline}{leading}"),
                        at + newline.len() + leading.len(),
                    )
                };
                self.standard_replace(text, range, at..at, value, collapsed(point), native)
            }
            Action::Tab if range.is_empty() => {
                let point = range.primary.index;
                let value = if indent == "\t" {
                    indent
                } else {
                    let column = point - text.line(point).0;
                    &indent[..indent.len() - column % indent.len()]
                };
                self.standard_replace(
                    text,
                    range,
                    point..point,
                    value.into(),
                    collapsed(point + value.len()),
                    native,
                )
            }
            Action::Indent | Action::Outdent | Action::Tab | Action::Comment => {
                let comment = matches!(action, Action::Comment);
                let prefix = if comment {
                    let Some(prefix) = comment_prefix(path) else {
                        self.notice =
                            Some("Line commenting is not available for this file type.".into());
                        return false;
                    };
                    prefix
                } else {
                    ""
                };
                let uncomment = comment
                    && source
                        .lines()
                        .filter(|s| !s.trim().is_empty())
                        .all(|s| s.trim_start_matches([' ', '\t']).starts_with(prefix));
                let comment_text = format!("{prefix} ");
                let mut value = String::with_capacity(source.len());
                let mut offset = block.start;
                let mut delta = [0_isize; 2];
                let points = [range.primary.index, range.secondary.index];
                // split_inclusive omits a trailing empty line, but an empty caret
                // line still accepts indentation.
                for line in source
                    .split_inclusive('\n')
                    .chain(source.is_empty().then_some(""))
                {
                    let leading = line
                        .bytes()
                        .take_while(|b| matches!(b, b' ' | b'\t'))
                        .count();
                    let (at, removed, added) = if comment && line.trim().is_empty() {
                        (0, 0, "")
                    } else if comment && uncomment {
                        let rest = &line[leading + prefix.len()..];
                        (
                            leading,
                            prefix.len() + usize::from(rest.starts_with(' ')),
                            "",
                        )
                    } else if comment {
                        (leading, 0, comment_text.as_str())
                    } else if matches!(action, Action::Outdent) {
                        (
                            0,
                            if line.starts_with('\t') {
                                1
                            } else {
                                line.bytes()
                                    .take(if indent == "\t" { 4 } else { indent.len() })
                                    .take_while(|b| *b == b' ')
                                    .count()
                            },
                            "",
                        )
                    } else {
                        (0, 0, indent)
                    };
                    value.push_str(&line[..at]);
                    value.push_str(added);
                    value.push_str(&line[at + removed..]);
                    for (point, delta) in points.iter().zip(&mut delta) {
                        if *point >= offset + at {
                            *delta +=
                                added.len() as isize - (*point - offset - at).min(removed) as isize;
                        }
                    }
                    offset += line.chars().count();
                }
                let mut after = *range;
                after.primary.index = points[0].saturating_add_signed(delta[0]);
                after.secondary.index = points[1].saturating_add_signed(delta[1]);
                self.standard_replace(text, range, block, value, after, native)
            }
            Action::Redo => unreachable!(),
        }
    }
}
