//! Inspect painted control frames so nested rows cannot hide vertical drift.
use super::*;

#[test]
fn ordinary_text_fields_keep_paint_inside_their_own_frames_when_scrolled() {
    for theme in crate::theme::Theme::builtins() {
        for scale in [1., 1.25, 2.] {
            for multiline in [false, true] {
                let ctx = egui::Context::default();
                crate::theme::install(&ctx, theme.palette);
                ctx.set_pixels_per_point(scale);
                let id = egui::Id::new("framed-text-field");
                let mut value = if multiline {
                    "Long text in a framed multiline input.\n".repeat(35)
                } else {
                    "A horizontally scrolling input field. ".repeat(35)
                };
                let mut bounds = egui::Rect::NOTHING;
                let mut output = None;
                for _ in 0..4 {
                    output = Some(ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(300., 220.),
                            )),
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    ui.memory_mut(|memory| memory.request_focus(id));
                                    let editor = if multiline {
                                        egui::TextEdit::multiline(&mut value).desired_rows(3)
                                    } else {
                                        egui::TextEdit::singleline(&mut value)
                                    };
                                    bounds = editor.id(id).show(ui).response.rect;
                                });
                            });
                        },
                    ));
                }
                let output = output.unwrap();
                let text = output.shapes.iter().find(|shape| {
                    matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == value)
                }).expect("Missing painted input");
                assert!(
                    bounds.contains_rect(text.clip_rect),
                    "{} at {scale}, multiline={multiline}: text clip {:?} escapes field {bounds:?}",
                    theme.id,
                    text.clip_rect,
                );
            }
        }
    }
}

#[test]
fn form_labels_align_with_text_and_numeric_controls_across_themes() {
    for theme in crate::theme::Theme::builtins() {
        for scale in [1., 1.25, 1.5] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx, theme.palette);
            ctx.set_pixels_per_point(scale);
            let mut value = String::new();
            let mut number = 22;
            let mut bounds = Vec::new();
            let mut output = None;
            for _ in 0..4 {
                bounds.clear();
                output = Some(ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(650., 500.),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            egui::Grid::new("form-alignment")
                                .num_columns(2)
                                .show(ui, |ui| {
                                    for label in ["Identity file", "Memory per node (GB)"] {
                                        field_label(ui, label, false);
                                        bounds.push((
                                            label,
                                            ui.text_edit_singleline(&mut value).rect,
                                        ));
                                        ui.end_row();
                                    }
                                    field_label(ui, "SSH port", false);
                                    bounds.push((
                                        "SSH port",
                                        ui.add(egui::DragValue::new(&mut number)).rect,
                                    ));
                                    ui.end_row();
                                });
                        });
                    },
                ));
            }
            let output = output.unwrap();
            for (label, control) in &bounds {
                let text = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) if text.galley.text() == *label => Some(text),
                        _ => None,
                    })
                    .unwrap();
                let label_center = text.pos.y + text.galley.rect.center().y;
                assert!(
                    (label_center - control.center().y).abs() <= 1. / scale + 0.1,
                    "{} at {scale}: {label} centre {label_center}, control {control:?}",
                    theme.id
                );
            }
        }
    }
}

pub(in crate::app) fn text_bounds(output: &egui::FullOutput, label: &str) -> egui::Rect {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.galley.rect.translate(text.pos.to_vec2()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing text {label:?}"))
}

#[test]
fn ssh_form_labels_align_beside_fields_and_stack_above_them_in_narrow_windows() {
    use crate::app::{EditorApp, Startup};
    for theme in crate::theme::Theme::builtins() {
        for width in [430., 700.] {
            let ctx = egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(
                &cc,
                Startup {
                    theme: Some(theme.id.clone()),
                    ..Default::default()
                },
            );
            app.connection_profiles.profiles.clear();
            app.connection_profiles.aliases.clear();
            app.connection_profiles.draft.identity = "entered-identity".into();
            app.connection_profiles.draft.config = "entered-config".into();
            let mut output = None;
            for _ in 0..4 {
                output = Some(ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 1400.),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| app.connect_dialog(ui, ctx));
                    },
                ));
            }
            let output = output.unwrap();
            for (label, value) in [
                ("Identity file", "entered-identity"),
                ("SSH config file", "entered-config"),
            ] {
                let text = text_bounds(&output, label);
                let control = button_frame(&output, value).0;
                if width < 460. {
                    assert!(
                        text.bottom() <= control.top(),
                        "{}: {label} must stack above its field",
                        theme.id
                    );
                } else {
                    assert!(
                        (text.center().y - control.center().y).abs() <= 1.1,
                        "{}: {label} {text:?}, field {control:?}",
                        theme.id
                    );
                }
            }
            assert_eq!(app.connection_profiles.draft.identity, "entered-identity");
        }
    }
}

