//! Navigation state and independently merged application preferences.
use super::*;
use tool_windows::{Surface, ToolTab};

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
        assert_eq!(app.tool_windows.surface, Surface::Tools);
        assert_eq!(app.tool_windows.panel, Some(Panel::Agent));
        assert!(!app.tools_visible());
        assert_eq!(
            app.agents.chat.messages.back().unwrap().text,
            "retained conversation"
        );
        assert_eq!(app.documents[0].text, "unsaved source");
        assert!(app.documents[0].dirty());
        app.open_area(Area::Projects, &ctx);
        assert!(app.tools_visible());
        assert_eq!(app.tool_windows.panel, Some(Panel::Agent));
    }
}

#[test]
fn scope_registry_keeps_host_and_application_tools_out_of_project_tabs() {
    for tab in ToolTab::ALL {
        if let ToolTab::Area(area) = tab {
            assert_eq!(area.tab(), Some(tab));
            assert_eq!(tab.scope(), Scope::Application);
        }
        if let ToolTab::Panel(panel) = tab {
            assert_eq!(
                Area::for_panel(panel).is_some(),
                tab.scope() != Scope::Project
            );
            if let Some(area) = Area::for_panel(panel) {
                assert!(Area::RAIL.contains(&area));
                assert_eq!(area.panel(), Some(panel));
            }
        }
    }
    assert_eq!(
        ToolTab::ALL
            .into_iter()
            .filter(|t| t.scope() == Scope::Project)
            .count(),
        7
    );
}

#[test]
fn area_windows_preserve_project_state_and_raise_without_replacing_the_root_view() {
    for detached_tools in [false, true] {
        for area in [
            Area::Connections,
            Area::Settings,
            Area::Monitor,
            Area::Transfers,
            Area::Stocks,
        ] {
            let ctx = egui::Context::default();
            let mut app = EditorApp::new(
                &eframe::CreationContext::_new_kittest(ctx.clone()),
                Startup::default(),
            );
            app.new_file();
            let document = app.documents[0].id;
            app.documents[0].fixture_content().text = "unsaved source".into();
            app.agents.chat.notice("retained conversation");
            app.toggle_panel(Panel::Agent, &ctx);
            app.tool_windows.detached = detached_tools;
            app.open_area(area, &ctx);
            app.detach_area(area, &ctx);
            let tab = area.tab().unwrap();
            let owner = app.tool_viewport(tab);
            assert_ne!(owner, egui::ViewportId::ROOT);
            assert_ne!(owner, EditorApp::tools_viewport());
            assert_eq!(app.floating_tab(owner), Some(tab));
            assert_eq!(app.areas.active, Area::Projects);

            // Repeated requests reuse the sole window and leave the root view alone.
            app.detach_area(area, &ctx);
            app.open_area(area, &ctx);
            assert_eq!(app.tool_viewport(tab), owner);
            assert_eq!(app.areas.active, Area::Projects);
            assert_eq!(app.tool_windows.surface, Surface::Tools);
            assert_eq!(app.tool_windows.panel, Some(Panel::Agent));
            app.reattach_tool_tab(tab, &ctx);
            assert!(!app.area_detached(area));
            assert_eq!(app.tool_viewport(tab), egui::ViewportId::ROOT);
            assert_eq!(app.areas.active, area);
            assert_eq!(app.tool_windows.panel, Some(Panel::Agent));
            assert_eq!(app.tool_windows.detached, detached_tools);
            assert_eq!(app.documents.owner(document), Some(egui::ViewportId::ROOT));
            assert_eq!(
                app.documents.by_id(document).unwrap().text,
                "unsaved source"
            );
            assert!(app.documents.by_id(document).unwrap().dirty());
            assert_eq!(
                app.agents.chat.messages.back().unwrap().text,
                "retained conversation"
            );
        }
    }
}

#[test]
fn application_dialogs_return_with_their_area_and_other_windows_keep_their_owner() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    app.open_area(Area::Settings, &ctx);
    app.set_dialog(Dialog::Shortcuts);
    app.detach_area(Area::Settings, &ctx);
    let settings = Area::Settings.tab().unwrap();
    let owner = app.tool_viewport(settings);
    assert_eq!(app.dialog_viewport, owner);
    app.detach_area(Area::Connections, &ctx);
    let connections = Area::Connections.tab().unwrap();
    assert_ne!(app.tool_viewport(connections), owner);
    assert_eq!(app.dialog_viewport, owner);
    app.reattach_tool_tab(connections, &ctx);
    assert_eq!(app.dialog_viewport, owner);
    assert!(app.foreign_modal(&ctx));
    app.reattach_tool_tab(settings, &ctx);
    assert_eq!(app.dialog_viewport, egui::ViewportId::ROOT);
    assert!(!app.foreign_modal(&ctx));
    assert!(app.dialog == Dialog::Shortcuts);
}

