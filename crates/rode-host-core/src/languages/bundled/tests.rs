//! Exercise the linked engines directly, independently of installed tools or PATH.
use super::*;

fn analyze_source(path: &str, source: &str, format: bool) -> Analysis {
    analyze(
        super::super::detect(Some(path)).unwrap(),
        path,
        source,
        format,
    )
    .unwrap()
}

#[test]
fn native_languages_format_unsaved_source_idempotently_and_check_without_edits() {
    for (path, source, expected) in [
        (
            "main.js",
            "const value={hello:'world'};",
            "const value = { hello: \"world\" };\n",
        ),
        (
            "main.jsx",
            "const view=<div>hello</div>;",
            "const view = <div>hello</div>;\n",
        ),
        (
            "main.ts",
            "const value:string='world';",
            "const value: string = \"world\";\n",
        ),
        (
            "main.tsx",
            "const view:V=<div>hello</div>;",
            "const view: V = <div>hello</div>;\n",
        ),
        (
            "data.json",
            "{\"hello\":\"世界 🦀\"}",
            "{ \"hello\": \"世界 🦀\" }\n",
        ),
        (
            "data.jsonc",
            "{\n// hello\n\"a\":1,\n}",
            "{\n  // hello\n  \"a\": 1,\n}\n",
        ),
        ("data.yaml", "a: [1,2,3]", "a: [1, 2, 3]\n"),
        (
            "index.html",
            "<div><p>hello</p></div>",
            "<div><p>hello</p></div>\n",
        ),
        ("style.css", "a{color:red}", "a {\n  color: red;\n}\n"),
        (
            "style.scss",
            "$color:red;a{color:$color}",
            "$color: red;\na {\n  color: $color;\n}\n",
        ),
        (
            "readme.md",
            "#  Heading\n\n* hello\n",
            "# Heading\n\n- hello\n",
        ),
    ] {
        let result = analyze_source(path, source, true);
        assert!(
            result.diagnostics.is_empty(),
            "{path}: {:?}",
            result.diagnostics
        );
        assert!(result.warnings.is_empty(), "{path}");
        assert_eq!(result.text, expected, "{path}");
        assert_eq!(
            analyze_source(path, &result.text, true).text,
            result.text,
            "{path}"
        );
        assert_eq!(analyze_source(path, source, false).text, source, "{path}");
    }
}

#[test]
fn syntax_errors_keep_source_and_report_scalar_positions_including_eof() {
    let mut positions = vec![];
    let mut expected = vec![];
    for (path, source, row, column) in [
        ("main.ts", "const text = '🦀'; const value = ;", 1, 33),
        ("main.jsx", "const x = <div>", 1, 12),
        ("data.json", "{\"🦀\": }", 1, 7),
        ("data.yaml", "a: [\n", 1, 4),
        ("index.html", "<div title=\"🦀\"><", 1, 17),
        ("style.css", "/* 🦀 */\na {", 2, 4),
        ("style.scss", "a { color: ", 1, 12),
    ] {
        for format in [true, false] {
            let result = analyze_source(path, source, format);
            assert_eq!(result.text, source, "{path}");
            assert!(!result.diagnostics.is_empty(), "{path}");
            let error = &result.diagnostics[0];
            positions.push((path, error.row, error.column));
            expected.push((path, row, column));
            assert_eq!(error.severity, "error");
            assert_eq!(result.warnings.is_empty(), !format);
        }
    }
    assert_eq!(positions, expected);
}

#[test]
fn preserves_bom_crlf_comments_and_import_and_package_order() {
    let source = "\u{feff}// hello 🦀\r\nimport z from 'z';\r\nimport a from 'a';\r\nconst value:string='世界';\r\n";
    let result = analyze_source("main.ts", source, true);
    assert!(result.text.starts_with("\u{feff}// hello 🦀\r\n"));
    assert!(result.text.contains("const value: string = \"世界\";\r\n"));
    assert!(result.text.find("from \"z\"").unwrap() < result.text.find("from \"a\"").unwrap());
    assert!(!result.text.replace("\r\n", "").contains('\n'));
    let result = analyze_source("package.json", "{\"z\":1,\"a\":2}", true);
    assert!(result.text.find("\"z\"").unwrap() < result.text.find("\"a\"").unwrap());
}

#[test]
fn formats_embedded_languages_and_keeps_incomplete_or_unknown_markdown_examples() {
    let result = analyze_source(
        "index.html",
        "<html><head><style>a{color:red}</style></head><body><script>const x=1;</script></body></html>",
        true,
    );
    assert!(result.diagnostics.is_empty());
    assert!(result.text.contains("color: red;"), "{}", result.text);
    assert!(result.text.contains("const x = 1;"), "{}", result.text);
    assert_eq!(
        analyze_source("index.html", &result.text, true).text,
        result.text
    );
    let source = "# Heading\n\n```ts\nconst x=1;\n```\n\n```ts\nconst x = ;\n```\n\n```unknown\n leave   this\n```\n";
    let result = analyze_source("readme.md", source, true);
    assert!(result.text.contains("const x = 1;"));
    assert!(result.text.contains("const x = ;"));
    assert!(result.text.contains(" leave   this"));
    let source = "<div>🦀</div>\n<script>const x = ;</script>";
    let result = analyze_source("index.html", source, true);
    assert_eq!(result.text, source);
    assert_eq!(
        (result.diagnostics[0].row, result.diagnostics[0].column),
        (2, 19)
    );
}

#[test]
fn offset_positions_clamp_unicode_and_trailing_newline() {
    assert_eq!(position("🦀\r\n", 1), (1, 1));
    assert_eq!(position("🦀\r\n", 4), (1, 2));
    assert_eq!(position("🦀\r\n", usize::MAX), (2, 1));
}
