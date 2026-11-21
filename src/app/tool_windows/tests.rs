use super::*;

fn render(app: &mut EditorApp, ctx: &egui::Context) {
    render_width(app, ctx, 1200.);
}

fn render_width(app: &mut EditorApp, ctx: &egui::Context, width: f32) {
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 800.),
            )),
            ..Default::default()
        },
        |ctx| {
            app.navigation.begin_layout();
            app.titlebar(ctx);
            app.activity(ctx);
            app.explorer(ctx);
            app.workspace_content(ctx);
        },
    );
}

#[test]
fn detached_headers_offer_only_tool_navigation_and_reattachment() {
    for individual in [false, true] {
        let ctx = egui::Context::default();
        let mut app = EditorApp::new(
            &eframe::CreationContext::_new_kittest(ctx.clone()),
            Startup::default(),
        );
        app.toggle_panel(Panel::Agent, &ctx);
        app.tool_windows.detached = !individual;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("header.json");
        ui_probe::install_at(&ctx, path.clone());
        for _ in 0..3 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(680., 420.),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if individual {
                            app.detached_tab_header(ToolTab::Panel(Panel::Agent), ui, ctx);
                        } else {
                            app.workspace_tabs(ui, ctx, true);
                        }
                    });
                },
            );
        }
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let has = |name: &str| {
            state["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b["name"] == name)
        };
        for name in ["workspace-code", "workspace-explorer", "workspace-sidebar"] {
            assert!(
                !has(name),
                "Root control {name} leaked into a tool window: {state}"
            );
        }
        assert_eq!(has("workspace-tab:Chat"), !individual);
        assert_eq!(has("workspace-dock"), !individual);
        assert_eq!(has("workspace-reattach:Chat"), individual);
    }
}

#[test]
fn project_tools_can_reopen_after_the_remembered_tab_detaches_without_starting_a_shell() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    app.new_file();
    app.documents[0].fixture_content().text = "unsaved source".into();
    app.agents.chat.notice("retained conversation");
    app.toggle_sidebar(&ctx);
    assert_eq!(app.tool_windows.panel, Some(Panel::Agent));
    app.detach_tab(ToolTab::Panel(Panel::Agent), &ctx);
    assert_eq!(app.tool_windows.surface, Surface::Editor);
    app.toggle_sidebar(&ctx);
    assert!(app.tools_visible());
    assert_eq!(app.tool_windows.panel, Some(Panel::Problems));
    assert!(app.tab_detached(ToolTab::Panel(Panel::Agent)));
    assert!(app.terminal.is_none() && !app.agents.chat.busy);
    app.toggle_sidebar(&ctx);
    assert_eq!(app.tool_windows.surface, Surface::Editor);
    assert_eq!(
        app.tool_windows.last_tool,
        Some(ToolTab::Panel(Panel::Problems))
    );
    assert_eq!(app.documents[0].text, "unsaved source");
    assert!(app.documents[0].dirty());
    assert_eq!(
        app.agents.chat.messages.back().unwrap().text,
        "retained conversation"
    );
}

#[test]
fn reopening_tools_does_not_start_a_terminal_when_enabled_panels_are_all_detached() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    let mut workspace = crate::app::tests::remote_workspace();
    workspace.connected = false;
    app.workspace.set_fixture(Some(workspace));
    app.toggle_panel(Panel::Agent, &ctx);
    for tab in ToolTab::ALL {
        if matches!(tab, ToolTab::Panel(panel) if tab.scope() == areas::Scope::Project && panel != Panel::Agent && app.panel_enabled(panel))
        {
            app.detach_tab(tab, &ctx);
        }
    }
    app.reveal_panel(Panel::Agent, &ctx);
    app.set_extension(crate::extensions::Feature::Agents.id(), false);
    assert_eq!(
        app.tool_windows.last_tool,
        Some(ToolTab::Panel(Panel::Agent))
    );
    assert!(!app.show_terminal);
    let windows = app.tool_windows.floating_tabs.clone();
    app.toggle_sidebar(&ctx);
    assert!(
        !app.show_terminal,
        "Opening tool navigation requested an unrelated terminal"
    );
    assert_ne!(app.tool_windows.surface, Surface::Terminal);
    assert_eq!(app.tool_windows.floating_tabs, windows);
    assert!(app.tab_detached(ToolTab::Panel(Panel::Problems)));
}

