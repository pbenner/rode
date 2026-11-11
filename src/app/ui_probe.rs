//! Opt-in, read-only geometry for native UI fixtures. Normal startup installs no
//! hooks or file I/O. Fixtures still use real X11 input and observe real effects;
//! this reports controls' geometry, focus and edit revisions, plus opaque IDs in
//! the open session manager and the current SSH endpoint, never chat or document text.
use eframe::egui::{self, Id};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize)]
struct Button {
    #[serde(skip)]
    id: Id,
    #[serde(skip)]
    layer: egui::LayerId,
    name: String,
    enabled: bool,
    focused: bool,
    hovered: bool,
    // Retain the edit's pass across repaints so a polling fixture cannot miss it.
    changed_pass: Option<u64>,
    rect: [f32; 4],
}

#[derive(Default)]
struct Controls {
    visible: Vec<Button>,
    last_change: HashMap<Id, u64>,
    session_ids: Option<Vec<String>>,
    ssh_master_socket: Option<PathBuf>,
}

#[derive(Clone, Default)]
struct Probe(Arc<Mutex<HashMap<egui::ViewportId, Controls>>>);

/// Enable explicit fixture metadata only when a destination is supplied at launch.
pub(super) fn install(ctx: &egui::Context) {
    if let Some(path) = std::env::var_os("RODE_UI_TEST_STATE").filter(|path| !path.is_empty()) {
        install_at(ctx, PathBuf::from(path));
    }
}

/// Install viewport-local test hooks that publish controls after accepted layout passes.
/// The caller supplies the output path; discarded passes do not publish temporary geometry.
pub(super) fn install_at(ctx: &egui::Context, path: PathBuf) {
    let probe = Probe::default();
    ctx.data_mut(|data| data.insert_temp(Id::new("native-ui-probe"), probe.clone()));
    let begin = probe.clone();
    ctx.on_begin_pass(
        "native-ui-probe",
        Arc::new(move |ctx| {
            let mut viewports = begin.0.lock().unwrap();
            let controls = viewports.entry(ctx.viewport_id()).or_default();
            controls.visible.clear();
            controls.session_ids = None;
            controls.ssh_master_socket = None;
        }),
    );
    ctx.on_end_pass(
        "native-ui-probe",
        Arc::new(move |ctx| {
            // Discarded layout passes can place controls at temporary positions.
            if ctx.will_discard() {
                return;
            }
            let mut viewports = probe.0.lock().unwrap();
            let controls = viewports.entry(ctx.viewport_id()).or_default();
            let mut buttons = controls.visible.clone();
            let session_ids = controls.session_ids.clone();
            let ssh_master_socket = controls.ssh_master_socket.clone();
            // Keep revisions across repaints, but discard hidden/closed widgets.
            controls
                .last_change
                .retain(|id, _| buttons.iter().any(|button| button.id == *id));
            drop(viewports);
            // Resolve focus and modal blocking after layout: a later modal can
            // take focus away from an editor registered earlier in the pass.
            ctx.memory(|memory| {
                for button in &mut buttons {
                    button.focused = memory.has_focus(button.id);
                    button.enabled &= memory.allows_interaction(button.layer);
                    button.hovered &= button.enabled;
                }
            });
            // Winit's X11 title getter returns an empty string. Child window
            // builders retain the title actually supplied by the application.
            let title = ctx
                .viewport(|viewport| viewport.builder.title.clone())
                .or_else(|| {
                    ctx.input(|input| input.viewport().title.clone())
                        .filter(|title| !title.is_empty())
                });
            let value = serde_json::json!({
                "pid": std::process::id(),
                "viewport": format!("{:016x}", ctx.viewport_id().0.value()),
                "title": title,
                "pass": ctx.cumulative_pass_nr(),
                "pixels_per_point": ctx.pixels_per_point(),
                "size": [ctx.content_rect().width(), ctx.content_rect().height()],
                "buttons": buttons,
                "session_ids": session_ids,
                "ssh_master_socket": ssh_master_socket,
            });
            let destination = viewport_path(&path, ctx.viewport_id());
            if let Err(error) = publish(&destination, &value) {
                eprintln!(
                    "Cannot publish native UI fixture state to {}: {error}",
                    destination.display()
                );
            }
        }),
    );
}

