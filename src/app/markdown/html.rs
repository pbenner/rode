//! Bounded, static HTML support for notebook Markdown. Convert semantic tags to
//! CommonMark before the ordinary scoped image/link preparation. Source cells
//! are never rewritten, and this adapter has no browser, CSS or resource loader.
use super::{MAX_BYTES, MAX_EVENTS, MdEvent, Parser, Tag, TagEnd, options};
use html5gum::{Token, Tokenizer, emitters::default::DefaultEmitter};

#[cfg(test)]
mod tests;

/// Preserve Markdown and code verbatim outside HTML parser spans. Adjacent HTML
/// spans are joined so multiline tags and raw-text elements tokenize together.
pub(super) fn markdown(source: &str) -> Result<String, String> {
    if source.len() > MAX_BYTES {
        return Err("Notebook Markdown preview is limited to 256 KiB.".into());
    }
    let mut ranges: Vec<(std::ops::Range<usize>, bool)> = Vec::new();
    let mut in_table = false;
    for (count, (event, range)) in Parser::new_ext(source, options())
        .into_offset_iter()
        .enumerate()
    {
        if count >= MAX_EVENTS {
            return Err("Notebook Markdown preview has too many elements.".into());
        }
        match &event {
            MdEvent::Start(Tag::Table(_)) => in_table = true,
            MdEvent::End(TagEnd::Table) => in_table = false,
            _ => {}
        }
        if matches!(event, MdEvent::Html(_) | MdEvent::InlineHtml(_)) {
            if let Some((previous, _)) = ranges
                .last_mut()
                .filter(|(previous, table)| previous.end == range.start && *table == in_table)
            {
                previous.end = range.end;
            } else {
                ranges.push((range, in_table));
            }
        }
    }
    let mut converted = Converter::default();
    let mut cursor = 0;
    for (range, in_table) in ranges {
        converted.markdown_table = in_table;
        converted.raw(&source[cursor..range.start]);
        let mut emitter = DefaultEmitter::default();
        emitter.naively_switch_states(true);
        for token in Tokenizer::new_with_emitter(&source[range.clone()], emitter).flatten() {
            converted.tokens += 1;
            if converted.tokens > MAX_EVENTS {
                return Err("Notebook HTML preview has too many elements.".into());
            }
            match token {
                Token::StartTag(tag) => {
                    let name = String::from_utf8_lossy(&tag.name);
                    let attribute = |name: &[u8]| {
                        tag.attributes
                            .get(name)
                            .map(|value| String::from_utf8_lossy(value).into_owned())
                            .unwrap_or_default()
                    };
                    converted.start(
                        &name,
                        attribute(b"href"),
                        attribute(b"src"),
                        attribute(b"alt"),
                        attribute(b"start"),
                        tag.self_closing,
                    )?;
                }
                Token::EndTag(tag) => converted.end(&String::from_utf8_lossy(&tag.name)),
                Token::String(text) => converted.text(&String::from_utf8_lossy(&text)),
                _ => {} // Comments, declarations and recoverable tokenizer errors have no display.
            }
            converted.check_size()?;
        }
        cursor = range.end;
    }
    converted.markdown_table = false;
    converted.raw(&source[cursor..]);
    while let Some(frame) = converted.stack.pop() {
        converted.close(frame.end);
    }
    converted.check_size()?;
    Ok(converted.output)
}

enum End {
    None,
    Marker(&'static str, usize),
    Link(String),
    Block,
    List,
    Item,
    Pre,
    Code,
    Table,
    Row,
    Cell,
    CellBlock,
}
struct Frame {
    name: String,
    end: End,
}
#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    cell: Option<String>,
}

struct List {
    next: Option<u64>,
    marker_width: usize,
}
impl Table {
    fn cell(&mut self) {
        if let Some(cell) = self.cell.take() {
            self.row.push(cell.trim().into());
        }
    }
    fn row(&mut self) {
        self.cell();
        if !self.row.is_empty() {
            self.rows.push(std::mem::take(&mut self.row));
        }
    }
}