/// Pair each label with its smallest enclosing painted control and clip bounds.
pub(in crate::app) fn button_frames(
    output: &egui::FullOutput,
    label: &str,
) -> Vec<(egui::Rect, egui::Rect)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                let bounds = egui::Rect::from_min_size(text.pos, text.galley.size());
                output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect) if rect.rect.contains_rect(bounds) => {
                            Some((rect.rect, shape.clip_rect))
                        }
                        _ => None,
                    })
                    .min_by(|(a, _), (b, _)| a.area().total_cmp(&b.area()))
            }
            _ => None,
        })
        .collect()
}

pub(in crate::app) fn button_frame(
    output: &egui::FullOutput,
    label: &str,
) -> (egui::Rect, egui::Rect) {
    button_frames(output, label)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("Missing painted button: {label}"))
}

pub(in crate::app) fn assert_button_alignment(
    output: &egui::FullOutput,
    palette: Palette,
    pixels_per_point: f32,
    context: &str,
) {
    let Some(style) = palette.ui else { return };
    let buttons: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Rect(rect)
                if rect.stroke.color == palette.control_border.unwrap_or(palette.border)
                    && rect.fill == palette.raised
                    && rect.rect.height() >= style.control_height - 1.
                    && rect.rect.height() <= style.control_height + 12.
                    && rect.rect.width() >= 30. =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect();
    let label = |rect: egui::Rect| {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if rect.contains(text.pos + text.galley.size() / 2.) => {
                    Some(text.galley.text())
                }
                _ => None,
            })
            .unwrap_or("unlabelled")
    };
    for (index, a) in buttons.iter().enumerate() {
        for b in &buttons[index + 1..] {
            let overlap = a.bottom().min(b.bottom()) - a.top().max(b.top());
            if (a.right() <= b.left() || b.right() <= a.left())
                && overlap >= a.height().min(b.height()) * 0.75
            {
                assert!(
                    (a.center().y - b.center().y).abs() <= 1. / pixels_per_point + 0.1,
                    "{context}: {} {a:?} and {} {b:?} are misaligned",
                    label(*a),
                    label(*b)
                );
            }
        }
    }
}

