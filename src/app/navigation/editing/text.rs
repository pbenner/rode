//! Character-indexed motions; scans run only for keyboard input, not redraws.
//!
//! All exposed positions and ranges are Unicode character indices compatible with egui
//! cursors. Convert to byte offsets only at string slicing boundaries; do not use display
//! columns as byte indices.
use super::*;
/// Borrowed UTF-8 source adapter whose motions and ranges use character offsets;
/// byte conversion happens only at string slicing boundaries.
pub(super) struct Text<'a> {
    pub text: &'a str,
    pub len: usize,
}
impl<'a> Text<'a> {
    /// Borrow UTF-8 text and cache its Unicode scalar count for character-indexed motions.
    pub fn new(text: &'a str) -> Self {
        Self {
            text,
            len: text.chars().count(),
        }
    }
    /// Convert a zero-based character offset to a byte boundary, clamping offsets beyond the source
    /// to its byte length.
    pub fn byte(&self, index: usize) -> usize {
        self.text
            .char_indices()
            .nth(index)
            .map_or(self.text.len(), |(b, _)| b)
    }
    /// Borrow a character-indexed half-open range. Callers must supply an ordered range; conversion
    /// never splits a UTF-8 character.
    pub fn slice(&self, range: std::ops::Range<usize>) -> &str {
        &self.text[self.byte(range.start)..self.byte(range.end)]
    }
    /// Character offsets of the containing logical line, excluding its newline. A point beyond the
    /// text is clamped to its end.
    pub fn line(&self, point: usize) -> (usize, usize) {
        let byte = self.byte(point.min(self.len));
        let start = self.text[..byte].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[byte..]
            .find('\n')
            .map_or(self.text.len(), |n| byte + n);
        let start = self.text[..start].chars().count();
        (
            start,
            start + self.text[self.byte(start)..end].chars().count(),
        )
    }
    /// Character offset of a zero-based logical row; rows beyond the document resolve to the end of
    /// text.
    pub fn line_at(&self, row: usize) -> usize {
        let mut start = 0;
        for (i, line) in self.text.split('\n').enumerate() {
            if i == row {
                return start;
            }
            start += line.chars().count() + 1;
        }
        self.len
    }
    /// Move by a signed logical-line count while preserving the character column where the target
    /// line permits it.
    pub fn vertical(&self, point: usize, delta: isize) -> usize {
        let (start, _) = self.line(point);
        let row = self.text[..self.byte(start)]
            .bytes()
            .filter(|b| *b == b'\n')
            .count();
        let target = row.saturating_add_signed(delta);
        let start_target = self.line_at(target);
        let (_, end) = self.line(start_target);
        start_target + (point - start).min(end - start_target)
    }
    /// Clamp a Vim normal-mode cursor onto the last character of a nonempty line instead of its
    /// newline/end insertion position.
    pub fn normal(&self, point: usize) -> usize {
        let point = point.min(self.len);
        let (start, end) = self.line(point);
        point.min(if end > start { end - 1 } else { start })
    }
    /// Character offset of the first non-whitespace character on the containing line, or its end if
    /// the line is blank.
    pub fn first(&self, point: usize) -> usize {
        let (start, end) = self.line(point);
        start
            + self
                .slice(start..end)
                .chars()
                .take_while(|c| c.is_whitespace())
                .count()
    }
    /// Apply counted word motions over whitespace, identifier and punctuation runs. Inputs/outputs
    /// are character offsets; `end` selects a word-end motion.
    pub fn word(&self, point: usize, count: usize, back: bool, end: bool) -> usize {
        fn class(c: char) -> u8 {
            if c.is_whitespace() {
                0
            } else if c.is_alphanumeric() || c == '_' {
                1
            } else {
                2
            }
        }
        if back {
            let mut iter = self.text[..self.byte(point)].chars().rev().peekable();
            let mut pos = point;
            for _ in 0..count {
                while iter.peek().is_some_and(|c| class(*c) == 0) {
                    iter.next();
                    pos -= 1;
                }
                let Some(c) = iter.peek() else {
                    break;
                };
                let kind = class(*c);
                while iter.peek().is_some_and(|c| class(*c) == kind) {
                    iter.next();
                    pos -= 1;
                }
            }
            pos
        } else {
            let mut pos = point.min(self.len);
            let mut iter = self.text[self.byte(pos)..].chars().peekable();
            for _ in 0..count {
                if end {
                    if iter.next().is_some() {
                        pos += 1;
                    }
                } else if let Some(c) = iter.peek() {
                    let kind = class(*c);
                    while kind != 0 && iter.peek().is_some_and(|c| class(*c) == kind) {
                        iter.next();
                        pos += 1;
                    }
                }
                while iter.peek().is_some_and(|c| class(*c) == 0) {
                    iter.next();
                    pos += 1;
                }
                if end && let Some(c) = iter.peek() {
                    let kind = class(*c);
                    while iter.peek().is_some_and(|c| class(*c) == kind) {
                        iter.next();
                        pos += 1;
                    }
                }
            }
            if end { pos.saturating_sub(1) } else { pos }
        }
    }
    /// Half-open character range covering counted logical lines from the containing line, including
    /// a trailing newline when present.
    pub fn lines(&self, point: usize, count: usize) -> std::ops::Range<usize> {
        let (start, _) = self.line(point);
        let last = self.vertical(start, count.saturating_sub(1) as isize);
        let (_, end) = self.line(last);
        start..(end + usize::from(end < self.len))
    }
    /// Construct an egui cursor range from two character offsets, retaining their primary/secondary
    /// ordering.
    pub fn range(a: usize, b: usize) -> CCursorRange {
        CCursorRange::two(CCursor::new(a), CCursor::new(b))
    }
}