#[test]
fn individual_tabs_keep_their_state_and_do_not_replace_other_windows_selections() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].fixture_content().text = "unsaved source".into();
    app.agents.chat.notice("retained conversation");
    app.toggle_panel(Panel::Agent, &ctx);
    app.detach_tab(ToolTab::Panel(Panel::Agent), &ctx);
    app.detach_tab(ToolTab::Panel(Panel::Settings), &ctx);
    app.toggle_panel(Panel::Problems, &ctx);
    render(&mut app, &ctx);
    assert!(app.panel_visible(Panel::Agent));
    assert!(app.panel_visible(Panel::Settings));
    assert!(app.panel_visible(Panel::Problems));
    assert_ne!(
        app.tool_viewport(ToolTab::Panel(Panel::Agent)),
        app.tool_viewport(ToolTab::Panel(Panel::Settings))
    );

    // Activity icons/shortcuts raise the floating tab without stealing the dock selection.
    app.toggle_panel(Panel::Agent, &ctx);
    app.detach_tab(ToolTab::Panel(Panel::Agent), &ctx);
    assert_eq!(app.tool_windows.panel, Some(Panel::Problems));
    assert_eq!(app.tool_windows.floating_tabs.len(), 2);
    assert!(app.terminal.is_none());

    app.reattach_tool_tab(ToolTab::Panel(Panel::Agent), &ctx);
    assert_eq!(app.tool_windows.panel, Some(Panel::Agent));
    assert!(app.tools_visible());
    assert!(app.tab_detached(ToolTab::Panel(Panel::Settings)));
    assert_eq!(
        app.agents.chat.messages.back().unwrap().text,
        "retained conversation"
    );
    assert_eq!(app.documents[0].text, "unsaved source");
    assert!(app.documents[0].dirty());
}

#[test]
fn detaching_an_unselected_terminal_keeps_the_sidebar_selection_and_visibility() {
    for (detached, hidden) in [(false, false), (false, true), (true, false), (true, true)] {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = EditorApp::new(&cc, Startup::default());
        let mut workspace = crate::app::tests::remote_workspace();
        workspace.connected = false;
        app.workspace.set_fixture(Some(workspace));
        app.toggle_panel(Panel::Settings, &ctx);
        app.tool_windows.detached = detached;
        app.tool_windows.tools_hidden = hidden;

        // Even a failed shell startup must leave the other tool selected.
        app.detach_tab(ToolTab::Terminal, &ctx);
        assert_eq!(app.tool_windows.surface, Surface::Tools);
        assert_eq!(app.tool_windows.panel, Some(Panel::Settings));
        assert_eq!(app.tool_windows.tools_hidden, hidden);
        assert_eq!(app.tools_visible(), !hidden);
        assert_eq!(app.tool_windows.detached, detached);
        assert!(app.tab_detached(ToolTab::Terminal));
    }
}

#[test]
fn individual_tabs_and_whole_sidebar_can_detach_and_dock_independently() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.toggle_panel(Panel::Settings, &ctx);
    app.tool_windows.detached = true;
    app.detach_tab(ToolTab::Panel(Panel::Settings), &ctx);
    assert!(app.tool_windows.detached);
    assert!(app.tools_visible());
    assert_ne!(app.tool_windows.panel, Some(Panel::Settings));
    app.reattach_tool_tab(ToolTab::Panel(Panel::Settings), &ctx);
    assert!(app.tool_windows.detached);
    assert_eq!(
        app.tool_viewport(ToolTab::Panel(Panel::Settings)),
        EditorApp::tools_viewport()
    );

    app.detach_tab(ToolTab::Panel(Panel::Settings), &ctx);
    app.dock_tools(&ctx);
    assert!(app.tab_detached(ToolTab::Panel(Panel::Settings)));
    assert!(!app.tool_windows.detached);
    app.workbench.reset_project();
    assert!(app.tab_detached(ToolTab::Panel(Panel::Settings)));
    app.reattach_tool_tab(ToolTab::Panel(Panel::Settings), &ctx);
    assert!(!app.tab_detached(ToolTab::Panel(Panel::Settings)));
    assert_eq!(app.tool_windows.panel, Some(Panel::Settings));
}

