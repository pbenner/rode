//! Actual unprivileged Landlock enforcement and local/SSH-peer policy agreement.
#[path = "support/python.rs"]
mod python_peer;

use rode::{files, sandbox};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

fn run_bounded(mut command: Command, input: Vec<u8>, timeout: Duration) -> anyhow::Result<Vec<u8>> {
    use std::io::{Read, Seek};
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let until = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Landlock test subprocess timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = writer.join().unwrap();
    stderr.rewind()?;
    let mut diagnostic = String::new();
    stderr.take(16384).read_to_string(&mut diagnostic)?;
    anyhow::ensure!(status.success(), "{status}: {diagnostic}");
    stdout.rewind()?;
    let mut bytes = Vec::new();
    stdout.take(1024 * 1024).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn options() -> sandbox::Options {
    sandbox::Options {
        enabled: true,
        backend: sandbox::Backend::Landlock,
        ..Default::default()
    }
}

fn peer(root: &Path, options: &sandbox::Options, path: Option<&Path>) -> sandbox::Plan {
    let mut command = Command::new(sandbox::find_program("python3").unwrap());
    command.args(["-c", &python_peer::source(files::REMOTE_HELPER)]);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let bytes = run_bounded(
        command,
        serde_json::to_vec(&files::Request::Prepare {
            path: root.to_string_lossy().into(),
            sandbox: options.clone(),
        })
        .unwrap(),
        Duration::from_secs(15),
    )
    .unwrap();
    serde_json::from_slice::<files::Response>(&bytes)
        .unwrap()
        .project
        .unwrap()
}

fn active(root: &Path, options: &sandbox::Options) -> Option<sandbox::Plan> {
    let local = sandbox::prepare(root.to_str().unwrap(), options).unwrap();
    let remote = peer(root, options, None);
    assert_eq!(
        local.active, remote.active,
        "{} / {}",
        local.reason, remote.reason
    );
    assert_eq!(local.prefix, remote.prefix);
    assert_eq!(local.backend, sandbox::Backend::Landlock);
    assert_eq!(remote.backend, local.backend);
    if !local.active {
        assert_ne!(
            std::env::var("RODE_TEST_REQUIRE_LANDLOCK").as_deref(),
            Ok("1"),
            "{}",
            local.reason
        );
        eprintln!("Landlock unavailable: {}", local.reason);
        return None;
    }
    assert!(local.label().contains("Landlock"));
    Some(local)
}

fn run(plan: &sandbox::Plan, code: &str, arguments: &[&str]) -> anyhow::Result<Vec<u8>> {
    let mut command = Command::new(&plan.prefix[0]);
    command
        .args(&plan.prefix[1..])
        .args(["/usr/bin/python3", "-I", "-c", code])
        .args(arguments);
    run_bounded(command, vec![], Duration::from_secs(10))
}

#[test]
fn saved_backend_is_explicit_and_old_preferences_keep_bwrap() {
    let old: sandbox::Options = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
    assert_eq!(old.backend, sandbox::Backend::Bwrap);
    let encoded = serde_json::to_string(&options()).unwrap();
    assert!(encoded.contains(r#""backend":"landlock""#));
    assert_eq!(
        serde_json::from_str::<sandbox::Options>(&encoded).unwrap(),
        options()
    );
    assert!(serde_json::from_str::<sandbox::Options>(r#"{"backend":"unknown"}"#).is_err());
    let missing = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let plan = peer(root.path(), &options(), Some(missing.path()));
    assert!(plan.requested && !plan.active && plan.prefix.is_empty());
    assert!(plan.reason.contains("python3"));
    assert_eq!(plan.label(), "NORMAL MODE · sandbox unavailable");
}

#[test]
fn real_file_helpers_and_children_obey_grants_and_cannot_escape_symlinks() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret");
    fs::write(&secret, "private").unwrap();
    std::os::unix::fs::symlink(&secret, root.path().join("escape")).unwrap();
    let Some(plan) = active(root.path(), &options()) else {
        return;
    };
    let helper = Path::new(env!("CARGO_BIN_EXE_rode"));
    let file = root.path().join("file").to_string_lossy().into_owned();
    let saved = plan
        .local_request_with_helper(
            &files::Request::Write {
                path: file.clone(),
                text: "saved".into(),
                expected: None,
            },
            helper,
        )
        .unwrap();
    let read = plan
        .local_request_with_helper(&files::Request::Read { path: file }, helper)
        .unwrap();
    assert_eq!(read.text, "saved");
    assert_eq!(read.revision, saved.revision);
    for path in [&secret, &root.path().join("escape")] {
        assert!(
            plan.local_request_with_helper(
                &files::Request::Read {
                    path: path.to_string_lossy().into(),
                },
                helper
            )
            .is_err()
        );
        let mut command = Command::new(&plan.prefix[0]);
        command.args(&plan.prefix[1..]).args([
            "python3",
            "-c",
            &python_peer::source(files::REMOTE_HELPER),
        ]);
        assert!(
            run_bounded(
                command,
                serde_json::to_vec(&files::Request::Read {
                    path: path.to_string_lossy().into(),
                })
                .unwrap(),
                Duration::from_secs(10)
            )
            .is_err()
        );
    }
    let code = r#"
import os, pathlib, subprocess, sys, tempfile
assert pathlib.Path.cwd() == pathlib.Path(sys.argv[1])
with tempfile.NamedTemporaryFile() as temporary:
    assert temporary.name.startswith(os.environ['TMPDIR'] + '/')
assert os.environ['UV_LINK_MODE'] == 'copy'
child = subprocess.run([sys.executable, '-c', 'open(__import__("sys").argv[1]).read()', sys.argv[2]], capture_output=True)
assert child.returncode != 0, child.stdout
assert b'Permission denied' in child.stderr, child.stderr
try:
    pathlib.Path(sys.argv[2]).write_text('changed')
except PermissionError:
    pass
else:
    raise AssertionError('Outside write succeeded')
"#;
    run(
        &plan,
        code,
        &[root.path().to_str().unwrap(), secret.to_str().unwrap()],
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&secret).unwrap(), "private");
}

#[test]
fn read_only_contents_truncation_and_nested_write_grants_are_enforced() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("protected"), "keep").unwrap();
    let output = root.path().join("output");
    fs::create_dir(&output).unwrap();
    let mut opts = options();
    opts.project_read_only = true;
    opts.read_write.push(output.to_string_lossy().into());
    let Some(plan) = active(root.path(), &opts) else {
        return;
    };
    assert!(plan.label().contains("READ ONLY"));
    run(
        &plan,
        r#"
import errno, os, pathlib
def denied(action):
    try: action()
    except OSError as error:
        assert error.errno in (errno.EACCES, errno.EPERM, errno.EINVAL), error
        return
    raise AssertionError('Read-only operation succeeded')
assert pathlib.Path('protected').read_text() == 'keep'
denied(lambda: open('protected', 'w'))
denied(lambda: os.open('protected', os.O_RDONLY | os.O_TRUNC))
denied(lambda: os.truncate('protected', 0))
fd = os.open('protected', os.O_RDONLY)
try: denied(lambda: os.ftruncate(fd, 0))
finally: os.close(fd)
denied(lambda: os.unlink('protected'))
denied(lambda: os.rename('protected', 'output/moved'))
denied(lambda: pathlib.Path('new').write_text('bad'))
pathlib.Path('output/new').write_text('allowed')
os.rename('output/new', 'output/renamed')
assert pathlib.Path('output/renamed').read_text() == 'allowed'
"#,
        &[],
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("protected")).unwrap(),
        "keep"
    );
}

