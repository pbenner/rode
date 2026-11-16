//! Native fallback through both executable entry points with no external tools in PATH.
use super::*;

fn analyze_peer(
    root: &Path,
    path: &Path,
    source: &str,
    format: bool,
    desktop: bool,
) -> anyhow::Result<project::Analysis> {
    let request = analyze_request(root, path, source, format);
    let mut command = if desktop {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
        command.arg("--file-helper");
        command
    } else {
        native_peer::command()
    };
    command.env("PATH", root.join("empty-path"));
    let Response::Analysis(analysis) = remote_command(&request, command)? else {
        panic!()
    };
    Ok(analysis)
}

#[test]
fn bundled_web_formatters_work_on_desktop_and_headless_hosts_without_any_tool_on_path() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("empty-path")).unwrap();
    // Fallback defaults never execute a project's JavaScript configuration.
    fs::write(
        root.path().join(".prettierrc.cjs"),
        "require('fs').writeFileSync('executed-config', 'bad'); throw Error('must not run')",
    )
    .unwrap();
    for (name, source, token) in [
        ("a.js", "const value={a:1};", "const value ="),
        ("a.jsx", "const view=<div />;", "const view ="),
        ("a.ts", "const value:number=1;", "value: number"),
        ("a.tsx", "const view:V=<div />;", "view: V ="),
        ("a.MTS", "const value:number=1;", "value: number"),
        ("a.CTS", "const value:number=1;", "value: number"),
        ("a.JSON", "{\"value\":1}", "\"value\": 1"),
        ("a.jsonc", "{\n// comment\n\"value\":1,\n}", "// comment"),
        ("a.yaml", "value: [1,2]", "[1, 2]"),
        ("a.htm", "<div  class='test'>hello</div>", "class=\"test\""),
        ("a.css", "a{color:red}", "color: red;"),
        ("a.scss", "$color:red;a{color:$color}", "$color: red;"),
        ("a.md", "#  Heading\n\n* hello", "# Heading\n\n- hello"),
    ] {
        let path = root.path().join(name);
        fs::write(&path, "untouched disk source").unwrap();
        let mut previous = None;
        for desktop in [false, true] {
            let result = analyze_peer(root.path(), &path, source, true, desktop).unwrap();
            assert!(result.text.contains(token), "{name}: {}", result.text);
            assert_ne!(result.text, source, "{name}");
            assert!(
                result.diagnostics.is_empty(),
                "{name}: {:?}",
                result.diagnostics
            );
            assert!(result.warnings.is_empty(), "{name}: {:?}", result.warnings);
            let check = analyze_peer(root.path(), &path, source, false, desktop).unwrap();
            assert_eq!(check.text, source);
            assert!(check.diagnostics.is_empty());
            if let Some(previous) = previous {
                assert_eq!(result.text, previous, "{name}");
            }
            previous = Some(result.text);
            assert_eq!(fs::read_to_string(&path).unwrap(), "untouched disk source");
        }
    }
    for (name, source) in [
        ("bad.ts", "const x = ;"),
        ("bad.json", "{\"🦀\": }"),
        ("bad.yaml", "value: ["),
        ("bad.html", "<div><"),
        ("bad.scss", "a { color: "),
    ] {
        for desktop in [false, true] {
            let path = root.path().join(name);
            let result = analyze_peer(root.path(), &path, source, true, desktop).unwrap();
            assert_eq!(result.text, source, "{name}");
            assert!(!result.diagnostics.is_empty(), "{name}");
            assert!(!path.exists());
        }
    }
    assert!(!root.path().join("executed-config").exists());
}

#[test]
fn installed_prettier_errors_and_success_remain_authoritative_except_missing_runtimes() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("node_modules/.bin")).unwrap();
    fs::create_dir(root.path().join("empty-path")).unwrap();
    let prettier = root.path().join("node_modules/.bin/prettier");
    let source = "const x=1;";
    let path = root.path().join("a.ts");
    for desktop in [false, true] {
        fs::write(
            &prettier,
            "#!/bin/sh\nprintf 'installed formatter chosen\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&prettier, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            analyze_peer(root.path(), &path, source, true, desktop)
                .unwrap()
                .text,
            "installed formatter chosen\n"
        );
        fs::write(
            &prettier,
            "#!/bin/sh\nprintf 'broken configuration\\n' >&2\nexit 2\n",
        )
        .unwrap();
        let error = analyze_peer(root.path(), &path, source, true, desktop).unwrap_err();
        assert!(
            format!("{error:#}").contains("broken configuration"),
            "{error:#}"
        );
        for script in ["#!/usr/bin/env node\n", "#!/missing/interpreter\n"] {
            fs::write(&prettier, script).unwrap();
            assert_eq!(
                analyze_peer(root.path(), &path, source, true, desktop)
                    .unwrap()
                    .text,
                "const x = 1;\n"
            );
        }
        fs::set_permissions(&prettier, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(analyze_peer(root.path(), &path, source, true, desktop).is_err());
    }
    assert!(!path.exists());
}