#[test]
fn tab_dialog_owner_follows_only_its_own_tab_when_docking() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let settings = ToolTab::Panel(Panel::Settings);
    let chat = ToolTab::Panel(Panel::Agent);
    app.detach_tab(settings, &ctx);
    app.detach_tab(chat, &ctx);
    app.ui_viewport = app.tool_viewport(settings);
    app.set_dialog(Dialog::Shortcuts);
    app.tool_windows.detached = true;
    app.dock_tools(&ctx);
    assert_eq!(app.dialog_viewport, settings.viewport());
    app.reattach_tool_tab(chat, &ctx);
    assert_eq!(app.dialog_viewport, settings.viewport());
    assert!(app.foreign_modal(&ctx));
    app.reattach_tool_tab(settings, &ctx);
    assert_eq!(app.dialog_viewport, egui::ViewportId::ROOT);
    assert!(!app.foreign_modal(&ctx));
    assert!(app.dialog == Dialog::Shortcuts);
}

#[test]
fn a_tool_command_from_a_floating_tab_focuses_the_requested_tools_host() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    let chat = ToolTab::Panel(Panel::Agent);
    app.detach_tab(chat, &ctx);
    app.toggle_panel(Panel::Problems, &ctx);
    let mut input = egui::RawInput {
        viewport_id: chat.viewport(),
        ..Default::default()
    };
    input.viewports.insert(chat.viewport(), Default::default());
    let output = ctx.run(input, |ctx| app.toggle_panel(Panel::Settings, ctx));
    assert_eq!(app.tool_windows.panel, Some(Panel::Settings));
    assert!(app.tools_visible());
    assert!(app.tab_detached(chat));
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .iter()
            .any(|command| matches!(command, egui::ViewportCommand::Focus))
    );
}

#[test]
fn tools_and_terminal_replace_code_without_narrowing_the_workspace_or_losing_edits() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].fixture_content().text = "unsaved code".into();
    render(&mut app, &ctx);
    let code = app.navigation.rect(navigation::Pane::Editor).unwrap();
    app.toggle_panel(Panel::Settings, &ctx);
    render(&mut app, &ctx);
    let tools = app.navigation.rect(navigation::Pane::Tools).unwrap();
    assert!((code.width() - tools.width()).abs() < 1.);
    assert!(app.navigation.rect(navigation::Pane::Editor).is_none());
    app.show_terminal = true;
    app.terminal_intent();
    render(&mut app, &ctx);
    let terminal = app.navigation.rect(navigation::Pane::Terminal).unwrap();
    assert!((code.width() - terminal.width()).abs() < 1.);
    assert!(app.navigation.rect(navigation::Pane::Tools).is_none());
    assert!(app.navigation.rect(navigation::Pane::Editor).is_none());
    app.focus_editor(&ctx);
    render(&mut app, &ctx);
    assert!(app.navigation.rect(navigation::Pane::Editor).is_some());
    assert_eq!(app.documents[0].text, "unsaved code");
    assert!(app.documents[0].dirty());
}

#[test]
fn opening_tools_at_minimum_width_does_not_resize_the_explorer() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.set_explorer_visible(egui::ViewportId::ROOT, true);
    render_width(&mut app, &ctx, 850.);
    let before = app
        .navigation
        .rect(navigation::Pane::Explorer)
        .unwrap()
        .width();
    app.toggle_panel(Panel::Settings, &ctx);
    render_width(&mut app, &ctx, 850.);
    let after = app
        .navigation
        .rect(navigation::Pane::Explorer)
        .unwrap()
        .width();
    assert!((before - after).abs() < 1.);
}

#[test]
fn tool_dialogs_keep_their_owner_until_docking_and_block_other_viewports() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.tool_windows.panel = Some(Panel::Settings);
    app.tool_windows.surface = Surface::Tools;
    app.tool_windows.detached = true;
    app.ui_viewport = EditorApp::tools_viewport();
    app.set_dialog(Dialog::Shortcuts);
    assert!(app.foreign_modal(&ctx));
    app.ui_viewport = egui::ViewportId::ROOT;
    app.dock_tools(&ctx);
    assert!(app.dialog == Dialog::Shortcuts);
    assert!(!app.foreign_modal(&ctx));
}

