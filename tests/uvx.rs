//! Exercise the native and concatenated SSH services with an isolated uvx peer.
use rode::{files, project};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    process::{Command, Stdio},
};

#[test]
fn default_ruff_uses_uvx_with_a_writable_registry_on_both_backends() {
    let directory = tempfile::tempdir().unwrap();
    let bin = directory.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let uvx = bin.join("uvx");
    fs::write(
        &uvx,
        r#"#!/usr/bin/python3
import json, os, sys
assert sys.argv[1] == "ruff", sys.argv
assert os.environ["UV_TOOL_DIR"] == os.path.join(os.environ["UV_CACHE_DIR"], "tools")
assert "RUFF_OUTPUT_FILE" not in os.environ
source = sys.stdin.read()
if sys.argv[2] == "format":
    print(source.replace("x=1", "x = 1"), end="")
else:
    assert "--no-fix" in sys.argv
    print(json.dumps([]))
"#,
    )
    .unwrap();
    fs::set_permissions(&uvx, fs::Permissions::from_mode(0o755)).unwrap();
    let local = directory.path().join(".local/bin/uvx");
    fs::create_dir_all(local.parent().unwrap()).unwrap();
    fs::copy(&uvx, &local).unwrap();
    let path = directory.path().join("source.py");
    fs::write(&path, "disk stays unchanged\n").unwrap();
    for on_path in [true, false] {
        for ruff in ["", "uvx ruff"] {
            let request = files::Request::Tool {
                request: project::Request::Analyze {
                    root: directory.path().to_string_lossy().into(),
                    path: path.to_string_lossy().into(),
                    text: "x=1\n".into(),
                    python: "auto".into(),
                    ruff: ruff.into(),
                    format: true,
                    check: true,
                    fix: false,
                },
            };
            for remote in [false, true] {
                let mut command = if remote {
                    let mut command = Command::new("python3");
                    command.args(["-c", files::REMOTE_HELPER]);
                    command
                } else {
                    let mut command = Command::new(env!("CARGO_BIN_EXE_rode"));
                    command.arg("--file-helper");
                    command
                };
                let mut paths = vec![
                    std::path::PathBuf::from("/usr/bin"),
                    std::path::PathBuf::from("/bin"),
                ];
                if on_path {
                    paths.insert(0, bin.clone());
                }
                let mut child = command
                    .env("PATH", std::env::join_paths(paths).unwrap())
                    .env("HOME", directory.path())
                    .env("UV_CACHE_DIR", directory.path().join("cache"))
                    .env_remove("UV_TOOL_DIR")
                    .env("RUFF_OUTPUT_FILE", "must-not-be-used")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(&serde_json::to_vec(&request).unwrap())
                    .unwrap();
                let output = child.wait_with_output().unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let response: files::Response = serde_json::from_slice(&output.stdout).unwrap();
                let Some(project::Response::Analysis(result)) = response.tool else {
                    panic!()
                };
                assert_eq!(result.text, "x = 1\n");
                assert!(result.diagnostics.is_empty());
                assert_eq!(fs::read_to_string(&path).unwrap(), "disk stays unchanged\n");
            }
        }
    }
}
