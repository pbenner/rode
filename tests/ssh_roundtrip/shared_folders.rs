//! Real binary SFTP over each existing master, without an SFTP subsystem or FUSE.
use super::*;
use rode_host_core::shared_folders::{Export, write_header};
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    process::Stdio,
};

fn number(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend(value.to_be_bytes());
}
fn string(bytes: &mut Vec<u8>, value: &[u8]) {
    number(bytes, value.len() as u32);
    bytes.extend(value);
}
fn exchange(
    input: &mut impl Write,
    output: &mut std::process::ChildStdout,
    body: Vec<u8>,
) -> Vec<u8> {
    if !body.is_empty() {
        input.write_all(&(body.len() as u32).to_be_bytes()).unwrap();
        input.write_all(&body).unwrap();
        input.flush().unwrap();
    }
    fn read(output: &mut std::process::ChildStdout, mut bytes: &mut [u8]) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !bytes.is_empty() {
            assert!(Instant::now() < deadline, "SFTP response timed out");
            let mut fd = libc::pollfd {
                fd: output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut fd, 1, 100) } <= 0 {
                continue;
            }
            let n = output.read(bytes).unwrap();
            assert!(n > 0, "SFTP stream ended early");
            bytes = &mut bytes[n..];
        }
    }
    let mut length = [0; 4];
    read(output, &mut length);
    let length = u32::from_be_bytes(length) as usize;
    assert!(length <= 1024 * 1024);
    let mut result = vec![0; length];
    read(output, &mut result);
    result
}
fn request(kind: u8, id: u32, path: &str) -> Vec<u8> {
    let mut body = vec![kind];
    number(&mut body, id);
    string(&mut body, path.as_bytes());
    body
}

#[test]
#[ignore = "requires the isolated sshd started by scripts/test-ssh.sh"]
fn shared_folder_exports_are_scoped_and_read_only_on_both_ssh_backends() {
    for backend in [rode::ssh::Backend::Russh, rode::ssh::Backend::OpenSsh] {
        let remote = Remote::new(Profile {
            host: "rode-test".into(),
            config: std::env::var("RODE_TEST_SSH_CONFIG").unwrap(),
            backend,
            ..Default::default()
        })
        .unwrap();
        let login = Terminal::remote(&eframe::egui::Context::default(), &remote).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !remote.connected() {
            assert!(
                login.is_alive() && Instant::now() < deadline,
                "{}",
                login.contents()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("chosen ' 日本");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(temp.path().join("outside"), b"private").unwrap();
        std::os::unix::fs::symlink(temp.path(), root.join("escape")).unwrap();
        for read_only in [true, false] {
            std::fs::write(root.join("file"), b"original").unwrap();
            let mut child = remote
                .command()
                .arg(format!(
                    "exec {} --shared-folder-export",
                    rode::ssh::quote(env!("CARGO_BIN_EXE_rode-sessiond"))
                ))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let mut input = child.stdin.take().unwrap();
            let mut output = child.stdout.take().unwrap();
            // One write deliberately pipelines INIT behind the header. A
            // launcher must leave those bytes on the descriptor inherited by
            // its captured-policy worker.
            let mut start = vec![];
            write_header(
                &mut start,
                &Export {
                    root: root.to_str().unwrap().into(),
                    read_only,
                    plan: Some(rode::sandbox::Plan {
                        root: root.to_str().unwrap().into(),
                        ..Default::default()
                    }),
                },
            )
            .unwrap();
            start.extend([0, 0, 0, 5, 1, 0, 0, 0, 3]);
            input.write_all(&start).unwrap();
            input.flush().unwrap();
            assert_eq!(exchange(&mut input, &mut output, vec![])[0], 2);
            for escape in ["../outside", "escape/outside"] {
                let mut open = request(3, 1, escape);
                number(&mut open, 1);
                number(&mut open, 0);
                let result = exchange(&mut input, &mut output, open);
                assert_eq!(result[0], 101);
                assert_ne!(&result[5..9], &0u32.to_be_bytes());
            }
            let mut open = request(3, 2, "file");
            number(&mut open, if read_only { 1 } else { 3 });
            number(&mut open, 0);
            let handle = exchange(&mut input, &mut output, open);
            assert_eq!(handle[0], 102);
            let length = u32::from_be_bytes(handle[5..9].try_into().unwrap()) as usize;
            let h = &handle[9..9 + length];
            let mut read = vec![5];
            number(&mut read, 3);
            string(&mut read, h);
            read.extend(0u64.to_be_bytes());
            number(&mut read, 64);
            assert_eq!(&exchange(&mut input, &mut output, read)[9..], b"original");
            let mut write = vec![6];
            number(&mut write, 4);
            string(&mut write, h);
            write.extend(0u64.to_be_bytes());
            string(&mut write, b"changed!");
            let result = exchange(&mut input, &mut output, write);
            assert_eq!(
                u32::from_be_bytes(result[5..9].try_into().unwrap()) == 0,
                !read_only
            );
            assert_eq!(
                std::fs::read(root.join("file")).unwrap(),
                if read_only { b"original" } else { b"changed!" }
            );
            drop(input);
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(status) = child.try_wait().unwrap() {
                    assert!(status.success());
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "Share export did not stop after EOF"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        drop(login);
        remote.cleanup();
    }
}
