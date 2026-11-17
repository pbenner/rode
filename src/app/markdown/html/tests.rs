use super::*;
use pulldown_cmark::{HeadingLevel, Tag};

#[test]
fn semantic_html_and_markdown_share_headings_emphasis_lists_and_tables() {
    let source = "# Markdown\n\n<div><h2>HTML &amp; café</h2><p>A <b>bold</b> and <em>gentle</em> introduction.<br>Next line.</p><ol start='3'><li>Alpha</li><li>Beta</li></ol><table><tr><th>Name</th><th>Value</th></tr><tr><td>λ</td><td>42</td></tr></table></div>\n\nMore **Markdown** with <strong>inline HTML</strong>.";
    let rendered = markdown(source).unwrap();
    let events: Vec<_> = Parser::new_ext(&rendered, options()).collect();
    for level in [HeadingLevel::H1, HeadingLevel::H2] {
        assert!(events.iter().any(|event| matches!(event, MdEvent::Start(Tag::Heading { level: found, .. }) if *found == level)), "{rendered}");
    }
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Start(Tag::Strong)))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Start(Tag::Emphasis)))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Start(Tag::List(Some(3))))),
        "{rendered}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Start(Tag::Table(_)))),
        "{rendered}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::HardBreak)),
        "{rendered}"
    );
    let text = events
        .iter()
        .filter_map(|event| match event {
            MdEvent::Text(text) => Some(text.as_ref()),
            _ => None,
        })
        .collect::<String>();
    for expected in [
        "HTML & café",
        "Alpha",
        "Beta",
        "λ",
        "42",
        "More",
        "inline HTML",
    ] {
        assert!(text.contains(expected), "{expected}: {rendered}");
    }
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, MdEvent::Html(_) | MdEvent::InlineHtml(_)))
    );
}

#[test]
fn attributes_entities_and_literal_code_cannot_create_unintended_markup() {
    let source = "<p><a\n href='../guide.md?x=1&amp;y=2'>Guide</a> <img src='attachment:plot one.png' alt='A ] figure'></p>\n\n```html\n<script>literal code</script>\n```\n\n<pre>&lt;img src='private.png'&gt; &#96;&#96;&#96;</pre>\n\n<code>a_b &amp; `x`</code>";
    let rendered = markdown(source).unwrap();
    let events: Vec<_> = Parser::new_ext(&rendered, options()).collect();
    assert!(events.iter().any(|event| matches!(event, MdEvent::Start(Tag::Link {dest_url, ..}) if dest_url.as_ref() == "../guide.md?x=1&y=2")), "{rendered}");
    let images: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            MdEvent::Start(Tag::Image { dest_url, .. }) => Some(dest_url.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(images, ["attachment:plot%20one.png"], "{rendered}");
    assert!(rendered.contains("<script>literal code</script>"));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Code(text) if text.as_ref() == "a_b & `x`")),
        "{rendered}"
    );
    assert!(events.iter().any(|event| matches!(event, MdEvent::Text(text) if text.contains("<img src='private.png'> ```"))), "{rendered}");
}

#[test]
fn html_whitespace_nested_lists_and_table_paragraphs_keep_their_structure() {
    let source = "<p>Start <strong> bold </strong> and <em> gentle </em>.</p><ol start='10'><li><p>Outer</p><ul><li>Inner</li></ul></li><li>Next</li></ol><table><tr><td><p>First</p><p>Second</p></td><td>Value</td></tr></table>";
    let rendered = markdown(source).unwrap();
    let events: Vec<_> = Parser::new_ext(&rendered, options()).collect();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Start(Tag::Strong))),
        "{rendered}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Start(Tag::Emphasis))),
        "{rendered}"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, MdEvent::Start(Tag::Item)))
            .count(),
        3,
        "{rendered}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Start(Tag::Table(_)))),
        "{rendered}"
    );
    assert!(rendered.contains("|First Second|Value|"), "{rendered}");
}

#[test]
fn table_code_and_url_pipes_cannot_split_columns() {
    let source = "<table><tr><th>Code</th><th>Escaped</th><th>Link</th><th>Image</th></tr><tr><td><code>a|b\nc</code></td><td><code>a\\|b</code></td><td><a href='https://example.org/a|b'>Link</a></td><td><img src='plot|one.png'></td></tr></table>";
    let rendered = markdown(source).unwrap();
    let events: Vec<_> = Parser::new_ext(&rendered, options()).collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, MdEvent::Start(Tag::TableCell)))
            .count(),
        8,
        "{rendered}"
    );
    let code: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            MdEvent::Code(code) => Some(code.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(code, ["a|b c", "a\\|b"], "{rendered}");
    assert!(events.iter().any(|event| matches!(event, MdEvent::Start(Tag::Link { dest_url, .. }) if dest_url.as_ref() == "https://example.org/a%7Cb")), "{rendered}");
    assert!(events.iter().any(|event| matches!(event, MdEvent::Start(Tag::Image { dest_url, .. }) if dest_url.as_ref() == "plot%7Cone.png")), "{rendered}");
}

#[test]
fn inline_html_keeps_markdown_tables_intact() {
    let source = "| Notes | Value |\n| --- | --- |\n| First<br>Second | <strong>Bold</strong> |\n| <code>a\\|b</code> | <a href='https://example.org/'>Link</a> |\n\nAfter the table.";
    let rendered = markdown(source).unwrap();
    let events: Vec<_> = Parser::new_ext(&rendered, options()).collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, MdEvent::Start(Tag::TableRow)))
            .count(),
        2,
        "{rendered}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Text(text) if text.as_ref() == "First Second")),
        "{rendered}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Start(Tag::Strong))),
        "{rendered}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, MdEvent::Code(code) if code.as_ref() == "a\\|b")),
        "{rendered}"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, MdEvent::Text(text) if text.as_ref() == "After the table.")
        ),
        "{rendered}"
    );
}

#[test]
fn inert_content_and_malformed_tags_stay_bounded() {
    let source = "<div>Visible<!-- comment --><script>bad('<img src=secret.png>')</script><style>bad{}</style><iframe src='https://example.org'>hidden</iframe><p>Kept";
    let rendered = markdown(source).unwrap();
    assert!(rendered.contains("Visible") && rendered.contains("Kept"));
    for ignored in ["secret.png", "example.org", "hidden", "bad", "comment"] {
        assert!(!rendered.contains(ignored), "{rendered}");
    }
    assert!(markdown(&format!("{}x{}", "<span>".repeat(65), "</span>".repeat(65))).is_err());
    assert!(markdown(&"x".repeat(MAX_BYTES + 1)).is_err());
    let ragged = format!(
        "<table><tr>{}</tr>{}</table>",
        "<td>x</td>".repeat(100),
        "<tr><td>x</td></tr>".repeat(100)
    );
    assert!(markdown(&ragged).is_err());
    // Malformed formatting can cross table-cell buffers; byte offsets from the
    // previous buffer must never truncate a Unicode scalar in the next one.
    assert!(markdown("<table><strong>abc<td>€€€€€</strong></td></table>").is_ok());
}