#[derive(Default)]
struct Converter {
    output: String,
    stack: Vec<Frame>,
    lists: Vec<List>,
    table: Option<Table>,
    markdown_table: bool,
    pre: Option<String>,
    code: Option<String>,
    hidden: Option<(String, usize)>,
    tokens: usize,
    overflow: bool,
}
impl Converter {
    /// Inline HTML in a Markdown table follows the same row constraints as
    /// content inside an HTML td/th, even though its table is not on our stack.
    fn in_table_cell(&self) -> bool {
        self.markdown_table
            || self
                .table
                .as_ref()
                .is_some_and(|table| table.cell.is_some())
    }
    fn sink(&mut self) -> &mut String {
        if let Some(pre) = &mut self.pre {
            return pre;
        }
        if let Some(code) = &mut self.code {
            return code;
        }
        if let Some(cell) = self.table.as_mut().and_then(|table| table.cell.as_mut()) {
            return cell;
        }
        &mut self.output
    }
    fn raw(&mut self, text: &str) {
        if self.hidden.is_none() {
            if self.pre.is_some() || self.code.is_some() {
                // Markdown leaves text between inline HTML tags untouched.
                // Within HTML code, character references still mean characters.
                for token in Tokenizer::new(text).flatten() {
                    if let Token::String(text) = token {
                        self.sink().push_str(&String::from_utf8_lossy(&text));
                    }
                }
            } else {
                self.sink().push_str(text);
            }
        }
    }
    fn separate(&mut self, lines: usize) {
        let sink = self.sink();
        while sink.ends_with(' ') {
            sink.pop();
        }
        if !sink.is_empty() {
            let existing = sink.bytes().rev().take_while(|b| *b == b'\n').count();
            for _ in existing..lines {
                sink.push('\n');
            }
        }
    }
    fn marker(&mut self, marker: &'static str) -> End {
        self.raw(marker);
        End::Marker(marker, self.sink().len())
    }
    fn text(&mut self, text: &str) {
        if self.hidden.is_some() {
            return;
        }
        if self.pre.is_some() || self.code.is_some() {
            self.sink().push_str(text);
            return;
        }
        let sink = self.sink();
        for character in text.chars() {
            if character.is_ascii_whitespace() {
                if !sink.is_empty() && !sink.ends_with([' ', '\n']) {
                    sink.push(' ');
                }
            } else {
                if "\\`*_[]<>!|#".contains(character) {
                    sink.push('\\');
                }
                sink.push(character);
            }
        }
    }
    fn start(
        &mut self,
        name: &str,
        href: String,
        src: String,
        alt: String,
        start: String,
        self_closing: bool,
    ) -> Result<(), String> {
        if let Some((hidden, depth)) = &mut self.hidden {
            if hidden == name && !self_closing {
                *depth += 1;
            }
            return Ok(());
        }
        if matches!(
            name,
            "script" | "style" | "iframe" | "object" | "svg" | "math" | "head" | "template"
        ) {
            if !self_closing {
                self.hidden = Some((name.into(), 1));
            }
            return Ok(());
        }
        if matches!(
            name,
            "area"
                | "base"
                | "embed"
                | "input"
                | "link"
                | "meta"
                | "param"
                | "source"
                | "track"
                | "wbr"
        ) {
            return Ok(());
        }
        if self.pre.is_some() || self.code.is_some() {
            if !self_closing {
                self.push(name, End::None)?;
            }
            return Ok(());
        }
        let end = match name {
            "br" => {
                if self.in_table_cell() {
                    self.text(" ");
                } else {
                    self.raw("  \n");
                }
                return Ok(());
            }
            "hr" => {
                self.separate(2);
                self.raw("---\n\n");
                return Ok(());
            }
            "img" => {
                self.raw("![");
                self.text(&alt);
                self.raw(&format!("]({})", destination(&src)));
                return Ok(());
            }
            "b" | "strong" => self.marker("**"),
            "em" | "i" => self.marker("*"),
            "s" | "del" | "strike" => self.marker("~~"),
            "a" if !href.is_empty() => {
                self.raw("[");
                End::Link(destination(&href))
            }
            "code" | "kbd" | "samp" => {
                self.code = Some(String::new());
                End::Code
            }
            "pre" => {
                self.separate(2);
                self.pre = Some(String::new());
                End::Pre
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.separate(2);
                self.raw(&format!(
                    "{} ",
                    "#".repeat((name.as_bytes()[1] - b'0') as usize)
                ));
                End::Block
            }
            "p" | "div" | "section" | "article" | "header" | "footer" | "center" => {
                if self.stack.last().is_some_and(|frame| frame.name == "p") {
                    self.end("p");
                }
                if self.in_table_cell() {
                    // CommonMark tables cannot contain block paragraphs. Keep
                    // their text in the owning cell instead of breaking a row.
                    self.text(" ");
                    End::CellBlock
                } else {
                    // A paragraph immediately after a list marker belongs on
                    // that line; a blank line here would create an empty item.
                    if !(self.stack.last().is_some_and(|frame| frame.name == "li")
                        && self.sink().ends_with(' '))
                    {
                        self.separate(2);
                    }
                    End::Block
                }
            }
            "ul" | "ol" => {
                self.separate(if self.lists.is_empty() { 2 } else { 1 });
                self.lists.push(List {
                    next: (name == "ol").then(|| start.parse().unwrap_or(1)),
                    marker_width: 0,
                });
                End::List
            }
            "li" => {
                if self.stack.last().is_some_and(|frame| frame.name == "li") {
                    self.end("li");
                }
                self.separate(1);
                let marker = match self.lists.last_mut().and_then(|list| list.next.as_mut()) {
                    Some(number) => {
                        let marker = format!("{number}. ");
                        *number = number.saturating_add(1);
                        marker
                    }
                    _ => "- ".into(),
                };
                let indent: usize = self
                    .lists
                    .iter()
                    .rev()
                    .skip(1)
                    .map(|list| list.marker_width)
                    .sum();
                if let Some(list) = self.lists.last_mut() {
                    list.marker_width = marker.len();
                }
                self.raw(&format!("{}{marker}", " ".repeat(indent)));
                End::Item
            }
            "table" => {
                if self.table.is_some() {
                    return Err("Nested HTML tables are not supported in notebook previews.".into());
                }
                self.separate(2);
                self.table = Some(Table::default());
                End::Table
            }
            "tr" if self.table.is_some() => {
                self.table.as_mut().unwrap().row();
                End::Row
            }
            "td" | "th" if self.table.is_some() => {
                let table = self.table.as_mut().unwrap();
                table.cell();
                table.cell = Some(String::new());
                End::Cell
            }
            _ => End::None,
        };
        self.push(name, end)?;
        if self_closing {
            self.end(name);
        }
        Ok(())
    }
    fn push(&mut self, name: &str, end: End) -> Result<(), String> {
        if self.stack.len() >= 64 {
            return Err("Notebook HTML preview nesting exceeds 64 levels.".into());
        }
        self.stack.push(Frame {
            name: name.into(),
            end,
        });
        Ok(())
    }
    fn end(&mut self, name: &str) {
        if let Some((hidden, depth)) = &mut self.hidden {
            if hidden == name {
                *depth -= 1;
                if *depth == 0 {
                    self.hidden = None;
                }
            }
            return;
        }
        if let Some(index) = self.stack.iter().rposition(|frame| frame.name == name) {
            while self.stack.len() > index {
                let frame = self.stack.pop().unwrap();
                self.close(frame.end);
            }
        }
    }
    fn close(&mut self, end: End) {
        match end {
            End::None => {}
            End::Marker(marker, start) => {
                let sink = self.sink();
                if start >= marker.len()
                    && sink.get(start..).is_some()
                    && sink.get(start - marker.len()..start) == Some(marker)
                {
                    // HTML permits whitespace inside emphasis tags, whereas
                    // CommonMark requires it outside the delimiter pair.
                    let text = sink.split_off(start);
                    sink.truncate(start - marker.len());
                    let leading = text.len() - text.trim_start().len();
                    let content = text.trim();
                    sink.push_str(&text[..leading]);
                    if !content.is_empty() {
                        sink.push_str(marker);
                        sink.push_str(content);
                        sink.push_str(marker);
                        sink.push_str(&text[leading + content.len()..]);
                    }
                }
            }
            End::Link(destination) => self.raw(&format!("]({destination})")),
            End::Block => self.separate(2),
            End::CellBlock => self.text(" "),
            End::Item => self.separate(1),
            End::List => {
                self.lists.pop();
                self.separate(if self.lists.is_empty() { 2 } else { 1 });
            }
            End::Pre => {
                let text = self.pre.take().unwrap_or_default();
                let fence = fence(&text, 3);
                self.raw(&format!(
                    "{fence}\n{}\n{fence}\n\n",
                    text.trim_end_matches('\n')
                ));
            }
            End::Code => {
                let mut text = self.code.take().unwrap_or_default();
                if self.in_table_cell() {
                    // Table rows are split before inline code is parsed. The
                    // table parser removes one escape before each literal pipe;
                    // keep any backslashes that belong to the code itself.
                    text = text
                        .replace('|', "\\|")
                        .replace("\r\n", " ")
                        .replace(['\r', '\n'], " ");
                }
                let fence = fence(&text, 1);
                self.raw(&format!("{fence} {text} {fence}"));
            }
            End::Row => {
                if let Some(table) = &mut self.table {
                    table.row();
                }
            }
            End::Cell => {
                if let Some(table) = &mut self.table {
                    table.cell();
                }
            }
            End::Table => {
                let Some(mut table) = self.table.take() else {
                    return;
                };
                table.row();
                let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
                // Padding a ragged table must not multiply a small HTML input
                // into an unbounded rectangular Markdown table.
                if columns.saturating_mul(table.rows.len()) > MAX_EVENTS / 4 {
                    self.overflow = true;
                    return;
                }
                for (index, row) in table.rows.iter().enumerate() {
                    self.raw("|");
                    for column in 0..columns {
                        self.raw(row.get(column).map_or("", String::as_str));
                        self.raw("|");
                    }
                    self.raw("\n");
                    if index == 0 {
                        self.raw(&format!("|{}\n", "---|".repeat(columns)));
                    }
                }
                self.separate(2);
            }
        }
    }
    fn check_size(&self) -> Result<(), String> {
        let table = self.table.as_ref().map_or(0, |table| {
            table.rows.iter().flatten().map(String::len).sum::<usize>()
                + table.row.iter().map(String::len).sum::<usize>()
                + table.cell.as_ref().map_or(0, String::len)
        });
        if self.overflow
            || self.output.len()
                + self.pre.as_ref().map_or(0, String::len)
                + self.code.as_ref().map_or(0, String::len)
                + table
                > MAX_BYTES
        {
            return Err("Rendered notebook Markdown exceeds its 256 KiB preview limit.".into());
        }
        Ok(())
    }
}

/// Percent-escape Markdown delimiters, preserving URL schemes and Unicode. The
/// existing preview layer still decides which links/images may be accessed.
fn destination(url: &str) -> String {
    let mut output = String::new();
    for character in url.chars() {
        if character.is_ascii_control() || " <>\\()[]\"|".contains(character) {
            for byte in character.to_string().bytes() {
                output.push_str(&format!("%{byte:02X}"));
            }
        } else {
            output.push(character);
        }
    }
    output
}
fn fence(text: &str, minimum: usize) -> String {
    let mut longest = 0;
    let mut run = 0;
    for character in text.chars() {
        run = if character == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat(minimum.max(longest + 1))
}
