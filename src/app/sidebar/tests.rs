//! Shared sidebar geometry across content switches, resize commands and native hosts.
use super::*;

fn render(app: &mut EditorApp, ctx: &egui::Context, viewport: egui::ViewportId, width: f32) {
    for _ in 0..3 {
        let mut input = egui::RawInput {
            viewport_id: viewport,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 700.),
            )),
            ..Default::default()
        };
        input.viewports.entry(viewport).or_default().focused = Some(true);
        let _ = ctx.run(input, |ctx| {
            app.navigation.begin_viewport_layout(viewport);
            app.workspace_sidebar(ctx);
            egui::CentralPanel::default().show(ctx, |_| {});
        });
    }
}

fn width(ctx: &egui::Context, viewport: egui::ViewportId, zoom: bool) -> f32 {
    egui::containers::panel::PanelState::load(ctx, panel_id(viewport, zoom))
        .unwrap()
        .rect
        .width()
}

#[test]
fn switching_components_preserves_default_and_resized_widths_in_both_directions() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    let root = egui::ViewportId::ROOT;
    app.set_explorer_visible(root, true);
    render(&mut app, &ctx, root, 1000.);
    assert_eq!(width(&ctx, root, false), DEFAULT_WIDTH);
    for requested in [180., 320., 450.] {
        app.editor.sidebar.width = Some(requested);
        render(&mut app, &ctx, root, 1000.);
        app.show_connections(&ctx);
        render(&mut app, &ctx, root, 1000.);
        assert_eq!(width(&ctx, root, false), requested);
        app.navigation.active = navigation::Pane::Connections;
        app.navigate(navigation::NavAction::Wider, &ctx);
        app.navigate(navigation::NavAction::Wider, &ctx);
        app.navigate(navigation::NavAction::Narrower, &ctx);
        render(&mut app, &ctx, root, 1000.);
        let resized = (requested + 32.).clamp(MIN_WIDTH, 450.);
        assert_eq!(width(&ctx, root, false), resized);
        app.set_explorer_visible(root, true);
        render(&mut app, &ctx, root, 1000.);
        assert_eq!(width(&ctx, root, false), resized);
    }
}

#[test]
fn zoom_hide_and_window_constraints_do_not_give_components_separate_widths() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    let root = egui::ViewportId::ROOT;
    app.set_explorer_visible(root, true);
    app.editor.sidebar.width = Some(340.);
    render(&mut app, &ctx, root, 1000.);
    for view in [View::Explorer, View::Connections] {
        match view {
            View::Explorer => app.set_explorer_visible(root, true),
            View::Connections => app.show_connections(&ctx),
        }
        let pane = if view == View::Explorer {
            navigation::Pane::Explorer
        } else {
            navigation::Pane::Connections
        };
        app.set_pane_zoom(root, Some(pane));
        render(&mut app, &ctx, root, 1000.);
        assert_eq!(width(&ctx, root, true), 1000.);
        assert_eq!(width(&ctx, root, false), 340.);
        app.set_pane_zoom(root, None);
        render(&mut app, &ctx, root, 1000.);
        assert_eq!(width(&ctx, root, false), 340.);
    }
    app.presentation.hide_sidebar();
    render(&mut app, &ctx, root, 1000.);
    app.set_explorer_visible(root, true);
    render(&mut app, &ctx, root, 1000.);
    assert_eq!(width(&ctx, root, false), 340.);
    render(&mut app, &ctx, root, 560.);
    let constrained = width(&ctx, root, false);
    assert!(constrained <= 240.);
    app.show_connections(&ctx);
    render(&mut app, &ctx, root, 560.);
    assert_eq!(width(&ctx, root, false), constrained);
}

#[test]
fn detached_editors_keep_independent_sidebar_geometry() {
    let ctx = egui::Context::default();
    let mut app = EditorApp::new(
        &eframe::CreationContext::_new_kittest(ctx.clone()),
        Startup::default(),
    );
    let root = egui::ViewportId::ROOT;
    let child = app.allocate_editor_window();
    app.set_explorer_visible(root, true);
    app.editor.sidebar.width = Some(330.);
    app.editor_view_mut(child).unwrap().sidebar.width = Some(210.);
    render(&mut app, &ctx, root, 1000.);
    render(&mut app, &ctx, child, 1000.);
    app.show_connections(&ctx);
    render(&mut app, &ctx, root, 1000.);
    assert_eq!(width(&ctx, root, false), 330.);
    assert_eq!(width(&ctx, child, false), 210.);
    // Root chrome can render while a detached editor owns the input context.
    app.activate_editor(child);
    app.editor_view_mut(root).unwrap().sidebar.width = Some(350.);
    render(&mut app, &ctx, root, 1000.);
    assert_eq!(width(&ctx, root, false), 350.);
    assert_eq!(width(&ctx, child, false), 210.);
}