#[test]
fn detaching_restores_code_and_docking_retains_the_selected_tool_and_chat() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.agents.chat.notice("retained conversation");
    render(&mut app, &ctx);
    let code = app.navigation.rect(navigation::Pane::Editor).unwrap();
    app.toggle_panel(Panel::Settings, &ctx);
    app.tool_windows.detached = true;
    render(&mut app, &ctx);
    assert_eq!(app.navigation.rect(navigation::Pane::Editor), Some(code));
    app.focus_editor(&ctx);
    assert!(app.tool_windows.detached);
    assert_eq!(app.tool_windows.surface, Surface::Tools);
    app.toggle_sidebar(&ctx);
    render(&mut app, &ctx);
    assert!(app.tool_windows.tools_hidden);
    assert_eq!(app.navigation.rect(navigation::Pane::Editor), Some(code));
    app.toggle_sidebar(&ctx);
    app.dock_tools(&ctx);
    render(&mut app, &ctx);
    assert!(!app.tool_windows.detached);
    assert!(app.navigation.rect(navigation::Pane::Editor).is_none());
    assert!(app.navigation.rect(navigation::Pane::Tools).unwrap().top() > code.top());
    assert_eq!(app.tool_windows.panel, Some(Panel::Settings));
    assert_eq!(
        app.agents.chat.messages.back().unwrap().text,
        "retained conversation"
    );
}
fn shortcut(app: &mut EditorApp, ctx: &egui::Context, key: Key, modifiers: egui::Modifiers) {
    let _ = ctx.run(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        },
        |ctx| app.shortcuts(ctx),
    );
}

#[test]
fn sidebar_shortcut_remembers_chat_and_other_tabs_without_starting_a_terminal() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.new_file();
    app.documents[0].fixture_content().text = "unsaved source".into();
    app.agents.chat.notice("retained conversation");
    // The first sidebar opening selects Chat without starting a shell or model turn.
    assert!(app.tool_windows.last_tool.is_none());
    shortcut(&mut app, &ctx, Key::Backtick, egui::Modifiers::CTRL);
    assert!(app.tools_visible());
    assert_eq!(app.tool_windows.panel, Some(Panel::Agent));
    assert!(app.agents.focus);
    assert!(app.terminal.is_none() && !app.agents.chat.busy);
    shortcut(&mut app, &ctx, Key::Backtick, egui::Modifiers::CTRL);
    assert_eq!(app.tool_windows.surface, Surface::Editor);
    for panel in [Panel::Agent, Panel::Settings, Panel::Problems] {
        app.toggle_panel(panel, &ctx);
        shortcut(&mut app, &ctx, Key::Backtick, egui::Modifiers::CTRL);
        assert!(!app.tools_visible());
        assert_eq!(app.tool_windows.surface, Surface::Editor);
        // Ctrl+Shift+` is the physical Ctrl+~ chord on US keyboards.
        shortcut(
            &mut app,
            &ctx,
            Key::Backtick,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        );
        assert!(app.tools_visible());
        assert_eq!(app.tool_windows.panel, Some(panel));
        assert!(app.terminal.is_none());
    }
    assert_eq!(app.documents[0].text, "unsaved source");
    assert_eq!(
        app.agents.chat.messages.back().unwrap().text,
        "retained conversation"
    );
    // Returning to editing also preserves the last tools tab.
    app.focus_editor(&ctx);
    app.workbench.reset_project();
    app.toggle_sidebar(&ctx);
    assert_eq!(app.tool_windows.panel, Some(Panel::Problems));
    app.navigation.zoom = Some(navigation::Pane::Editor);
    app.toggle_sidebar(&ctx);
    assert!(app.tools_visible());
    assert_eq!(app.tool_windows.panel, Some(Panel::Problems));
}

