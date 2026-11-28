//! nbformat preservation and versioned outputs; real installed Jupyter kernels are enabled only
//! through the test override.

#[path = "support/native.rs"]
mod native_peer;

use rode::{
    notebook::{
        self, Notebook,
        kernel::{Event, Kernel, Run},
    },
    sandbox,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
const FIXTURE: &str = include_str!("fixtures/notebook.ipynb");

#[test]
fn bridge_accounting_fragments_and_nonfatal_limits_need_no_jupyter_installation() {
    let result = std::process::Command::new("python3")
        .args([
            "-I",
            "-c",
            include_str!("notebooks/bridge.py"),
            include_str!("../crates/rode-host-core/src/notebook_kernel.py"),
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn large_plots_updates_and_output_overflow_preserve_the_running_kernel() {
    let Some(python) = interpreter() else {
        return;
    };
    for managed in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let plan =
            native_peer::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap();
        let mut kernel = if managed {
            Kernel::start_managed(
                plan,
                None,
                python.clone(),
                "python3".into(),
                None,
                root.path().join("plots.ipynb").to_string_lossy().into(),
                false,
                Arc::new(|| {}),
            )
        } else {
            Kernel::start(
                plan,
                None,
                python.clone(),
                "python3".into(),
                Arc::new(|| {}),
            )
        }
        .unwrap();
        let run = |uid: u64, code: String| Run {
            uid,
            cell: format!("cell-{uid}"),
            version: 0,
            code,
        };
        let plot = run(1, r#"
import os, base64, numpy as np
import matplotlib.pyplot as plt
from IPython.display import display, update_display
sentinel = 123
kernel_pid = os.getpid()
fig = plt.figure(figsize=(10.24, 10.24), dpi=100)
ax = fig.add_axes([0, 0, 1, 1])
ax.set_axis_off()
ax.imshow(np.random.default_rng(7).integers(0, 256, (1024, 1024, 3), dtype=np.uint8), interpolation='nearest')
fig.savefig('large-plot.png', dpi=100)
plt.close(fig)
with open('large-plot.png', 'rb') as image:
    encoded_plot = base64.b64encode(image.read()).decode('ascii')
display({'image/png': encoded_plot, 'text/plain': '日本語 plot'}, raw=True, display_id='large-plot')
for _ in range(3):
    update_display({'image/png': encoded_plot, 'text/plain': 'Updated plot'}, raw=True, display_id='large-plot')
"#.into());
        kernel.bind(vec![plot.clone()]);
        kernel.run(vec![plot]).unwrap();
        let events = wait(&mut kernel, 1);
        let warnings = events
            .iter()
            .filter_map(|event| match event {
                Event::Warning(warning) => Some(warning.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(warnings.is_empty(), "{warnings:?}");
        let png = output_values(&events)
            .find_map(|output| output["data"]["image/png"].as_str())
            .expect("No high-resolution plot output");
        assert!(png.len() > 2 * 1024 * 1024);
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        assert_eq!(
            STANDARD.decode(png).unwrap(),
            std::fs::read(root.path().join("large-plot.png")).unwrap()
        );

        let overflow = run(
            2,
            format!(
                "print('x' * {})\noverflow_finished = True",
                64 * 1024 * 1024 + 1
            ),
        );
        let following = run(
            3,
            "assert overflow_finished\nopen('queue-finished', 'w').write('ok')".into(),
        );
        kernel.bind(vec![overflow.clone(), following.clone()]);
        kernel
            .run_with_output_budget(vec![overflow, following], 64.try_into().unwrap())
            .unwrap();
        let events = wait(&mut kernel, 2);
        assert!(events.iter().any(
            |event| matches!(event, Event::Warning(message) if message.contains("preserved"))
        ));
        assert!(
            events
                .iter()
                .filter(|event| matches!(event, Event::Done(_, status) if status == "ok"))
                .count()
                >= 2
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("queue-finished")).unwrap(),
            "ok"
        );
        kernel.clear_outputs(Some("cell-2".into())).unwrap();
        let reuse = run(
            4,
            "assert sentinel == 123\nassert os.getpid() == kernel_pid\nprint('same kernel')".into(),
        );
        kernel.bind(vec![reuse.clone()]);
        kernel.run(vec![reuse]).unwrap();
        let events = wait(&mut kernel, 1);
        assert!(output_values(&events).any(|output| output["text"] == "same kernel\n"));

        let progress = run(
            5,
            "for i in range(350):\n    print(f'progress {i}', flush=True)".into(),
        );
        kernel.bind(vec![progress.clone()]);
        kernel.run(vec![progress]).unwrap();
        let events = wait(&mut kernel, 1);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::Warning(_)))
        );
        let mut retained = Notebook::default();
        let uid = retained.cells[0].uid;
        for event in events {
            match event {
                Event::Clear(_) => {
                    retained.apply_output(uid, 0, None, None, true).unwrap();
                }
                Event::Edits {
                    reset,
                    edits,
                    count,
                    ..
                } => {
                    retained.apply_edits(uid, 0, reset, &edits, count).unwrap();
                }
                Event::Output { output, .. } => {
                    retained
                        .apply_output(uid, 0, Some(output), None, false)
                        .unwrap();
                }
                _ => {}
            }
        }
        assert_eq!(retained.cells[0].outputs().len(), 1);
        assert_eq!(
            retained.cells[0].outputs()[0]["text"],
            (0..350)
                .map(|index| format!("progress {index}\n"))
                .collect::<String>()
        );
        let records = run(
            7,
            "for i in range(600):\n    display({'text/plain': f'row {i}'}, raw=True)".into(),
        );
        kernel.bind(vec![records.clone()]);
        kernel.run(vec![records]).unwrap();
        let events = wait(&mut kernel, 1);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::Warning(_)))
        );
        let mut many = Notebook::default();
        let uid = many.cells[0].uid;
        for event in events {
            match event {
                Event::Clear(_) => {
                    many.apply_output(uid, 0, None, None, true).unwrap();
                }
                Event::Edits {
                    reset,
                    edits,
                    count,
                    ..
                } => {
                    many.apply_edits(uid, 0, reset, &edits, count).unwrap();
                }
                Event::Output { output, .. } => {
                    many.apply_output(uid, 0, Some(output), None, false)
                        .unwrap();
                }
                _ => {}
            }
        }
        assert_eq!(many.cells[0].outputs().len(), 600);
        assert_eq!(
            many.cells[0].outputs()[599]["data"]["text/plain"],
            "row 599"
        );
        // A lower project allowance and a later larger one apply to new runs
        // on the same live kernel, including HTML MIME output alongside stdout.
        for mib in [64, 128] {
            let verbose = run(6, "assert os.getpid() == kernel_pid\nhtml = '<html><script>' + 'x' * (20 * 1024 * 1024) + '</script></html>'\nwith open('sites_all_layers.html', 'w') as saved:\n    saved.write(html)\nprint('v' * (1536 * 1024))\ndisplay({'text/html': html, 'text/plain': 'standalone HTML'}, raw=True)".into());
            kernel.bind(vec![verbose.clone()]);
            kernel
                .run_with_output_budget(vec![verbose], mib.try_into().unwrap())
                .unwrap();
            let events = wait(&mut kernel, 1);
            let saved_html =
                std::fs::read_to_string(root.path().join("sites_all_layers.html")).unwrap();
            assert_eq!(saved_html.len(), 20 * 1024 * 1024 + 30);
            assert!(
                saved_html.starts_with("<html><script>")
                    && saved_html.ends_with("</script></html>")
            );
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Event::Warning(_)))
            );
            assert!(output_values(&events).any(|output| {
                output["text"]
                    .as_str()
                    .is_some_and(|text| text.len() == 1536 * 1024 + 1)
            }));
            assert!(output_values(&events).any(
                |output| output["data"]["text/html"].as_str() == Some(saved_html.as_str())
            ));
        }
        kernel.stop();
        let until = Instant::now() + Duration::from_secs(10);
        while kernel.pending() {
            kernel.poll();
            assert!(Instant::now() < until, "Kernel shutdown did not complete");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn managed_kernel_accepts_large_atomic_source_queues() {
    let Some(python) = interpreter() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let plan = native_peer::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap();
    let mut kernel = Kernel::start_managed(
        plan,
        None,
        python,
        "python3".into(),
        None,
        root.path().join("queue.ipynb").to_string_lossy().into(),
        false,
        Arc::new(|| {}),
    )
    .unwrap();
    let padding = "x".repeat(100 * 1024);
    let runs: Vec<_> = (0..24)
        .map(|uid| Run {
            uid,
            cell: format!("queue-{uid}"),
            version: 0,
            code: format!("#{padding}\nprint({uid})"),
        })
        .collect();
    assert!(serde_json::to_vec(&runs).unwrap().len() > 2 * 1024 * 1024);
    kernel.bind(runs.clone());
    kernel.run(runs).unwrap();
    let events = wait(&mut kernel, 24);
    assert!(output_values(&events).any(|output| output["text"] == "23\n"));
    kernel.stop();
}

#[test]
fn restoring_edited_source_resyncs_outputs_skipped_during_host_execution() {
    let Some(python) = interpreter() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let plan = native_peer::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap();
    let mut book = Notebook::default();
    let code = "import time\nfrom pathlib import Path\nprint('first', flush=True)\nwhile not Path('continue').exists():\n    time.sleep(0.01)\nprint('second', flush=True)";
    book.cells[0].set_source(code.into());
    let run = Run {
        uid: book.cells[0].uid,
        cell: "stream".into(),
        version: book.cells[0].version,
        code: code.into(),
    };
    let mut kernel = Kernel::start_managed(
        plan,
        None,
        python,
        "python3".into(),
        None,
        root.path().join("stream.ipynb").to_string_lossy().into(),
        false,
        Arc::new(|| {}),
    )
    .unwrap();
    kernel.bind(vec![run.clone()]);
    kernel.run(vec![run.clone()]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while book.cells[0].outputs().is_empty() {
        apply_events(&mut book, kernel.poll());
        assert!(
            Instant::now() < deadline,
            "First output did not arrive: {}",
            kernel.status
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    book.cells[0].set_source("# edited during execution".into());
    kernel.bind(vec![Run {
        version: book.cells[0].version,
        code: book.cells[0].source.clone(),
        ..run.clone()
    }]);
    std::fs::write(root.path().join("continue"), "").unwrap();
    // Idle is delivered after all output packets. Reaching it proves the view
    // consumed and rejected the source-incompatible second output.
    while kernel.busy() {
        for event in kernel.poll() {
            assert!(!matches!(event, Event::Output { .. } | Event::Edits { .. }));
            if let Event::Error(error) = event {
                panic!("{error}");
            }
        }
        assert!(Instant::now() < deadline, "Edited execution did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(book.cells[0].outputs()[0]["text"], "first\n");
    book.cells[0].set_source(code.into());
    kernel.bind(vec![Run {
        version: book.cells[0].version,
        ..run
    }]);
    apply_events(&mut book, wait(&mut kernel, 1));
    assert_eq!(book.cells[0].outputs().len(), 1);
    assert_eq!(book.cells[0].outputs()[0]["text"], "first\nsecond\n");
    assert_eq!(
        book.cells[0].data["execution_count"], 1,
        "Resync must not execute code again"
    );
    kernel.stop();
}

fn apply_events(book: &mut Notebook, events: Vec<Event>) {
    for event in events {
        match event {
            Event::Clear(run) => {
                book.apply_output(run.uid, run.version, None, None, true)
                    .unwrap();
            }
            Event::Output { run, output, .. } => {
                book.apply_output(run.uid, run.version, Some(output), None, false)
                    .unwrap();
            }
            Event::Edits {
                run,
                reset,
                edits,
                count,
            } => {
                book.apply_edits(run.uid, run.version, reset, &edits, count)
                    .unwrap();
            }
            Event::Count(run, count) => {
                book.apply_output(run.uid, run.version, None, Some(count), false)
                    .unwrap();
            }
            Event::Error(error) => panic!("{error}"),
            _ => {}
        }
    }
}

#[test]
fn stream_fragments_coalesce_without_reordering_outputs_or_bypassing_output_budgets() {
    use serde_json::json;
    let mut book = Notebook::default();
    let uid = book.cells[0].uid;
    for _ in 0..350 {
        book.apply_output(
            uid,
            0,
            Some(json!({"output_type":"stream", "name":"stdout", "text":"日🦀\n"})),
            None,
            false,
        )
        .unwrap();
    }
    assert_eq!(book.cells[0].outputs().len(), 1);
    assert_eq!(book.cells[0].outputs()[0]["text"], "日🦀\n".repeat(350));
    for output in [
        json!({"output_type":"stream", "name":"stderr", "text":"error"}),
        json!({"output_type":"display_data", "data":{"text/plain":"result"}}),
        json!({"output_type":"stream", "name":"stdout", "text":"after display"}),
    ] {
        book.apply_output(uid, 0, Some(output), None, false)
            .unwrap();
    }
    assert_eq!(book.cells[0].outputs().len(), 4);
    let before = book.serialize().unwrap();
    assert!(
        book.apply_edits(
            uid,
            0,
            false,
            &[rode_runtime_core::service::OutputEdit::Text {
                index: 99,
                text: "new text".into()
            }],
            None
        )
        .is_err()
    );
    assert_eq!(book.serialize().unwrap(), before);
}

#[test]
fn notebook_ram_admission_checks_display_fanout_atomically_and_snapshots_share_payloads() {
    use serde_json::json;
    let mut book = Notebook::default();
    book.cells = (0..17).map(|_| notebook::Cell::new("code")).collect();
    book.set_output_budget(64.try_into().unwrap());
    let targets = book
        .cells
        .iter()
        .map(|cell| (cell.uid, 0, 0))
        .collect::<Vec<_>>();
    for &(uid, version, _) in &targets {
        book.apply_output(
            uid,
            version,
            Some(json!({"output_type":"display_data", "data":{"text/plain":"before"}})),
            Some(1),
            false,
        )
        .unwrap();
    }
    let original = book.serialize().unwrap();
    let replacement =
        json!({"output_type":"display_data", "data":{"text/html":"x".repeat(4 * 1024 * 1024)}});
    assert!(book.update_display_outputs(&targets, &replacement).is_err());
    assert_eq!(book.serialize().unwrap(), original);
    assert!(book.cells.iter().all(|cell| cell.outputs_version == 1));
    book.set_output_budget(128.try_into().unwrap());
    assert_eq!(
        book.update_display_outputs(&targets, &replacement)
            .unwrap()
            .len(),
        targets.len()
    );
    let snapshot = book.clone();
    for cell in &snapshot.cells {
        assert!(Arc::ptr_eq(&cell.outputs()[0], &book.cells[0].outputs()[0]));
    }
    book.cells[0].clear_outputs();
    assert_eq!(
        snapshot.cells[0].outputs()[0]["data"]["text/html"]
            .as_str()
            .unwrap()
            .len(),
        4 * 1024 * 1024
    );
    // Explicit clear releases the charged allowance without disturbing a snapshot.
    for cell in &mut book.cells {
        cell.clear_outputs();
    }
    book.set_output_budget(64.try_into().unwrap());
    let uid = book.cells[0].uid;
    assert!(
        book.apply_output(uid, 0, Some(replacement), Some(2), false)
            .unwrap()
    );
}

fn output_values(events: &[Event]) -> impl Iterator<Item = &serde_json::Value> {
    events.iter().flat_map(|event| match event {
        Event::Output { output, .. } => vec![output],
        Event::Edits { edits, .. } => edits
            .iter()
            .filter_map(|edit| match edit {
                rode_runtime_core::service::OutputEdit::Append { output }
                | rode_runtime_core::service::OutputEdit::Replace { output, .. } => {
                    Some(output.value.as_ref())
                }
                _ => None,
            })
            .collect(),
        _ => vec![],
    })
}
fn interpreter() -> Option<String> {
    native_peer::capabilities::tool("jupyter", &["RODE_TEST_JUPYTER"])
}
fn wait(kernel: &mut Kernel, done: usize) -> Vec<Event> {
    let mut events = vec![];
    let mut completed = 0;
    let start = Instant::now();
    // A host-managed completion includes the final idle acknowledgement after
    // its retained output has crossed the attachment stream.
    while completed < done || kernel.busy() {
        assert!(
            start.elapsed() < Duration::from_secs(35),
            "kernel timeout: {}",
            kernel.status
        );
        for event in kernel.poll() {
            if let Event::Error(error) = &event {
                panic!("{error}");
            }
            if matches!(&event, Event::Done(..)) {
                completed += 1;
            }
            events.push(event);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    events
}
#[test]
fn cell_edits_preserve_metadata_attachments_unknown_fields_and_saved_outputs() {
    let mut original: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    original["future_root"] = serde_json::json!({"preserve":true});
    let mut book = Notebook::parse(&original.to_string()).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&book.serialize().unwrap()).unwrap(),
        original
    );
    book.cells[1].set_source("counter = 41".into());
    let changed: serde_json::Value = serde_json::from_str(&book.serialize().unwrap()).unwrap();
    assert_eq!(changed["metadata"], original["metadata"]);
    assert_eq!(changed["future_root"], original["future_root"]);
    assert_eq!(changed["cells"][0], original["cells"][0]);
    assert_eq!(changed["cells"][2], original["cells"][2]);
    assert_eq!(
        changed["cells"][1]["metadata"],
        original["cells"][1]["metadata"]
    );
    book.cells.swap(1, 2);
    assert_eq!(book.cells[1].data["id"], "result");
    assert_eq!(book.cells[2].source, "counter = 41");
    book.cells[2].change_kind("markdown");
    assert!(book.cells[2].data.get("outputs").is_none());
    assert_eq!(book.cells[2].data["metadata"]["custom"], 42);
    assert!(notebook::is_path("demo.IPYNB"));
    assert!(!notebook::is_path("demo.ipynb.rs"));
}
#[test]
fn execution_results_are_version_checked_and_output_limits_roll_back_atomically() {
    let mut book = Notebook::default();
    let uid = book.cells[0].uid;
    book.cells[0].set_source("print(42)".into());
    let output = serde_json::json!({"output_type":"stream","name":"stdout","text":"42\n"});
    assert!(
        !book
            .apply_output(uid, 0, Some(output.clone()), Some(1), false)
            .unwrap()
    );
    assert!(book.cells[0].outputs().is_empty());
    assert!(
        book.apply_output(uid, 1, Some(output), Some(1), false)
            .unwrap()
    );
    assert_eq!(book.cells[0].outputs()[0]["text"], "42\n");
    let before = book.serialize().unwrap();
    assert!(
        book.apply_edits(
            uid,
            1,
            false,
            &[rode_runtime_core::service::OutputEdit::Replace {
                index: 99,
                output: rode_runtime_core::service::Output::new(
                    serde_json::json!({"text":"new text"}),
                    None
                )
                .unwrap()
            }],
            Some(99)
        )
        .is_err()
    );
    assert_eq!(book.serialize().unwrap(), before);
    assert!(!book.apply_output(0, 1, None, None, true).unwrap());
    for invalid in [
        "{}",
        "[]",
        r#"{"nbformat":3,"metadata":{},"cells":[]}"#,
        r#"{"nbformat":4,"metadata":{},"cells":[{"cell_type":"code","metadata":{},"source":null,"outputs":[]}]}"#,
    ] {
        assert!(Notebook::parse(invalid).is_err());
    }
    let large = serde_json::json!({"nbformat":4,"nbformat_minor":5,"metadata":{"retained":"x".repeat(8 * 1024 * 1024 + 1)},"cells":[]});
    let large = Notebook::parse(&large.to_string())
        .unwrap()
        .serialize()
        .unwrap();
    assert!(large.len() > 8 * 1024 * 1024);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&large).unwrap()["metadata"]["retained"]
            .as_str()
            .unwrap()
            .len(),
        8 * 1024 * 1024 + 1
    );
}
#[test]
fn installed_python_kernel_keeps_state_streams_errors_and_real_png_outputs() {
    let Some(python) = interpreter() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let plan = native_peer::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap();
    let mut kernel = Kernel::start(plan, None, python, "python3".into(), Arc::new(|| {})).unwrap();
    kernel
        .run(vec![
            Run {
                cell: String::new(),
                uid: 1,
                version: 0,
                code: "counter=40; print('first')".into(),
            },
            Run {
                cell: String::new(),
                uid: 2,
                version: 3,
                code: "print(counter+2)".into(),
            },
            Run {
                cell: String::new(),
                uid: 3,
                version: 0,
                code: "import matplotlib.pyplot as plt\nplt.plot([1,2],[3,4]); plt.show()".into(),
            },
        ])
        .unwrap();
    let events = wait(&mut kernel, 3);
    assert!(
        events.iter().any(
            |e| matches!(e,Event::Output{run,output,..} if run.uid==2&&output["text"]=="42\n")
        )
    );
    assert!(
        events.iter().any(
            |e| matches!(e,Event::Output{output,..} if output["data"]["image/png"].is_string())
        )
    );
    kernel
        .run(vec![
            Run {
                cell: String::new(),
                uid: 4,
                version: 0,
                code: "raise ValueError('expected failure')".into(),
            },
            Run {
                cell: String::new(),
                uid: 5,
                version: 0,
                code: "print('must not run')".into(),
            },
        ])
        .unwrap();
    let events = wait(&mut kernel, 1);
    assert!(
        events
            .iter()
            .any(|e| matches!(e,Event::Done(_,status) if status=="error"))
    );
    assert!(!kernel.busy());
    kernel
        .run(vec![Run {
            cell: String::new(),
            uid: 6,
            version: 0,
            code: "print(counter)".into(),
        }])
        .unwrap();
    let events = wait(&mut kernel, 1);
    assert!(
        events
            .iter()
            .any(|e| matches!(e,Event::Output{output,..} if output["text"]=="40\n"))
    );
    kernel.stop();
}
#[test]
fn kernel_interrupt_and_scope_cleanup_stop_only_the_owned_kernel() {
    let Some(python) = interpreter() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let plan = native_peer::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap();
    let mut kernel = Kernel::start(plan, None, python, "python3".into(), Arc::new(|| {})).unwrap();
    let marker = root.path().join("pid");
    let pending = root.path().join("pid.pending");
    let release = root.path().join("publish-pid");
    let code = format!(
        r#"import os
import time
from pathlib import Path

marker = Path({:?})
pending = Path({:?})
release = Path({:?})
with pending.open("w", encoding="ascii") as output:
    while not release.exists():
        time.sleep(0.01)
    output.write(str(os.getpid()))
pending.replace(marker)
time.sleep(30)
"#,
        marker.to_string_lossy(),
        pending.to_string_lossy(),
        release.to_string_lossy(),
    );
    kernel
        .run(vec![Run {
            cell: String::new(),
            uid: 1,
            version: 0,
            code,
        }])
        .unwrap();
    let start = Instant::now();
    let mut wait_for_marker = |path: &Path| {
        while !path.exists() {
            for event in kernel.poll() {
                match event {
                    Event::Error(error) => panic!("{error}"),
                    Event::Done(_, status) => panic!(
                        "Kernel finished ({status}) before publishing {}",
                        path.display()
                    ),
                    _ => {}
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(25),
                "Kernel did not publish {}: {}",
                path.display(),
                kernel.status
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    // Force creation-before-write instead of relying on the runner's scheduler.
    // Only the renamed, closed file is a readiness signal carrying a usable PID.
    wait_for_marker(&pending);
    assert!(std::fs::read(&pending).unwrap().is_empty());
    assert!(!marker.exists());
    std::fs::write(release, b"").unwrap();
    wait_for_marker(&marker);
    let published = std::fs::read_to_string(marker).unwrap();
    let pid: i32 = published
        .parse()
        .unwrap_or_else(|error| panic!("Invalid published kernel PID {published:?}: {error}"));
    assert!(pid > 0, "Kernel PID must identify one process: {pid}");
    kernel.interrupt().unwrap();
    let events = wait(&mut kernel, 1);
    assert!(
        events.iter().any(
            |event| matches!(event, Event::Output { output, .. } if output["ename"] == "KeyboardInterrupt")
        ),
        "Interrupt did not stop the active execution: {events:?}"
    );
    assert!(!kernel.busy());
    drop(kernel);
    let start = Instant::now();
    while unsafe { libc::kill(pid, 0) } == 0 {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "owned kernel survived closing its session"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn actual_sandbox_encloses_kernel_and_its_file_operations_without_network_sharing() {
    let Some(python) = interpreter() else {
        return;
    };
    let python = PathBuf::from(python);
    let env = python.parent().unwrap().parent().unwrap().to_path_buf();
    let base = tempfile::tempdir().unwrap();
    let root = base.path().join("project");
    std::fs::create_dir(&root).unwrap();
    let outside = base.path().join("secret");
    std::fs::write(&outside, "private").unwrap();
    let plan = native_peer::prepare(
        root.to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            share_network: false,
            read_only: vec![env.to_string_lossy().into()],
            ..Default::default()
        },
    )
    .unwrap();
    if !plan.active {
        native_peer::capabilities::skip("bwrap", &plan.reason);
        return;
    }
    let mut kernel = Kernel::start(
        plan,
        None,
        python.to_string_lossy().into(),
        "python3".into(),
        Arc::new(|| {}),
    )
    .unwrap();
    let code = format!(
        concat!(
            "import os\nfrom ipykernel.connect import get_connection_info\n",
            "connection = get_connection_info(unpack=True)\n",
            "assert connection['transport'] == 'ipc'\n",
            "assert os.path.realpath(connection['ip']).startswith(os.path.realpath(os.environ['TMPDIR']) + '/')\n",
            "print(os.path.exists({:?}))\nopen('inside','w').write('sandbox')"
        ),
        outside.to_string_lossy()
    );
    kernel
        .run(vec![Run {
            cell: String::new(),
            uid: 1,
            version: 0,
            code,
        }])
        .unwrap();
    let events = wait(&mut kernel, 1);
    assert!(
        events
            .iter()
            .any(|e| matches!(e,Event::Output{output,..} if output["text"]=="False\n"))
    );
    assert_eq!(
        std::fs::read_to_string(root.join("inside")).unwrap(),
        "sandbox"
    );
    kernel.stop();
}

#[test]
fn default_python_kernel_uses_the_selected_interpreter_despite_a_shadowing_kernelspec() {
    use std::os::unix::fs::PermissionsExt;
    let Some(python) = interpreter() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let kernels = root.path().join("jupyter/kernels/python3");
    std::fs::create_dir_all(&kernels).unwrap();
    std::fs::write(kernels.join("kernel.json"), serde_json::json!({
        "argv": ["/nonexistent/foreign-python", "-m", "ipykernel_launcher", "-f", "{connection_file}"],
        "display_name": "Foreign Python", "language": "python"
    }).to_string()).unwrap();
    let launcher = root.path().join("selected-python");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nexport JUPYTER_PATH={}\nexec {} \"$@\"\n",
            rode::ssh::quote(&root.path().join("jupyter").to_string_lossy()),
            rode::ssh::quote(&python)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
    let plan = native_peer::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap();
    let mut kernel = Kernel::start(
        plan,
        None,
        launcher.to_string_lossy().into(),
        "python3".into(),
        Arc::new(|| {}),
    )
    .unwrap();
    kernel
        .run(vec![Run {
            cell: String::new(),
            uid: 1,
            version: 0,
            code: "import sys; print(sys.executable)".into(),
        }])
        .unwrap();
    let events = wait(&mut kernel, 1);
    assert!(events.iter().any(|event| matches!(event, Event::Output{output,..} if output["text"].as_str().is_some_and(|text| text.trim() == python))));
}
