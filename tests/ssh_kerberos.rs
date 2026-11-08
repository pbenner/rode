//! Real Kerberos/GSSAPI logins. Run through scripts/test-kerberos.py.
#![cfg(target_os = "linux")]
use rode::ssh::{Profile, Remote, quote};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct Connection {
    remote: Remote,
    child: Child,
    log: tempfile::NamedTempFile,
    _config: tempfile::NamedTempFile,
}

fn fixture() -> PathBuf {
    std::env::var_os("RODE_TEST_KERBEROS_DIR")
        .expect("Run scripts/test-kerberos.py")
        .into()
}

impl Connection {
    fn new(host: &str, jump: bool, options: &str, cache: &str) -> Self {
        let root = fixture();
        let config = tempfile::NamedTempFile::new().unwrap();
        fs::write(
            config.path(),
            format!(
                "{options}\n{}",
                fs::read_to_string(root.join("ssh_config")).unwrap()
            ),
        )
        .unwrap();
        let remote = Remote::new(Profile {
            host: host.into(),
            jump: if jump { "jump".into() } else { String::new() },
            config: config.path().to_string_lossy().into(),
            x11_forwarding: false,
            ..Default::default()
        })
        .unwrap();
        let log = tempfile::NamedTempFile::new().unwrap();
        let child = Command::new(remote.client_program())
            .args(remote.master_args().unwrap())
            .env("PATH", "") // No ssh, kinit or other client executable available.
            .env("KRB5CCNAME", format!("FILE:{}", root.join(cache).display()))
            .env_remove("KRB5_CLIENT_KTNAME")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log.reopen().unwrap())
            .spawn()
            .unwrap();
        Self {
            remote,
            child,
            log,
            _config: config,
        }
    }

    fn text(&self) -> String {
        fs::read_to_string(self.log.path()).unwrap()
    }

    fn wait(&mut self, success: bool) {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if self.remote.connected() {
                assert!(success, "{}", self.text());
                return;
            }
            if self.child.try_wait().unwrap().is_some() {
                assert!(!success, "{}", self.text());
                return;
            }
            assert!(Instant::now() < until, "Login timed out: {}", self.text());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn output(&self, command: &str) -> String {
        let output = self
            .remote
            .command()
            .arg(command)
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}\n{}", self.text());
        String::from_utf8(output.stdout).unwrap()
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.remote.cleanup();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "requires private Kerberos realm and sshd"]
fn kerberos_login_reuses_channels_and_authenticates_each_jump_without_client_tools() {
    for jump in [false, true] {
        let mut connection =
            Connection::new("target", jump, "PubkeyAuthentication no", "valid.ccache");
        connection.wait(true);
        assert_eq!(
            connection
                .text()
                .matches("Authenticated using GSSAPI/Kerberos.")
                .count(),
            if jump { 2 } else { 1 }
        );
        for _ in 0..3 {
            assert_eq!(
                connection.output("printf kerberos-channel"),
                "kerberos-channel"
            );
        }
    }
}

#[test]
#[ignore = "requires private Kerberos realm and sshd"]
fn kerberos_delegation_requires_explicit_host_configuration() {
    for delegate in [false, true] {
        // The jump authenticates from the desktop cache without delegation.
        let options = format!(
            "Host target\n GSSAPIDelegateCredentials {}\nHost *\n PubkeyAuthentication no",
            if delegate { "yes" } else { "no" }
        );
        let mut connection = Connection::new("target", true, &options, "valid.ccache");
        connection.wait(true);
        let klist = quote(&std::env::var("RODE_TEST_KLIST").unwrap());
        let check = format!(
            "if [ -n \"$KRB5CCNAME\" ] && {klist} -s; then printf delegated; else printf none; fi"
        );
        assert_eq!(
            connection.output(&check),
            if delegate { "delegated" } else { "none" }
        );
    }
}

#[test]
#[ignore = "requires private Kerberos realm and sshd"]
fn missing_and_expired_tickets_fall_back_only_to_enabled_methods() {
    for cache in ["missing.ccache", "expired.ccache"] {
        for keys in [true, false] {
            let mut connection = Connection::new(
                "target",
                false,
                &format!("PubkeyAuthentication {}", if keys { "yes" } else { "no" }),
                cache,
            );
            connection.wait(keys);
            assert!(
                connection.text().contains("GSSAPI/Kerberos:"),
                "{}",
                connection.text()
            );
            assert!(
                !connection
                    .text()
                    .contains("Authenticated using GSSAPI/Kerberos.")
            );
            if keys {
                assert_eq!(connection.output("printf key-fallback"), "key-fallback");
            } else {
                assert!(connection.text().contains("SSH authentication failed"));
            }
        }
    }
}

#[test]
#[ignore = "requires private Kerberos realm and sshd"]
fn disabled_kerberos_and_host_verification_are_respected_with_valid_tickets() {
    let mut keys = Connection::new("target", false, "GSSAPIAuthentication no", "valid.ccache");
    keys.wait(true);
    assert!(!keys.text().contains("GSSAPI/Kerberos"));
    let mut refused = Connection::new("target", false, "HostKeyAlias untrusted", "valid.ccache");
    refused.wait(false);
    assert!(
        refused.text().contains("Unknown SSH host key"),
        "{}",
        refused.text()
    );
    assert!(
        !refused
            .text()
            .contains("Authenticated using GSSAPI/Kerberos.")
    );
}

#[test]
#[ignore = "requires private Kerberos realm and sshd"]
fn kerberos_partial_success_still_requires_the_public_key_factor() {
    let mut success = Connection::new("second-factor", false, "", "valid.ccache");
    success.wait(true);
    assert_eq!(success.output("printf two-factors"), "two-factors");
    assert!(
        success
            .text()
            .contains("Kerberos authentication did not complete login")
    );
    let mut failure = Connection::new(
        "second-factor",
        false,
        "PubkeyAuthentication no",
        "valid.ccache",
    );
    failure.wait(false);
    assert!(failure.text().contains("SSH authentication failed"));
}
