//! Reproducible CPU frame measurements for the actual source widget. Run with
//! `python3 scripts/rust_tests.py --profile ci -p rode --lib source_frame_profile
//! -- --ignored --nocapture`. Timing is reported, never asserted on shared CI.
use super::*;

#[test]
#[ignore = "manual source-frame benchmark"]
fn source_frame_profile() {
    for lines in [2_000, 20_000] {
        let ctx = egui::Context::default();
        let mut app = EditorApp::new(
            &eframe::CreationContext::_new_kittest(ctx.clone()),
            Startup::default(),
        );
        app.new_file();
        app.documents[0].fixture_content().path = Some("/benchmark/source.rs".into());
        let source: String = (0..lines)
            .map(|i| format!("fn item_{i}() -> usize {{ let value = {i}; value + 1 }} // source\n"))
            .collect();
        let bytes = source.len();
        app.documents[0].set_source(source);
        let mut elapsed = 0.;
        let mut frame = |app: &mut EditorApp, edit| {
            elapsed += 0.1;
            let before = Instant::now();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1380., 900.),
                    )),
                    time: Some(elapsed),
                    events: if edit {
                        vec![egui::Event::Text("x".into())]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
                |ctx| app.editor(ctx),
            );
            before.elapsed().as_secs_f64() * 1000.
        };
        let cold = frame(&mut app, false);
        let ready = Instant::now();
        while !app.editor.source_syntax.settled() {
            assert!(ready.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(2));
            frame(&mut app, false);
        }
        let syntax_ready = ready.elapsed().as_secs_f64() * 1000.;
        let mut idle: Vec<_> = (0..7).map(|_| frame(&mut app, false)).collect();
        let mut edited = Vec::new();
        let mut ready_times = Vec::new();
        for _ in 0..7 {
            let ready = Instant::now();
            edited.push(frame(&mut app, true));
            while !app.editor.source_syntax.settled() {
                assert!(ready.elapsed() < Duration::from_secs(30));
                std::thread::sleep(Duration::from_millis(2));
                frame(&mut app, false);
            }
            ready_times.push(ready.elapsed().as_secs_f64() * 1000.);
        }
        ready_times.sort_by(f64::total_cmp);
        idle.sort_by(f64::total_cmp);
        edited.sort_by(f64::total_cmp);
        assert!(
            app.documents[0].text.len() > bytes,
            "Native source input was not measured"
        );
        println!(
            "source {lines} lines / {bytes} bytes: cold {cold:.2} ms, syntax ready {syntax_ready:.2} ms, idle median {:.2} ms, edit median {:.2} ms, edit-to-syntax median {:.2} ms",
            idle[3], edited[3], ready_times[3]
        );
    }
}