#[test]
fn restrictive_nested_and_hidden_alias_grants_never_become_active() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("data");
    fs::create_dir(&nested).unwrap();
    let mut opts = options();
    opts.read_only.push(nested.to_string_lossy().into());
    for plan in [
        sandbox::prepare(root.path().to_str().unwrap(), &opts).unwrap(),
        peer(root.path(), &opts, None),
    ] {
        assert!(!plan.active && plan.prefix.is_empty());
        assert!(
            plan.reason.contains("restrictive nested") || plan.reason.contains("ABI"),
            "{}",
            plan.reason
        );
    }
    // /bin is commonly a symlink into /usr: hiding the alias cannot subtract it.
    if Path::new("/bin")
        .canonicalize()
        .unwrap()
        .starts_with("/usr")
    {
        let mut opts = options();
        opts.overrides
            .insert("/bin".into(), sandbox::Access::Hidden);
        let plan = peer(root.path(), &opts, None);
        assert!(!plan.active && plan.prefix.is_empty());
    }
}

#[test]
fn network_toggle_covers_tcp_udp_unix_and_child_processes() {
    let root = tempfile::tempdir().unwrap();
    for network in [true, false] {
        let opts = sandbox::Options {
            share_network: network,
            ..options()
        };
        let Some(plan) = active(root.path(), &opts) else {
            return;
        };
        run(&plan, r#"
import socket, subprocess, sys
allowed = sys.argv[1] == 'true'
for family, kind in [(socket.AF_INET,socket.SOCK_STREAM), (socket.AF_INET,socket.SOCK_DGRAM), (socket.AF_UNIX,socket.SOCK_STREAM)]:
    try: client = socket.socket(family, kind)
    except PermissionError:
        assert not allowed
    else:
        client.close()
        assert allowed
child = subprocess.run([sys.executable, '-c', 'import socket; socket.socket()'], capture_output=True)
assert (child.returncode == 0) == allowed, child.stderr
if allowed:
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        listener.listen()
        with socket.create_connection(listener.getsockname(), timeout=2) as client:
            connection, _ = listener.accept()
            connection.close()
"#, &[if network {"true"} else {"false"}]).unwrap();
    }
}

#[test]
fn active_failure_does_not_retry_and_policy_identity_changes_with_backend() {
    let root = tempfile::tempdir().unwrap();
    let Some(mut plan) = active(root.path(), &options()) else {
        return;
    };
    let original = rode::sessions::fingerprint(&plan);
    let bwrap = sandbox::prepare(
        root.path().to_str().unwrap(),
        &sandbox::Options {
            enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_ne!(original, rode::sessions::fingerprint(&bwrap));
    plan.prefix[0] = "/missing-landlock-launcher".into();
    let file = root.path().join("must-not-exist");
    assert!(
        plan.local_request_with_helper(
            &files::Request::Write {
                path: file.to_string_lossy().into(),
                text: "unsafe".into(),
                expected: None,
            },
            Path::new(env!("CARGO_BIN_EXE_rode"))
        )
        .is_err()
    );
    assert!(!file.exists());
}

#[test]
fn preopened_file_descriptors_do_not_escape_into_the_command() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), "private").unwrap();
    let Some(plan) = active(root.path(), &options()) else {
        return;
    };
    // The unrestricted parent deliberately passes a capability the child could
    // otherwise read despite Landlock path checks.
    let driver = r#"
import json, os, resource, subprocess, sys
plan = json.load(sys.stdin)
fd = os.open(sys.argv[1], os.O_RDONLY)
os.dup2(fd, 512)
os.close(fd)
fd = 512
resource.setrlimit(resource.RLIMIT_NOFILE, (128, 128))
code = 'import os,sys\ntry: os.read(int(sys.argv[1]),64)\nexcept OSError: pass\nelse: raise AssertionError("Leaked descriptor")'
result = subprocess.run(plan['prefix'] + ['/usr/bin/python3', '-c', code, str(fd)], pass_fds=(fd,))
sys.exit(result.returncode)
"#;
    let mut command = Command::new("python3");
    command
        .args(["-c", driver])
        .arg(outside.path().join("secret"));
    run_bounded(
        command,
        serde_json::to_vec(&plan).unwrap(),
        Duration::from_secs(10),
    )
    .unwrap();
}

