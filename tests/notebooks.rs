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
        let png = events
            .iter()
            .find_map(|event| match event {
                Event::Output { output, .. } => output["data"]["image/png"].as_str(),
                _ => None,
            })
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
                rode_runtime_core::notebook::outputs::MAX_TEXT_BYTES + 1
            ),
        );
        let following = run(
            3,
            "assert overflow_finished\nopen('queue-finished', 'w').write('ok')".into(),
        );
        kernel.bind(vec![overflow.clone(), following.clone()]);
        kernel.run(vec![overflow, following]).unwrap();
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
        let reuse = run(
            4,
            "assert sentinel == 123\nassert os.getpid() == kernel_pid\nprint('same kernel')".into(),
        );
        kernel.bind(vec![reuse.clone()]);
        kernel.run(vec![reuse]).unwrap();
        let events = wait(&mut kernel, 1);
        assert!(events.iter().any(
            |event| matches!(event, Event::Output{output,..} if output["text"]=="same kernel\n")
        ));

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
fn stream_fragments_coalesce_without_reordering_outputs_or_bypassing_text_limits() {
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
    assert!(book.apply_output(uid, 0, Some(json!({"output_type":"stream", "name":"stdout", "text":"x".repeat(rode_runtime_core::notebook::outputs::MAX_TEXT_BYTES)})), None, false).is_err());
    assert_eq!(book.serialize().unwrap(), before);
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
    let huge = serde_json::json!({"output_type":"stream","name":"stdout","text":"x".repeat(notebook::MAX_OUTPUT_BYTES+1)});
    assert!(
        book.apply_output(uid, 1, Some(huge), Some(99), false)
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
