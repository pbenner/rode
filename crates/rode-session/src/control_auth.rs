//! Host-owned control capability. Landlock ABI 4/5 cannot scope abstract sockets.
//! The token stays in a sealed memory file, accessible only through a ptrace-checked
//! proc descriptor. Sharing the runtime directory reveals no usable secret.
use super::*;

const TOKEN_FILE: &str = "control.capability";
const TOKEN_BYTES: usize = 32;

pub(super) struct Token {
    bytes: [u8; TOKEN_BYTES],
    // Keep the anonymous inode alive for this service generation only.
    _file: File,
}

fn memory_file() -> std::io::Result<File> {
    memory_file_with(|flags| {
        let descriptor = unsafe { libc::memfd_create(c"rode-control".as_ptr(), flags) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // A successful memfd transfers ownership exactly once. No secret ever
        // has a filesystem name, even transiently in an overridden TMPDIR.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    })
}

fn memory_file_with(mut create: impl FnMut(u32) -> std::io::Result<File>) -> std::io::Result<File> {
    let flags = libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING;
    match create(flags | libc::MFD_NOEXEC_SEAL) {
        // The session service also runs on kernels predating this flag. Retry
        // only unsupported flags, never a permission/seccomp policy denial.
        Err(error) if error.raw_os_error() == Some(libc::EINVAL) => create(flags),
        result => result,
    }
}

impl Token {
    /// Rotate before publishing a listener; the service lock serializes writers.
    pub(super) fn publish(dir: &Path) -> Result<Self> {
        let mut bytes = [0; TOKEN_BYTES];
        File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let mut file = memory_file().context("Cannot create terminal control capability")?;
        // Older kernels cannot seal execution bits; still start without any
        // executable permissions. Modern kernels additionally seal that choice.
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&bytes)?;
        ensure!(
            unsafe {
                libc::fcntl(
                    file.as_raw_fd(),
                    libc::F_ADD_SEALS,
                    libc::F_SEAL_SEAL
                        | libc::F_SEAL_GROW
                        | libc::F_SEAL_SHRINK
                        | libc::F_SEAL_WRITE,
                )
            } == 0,
            "Cannot seal terminal control capability: {}",
            std::io::Error::last_os_error()
        );
        let mut reference = tempfile::NamedTempFile::new_in(dir)?;
        write!(
            reference,
            "/proc/{}/fd/{}",
            std::process::id(),
            file.as_raw_fd()
        )?;
        reference.persist(dir.join(TOKEN_FILE))?;
        Ok(Self { bytes, _file: file })
    }

    /// Check a fixed-size frame before reading any request or attaching a view.
    pub(super) fn verify(&self, stream: &mut UnixStream) -> Result<()> {
        let mut size = [0; 4];
        stream.read_exact(&mut size)?;
        ensure!(
            u32::from_be_bytes(size) == (TOKEN_BYTES + 1) as u32,
            "Terminal control authentication required outside the project sandbox"
        );
        let mut proof = [0; TOKEN_BYTES + 1];
        stream.read_exact(&mut proof)?;
        let different = proof[1..]
            .iter()
            .zip(self.bytes)
            .fold(0, |different, (a, b)| different | (a ^ b));
        ensure!(
            proof[0] == b'A' && different == 0,
            "Terminal control authentication failed outside the project sandbox"
        );
        Ok(())
    }
}

/// Landlock's ptrace boundary (present since ABI 1) prevents sandboxed clients
/// from opening the service's proc descriptor, even with a runtime-path grant.
/// Never put the token in arguments, environment or a named filesystem inode.
pub(super) fn authenticate(stream: &mut UnixStream, dir: &Path) -> Result<()> {
    let reference = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join(TOKEN_FILE))?;
    let meta = reference.metadata()?;
    ensure!(
        meta.is_file()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.mode() & 0o077 == 0
            && meta.len() <= 128,
        "Unsafe terminal control token"
    );
    let mut path = String::new();
    reference.take(129).read_to_string(&mut path)?;
    let fields = path.split('/').collect::<Vec<_>>();
    ensure!(
        fields.len() == 5 && fields[0].is_empty() && fields[1] == "proc" && fields[3] == "fd",
        "Invalid terminal control capability"
    );
    let pid = fields[2].parse::<libc::pid_t>()?;
    let descriptor = fields[4].parse::<libc::c_int>()?;
    let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    ensure!(
        unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut peer as *mut libc::ucred).cast(),
                &mut size,
            )
        } == 0
            && peer.pid == pid
            && peer.uid == unsafe { libc::geteuid() }
            && descriptor > 2,
        "Terminal control capability does not belong to the service"
    );
    // A writable runtime directory must not let a substituted FIFO or device
    // block a host client before we can validate the opened inode.
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    let meta = file.metadata()?;
    ensure!(
        meta.is_file()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.len() == TOKEN_BYTES as u64,
        "Invalid terminal control token length"
    );
    let mut bytes = [0; TOKEN_BYTES];
    file.read_exact(&mut bytes)?;
    frame(stream, b'A', &bytes)?;
    ensure!(
        matches!(recv_json::<_, Response>(stream)?.checked()?, Response::Ok),
        "Unexpected terminal control authentication reply"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_reference_discloses_no_secret_and_the_memory_file_is_sealed() {
        let dir = tempfile::tempdir().unwrap();
        let token = Token::publish(dir.path()).unwrap();
        let path = fs::read_to_string(dir.path().join(TOKEN_FILE)).unwrap();
        assert!(path.starts_with(&format!("/proc/{}/fd/", std::process::id())));
        let mut bytes = [0; TOKEN_BYTES];
        File::open(&path).unwrap().read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, token.bytes);
        assert_eq!(token._file.metadata().unwrap().mode() & 0o777, 0o600);
        assert!(
            OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .write_all(&[0; TOKEN_BYTES])
                .is_err()
        );
    }

    #[test]
    fn older_kernels_retry_unsupported_noexec_flags_without_relaxing_policy_denials() {
        let mut calls = Vec::new();
        let _file = memory_file_with(|flags| {
            calls.push(flags);
            if flags & libc::MFD_NOEXEC_SEAL != 0 {
                Err(std::io::Error::from_raw_os_error(libc::EINVAL))
            } else {
                tempfile::tempfile()
            }
        })
        .unwrap();
        assert_eq!(
            calls,
            [
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING | libc::MFD_NOEXEC_SEAL,
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING
            ]
        );
        for errno in [libc::EACCES, libc::EPERM, libc::ENOSYS] {
            let mut calls = 0;
            let error = memory_file_with(|_| {
                calls += 1;
                Err(std::io::Error::from_raw_os_error(errno))
            })
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(errno));
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn invalid_authentication_lengths_are_rejected_without_reading_a_payload() {
        let dir = tempfile::tempdir().unwrap();
        let token = Token::publish(dir.path()).unwrap();
        for size in [0u32, 1, 32, 34, u32::MAX] {
            let (mut sender, mut receiver) = UnixStream::pair().unwrap();
            receiver
                .set_read_timeout(Some(Duration::from_millis(100)))
                .unwrap();
            sender.write_all(&size.to_be_bytes()).unwrap();
            assert!(
                token
                    .verify(&mut receiver)
                    .unwrap_err()
                    .to_string()
                    .contains("authentication required")
            );
        }
    }

    #[test]
    fn reference_must_be_private_and_point_to_the_connected_service() {
        let dir = tempfile::tempdir().unwrap();
        let _token = Token::publish(dir.path()).unwrap();
        let reference = dir.path().join(TOKEN_FILE);
        let (mut client, _server) = UnixStream::pair().unwrap();
        fs::write(&reference, "/proc/0/fd/3").unwrap();
        assert!(
            authenticate(&mut client, dir.path())
                .unwrap_err()
                .to_string()
                .contains("does not belong")
        );
        fs::set_permissions(&reference, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            authenticate(&mut client, dir.path())
                .unwrap_err()
                .to_string()
                .contains("Unsafe")
        );
        fs::remove_file(&reference).unwrap();
        std::os::unix::fs::symlink("/proc/self/fd/3", &reference).unwrap();
        assert!(authenticate(&mut client, dir.path()).is_err());
    }

    #[test]
    fn substituted_fifos_cannot_block_reference_or_token_reads() {
        for replace_reference in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let _token = Token::publish(dir.path()).unwrap();
            let reference = dir.path().join(TOKEN_FILE);
            let fifo = if replace_reference {
                reference.clone()
            } else {
                dir.path().join("fifo")
            };
            if replace_reference {
                fs::remove_file(&reference).unwrap();
            }
            let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
            // O_PATH holds the FIFO without acting as a reader or writer.
            let descriptor = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH)
                .open(&fifo)
                .unwrap();
            if !replace_reference {
                fs::write(
                    &reference,
                    format!("/proc/{}/fd/{}", std::process::id(), descriptor.as_raw_fd()),
                )
                .unwrap();
            }
            let (mut client, _server) = UnixStream::pair().unwrap();
            let (sender, receiver) = mpsc::channel();
            let path = dir.path().to_owned();
            let reader = std::thread::spawn(move || {
                let _ = sender.send(authenticate(&mut client, &path));
            });
            let result = receiver.recv_timeout(Duration::from_secs(5));
            if result.is_err() {
                // Wake a regressed blocking open before failing, so the test
                // neither hangs nor leaves an owned worker behind.
                let _wake = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&fifo)
                    .unwrap();
                reader.join().unwrap();
                panic!("Control authentication blocked on a FIFO (reference={replace_reference})");
            }
            reader.join().unwrap();
            let diagnostic = result.unwrap().unwrap_err().to_string();
            assert!(
                diagnostic.contains(if replace_reference {
                    "Unsafe terminal control token"
                } else {
                    "Invalid terminal control token length"
                }),
                "{diagnostic}"
            );
        }
    }
}