#[test]
fn document_terminal_and_dialog_buttons_align_at_multiple_sizes_and_scales() {
    use crate::app::{Dialog, EditorApp, Startup};

    for theme in ["studio", "studio-neutral", "studio-hacker"] {
        for width in [650., 1280.] {
            for scale in [0.75, 1., 1.25, 1.5] {
                let ctx = egui::Context::default();
                let cc = eframe::CreationContext::_new_kittest(ctx.clone());
                let mut app = EditorApp::new(
                    &cc,
                    Startup {
                        theme: Some(theme.into()),
                        ..Default::default()
                    },
                );
                ctx.set_zoom_factor(scale);
                app.project_picker.needs_load = false;
                app.file_browser.needs_load = false;
                app.new_notebook();
                let mut image = crate::app::image_view::ImageView::new(crate::images::Decoded {
                    pixels: std::sync::Arc::new(egui::ColorImage::filled(
                        [2, 2],
                        egui::Color32::WHITE,
                    )),
                    original: [2, 2],
                    format: "PNG".into(),
                    encoded_bytes: 16,
                    lease: std::sync::Arc::new(app.image_budget.reserve(16).unwrap()),
                });
                // Include native-only actions without starting a host service.
                let (mut terminal, _stream) =
                    crate::terminal::Terminal::stream(&ctx, "Layout fixture".into(), |_| {});
                terminal.native = true;
                app.terminal = Some(terminal);
                for scene in [
                    "terminal", "notebook", "image", "binary", "markdown", "slides", "dialogs",
                ] {
                    if scene == "binary" {
                        app.new_file();
                        app.documents.active_mut().unwrap().hex =
                            Some(crate::app::binary_view::HexView::new(
                                crate::binary::Loaded::decode("AAECAwQF", &app.image_budget)
                                    .unwrap(),
                            ));
                    } else if scene == "markdown" {
                        app.new_file();
                        let doc = app.documents.active_mut().unwrap();
                        doc.path = Some("preview.md".into());
                        doc.text = "# Preview".into();
                        let mut preview =
                            crate::app::markdown::Preview::new(app.markdown_images.clone());
                        preview.showing = true;
                        doc.markdown = Some(preview);
                    } else if scene == "slides" {
                        app.new_marp();
                        // Paint the toolbar without starting a project-host renderer.
                        app.workspace
                            .set_fixture(Some(crate::app::tests::remote_workspace()));
                        app.workspace.as_mut().unwrap().connected = false;
                    }
                    let dialogs = if scene == "dialogs" {
                        vec![
                            Dialog::Connect,
                            Dialog::Folder,
                            Dialog::Open,
                            Dialog::Slurm,
                            Dialog::Sessions,
                            Dialog::AgentSessions,
                            Dialog::Extensions,
                            Dialog::Shortcuts,
                            Dialog::Encrypted,
                        ]
                    } else {
                        vec![Dialog::None]
                    };
                    for (dialog_index, dialog) in dialogs.into_iter().enumerate() {
                        app.dialog = dialog;
                        for _ in 0..3 {
                            let output = ctx.run(
                                egui::RawInput {
                                    screen_rect: Some(egui::Rect::from_min_size(
                                        egui::Pos2::ZERO,
                                        egui::vec2(width, 900.),
                                    )),
                                    ..Default::default()
                                },
                                |ctx| {
                                    if scene == "dialogs" {
                                        app.dialogs(ctx);
                                    } else {
                                        egui::CentralPanel::default().show(ctx, |ui| match scene {
                                            "terminal" => app.terminal_header(ui, ctx),
                                            "notebook" => {
                                                app.notebook_ui(ui, ctx);
                                            }
                                            "image" => {
                                                image.ui(
                                                    ui,
                                                    ctx,
                                                    900,
                                                    app.themes.active().palette,
                                                    false,
                                                );
                                            }
                                            "binary" => {
                                                app.hex_ui(ui, ctx);
                                            }
                                            "markdown" => {
                                                app.markdown_ui(ui, ctx);
                                            }
                                            "slides" => app.marp_ui(ui, ctx),
                                            _ => unreachable!(),
                                        });
                                    }
                                },
                            );
                            assert_button_alignment(
                                &output,
                                app.themes.active().palette,
                                ctx.pixels_per_point(),
                                &format!(
                                    "{theme}/{scene}/dialog {dialog_index} at {width}, {scale}"
                                ),
                            );
                            if scene == "terminal" {
                                let persist = button_frame(&output, "Persist: off").0;
                                let mut labels = vec!["Slurm…", "New terminal"];
                                if button_frames(&output, "More").is_empty() {
                                    labels.extend(["Copy", "Sessions", "Detach session"]);
                                } else {
                                    labels.push("More");
                                }
                                for label in labels {
                                    let bounds = button_frame(&output, label).0;
                                    assert!(
                                        (persist.top() - bounds.top()).abs() <= 0.5
                                            && (persist.bottom() - bounds.bottom()).abs() <= 0.5,
                                        "{theme}/{width}/{scale}: {label}: {bounds:?} vs Persist {persist:?}"
                                    );
                                }
                            } else if scene == "image" {
                                let actual = button_frame(&output, "100%").0;
                                let fit = button_frame(&output, "Fit").0;
                                let reload = button_frame(&output, "Reload").0;
                                assert!(
                                    (actual.top() - reload.top()).abs() <= 0.5
                                        && (actual.bottom() - reload.bottom()).abs() <= 0.5
                                        && (actual.height() - fit.height()).abs() <= 0.5,
                                    "Image controls: {actual:?} vs {reload:?}"
                                );
                            } else {
                                let labels: &[&str] = match scene {
                                    "notebook" => &["Run all", "Run", "Down"],
                                    "binary" => &["Go", "Analyze / decode here", "Reload"],
                                    "markdown" => &["Preview", "Reload images", "Slides"],
                                    "slides" => &["Refresh slides", "Export", "Marp settings"],
                                    _ => &[],
                                };
                                for label in labels {
                                    let bounds = button_frame(&output, label).0;
                                    assert!(
                                        bounds.height()
                                            >= app
                                                .themes
                                                .active()
                                                .palette
                                                .ui
                                                .unwrap()
                                                .control_height,
                                        "{scene}: {label} has a shorter frame: {bounds:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
