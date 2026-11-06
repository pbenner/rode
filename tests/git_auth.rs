//! The real headless credential helper: origin confinement and read-only lifecycle.
#![cfg(target_os = "linux")]
use std::{
    io::Write,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Command, Stdio},
};

fn helper(action: &str, input: &str) -> std::process::Output {
    helper_memory(action, input, true)
}

fn helper_memory(action: &str, input: &str, sealed: bool) -> std::process::Output {
    let bytes = br#"[{"origin":"https://git.example.test:8443","username":"private-user","password":"private-password"}]"#;
    let fd = unsafe {
        libc::memfd_create(
            c"rode-git-auth-fixture".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    assert!(fd >= 0);
    let mut memory = unsafe { std::fs::File::from_raw_fd(fd) };
    memory.write_all(bytes).unwrap();
    if sealed {
        assert_eq!(
            unsafe {
                libc::fcntl(
                    fd,
                    libc::F_ADD_SEALS,
                    libc::F_SEAL_SEAL
                        | libc::F_SEAL_SHRINK
                        | libc::F_SEAL_GROW
                        | libc::F_SEAL_WRITE,
                )
            },
            0
        );
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_rode-sessiond"));
    command
        .args(["--git-credential", &fd.to_string(), action])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let source = memory.as_raw_fd();
    unsafe {
        command.pre_exec(move || {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // Exercise a small soft limit without raising the host's hard limit.
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            limit.rlim_cur = limit.rlim_cur.min(128);
            if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) < 0
                || libc::fcntl(source, libc::F_SETFD, 0) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn memory_credentials_are_supplied_only_to_the_exact_https_origin() {
    let output = helper(
        "get",
        "protocol=https\nhost=git.example.test:8443\npath=repo.git\n\n",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        b"username=private-user\npassword=private-password\n\n"
    );
    for context in [
        "protocol=https\nhost=other.example.test:8443\n\n",
        "protocol=https\nhost=git.example.test:9443\n\n",
        "protocol=http\nhost=git.example.test:8443\n\n",
    ] {
        let output = helper("get", context);
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn store_and_erase_are_ignored_and_context_injection_is_rejected() {
    for action in ["store", "erase", "unknown"] {
        let output = helper(
            action,
            "protocol=https\nhost=git.example.test:8443\nusername=private-user\npassword=private-password\n\n",
        );
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    let output = helper(
        "get",
        "protocol=https\nhost=other.example.test@git.example.test:8443\n\n",
    );
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private"));
}

#[test]
fn closed_and_mutable_descriptors_fail_without_an_io_safety_abort_or_secret_output() {
    for descriptor in ["-1", "0", "2", "2147483647"] {
        let output = Command::new(env!("CARGO_BIN_EXE_rode-sessiond"))
            .args(["--git-credential", descriptor, "get"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Invalid Git credential descriptor")
        );
    }
    let output = helper_memory(
        "get",
        "protocol=https\nhost=git.example.test:8443\n\n",
        false,
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not sealed memory"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private"));
}
