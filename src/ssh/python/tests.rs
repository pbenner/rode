//! Real staged-helper execution and exact preservation of following protocol bytes.

use super::*;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

struct Process {
    child: Child,
    owned: Option<i32>,
}
impl Process {
    fn spawn(script: &Script, directory: &std::path::Path, argument: &str) -> Self {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(format!("{} {argument}", script.command()))
            .env("TMPDIR", directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::process::new_session(&mut command);
        Self {
            child: command.spawn().unwrap(),
            owned: None,
        }
    }
    fn wait(&mut self) -> std::process::ExitStatus {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < until, "Python helper did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
        if let Some(pid) = self.owned {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}

#[test]
fn streamed_script_preserves_fragmented_source_coalesced_protocol_and_arguments() {
    let script = Script {
        name: "rode-loader-test.py",
        source: r#"from __future__ import annotations
import json, os, sys
marker = 'original-source-marker-λ'
data = sys.stdin.buffer.readline()
print(json.dumps(dict(size=len(data), marker=marker, args=sys.argv[1:],
    path=__file__, absent=not os.path.exists(__file__),
    directory_absent=not os.path.exists(os.path.dirname(__file__)),
    cmdline=open('/proc/self/cmdline','rb').read().decode() if sys.platform.startswith('linux') else '')))
"#,
    };
    assert!(script.command().len() < 512);
    assert!(!script.command().contains("original-source-marker"));
    for fragmented in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut process = Process::spawn(&script, temp.path(), "--example");
        let mut input = process.child.stdin.take().unwrap();
        let payload = format!("{}λ\n", "x".repeat(400_000));
        let framed = script.input(payload.as_bytes().to_vec()).unwrap();
        if fragmented {
            let header = 4 + u32::from_be_bytes(framed[..4].try_into().unwrap()) as usize;
            for chunk in framed[..header].chunks(3) {
                input.write_all(chunk).unwrap();
            }
            input.write_all(&framed[header..]).unwrap();
        } else {
            input.write_all(&framed).unwrap();
        }
        drop(input);
        let mut reader = BufReader::new(process.child.stdout.take().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let result: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(process.wait().success());
        assert_eq!(result["size"], payload.len());
        assert_eq!(result["marker"], "original-source-marker-λ");
        assert_eq!(result["args"], serde_json::json!(["--example"]));
        assert_eq!(result["absent"], true);
        assert_eq!(result["directory_absent"], true);
        if cfg!(target_os = "linux") {
            let argv = result["cmdline"].as_str().unwrap();
            assert!(argv.contains("rode-loader-test.py"), "{argv}");
            assert!(!argv.contains("original-source-marker"), "{argv}");
            assert!(argv.len() < 256, "{argv}");
        }
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[test]
fn incomplete_oversized_and_invalid_source_leave_no_staged_files() {
    let script = Script {
        name: "rode-failing-test.py",
        source: "def broken(",
    };
    let complete = script.input(vec![]).unwrap();
    for data in [
        vec![],
        vec![0, 0, 0],
        vec![0, 0, 0, 0],
        ((LIMIT + 1) as u32).to_be_bytes().to_vec(),
        vec![0, 0, 0, 8, b'x'],
        complete,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut process = Process::spawn(&script, temp.path(), "");
        let mut input = process.child.stdin.take().unwrap();
        let _ = input.write_all(&data);
        drop(input);
        assert!(!process.wait().success());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}

#[test]
fn named_supervisor_forwards_large_messages_and_reaps_its_child_on_eof() {
    let temp = tempfile::tempdir().unwrap();
    let mut process = Process::spawn(&PROCESS, temp.path(), "");
    let mut input = process.child.stdin.take().unwrap();
    let child = "import json,os,signal,sys,time\nsignal.signal(signal.SIGTERM,signal.SIG_IGN)\nline=sys.stdin.buffer.readline()\nprint(json.dumps(dict(pid=os.getpid(),parent=os.getppid(),size=len(line))),flush=True)\ntime.sleep(60)\n";
    let launch = serde_json::json!({"root":temp.path(), "prefix":[], "argv":["python3","-c",child], "python":"off"});
    let payload = serde_json::to_vec(&serde_json::json!({"data":"x".repeat(400_000)})).unwrap();
    let mut data = format!("{launch}\n").into_bytes();
    data.extend_from_slice(&payload);
    data.push(b'\n');
    input.write_all(&PROCESS.input(data).unwrap()).unwrap();
    input.flush().unwrap();
    let (tx, received) = mpsc::channel();
    let stdout = process.child.stdout.take().unwrap();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = received.recv_timeout(Duration::from_secs(10)).unwrap();
    let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
    let pid = reply["pid"].as_i64().unwrap() as i32;
    process.owned = Some(pid);
    assert_eq!(reply["size"], payload.len() + 1);
    if cfg!(target_os = "linux") {
        let argv = fs::read_to_string(format!("/proc/{}/cmdline", reply["parent"])).unwrap();
        assert!(argv.contains("rode-ssh-process.py"), "{argv}");
        assert!(!argv.contains("Supervise one project-host"), "{argv}");
        assert!(argv.len() < 256, "{argv}");
    }
    drop(input);
    assert!(!process.wait().success()); // The deliberately TERM-resistant child is killed.
    if cfg!(target_os = "linux") {
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
    process.owned = None;
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}
