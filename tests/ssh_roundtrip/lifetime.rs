//! Private SSH masters follow the creating process, including callers without a PTY.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    net::TcpListener,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::PathBuf,
    process::{Child, Stdio},
};

const OWNER_ENV: &str = "RODE_TEST_SSH_MASTER_OWNER";
const OWNER_CASE: &str = "lifetime::owner_exit_cancels_pending_authentication";

fn can_pin_master() -> bool {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, std::process::id(), 0) } as i32;
    if fd >= 0 {
        drop(unsafe { OwnedFd::from_raw_fd(fd) });
        return true;
    }
    let error = std::io::Error::last_os_error();
    assert!(
        matches!(
            error.raw_os_error(),
            Some(libc::ENOSYS | libc::EPERM | libc::EACCES)
        ),
        "{error}"
    );
    super::capabilities::skip(
        "pidfd",
        &format!("Cannot safely pin SSH fixture processes: {error}"),
    );
    false
}

#[derive(Serialize, Deserialize)]
struct Launch {
    program: PathBuf,
    args: Vec<String>,
    ready: PathBuf,
}

/// Pin the exact fixture master so failure cleanup cannot signal a reused PID.
struct Master(OwnedFd);
impl Master {
    fn open(pid: u32) -> Self {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
        assert!(fd >= 0, "{}", std::io::Error::last_os_error());
        Self(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn exited(&self) -> bool {
        let mut event = libc::pollfd {
            fd: self.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe { libc::poll(&mut event, 1, 0) > 0 }
    }
}
impl Drop for Master {
    fn drop(&mut self) {
        if !self.exited() {
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    self.0.as_raw_fd(),
                    libc::SIGKILL,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }
    }
}

struct Owner {
    directory: tempfile::TempDir,
    child: Child,
    remote: Remote,
    master: Option<Master>,
}
impl Owner {
    fn start(profile: Profile) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let remote = Remote::new(profile).unwrap();
        let ready = directory.path().join("ready");
        let launch = Launch {
            program: remote.client_program(),
            args: remote.master_args().unwrap(),
            ready: ready.clone(),
        };
        let log = fs::File::create(directory.path().join("owner.log")).unwrap();
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", OWNER_CASE, "--nocapture"])
            .env(OWNER_ENV, serde_json::to_string(&launch).unwrap())
            .stdin(Stdio::piped())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let mut owner = Self {
            directory,
            child,
            remote,
            master: None,
        };
        let until = Instant::now() + Duration::from_secs(15);
        while !ready.exists() {
            assert!(
                owner.child.try_wait().unwrap().is_none() && Instant::now() < until,
                "{}",
                owner.diagnostics()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        owner.master = Some(Master::open(
            fs::read_to_string(ready).unwrap().parse().unwrap(),
        ));
        owner
    }

    fn diagnostics(&self) -> String {
        ["owner.log", "master.log"]
            .map(|name| fs::read_to_string(self.directory.path().join(name)).unwrap_or_default())
            .join("\n")
    }

    fn end_owner(&mut self) {
        assert!(
            !self.master.as_ref().unwrap().exited(),
            "{}",
            self.diagnostics()
        );
        self.child.kill().unwrap();
        self.child.wait().unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while !self.master.as_ref().unwrap().exited() {
            assert!(
                Instant::now() < until,
                "SSH master outlived its owner: {}",
                self.diagnostics()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!self.remote.socket.exists());
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.master.take();
        self.remote.cleanup();
    }
}

fn owner_child() -> bool {
    let Some(value) = std::env::var_os(OWNER_ENV) else {
        return false;
    };
    let launch: Launch = serde_json::from_str(&value.to_string_lossy()).unwrap();
    let ready = launch.ready.clone();
    // The spawning worker exits before ownership is acknowledged. Only process
    // death should end the master; creator-thread death must leave it usable.
    let mut master = std::thread::spawn(move || {
        let log = fs::File::create(launch.ready.with_file_name("master.log")).unwrap();
        Command::new(launch.program)
            .args(launch.args)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap()
    })
    .join()
    .unwrap();
    let temporary = ready.with_extension("tmp");
    fs::write(&temporary, master.id().to_string()).unwrap();
    fs::rename(temporary, ready).unwrap();
    let _ = std::io::stdin().read_exact(&mut [0]);
    let _ = master.kill();
    let _ = master.wait();
    true
}

#[test]
fn owner_exit_cancels_pending_authentication() {
    if owner_child() {
        return;
    }
    if !can_pin_master() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut owner = Owner::start(Profile {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port().to_string(),
        x11_forwarding: false,
        ..Default::default()
    });
    let until = Instant::now() + Duration::from_secs(10);
    let _peer = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < until, "{}", owner.diagnostics());
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("{error}"),
        }
    };
    // Keep the peer open without an SSH banner: network timeout or EOF must not
    // masquerade as successful owner-loss cancellation.
    owner.end_owner();
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn owner_exit_closes_only_its_authenticated_master() {
    if !can_pin_master() {
        return;
    }
    let profile = Profile {
        host: "rode-test".into(),
        config: std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh"),
        x11_forwarding: false,
        ..Default::default()
    };
    let other = Remote::new(profile.clone()).unwrap();
    let login = Terminal::remote(&eframe::egui::Context::default(), &other).unwrap();
    let mut owner = Owner::start(profile);
    let until = Instant::now() + Duration::from_secs(15);
    while !other.connected() || !owner.remote.connected() {
        assert!(
            login.is_alive() && Instant::now() < until,
            "{}\n{}",
            login.contents(),
            owner.diagnostics()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    owner.end_owner();
    assert!(
        other.connected(),
        "Owner loss stopped another window's master"
    );
    let mut command = other.command();
    command.arg("printf OWNER_SURVIVOR");
    let output = rode::project::run(command, vec![], Duration::from_secs(5)).unwrap();
    assert_eq!(output.checked().unwrap(), "OWNER_SURVIVOR");
    drop(login);
    other.cleanup();
}
