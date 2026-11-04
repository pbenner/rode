//! Preset inheritance is a bounded graph, independent of presentation order.
use super::*;

#[test]
fn forward_bases_and_multiple_levels_preserve_independent_theme_roles() {
    // A menu may list variants before their base. Each layer changes one role.
    let definitions = [
        (
            "child",
            r##"{"name":"Child","base":"parent","colors":{"accent":"#abbccd"},"ui":{"control_radius":4}}"##,
        ),
        (
            "grandchild",
            r#"{"name":"Grandchild","base":"child","chrome":"bracketed"}"#,
        ),
        (
            "parent",
            r##"{"name":"Parent","colors":{"background":"#121314"},"syntax":{"comment":"#abcdef"},"ui":{}}"##,
        ),
    ];
    let mut chain = Vec::new();
    let theme = resolve("grandchild", &definitions, &mut chain).unwrap();
    assert!(
        chain.is_empty(),
        "Resolution must release all ancestry state"
    );
    assert_eq!(theme.id, "grandchild");
    assert_eq!(theme.name, "Grandchild");
    assert_eq!(theme.palette.background, parse_color("#121314").unwrap());
    assert_eq!(theme.palette.accent, parse_color("#abbccd").unwrap());
    assert_eq!(theme.palette.chrome, Chrome::Bracketed);
    assert_eq!(theme.palette.ui.unwrap().control_radius, 4);
    assert_eq!(
        theme.palette.ui.unwrap().button_padding,
        super::super::InterfaceStyle::default().button_padding
    );
    assert_eq!(theme.syntax["comment"], parse_color("#abcdef").unwrap());
    assert_eq!(theme.palette.ansi, Palette::default().ansi);
}

#[test]
fn missing_bases_and_cycles_fail_without_leaving_resolution_state() {
    for (definitions, reason) in [
        (
            vec![("first", r#"{"name":"First","base":"missing"}"#)],
            "missing",
        ),
        (
            vec![("first", r#"{"name":"First","base":"first"}"#)],
            "cycle",
        ),
        (
            vec![
                ("first", r#"{"name":"First","base":"second"}"#),
                ("second", r#"{"name":"Second","base":"first"}"#),
            ],
            "cycle",
        ),
    ] {
        let mut chain = Vec::new();
        let error = resolve("first", &definitions, &mut chain)
            .err()
            .expect("Invalid inheritance must fail");
        assert!(format!("{error:#}").contains(reason), "{error:#}");
        assert!(chain.is_empty(), "Failed resolution left ancestry state");
    }
}
