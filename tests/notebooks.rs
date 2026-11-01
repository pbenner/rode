//! nbformat preservation and versioned outputs; real installed Jupyter kernels are enabled only
//! through the test override.

use rode::{
    notebook::{
        self, Notebook,
        kernel::{Event, Kernel, Run},
    },
    sandbox,
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
const FIXTURE: &str = include_str!("fixtures/notebook.ipynb");
fn interpreter() -> Option<String> {
    std::env::var("RODE_TEST_JUPYTER").ok()
}
fn wait(kernel: &mut Kernel, done: usize) -> Vec<Event> {
    let mut events = vec![];
    let mut completed = 0;
    let start = Instant::now();
    while completed < done {
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
    assert!(Notebook::parse(&" ".repeat(notebook::MAX_BYTES + 1)).is_err());
}
#[test]
fn installed_python_kernel_keeps_state_streams_errors_and_real_png_outputs() {
    let Some(python) = interpreter() else {
        eprintln!(
            "Set RODE_TEST_JUPYTER to an interpreter with jupyter_client, ipykernel and matplotlib."
        );
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let plan = sandbox::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap();
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
    let plan = sandbox::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap();
    let mut kernel = Kernel::start(plan, None, python, "python3".into(), Arc::new(|| {})).unwrap();
    let marker = root.path().join("pid");
    let code = format!(
        "import os,time\nopen({:?},'w').write(str(os.getpid()))\ntime.sleep(30)",
        marker.to_string_lossy()
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
    while !marker.exists() {
        for e in kernel.poll() {
            if let Event::Error(error) = e {
                panic!("{error}");
            }
        }
        assert!(start.elapsed() < Duration::from_secs(25));
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid: i32 = std::fs::read_to_string(marker).unwrap().parse().unwrap();
    kernel.interrupt().unwrap();
    let _ = wait(&mut kernel, 1);
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
    let plan = sandbox::prepare(
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
        eprintln!("bwrap unavailable: {}", plan.reason);
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
        "import os\nprint(os.path.exists({:?}))\nopen('inside','w').write('sandbox')",
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
    let plan = sandbox::prepare(root.path().to_str().unwrap(), &Default::default()).unwrap();
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