#[test]
fn closing_a_tool_and_hiding_a_detached_window_retain_the_selection() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.toggle_panel(Panel::Settings, &ctx);
    app.toggle_panel(Panel::Settings, &ctx);
    assert!(app.tool_windows.panel.is_none());
    app.toggle_sidebar(&ctx);
    assert!(app.tools_visible());
    assert_eq!(app.tool_windows.panel, Some(Panel::Settings));
    app.tool_windows.detached = true;
    app.toggle_sidebar(&ctx);
    assert!(app.tool_windows.detached);
    assert!(app.tool_windows.tools_hidden);
    assert!(!app.tools_visible());
    app.toggle_sidebar(&ctx);
    assert!(app.tool_windows.detached);
    assert!(!app.tool_windows.tools_hidden);
    assert!(app.tools_visible());
    assert_eq!(app.tool_windows.panel, Some(Panel::Settings));
}

#[test]
fn terminal_shortcut_is_separate_and_dialogs_do_not_toggle_the_sidebar() {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let mut app = EditorApp::new(&cc, Startup::default());
    app.show_terminal = true;
    app.terminal_intent();
    shortcut(
        &mut app,
        &ctx,
        Key::T,
        egui::Modifiers::CTRL | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
    );
    assert!(!app.terminal_visible());
    assert_eq!(app.tool_windows.last_tool, Some(ToolTab::Terminal));
    app.toggle_panel(Panel::Agent, &ctx);
    app.set_dialog(Dialog::Open);
    shortcut(&mut app, &ctx, Key::Backtick, egui::Modifiers::CTRL);
    assert!(app.tools_visible());
    assert_eq!(app.tool_windows.panel, Some(Panel::Agent));
}

#[test]
fn terminal_shortcut_restores_editor_focus_before_following_text() {
    use crate::terminal::StreamControl;
    use std::sync::mpsc;

    fn frame(app: &mut EditorApp, ctx: &egui::Context, size: egui::Vec2, events: Vec<egui::Event>) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ctx| {
                app.route_input(ctx);
                app.navigation.begin_layout();
                app.activity(ctx);
                app.explorer(ctx);
                app.workspace_content(ctx);
                app.navigation_finish(ctx);
            },
        );
    }

    let toggle = || egui::Event::Key {
        key: Key::T,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
    };
    for theme in ["carbon", "phosphor"] {
        for size in [egui::vec2(850., 560.), egui::vec2(1380., 900.)] {
            for batched in [false, true] {
                let ctx = egui::Context::default();
                let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                let mut app = EditorApp::new(&cc, Startup::default());
                app.themes.select(theme, &ctx).unwrap();
                app.new_file();
                app.documents[0].fixture_content().text = "retained source".into();
                let (input, received) = mpsc::channel();
                let (terminal, _stream) =
                    Terminal::stream(&ctx, "Existing shell".into(), move |event| {
                        if let StreamControl::Input(bytes) = event {
                            input.send(bytes).unwrap();
                        }
                    });
                app.terminal = Some(terminal);
                for _ in 0..2 {
                    frame(&mut app, &ctx, size, vec![]);
                }
                frame(&mut app, &ctx, size, vec![toggle()]);
                assert!(app.terminal_visible() && Terminal::focused(&ctx));
                frame(
                    &mut app,
                    &ctx,
                    size,
                    vec![egui::Event::Text("shell".into())],
                );
                assert_eq!(
                    received.recv_timeout(Duration::from_secs(2)).unwrap(),
                    b"shell"
                );
                let mut events = vec![toggle()];
                if batched {
                    events.push(egui::Event::Text("editor".into()));
                }
                frame(&mut app, &ctx, size, events);
                assert!(!app.terminal_visible());
                assert!(ctx.memory(|memory| {
                    memory.has_focus(egui::Id::new(("document", app.documents[0].id)))
                }));
                if !batched {
                    frame(
                        &mut app,
                        &ctx,
                        size,
                        vec![egui::Event::Text("editor".into())],
                    );
                }
                assert!(
                    app.documents[0].text.contains("editor")
                        && app.documents[0].text.contains("retained source")
                );
                assert!(
                    received.try_recv().is_err(),
                    "Editor input reached the hidden terminal"
                );
            }
        }
    }
}
