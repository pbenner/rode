//! Protect TeX delimiters from Markdown escape processing, leaving code literal.
use pulldown_cmark::{Event, Options, Parser, Tag};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

/// One recognized TeX expression and whether it uses display layout.
pub(super) struct Formula {
    /// TeX source with its delimiters and normalized Markdown container prefixes.
    pub source: String,
    /// Whether the expression uses block/display layout instead of inline layout.
    pub display: bool,
}

/// Markdown rewritten with formula slots and their byte-offset identities.
/// Recorded guards distinguish inserted separators from literal source text.
pub(super) struct Prepared {
    /// Rewritten Markdown containing synthetic code slots for recognized formulas.
    pub text: String,
    /// Slot start byte offset in `text` mapped to its index in `formulas`.
    pub offsets: HashMap<usize, usize>,
    /// Byte offsets of inserted separators that the renderer may remove.
    pub guards: HashSet<usize>,
    /// Original expressions in slot order.
    pub formulas: Vec<Formula>,
}

/// Replace recognized math with renderer slots while preserving code/link regions.
/// Offsets and guards address UTF-8 bytes in the returned Markdown, not source character indices.
pub(super) fn prepare(source: &str, options: Options) -> Prepared {
    if !source.contains(['\\', '$']) {
        return Prepared {
            text: source.into(),
            offsets: HashMap::new(),
            guards: HashSet::new(),
            formulas: vec![],
        };
    }
    let mut protected = vec![];
    let mut spans = vec![];
    let mut maths = HashMap::new();
    let mut quotes = vec![];
    for (event, range) in Parser::new_ext(source, options | Options::ENABLE_MATH).into_offset_iter()
    {
        match event {
            Event::InlineMath(math) => {
                maths.insert(range.start, format!("${math}$"));
                spans.push((range.clone(), false));
            }
            Event::DisplayMath(math) => {
                maths.insert(range.start, format!("$${math}$$"));
                spans.push((range.clone(), true));
            }
            Event::Start(Tag::BlockQuote(_)) => quotes.push(range),
            Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::Start(Tag::CodeBlock(_) | Tag::Link { .. } | Tag::Image { .. }) => {
                protected.push(range)
            }
            _ => {}
        }
    }
    protected.sort_by_key(|range| range.start);
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut next = 0;
    let mut openings = [None, None];
    while i < bytes.len() {
        while next < protected.len() && protected[next].end <= i {
            next += 1;
        }
        if let Some(range) = protected.get(next)
            && range.contains(&i)
        {
            i = range.end;
            openings = [None, None];
            continue;
        }
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        match bytes.get(i + 1) {
            Some(b'(') => {
                openings[0].get_or_insert(i);
            }
            Some(b'[') => {
                openings[1].get_or_insert(i);
            }
            Some(close @ (b')' | b']')) => {
                let kind = usize::from(*close == b']');
                if let Some(start) = openings[kind].take() {
                    spans.push((start..i + 2, kind == 1));
                }
            }
            _ => {}
        }
        i += 2;
    }
    spans.sort_by_key(|(range, _)| range.start);
    let mut text = String::with_capacity(source.len());
    let mut formulas = vec![];
    let mut offsets = HashMap::new();
    let mut guards = HashSet::new();
    let mut cursor = 0;
    for (Range { start, end }, display) in spans {
        if start < cursor {
            continue;
        }
        text.push_str(&source[cursor..start]);
        // Keep adjacent formulas/code spans from merging their backtick delimiters.
        // The event renderer removes only these recorded synthetic characters.
        guards.insert(text.len());
        text.push('\u{200b}');
        // Identify generated code slots by parser offsets, never by their content.
        // Literal agent text cannot spoof a slot, even if it contains the same marker.
        offsets.insert(text.len(), formulas.len());
        text.push_str("`\u{e000}`");
        guards.insert(text.len());
        text.push('\u{200b}');
        formulas.push(Formula {
            // Dollar math events already remove Markdown container prefixes.
            // Apply the same treatment to bracket math before passing it to TeX.
            source: maths.remove(&start).unwrap_or_else(|| {
                let depth = quotes.iter().filter(|range| range.contains(&start)).count();
                strip_quote_prefixes(&source[start..end], depth)
            }),
            display,
        });
        cursor = end;
    }
    text.push_str(&source[cursor..]);
    Prepared {
        text,
        offsets,
        guards,
        formulas,
    }
}

fn strip_quote_prefixes(source: &str, depth: usize) -> String {
    if depth == 0 {
        return source.into();
    }
    source
        .split_inclusive('\n')
        .enumerate()
        .map(|(index, mut line)| {
            if index > 0 {
                for _ in 0..depth {
                    let Some(rest) = line.trim_start_matches([' ', '\t']).strip_prefix('>') else {
                        break;
                    };
                    line = rest.strip_prefix(' ').unwrap_or(rest);
                }
            }
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_both_delimiter_styles_without_consuming_code_or_links() {
        let source = r"Inline \(v\mapsto Rv\), $x_i$, display \[a=b\] and $$c=d$$.
`\(literal\)` and `$literal$` and [link](https://example.invalid/\(path\)).

```latex
\[code\] $code$
```

    \(indented\)

Escaped \\(literal\\) and \$5.";
        let prepared = prepare(source, Options::empty());
        assert_eq!(
            prepared
                .formulas
                .iter()
                .map(|formula| formula.source.as_str())
                .collect::<Vec<_>>(),
            [r"\(v\mapsto Rv\)", "$x_i$", r"\[a=b\]", "$$c=d$$"]
        );
        assert_eq!(
            prepared
                .formulas
                .iter()
                .map(|formula| formula.display)
                .collect::<Vec<_>>(),
            [false, false, true, true]
        );
        assert!(prepared.text.contains(r"\[code\] $code$"));
        assert!(prepared.text.contains(r"\(indented\)"));
    }

    #[test]
    fn streaming_unicode_and_marker_collisions_preserve_source() {
        for source in [r"α \(x", r"α \[x", "`\u{e000}rode-math-0` and \\(literal)"] {
            assert_eq!(prepare(source, Options::empty()).text, source);
        }
        let source = "`\u{e000}rode-math-0` and \\(x\\)";
        let prepared = prepare(source, Options::empty());
        assert_eq!(prepared.formulas[0].source, r"\(x\)");
        assert_eq!(prepared.offsets.len(), 1);
    }

    #[test]
    fn multiline_math_excludes_markdown_quote_prefixes() {
        for (source, expected) in [
            ("> $$\n> x_i > 0\n> $$", "$$\nx_i > 0\n$$"),
            ("> \\[\n> x_i > 0\n> \\]", "\\[\nx_i > 0\n\\]"),
            ("> > \\[\n> > x_i > 0\n> > \\]", "\\[\nx_i > 0\n\\]"),
        ] {
            let prepared = prepare(source, Options::empty());
            assert_eq!(prepared.formulas.len(), 1);
            assert_eq!(prepared.formulas[0].source, expected);
        }
    }
}