/// Report the connection-owned endpoint without reconstructing its directory layout.
/// Calling this without an installed probe does not allocate paths or write files.
pub(super) fn ssh_master_socket(ctx: &egui::Context, socket: &Path) {
    if let Some(probe) = ctx.data(|data| data.get_temp::<Probe>(Id::new("native-ui-probe"))) {
        probe
            .0
            .lock()
            .unwrap()
            .entry(ctx.viewport_id())
            .or_default()
            .ssh_master_socket = Some(socket.to_path_buf());
    }
}

/// Report the manager's complete cached list, including rows outside its scroll
/// viewport. A clipped row or closed dialog must not look like a removed session.
pub(super) fn session_list<'a>(ui: &egui::Ui, ids: impl IntoIterator<Item = &'a str>) {
    if let Some(probe) = ui
        .ctx()
        .data(|data| data.get_temp::<Probe>(Id::new("native-ui-probe")))
    {
        probe
            .0
            .lock()
            .unwrap()
            .entry(ui.ctx().viewport_id())
            .or_default()
            .session_ids = Some(ids.into_iter().map(str::to_owned).collect());
    }
}

/// Keep the root snapshot's existing path; native children publish separate files
/// so equal-sized windows and nested passes cannot overwrite each other's controls.
fn viewport_path(path: &Path, viewport: egui::ViewportId) -> PathBuf {
    if viewport == egui::ViewportId::ROOT {
        path.to_path_buf()
    } else {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{:016x}.json", viewport.0.value()));
        path.with_file_name(name)
    }
}

fn publish(path: &Path, value: &serde_json::Value) -> anyhow::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    serde_json::to_writer(file.as_file_mut(), value)?;
    file.persist(path)?;
    Ok(())
}

/// Acknowledge a complete field value or document identity without publishing
/// its contents. Hash only when the read-only fixture probe is enabled.
pub(super) fn value(ui: &egui::Ui, name: &str, response: &egui::Response, value: &str) {
    if ui
        .ctx()
        .data(|data| data.get_temp::<Probe>(Id::new("native-ui-probe")))
        .is_some()
    {
        button(
            ui,
            &format!("{name}:{}", crate::files::digest(value.as_bytes())),
            response,
        );
    }
}

