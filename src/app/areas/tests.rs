//! Navigation state and independently merged application preferences.
use super::*;
use workbench::{Surface, ToolTab};

#[derive(Default)]
struct Storage(HashMap<String, String>);
impl eframe::Storage for Storage {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.into(), value);
    }
    fn flush(&mut self) {}
}

#[test]
fn global_areas_preserve_project_selection_drafts_and_unsaved_source() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    app.new_file();
    app.documents[0].fixture_content().text = "unsaved source".into();
    app.agents.chat.notice("retained conversation");
    app.toggle_panel(Panel::Agent, &ctx);
    for area in [
        Area::Connections,
        Area::Settings,
        Area::Stocks,
        Area::Monitor,
        Area::Transfers,
    ] {
        app.open_area(area, &ctx);
        assert_eq!(app.areas.active, area);
        assert_eq!(app.workbench.surface, Surface::Tools);
        assert_eq!(app.workbench.panel, Some(Panel::Agent));
        assert!(!app.tools_visible());
        assert_eq!(
            app.agents.chat.messages.back().unwrap().text,
            "retained conversation"
        );
        assert_eq!(app.documents[0].text, "unsaved source");
        assert!(app.documents[0].dirty());
        app.open_area(Area::Projects, &ctx);
        assert!(app.tools_visible());
        assert_eq!(app.workbench.panel, Some(Panel::Agent));
    }
}

#[test]
fn scope_registry_keeps_host_and_application_tools_out_of_project_extras() {
    for tab in ToolTab::ALL {
        if let ToolTab::Panel(panel) = tab {
            assert_eq!(
                Area::for_panel(panel).is_some(),
                tab.scope() != Scope::Project
            );
            if let Some(area) = Area::for_panel(panel) {
                assert!(Area::RAIL.contains(&area));
                assert_eq!(area.panel(), Some(panel));
                assert!(!tab.project_extra());
            }
        }
    }
    assert_eq!(
        ToolTab::ALL
            .into_iter()
            .filter(|t| t.project_extra())
            .count(),
        4
    );
}

#[test]
fn pins_are_bounded_validated_and_unrelated_window_edits_are_merged() {
    let mut storage = Storage::default();
    storage.0.insert(
        "project_toolbar_pins".into(),
        r#"["Monitor","Tasks","Tasks","Debugger","Problems"]"#.into(),
    );
    let mut pins = State::load(Some(&storage));
    assert_eq!(pins.prefs.pinned, ["Tasks", "Debugger"]);
    pins.pin(ToolTab::Panel(Panel::Problems), true);
    assert_eq!(pins.prefs.pinned, ["Tasks", "Debugger"]);
    let mut monitor = State::load(Some(&storage));
    monitor.monitor_configured = true;
    monitor.prefs.monitor_interval = 7;
    monitor.pending = true;
    monitor.store(&mut storage);
    pins.pin(ToolTab::Panel(Panel::Tasks), false);
    pins.pin(ToolTab::Panel(Panel::Problems), true);
    pins.store(&mut storage);
    let loaded = State::load(Some(&storage));
    assert_eq!(loaded.prefs.monitor_interval, 7);
    assert_eq!(loaded.prefs.pinned, ["Debugger", "Problems"]);
    // An interval-only edit in another window must not restore an obsolete pin list.
    monitor.prefs.monitor_interval = 9;
    monitor.pending = true;
    monitor.store(&mut storage);
    assert_eq!(
        State::load(Some(&storage)).prefs.pinned,
        ["Debugger", "Problems"]
    );
}

#[test]
fn global_root_navigation_does_not_hide_a_secondary_editors_explorer() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    let child = app.allocate_editor_window();
    app.open_area(Area::Connections, &ctx);
    for viewport in [egui::ViewportId::ROOT, child] {
        let mut input = egui::RawInput {
            viewport_id: viewport,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900., 700.),
            )),
            ..Default::default()
        };
        input.viewports.insert(viewport, Default::default());
        let _ = ctx.run(input, |ctx| {
            app.in_editor(viewport, |app| {
                app.navigation.begin_layout();
                assert_eq!(app.editing_allowed(ctx), viewport == child);
                app.explorer(ctx);
                assert_eq!(
                    app.navigation.rect(navigation::Pane::Explorer).is_some(),
                    viewport == child
                );
            })
        });
    }
    assert_eq!(app.areas.active, Area::Connections);
}

#[test]
fn global_navigation_keeps_visible_detached_project_tools_refreshing() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    app.toggle_panel(Panel::Problems, &ctx);
    app.workbench.detached = true;
    app.open_area(Area::Connections, &ctx);
    assert!(app.tools_visible());
    assert!(app.panel_visible(Panel::Problems));
    app.workbench.tools_hidden = true;
    assert!(!app.panel_visible(Panel::Problems));
    assert_eq!(app.areas.active, Area::Connections);
}