#[test]
fn monitor_updates_leave_obsolete_toolbar_preferences_untouched() {
    let mut storage = Storage::default();
    let legacy = r#"["Tasks","Debugger"]"#;
    storage
        .0
        .insert("project_toolbar_pins".into(), legacy.into());
    let mut monitor = State::load(Some(&storage));
    monitor.monitor_configured = true;
    monitor.prefs.monitor_interval = 7;
    monitor.pending = true;
    monitor.store(&mut storage);
    assert_eq!(State::load(Some(&storage)).prefs.monitor_interval, 7);
    assert_eq!(
        storage.0.get("project_toolbar_pins").map(String::as_str),
        Some(legacy)
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
    app.tool_windows.detached = true;
    app.open_area(Area::Connections, &ctx);
    assert!(app.tools_visible());
    assert!(app.panel_visible(Panel::Problems));
    app.tool_windows.tools_hidden = true;
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
    assert_eq!(app.tool_windows.panel, Some(Panel::Problems));
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
        assert_eq!(app.tool_windows.panel, Some(panel));
        assert!(app.tools_visible());
        app.open_area(Area::Connections, &ctx);
        app.reveal_panel(panel, &ctx);
        assert_eq!(app.areas.active, Area::Projects);
        assert!(app.tools_visible());
        // The ordinary project command keeps its existing toggle behavior.
        app.toggle_panel(panel, &ctx);
        assert!(app.tool_windows.panel.is_none());
    }
}