/// Register an existing response without changing its layout, state or input handling.
pub(super) fn button(ui: &egui::Ui, name: &str, response: &egui::Response) {
    let Some(probe) = ui
        .ctx()
        .data(|data| data.get_temp::<Probe>(Id::new("native-ui-probe")))
    else {
        return;
    };
    let rect = response.rect.intersect(ui.clip_rect());
    if rect.is_positive() {
        let changed = response.changed().then(|| ui.ctx().cumulative_pass_nr());
        let mut viewports = probe.0.lock().unwrap();
        let controls = viewports.entry(ui.ctx().viewport_id()).or_default();
        if let Some(pass) = changed {
            controls.last_change.insert(response.id, pass);
        }
        let changed_pass = controls.last_change.get(&response.id).copied();
        controls.visible.push(Button {
            id: response.id,
            layer: response.layer_id,
            name: name.into(),
            enabled: response.enabled() && !ui.is_sizing_pass(),
            focused: false,
            hovered: response.hovered() && ui.input(|input| input.pointer.hover_pos().is_some()),
            changed_pass,
            rect: [rect.left(), rect.top(), rect.right(), rect.bottom()],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_endpoints_follow_reconnections_and_clear_without_assuming_a_directory_layout() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let first = directory.path().join("custom layout/first connection.sock");
        let second = directory
            .path()
            .join("another layout/second-ユニコード.sock");
        let ctx = egui::Context::default();
        let _ = ctx.run(Default::default(), |ctx| ssh_master_socket(ctx, &first));
        assert!(!path.exists() && !first.exists());
        install_at(&ctx, path.clone());
        for endpoint in [Some(first.as_path()), Some(second.as_path()), None] {
            let _ = ctx.run(Default::default(), |ctx| {
                if let Some(endpoint) = endpoint {
                    ssh_master_socket(ctx, endpoint);
                }
            });
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(
                state["ssh_master_socket"],
                serde_json::to_value(endpoint).unwrap()
            );
        }
        assert!(
            !first.exists() && !second.exists(),
            "The probe created an endpoint"
        );
    }

    #[test]
    fn complete_values_are_distinct_from_prefixes_and_never_publish_the_input() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let ctx = egui::Context::default();
        install_at(&ctx, path.clone());
        let full = "/private/project/document with spaces.txt";
        let expected = format!("input-value:{}", crate::files::digest(full.as_bytes()));
        for input in ["/private/project/doc", full] {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut text = input.to_owned();
                    let response = ui.text_edit_singleline(&mut text);
                    value(ui, "input-value", &response, &text);
                });
            });
            let raw = std::fs::read_to_string(&path).unwrap();
            assert!(!raw.contains(input));
            let state: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(
                state["buttons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|button| button["name"] == expected),
                input == full,
            );
        }
    }

    #[test]
    fn native_viewports_keep_separate_controls_revisions_and_session_lists() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let child = egui::ViewportId::from_hash_of("independent-chat");
        let endpoint = directory.path().join("connection/socket");
        install_at(&ctx, path.clone());
        let render = |viewport, changed| {
            let _ = ctx.run(
                egui::RawInput {
                    viewport_id: viewport,
                    viewports: [
                        (
                            egui::ViewportId::ROOT,
                            egui::ViewportInfo {
                                title: Some("RODE root".into()),
                                ..Default::default()
                            },
                        ),
                        (
                            child,
                            egui::ViewportInfo {
                                title: Some(String::new()),
                                parent: Some(egui::ViewportId::ROOT),
                                ..Default::default()
                            },
                        ),
                    ]
                    .into_iter()
                    .collect(),
                    ..Default::default()
                },
                |ctx| {
                    if viewport == egui::ViewportId::ROOT {
                        ssh_master_socket(ctx, &endpoint);
                    }
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let mut response = ui.button("Fixture control");
                        if changed {
                            response.mark_changed();
                        }
                        let name = if viewport == child { "child" } else { "root" };
                        button(ui, name, &response);
                        session_list(ui, [name]);
                    });
                    if viewport == egui::ViewportId::ROOT {
                        ctx.show_viewport_deferred(
                            child,
                            egui::ViewportBuilder::default().with_title("RODE · Chat"),
                            |_, _| {},
                        );
                    }
                },
            );
        };
        render(egui::ViewportId::ROOT, true);
        let before = std::fs::read(&path).unwrap();
        let root: serde_json::Value = serde_json::from_slice(&before).unwrap();
        render(child, false);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "A child pass overwrote the root snapshot"
        );
        let other: serde_json::Value =
            serde_json::from_slice(&std::fs::read(viewport_path(&path, child)).unwrap()).unwrap();
        assert_eq!(other["title"], "RODE · Chat");
        assert_eq!(other["buttons"][0]["name"], "child");
        assert!(other["buttons"][0]["changed_pass"].is_null());
        assert_eq!(other["session_ids"], serde_json::json!(["child"]));
        assert!(other["ssh_master_socket"].is_null());
        render(egui::ViewportId::ROOT, false);
        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(after["buttons"][0]["name"], "root");
        assert_eq!(
            after["buttons"][0]["changed_pass"],
            root["buttons"][0]["changed_pass"]
        );
        assert_eq!(after["session_ids"], serde_json::json!(["root"]));
        assert_eq!(
            after["ssh_master_socket"],
            serde_json::to_value(&endpoint).unwrap()
        );
    }

    #[test]
    fn hover_acknowledges_real_pointer_input_and_is_cleared_when_it_leaves() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        install_at(&ctx, path.clone());
        let render = |events| {
            let _ = ctx.run(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let response = ui.button("Attach");
                        button(ui, "attach", &response);
                    });
                },
            );
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap()
        };
        render(vec![]);
        let state = render(vec![]);
        assert_eq!(state["buttons"][0]["hovered"], false);
        let rect: [f32; 4] = serde_json::from_value(state["buttons"][0]["rect"].clone()).unwrap();
        let point = egui::pos2((rect[0] + rect[2]) / 2., (rect[1] + rect[3]) / 2.);
        let hovered = render(vec![egui::Event::PointerMoved(point)]);
        assert_eq!(hovered["buttons"][0]["hovered"], true);
        let left = render(vec![egui::Event::PointerGone]);
        assert_eq!(left["buttons"][0]["hovered"], false);
    }

    #[test]
    fn snapshots_do_not_advertise_controls_behind_a_later_modal_as_clickable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        install_at(&ctx, path.clone());
        for pass in 0..3 {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.button("Fetch");
                    button(ui, "background-fetch", &response);
                });
                egui::Modal::new(Id::new("credentials")).show(ctx, |ui| {
                    let response = ui.button("Retry operation");
                    button(ui, "modal-retry", &response);
                });
            });
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            let background = state["buttons"]
                .as_array()
                .unwrap()
                .iter()
                .find(|b| b["name"] == "background-fetch")
                .unwrap();
            if pass > 0 {
                assert_eq!(
                    background["enabled"], false,
                    "The modal blocks this control despite its widget enabled flag"
                );
            }
        }
    }

    #[test]
    fn reselecting_terminal_tab_keeps_shell_input_with_and_without_the_probe() {
        use crate::{
            app::{EditorApp, sessions::Meta},
            terminal::{StreamControl, Terminal},
        };
        use std::{sync::mpsc, time::Duration};

        fn frame(
            app: &mut EditorApp,
            ctx: &egui::Context,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200., 800.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    app.navigation.begin_layout();
                    app.activity(ctx);
                    app.explorer(ctx);
                    app.workspace_content(ctx);
                },
            )
        }

        for probe_enabled in [false, true] {
            for batched_click in [false, true] {
                let directory = tempfile::tempdir().unwrap();
                let ctx = egui::Context::default();
                if probe_enabled {
                    install_at(&ctx, directory.path().join("widgets.json"));
                }
                let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                let mut app = EditorApp::new(&cc, Default::default());
                let (tx, rx) = mpsc::channel();
                let (terminal, _stream) =
                    Terminal::stream(&ctx, "Existing shell".into(), move |event| {
                        if let StreamControl::Input(bytes) = event {
                            tx.send(bytes).unwrap();
                        }
                    });
                app.terminal = Some(terminal);
                app.sessions.active = Some(Meta {
                    key: "existing-session".into(),
                    order: 0,
                    name: "Existing shell".into(),
                    native: None,
                    policy: String::new(),
                    mode: "NORMAL MODE".into(),
                });
                app.show_terminal = true;
                app.terminal_intent();
                frame(&mut app, &ctx, vec![]);
                let output = frame(&mut app, &ctx, vec![]);
                assert!(Terminal::focused(&ctx));
                let point = output
                    .shapes
                    .iter()
                    .find_map(|shape| {
                        if let egui::Shape::Text(text) = &shape.shape
                            && text.galley.text() == "Terminal"
                        {
                            Some(text.pos + text.galley.size() / 2.)
                        } else {
                            None
                        }
                    })
                    .unwrap();
                let button = |pressed| egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                };
                frame(&mut app, &ctx, vec![egui::Event::PointerMoved(point)]);
                if batched_click {
                    frame(&mut app, &ctx, vec![button(true), button(false)]);
                } else {
                    frame(&mut app, &ctx, vec![button(true)]);
                    frame(&mut app, &ctx, vec![button(false)]);
                }
                frame(&mut app, &ctx, vec![]);
                assert!(
                    Terminal::focused(&ctx),
                    "probe={probe_enabled}, batched={batched_click}"
                );
                frame(&mut app, &ctx, vec![egui::Event::Text("retained".into())]);
                assert_eq!(
                    rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                    b"retained"
                );
            }
        }
    }

    #[test]
    fn palette_match_requires_the_complete_native_query() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        install_at(&ctx, path.clone());
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = crate::app::EditorApp::new(&cc, Default::default());
        app.set_dialog(crate::app::Dialog::Commands);
        for (text, matched) in [
            ("Terminal: Sessions", false),
            (" and Slurm allocations", true),
        ] {
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Text(text.into())],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.command_dialog(ui, ctx));
                },
            );
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(
                state["buttons"].as_array().unwrap().iter().any(|button| {
                    button["name"] == "command-match:Terminal: Sessions and Slurm allocations"
                }),
                matched,
            );
        }
        assert_eq!(app.dialog_input, "Terminal: Sessions and Slurm allocations");
        assert!(matches!(app.dialog, crate::app::Dialog::Commands));
    }

    #[test]
    fn manager_snapshot_distinguishes_clipped_rows_empty_list_and_closed_dialog() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        install_at(&ctx, path.clone());
        for ids in [
            Some(vec!["visible", "outside-scroll-viewport"]),
            Some(vec![]),
            None,
        ] {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.set_clip_rect(egui::Rect::NOTHING);
                    if let Some(ids) = &ids {
                        session_list(ui, ids.iter().copied());
                    }
                });
            });
            let state: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(state["session_ids"], serde_json::json!(ids));
        }
    }

    #[test]
    fn snapshots_follow_enabled_visible_controls_and_clear_hidden_or_discarded_layout() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.5);
        install_at(&ctx, path.clone());
        let mut expected = None;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let response = ui.add_enabled(false, egui::Button::new("Install Codex"));
                expected = Some(response.rect);
                button(ui, "Install Codex", &response);
                if ctx.current_pass_index() == 0 {
                    let transient = ui.button("Temporary layout");
                    button(ui, "Temporary layout", &transient);
                    ctx.request_discard("fixture verifies final layout only");
                }
            });
        });
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let rect = expected.unwrap();
        assert_eq!(state["buttons"].as_array().unwrap().len(), 1);
        assert_eq!(state["buttons"][0]["enabled"], false);
        assert_eq!(
            state["buttons"][0]["rect"],
            serde_json::json!([rect.left(), rect.top(), rect.right(), rect.bottom()])
        );
        assert_eq!(state["pixels_per_point"], 1.5);
        let _ = ctx.run(Default::default(), |_| {});
        let hidden: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(hidden["buttons"].as_array().unwrap().is_empty());
        assert!(hidden["pass"].as_u64().unwrap() > state["pass"].as_u64().unwrap());
    }

    #[test]
    fn focus_is_final_and_edit_revisions_survive_repaints_but_not_closed_widgets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("widgets.json");
        let ctx = egui::Context::default();
        install_at(&ctx, path.clone());
        let id = Id::new("source");
        let mut text = String::new();
        ctx.memory_mut(|memory| memory.request_focus(id));
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Text("native input".into())],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.add(egui::TextEdit::singleline(&mut text).id(id));
                    button(ui, "source-editor", &response);
                    // Simulate a modal taking focus after source was registered.
                    let later = ui.button("Later control");
                    later.request_focus();
                    button(ui, "later", &later);
                });
            },
        );
        let edited: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(text, "native input");
        assert_eq!(edited["buttons"][0]["focused"], false);
        assert_eq!(edited["buttons"][1]["focused"], true);
        let changed = edited["buttons"][0]["changed_pass"].as_u64().unwrap();
        let render = |text: &mut String| {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let response = ui.add(egui::TextEdit::singleline(text).id(id));
                    button(ui, "source-editor", &response);
                });
            });
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(&path).unwrap()).unwrap()
        };
        let repainted = render(&mut text);
        assert_eq!(repainted["buttons"][0]["changed_pass"], changed);
        assert!(repainted["pass"].as_u64().unwrap() > changed);
        let _ = ctx.run(Default::default(), |_| {});
        let reopened = render(&mut text);
        assert!(reopened["buttons"][0]["changed_pass"].is_null());
    }
}