#[test]
fn a_granted_child_cannot_redirect_later_launches_to_private_files() {
    // Exercise both the granted directory itself and a writable ancestor.
    for child in ["cache", "cache/nested"] {
        let project = tempfile::tempdir().unwrap();
        let private = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join(child)).unwrap();
        fs::create_dir_all(private.path().join("nested")).unwrap();
        let secret = private.path().join("nested/secret");
        fs::write(&secret, "private fixture").unwrap();
        let mut options = options();
        options
            .read_write
            .push(project.path().join(child).to_string_lossy().into());
        let Some(plan) = active(project.path(), &options) else {
            return;
        };
        run(
            &plan,
            "import os,sys; os.rename('cache','original'); os.symlink(sys.argv[1],'cache')",
            &[private.path().to_str().unwrap()],
        )
        .unwrap();
        let result = run(
            &plan,
            "import sys; print(open(sys.argv[1]).read())",
            &[secret.to_str().unwrap()],
        );
        assert!(
            result.is_err(),
            "A changed {child} grant exposed a private file: {result:?}"
        );
    }
}

#[test]
fn scratch_never_overrides_a_read_only_grant() {
    let project = tempfile::tempdir().unwrap();
    let Some(plan) = active(project.path(), &options()) else {
        return;
    };
    let scratch =
        String::from_utf8(run(&plan, "import os; print(os.environ['TMPDIR'])", &[]).unwrap())
            .unwrap();
    let scratch = Path::new(scratch.trim());
    let protected = scratch.join("protected");
    fs::create_dir(&protected).unwrap();
    for path in [scratch, protected.as_path()] {
        let mut opts = options();
        opts.read_only.push(path.to_string_lossy().into());
        for plan in [
            sandbox::prepare(project.path().to_str().unwrap(), &opts).unwrap(),
            peer(project.path(), &opts, None),
        ] {
            assert!(!plan.active && plan.prefix.is_empty());
            assert!(
                plan.reason.contains("restrictive nested"),
                "{}",
                plan.reason
            );
        }
    }
    fs::remove_dir(protected).unwrap();
    fs::remove_dir(scratch).unwrap();
}