#[test]
fn the_project_tools_shortcut_stays_in_projects_when_no_tool_is_selected() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    app.focus_pane(navigation::Pane::Tools, &ctx);
    assert_eq!(app.areas.active, Area::Projects);
    assert_eq!(app.workbench.panel, Some(Panel::Problems));
    assert!(app.tools_visible());
}

#[test]
fn invoking_a_remembered_project_tool_from_a_global_area_reveals_it() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    for panel in [Panel::Problems, Panel::Agent, Panel::Git, Panel::Settings] {
        app.toggle_panel(panel, &ctx);
        app.open_area(Area::Connections, &ctx);
        app.toggle_panel(panel, &ctx);
        assert_eq!(app.areas.active, Area::Projects);
        assert_eq!(app.workbench.panel, Some(panel));
        assert!(app.tools_visible());
        app.open_area(Area::Connections, &ctx);
        app.reveal_panel(panel, &ctx);
        assert_eq!(app.areas.active, Area::Projects);
        assert!(app.tools_visible());
        // The ordinary project command keeps its existing toggle behavior.
        app.toggle_panel(panel, &ctx);
        assert!(app.workbench.panel.is_none());
    }
}

#[test]
fn settings_hide_disabled_tool_pins_without_forgetting_the_saved_choice() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    app.areas.pin(ToolTab::Panel(Panel::Debug), true);
    app.open_area(Area::Settings, &ctx);
    for enabled in [false, true] {
        app.set_extension(crate::extensions::Feature::Debugger.id(), enabled);
        let output = ctx.run(Default::default(), |ctx| app.global_content(ctx));
        let visible = output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == "Debugger"));
        assert_eq!(visible, enabled);
        assert_eq!(app.areas.prefs.pinned, ["Debugger"]);
    }
}

#[test]
fn compact_activity_areas_do_not_overlap_the_fixed_footer() {
    for size in [
        egui::vec2(850., 560.),
        egui::vec2(425., 280.),
        egui::vec2(320., 210.),
    ] {
        let ctx = egui::Context::default();
        let mut app = EditorApp::new(
            &eframe::CreationContext::_new_kittest(ctx.clone()),
            Startup::default(),
        );
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("rail.json");
        ui_probe::install_at(&ctx, path.clone());
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| {
                    app.titlebar(ctx);
                    app.activity(ctx);
                },
            );
        }
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let buttons = state["buttons"].as_array().unwrap();
        let footer = buttons
            .iter()
            .filter(|b| {
                b["name"].as_str().is_some_and(|name| {
                    matches!(
                        name,
                        "activity:App settings" | "activity:Extensions…" | "activity:Help"
                    )
                })
            })
            .map(|b| b["rect"][1].as_f64().unwrap())
            .reduce(f64::min)
            .unwrap();
        for button in buttons.iter().filter(|b| {
            b["name"].as_str().is_some_and(|name| {
                Area::RAIL
                    .into_iter()
                    .any(|area| name == format!("activity:{}", area.command().definition().label))
            })
        }) {
            assert!(
                button["rect"][3].as_f64().unwrap() <= footer,
                "Area overlaps footer at {size:?}: {button}"
            );
        }
    }
}

#[test]
fn late_file_reads_keep_global_areas_visible_and_existing_edits_intact() {
    for area in [
        Area::Connections,
        Area::Monitor,
        Area::Transfers,
        Area::Stocks,
        Area::Settings,
    ] {
        for existing_alias in [false, true] {
            let ctx = egui::Context::default();
            let mut app = EditorApp::new(
                &eframe::CreationContext::_new_kittest(ctx.clone()),
                Startup::default(),
            );
            app.new_file();
            let old = app.documents[0].id;
            app.documents[0].fixture_content().text = "unsaved original".into();
            let canonical = "/project/late.txt";
            let mut destination = egui::ViewportId::ROOT;
            if existing_alias {
                app.documents[0].fixture_content().path = Some(canonical.into());
                destination = app.allocate_editor_window();
            }
            app.open_area(area, &ctx);
            assert!(
                !app.navigation.used,
                "Exercise mouse navigation before keyboard pane mode"
            );
            app.submit_editor_operation(
                editor_operations::Operation::Opened {
                    path: if existing_alias {
                        "/project/alias.txt"
                    } else {
                        canonical
                    }
                    .into(),
                    viewport: destination,
                    response: Box::new(Response {
                        path: canonical.into(),
                        text: "late disk reply".into(),
                        revision: "r1".into(),
                        ..Default::default()
                    }),
                },
                &ctx,
            );
            assert_eq!(
                app.areas.active, area,
                "Late file completion stole the application area"
            );
            assert_eq!(app.documents.by_id(old).unwrap().text, "unsaved original");
            assert!(app.documents.by_id(old).unwrap().dirty());
            let loaded = app
                .documents
                .iter()
                .find(|doc| doc.path.as_deref() == Some(canonical))
                .unwrap();
            assert_eq!(app.documents.owner(loaded.id), Some(egui::ViewportId::ROOT));
            if !existing_alias {
                assert_eq!(loaded.text, "late disk reply");
            }
        }
    }
}