#[test]
fn project_tabs_wrap_without_overflow_and_hide_disabled_extensions() {
    for (width, font_scale) in [
        (1250., 1.),
        (850., 1.),
        (480., 1.),
        (850., 1.5),
        (480., 1.5),
    ] {
        for debug in [false, true] {
            for mode in 0..3 {
                let ctx = egui::Context::default();
                let mut app = EditorApp::new(
                    &eframe::CreationContext::_new_kittest(ctx.clone()),
                    Startup::default(),
                );
                ctx.style_mut(|style| {
                    style
                        .text_styles
                        .get_mut(&egui::TextStyle::Button)
                        .unwrap()
                        .size *= font_scale;
                    style.spacing.button_padding *= font_scale;
                    style.spacing.interact_size.y *= font_scale;
                });
                app.new_file();
                if mode > 0 {
                    app.reveal_panel(Panel::Problems, &ctx);
                    app.tool_windows.detached = mode == 2;
                }
                app.set_extension(crate::extensions::Feature::Debugger.id(), debug);
                let temp = tempfile::tempdir().unwrap();
                let path = temp.path().join("tabs.json");
                ui_probe::install_at(&ctx, path.clone());
                let mut content_top = 0.;
                for _ in 0..3 {
                    let _ = ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width, 700.),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            app.navigation.begin_layout();
                            app.titlebar(ctx);
                            content_top = ctx.available_rect().top();
                            app.activity(ctx);
                            egui::SidePanel::left("fixture-explorer")
                                .exact_width(168.)
                                .show(ctx, |_| {});
                            app.workspace_content(ctx);
                        },
                    );
                }
                let state: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                let buttons = state["buttons"].as_array().unwrap();
                let has = |name: &str| buttons.iter().any(|b| b["name"] == name);
                let pane = if mode == 1 {
                    navigation::Pane::Tools
                } else {
                    navigation::Pane::Editor
                };
                let top = app.navigation.rect(pane).unwrap().top();
                if mode == 1 {
                    assert!(top > content_top + 20., "Tool tabs have no reserved space");
                } else {
                    assert!(
                        (top - content_top).abs() < 1.,
                        "Closed/detached tools still consume editor space: {state}"
                    );
                }
                for name in ["Chat", "Terminal", "Git", "Problems", "Tasks", "Settings"] {
                    assert_eq!(
                        has(&format!("workspace-tab:{name}")),
                        mode == 1,
                        "Missing {name} at {width}"
                    );
                }
                assert_eq!(has("workspace-tab:Debugger"), debug && mode == 1);
                for name in [
                    "workspace-sidebar",
                    "workspace-explorer",
                    "open-tunnels",
                    "open-project",
                    "connect-ssh",
                ] {
                    assert!(has(name), "Navigation control {name} missing: {state}");
                }
                let rect = |button: &serde_json::Value| {
                    let r: [f32; 4] = serde_json::from_value(button["rect"].clone()).unwrap();
                    egui::Rect::from_min_max(egui::pos2(r[0], r[1]), egui::pos2(r[2], r[3]))
                };
                let mut previous = None;
                for name in [
                    "activity:Projects",
                    "workspace-explorer",
                    "workspace-sidebar",
                ] {
                    let bounds = rect(
                        buttons
                            .iter()
                            .find(|button| button["name"] == name)
                            .unwrap(),
                    );
                    assert!(
                        bounds.left() >= 0. && bounds.right() <= 52. && bounds.top() >= content_top,
                        "Project control is not in the activity rail: {state}"
                    );
                    if let Some(bottom) = previous {
                        assert!(
                            bounds.top() >= bottom,
                            "Project rail controls overlap: {state}"
                        );
                    }
                    previous = Some(bounds.bottom());
                }
                let header: Vec<_> = buttons
                    .iter()
                    .filter(|button| {
                        button["name"].as_str().is_some_and(|name| {
                            name.starts_with("menu-")
                                || matches!(
                                    name,
                                    "workspace-search"
                                        | "open-tunnels"
                                        | "open-project"
                                        | "connect-ssh"
                                )
                        })
                    })
                    .map(rect)
                    .collect();
                for (i, bounds) in header.iter().enumerate() {
                    assert!(
                        bounds.left() >= 0.
                            && bounds.right() <= width
                            && bounds.top() >= 0.
                            && bounds.bottom() <= content_top,
                        "Header control outside its reserved space: {state}"
                    );
                    assert!(
                        header[i + 1..]
                            .iter()
                            .all(|other| !bounds.intersects(*other)),
                        "Overlapping header controls: {state}"
                    );
                }
                assert!(!has("workspace-code"));
                assert!(!has("workspace-tools"));
                assert!(!has("workspace-project-menu"));
                assert_eq!(has("workspace-detach"), mode == 1);
                assert!(!has("workspace-dock"));
                assert_eq!(has("workspace-detach-tab:Problems"), mode == 1);
                assert_eq!(
                    has("workspace-window-separator"),
                    mode == 1,
                    "{width}/{font_scale}/{mode}: {state}"
                );
                assert!(!has("workspace-tab:Environment"));
                let rects: Vec<egui::Rect> = buttons
                    .iter()
                    .filter(|b| {
                        b["name"].as_str().is_some_and(|name| {
                            name.starts_with("workspace-tab:")
                                || name.starts_with("workspace-detach")
                                || matches!(name, "workspace-dock" | "workspace-window-separator")
                        })
                    })
                    .map(rect)
                    .collect();
                if mode == 1 {
                    let right = rects.iter().map(|rect| rect.right()).fold(0., f32::max);
                    assert!(
                        (right - (width - theme::PANEL_MARGIN.right as f32)).abs() < 2.,
                        "Window controls are not right-aligned: {state}"
                    );
                }
                for (i, rect) in rects.iter().enumerate() {
                    assert!(
                        rect.left() >= 220. && rect.right() <= width,
                        "Tab outside available width: {state}"
                    );
                    assert!(
                        rects[i + 1..].iter().all(|other| !rect.intersects(*other)),
                        "Overlapping tabs: {state}"
                    );
                }
            }
        }
    }
}

#[test]
fn compact_activity_areas_do_not_overlap_the_fixed_footer() {
    for size in [
        egui::vec2(850., 560.),
        egui::vec2(425., 280.),
        egui::vec2(320., 210.),
    ] {
        for project in [false, true] {
            let ctx = egui::Context::default();
            let mut app = EditorApp::new(
                &eframe::CreationContext::_new_kittest(ctx.clone()),
                Startup::default(),
            );
            if project {
                app.new_file();
            }
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
                        .map(Area::command)
                        .chain([Command::Explorer, Command::Sidebar])
                        .any(|command| name == format!("activity:{}", command.definition().label))
                })
            }) {
                assert!(
                    button["rect"][3].as_f64().unwrap() <= footer,
                    "Area overlaps footer at {size:?}: {button}"
                );
            }
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