#[test]
fn replacing_scratch_after_validation_does_not_grant_its_symlink_target() {
    let project = tempfile::tempdir().unwrap();
    let private = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fs::write(private.path().join("secret"), "private fixture").unwrap();
    if active(project.path(), &options()).is_none() {
        return;
    }
    // Force the exact validation/open interleaving, without a scheduler-dependent
    // race or any mutation of the user's real cache directory.
    let driver = r#"
import json, os, sys
fixture = json.load(sys.stdin)
namespace = {'__name__': 'launcher_fixture'}
exec(fixture['launcher'], namespace)
original = namespace['private_scratch']
def replace_after_validation(home, root):
    captured = original(home, root)
    path = captured[0] if isinstance(captured, tuple) else captured
    os.rename(path, path + '-original')
    os.symlink(fixture['private'], path)
    return captured
namespace['private_scratch'] = replace_after_validation
sys.argv = fixture['arguments'] + ['/usr/bin/python3', '-I', '-c', '''
import os, pathlib, sys
for path in (sys.argv[1] + '/secret', os.environ['TMPDIR'] + '/secret'):
    try: pathlib.Path(path).read_text()
    except PermissionError: pass
    else: raise AssertionError('Scratch replacement exposed a private file')
''', fixture['private']]
namespace['main']()
"#;
    let binary = sandbox::find_program("python3").unwrap();
    let arguments = sandbox::arguments(&binary, project.path(), home.path(), &options()).unwrap();
    let mut command = Command::new(binary);
    command.args(["-I", "-S", "-c", driver]);
    run_bounded(
        command,
        serde_json::to_vec(&serde_json::json!({
            "launcher": include_str!("../src/landlock_launch.py"),
            "arguments": arguments,
            "private": private.path(),
        }))
        .unwrap(),
        Duration::from_secs(10),
    )
    .unwrap();
}

#[test]
fn a_project_python_alias_cannot_replace_the_sandbox_launcher() {
    let project = tempfile::tempdir().unwrap();
    let private = tempfile::tempdir().unwrap();
    let tools = project.path().join("bin");
    fs::create_dir(&tools).unwrap();
    let interpreter = tools.join("python3");
    let python = sandbox::find_program("python3")
        .unwrap()
        .canonicalize()
        .unwrap();
    std::os::unix::fs::symlink(&python, &interpreter).unwrap();
    let secret = private.path().join("secret");
    fs::write(&secret, "private fixture").unwrap();
    if active(project.path(), &options()).is_none() {
        return;
    }
    let plan = peer(project.path(), &options(), Some(&tools));
    assert!(plan.active, "{}", plan.reason);
    run(
        &plan,
        r#"
import os, pathlib, shlex, sys
path = pathlib.Path('bin/python3')
path.unlink()
path.write_text('#!/bin/sh\ncat ' + shlex.quote(sys.argv[1]) + '\n')
os.chmod(path, 0o755)
"#,
        &[secret.to_str().unwrap()],
    )
    .unwrap();
    let mut command = Command::new(&plan.prefix[0]);
    command
        .args(&plan.prefix[1..])
        .args([
            "/usr/bin/python3",
            "-c",
            "import sys; print(open(sys.argv[1]).read())",
        ])
        .arg(&secret);
    let result = run_bounded(command, vec![], Duration::from_secs(10));
    assert!(
        result.is_err(),
        "A replaced interpreter escaped: {result:?}"
    );
}

#[test]
fn writable_interpreters_and_standard_libraries_cannot_back_an_active_sandbox() {
    let project = tempfile::tempdir().unwrap();
    let Some(plan) = active(project.path(), &options()) else {
        return;
    };
    let mut interpreter = Command::new(&plan.prefix[0]);
    interpreter.args([
        "-I",
        "-S",
        "-c",
        "import json,sys; print(json.dumps(sys.path))",
    ]);
    let paths: Vec<String> =
        serde_json::from_slice(&run_bounded(interpreter, vec![], Duration::from_secs(10)).unwrap())
            .unwrap();
    let library = paths.iter().find(|path| Path::new(path).is_dir()).unwrap();
    let mut opts = options();
    opts.read_write.push(library.clone());
    for plan in [
        sandbox::prepare(project.path().to_str().unwrap(), &opts).unwrap(),
        peer(project.path(), &opts, None),
    ] {
        assert!(!plan.active && plan.prefix.is_empty());
        assert!(plan.reason.contains("runtime overlaps"), "{}", plan.reason);
    }

    let tools = project.path().join("bin");
    fs::create_dir(&tools).unwrap();
    fs::copy(&plan.prefix[0], tools.join("python3")).unwrap();
    let copied = peer(project.path(), &options(), Some(&tools));
    assert!(!copied.active && copied.prefix.is_empty());
    assert!(
        copied.reason.contains("runtime overlaps"),
        "{}",
        copied.reason
    );
}
