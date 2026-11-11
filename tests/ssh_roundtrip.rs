//! Run through scripts/test-ssh.sh, which creates an isolated localhost SSH server.
#[path = "ssh_roundtrip/agent_forwarding.rs"]
mod agent_forwarding;
#[path = "ssh_roundtrip/explorer.rs"]
mod explorer;
#[path = "support/process.rs"]
mod fixture_process;
#[path = "ssh_roundtrip/installed_helpers.rs"]
mod installed_helpers;
#[path = "ssh_roundtrip/large_files.rs"]
mod large_files;
#[path = "support/terminal.rs"]
mod terminal_ui;
#[path = "ssh_roundtrip/tunnels.rs"]
mod tunnels;
#[path = "ssh_roundtrip/x11.rs"]
mod x11;

#[path = "support/capabilities.rs"]
mod capabilities;

use rode::{
    files::Request,
    ssh::{Profile, Remote},
    terminal::Terminal,
};
use std::{
    process::Command,
    time::{Duration, Instant},
};

#[path = "ssh_roundtrip/agents.rs"]
mod agents;
#[path = "ssh_roundtrip/policy.rs"]
mod policy;
use agents::{agent_roundtrip, bundled_runtime_roundtrip};
#[path = "ssh_roundtrip/runtimes.rs"]
mod runtimes;
use runtimes::{
    debugger_roundtrip, interactive_allocation_roundtrip, interpreter_grants, native_attach,
    native_debugger_roundtrip, native_terminal_start,
};
#[path = "ssh_roundtrip/documents.rs"]
mod documents;
use documents::{
    binary_extension_roundtrip, encrypted_roundtrip, latex_roundtrip, marp_roundtrip,
    notebook_roundtrip,
};
#[path = "ssh_roundtrip/transport.rs"]
mod transport;

/// Policy probes use the system runtime covered by the built-in /usr grant.
/// PATH-managed interpreters can depend on standard libraries outside that grant;
/// isolated mode also excludes Python environment variables and user packages.
fn policy_python(code: &str) -> [String; 4] {
    [
        "/usr/bin/python3".into(),
        "-I".into(),
        "-c".into(),
        code.into(),
    ]
}

#[test]
fn policy_python_ignores_path_managers_and_python_environment() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let shadow = root.path().join("python3");
    std::fs::write(
        &shadow,
        "#!/bin/sh\nprintf 'unexpected PATH Python\\n' >&2\nexit 99\n",
    )
    .unwrap();
    std::fs::set_permissions(&shadow, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        root.path().join("encodings.py"),
        "raise RuntimeError('project Python module loaded')\n",
    )
    .unwrap();
    let args = policy_python(
        "import encodings, sys; assert sys.flags.isolated; print('SYSTEM_PYTHON_OK')",
    );
    let output = Command::new(&args[0])
        .args(&args[1..])
        .current_dir(root.path())
        .env("PATH", root.path())
        .env("PYTHONHOME", root.path().join("missing-runtime"))
        .env("PYTHONPATH", root.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "SYSTEM_PYTHON_OK"
    );
}
