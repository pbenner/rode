//! Bounded stdio for transient SSH connections. A surviving sshd pipe is not
//! evidence that the desktop still exists; only incoming bytes renew its lease.
use std::{
    fs::File,
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd, RawFd},
    os::unix::fs::OpenOptionsExt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Last desktop input, shared with a supervisor that may have a blocked writer.
pub struct Lease {
    touched: Mutex<Instant>,
    timeout: Duration,
}
impl Lease {
    fn remaining(&self) -> Duration {
        self.timeout
            .saturating_sub(self.touched.lock().unwrap().elapsed())
    }
    /// True even when the input worker is blocked forwarding to a child.
    pub fn expired(&self) -> bool {
        self.remaining().is_zero()
    }
}

fn duplicate(fd: RawFd) -> io::Result<File> {
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if copy < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(copy) })
}

fn ready(fd: RawFd, events: libc::c_short, timeout: Option<Duration>) -> io::Result<()> {
    if timeout.is_some_and(|timeout| timeout.is_zero()) {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Desktop connection lease expired",
        ));
    }
    let mut poll = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    let millis = timeout.map_or(-1, |timeout| {
        timeout.as_millis().saturating_add(1).min(i32::MAX as u128) as i32
    });
    match unsafe { libc::poll(&mut poll, 1, millis) } {
        -1 => Err(io::Error::last_os_error()),
        0 => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Desktop connection lease expired",
        )),
        _ => Ok(()),
    }
}

/// Pipe reader whose deadline also covers partial headers and partial JSON lines.
pub struct Input {
    file: File,
    lease: Arc<Lease>,
    bounded: bool,
}
impl Input {
    fn new(file: File, timeout: Duration) -> Self {
        Self {
            file,
            lease: Arc::new(Lease {
                touched: Mutex::new(Instant::now()),
                timeout,
            }),
            bounded: true,
        }
    }
    /// Read raw stdin without the standard library's hidden input buffer.
    pub fn stdin() -> io::Result<Self> {
        Ok(Self::new(
            duplicate(0)?,
            rode_host_protocol::framing::CONNECTION_TIMEOUT,
        ))
    }
    /// Preserve legacy streams that did not negotiate periodic heartbeats.
    pub fn unbounded(&mut self) {
        self.bounded = false;
    }
    /// Observe expiry independently of the forwarding thread.
    pub fn lease(&self) -> Arc<Lease> {
        self.lease.clone()
    }
}
impl Read for Input {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            // A PTY may share stdin/stdout's open-file flags. Poll even for
            // legacy input after the output writer enables O_NONBLOCK.
            match ready(
                self.file.as_raw_fd(),
                libc::POLLIN,
                self.bounded.then(|| self.lease.remaining()),
            ) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            }
            match self.file.read(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) =>
                {
                    continue;
                }
                result => {
                    if matches!(result, Ok(n) if n > 0) {
                        *self.lease.touched.lock().unwrap() = Instant::now();
                    }
                    return result;
                }
            }
        }
    }
}

/// Nonblocking output prevents an abandoned SSH pipe from trapping a bridge
/// after its service socket has already disconnected.
pub struct Output {
    file: File,
    timeout: Duration,
}
impl Output {
    /// Bound stdout pipe/terminal writes, preserving regular-file redirection.
    pub fn stdout() -> io::Result<Self> {
        Self::open(1)
    }
    /// Bound diagnostics too: a dead desktop may leave stderr full as well.
    pub fn stderr() -> io::Result<Self> {
        Self::open(2)
    }
    /// Use a shorter deadline for best-effort final diagnostics.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    fn open(fd: RawFd) -> io::Result<Self> {
        let original = duplicate(fd)?;
        if original.metadata()?.is_file() {
            // O_NONBLOCK cannot bound regular-file I/O. Retain the shared
            // offset and O_APPEND so diagnostics cannot overwrite older logs.
            return Ok(Self {
                file: original,
                timeout: rode_host_protocol::framing::CONNECTION_TIMEOUT,
            });
        }
        // dup shares file status flags with the launching shell and its other
        // children. Reopen the pipe/PTY so O_NONBLOCK cannot break their input.
        Self::new(
            File::options()
                .write(true)
                .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
                .open(format!("/proc/self/fd/{}", original.as_raw_fd()))?,
        )
    }
    fn new(file: File) -> io::Result<Self> {
        let fd = file.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            file,
            timeout: rode_host_protocol::framing::CONNECTION_TIMEOUT,
        })
    }
    fn write_until(&mut self, bytes: &[u8], until: Instant) -> io::Result<usize> {
        loop {
            match self.file.write(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    ready(
                        self.file.as_raw_fd(),
                        libc::POLLOUT,
                        Some(until.saturating_duration_since(Instant::now())),
                    )?;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                result => return result,
            }
        }
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_until(bytes, Instant::now() + self.timeout)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn silent_open_peer_expires_even_mid_message() {
        let (read, mut write) = UnixStream::pair().unwrap();
        let mut input = Input::new(
            duplicate(read.as_raw_fd()).unwrap(),
            Duration::from_millis(50),
        );
        write.write_all(b"partial").unwrap();
        let mut bytes = [0; 7];
        input.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"partial");
        assert_eq!(
            input.read(&mut bytes).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert!(input.lease().expired());
        drop(write);
    }

    #[test]
    fn incoming_bytes_renew_the_deadline_and_eof_is_immediate() {
        let (read, mut write) = UnixStream::pair().unwrap();
        let mut input = Input::new(
            duplicate(read.as_raw_fd()).unwrap(),
            Duration::from_millis(100),
        );
        let sender = std::thread::spawn(move || {
            for _ in 0..4 {
                std::thread::sleep(Duration::from_millis(40));
                write.write_all(b"H").unwrap();
            }
        });
        let mut bytes = vec![];
        input.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"HHHH");
        assert!(!input.lease().expired());
        sender.join().unwrap();
    }

    #[test]
    fn full_output_pipe_has_a_deadline() {
        let (read, write) = UnixStream::pair().unwrap();
        let mut output = Output::new(duplicate(write.as_raw_fd()).unwrap()).unwrap();
        let bytes = [0; 8192];
        while output.file.write(&bytes).is_ok() {}
        assert_eq!(
            output
                .write_until(b"X", Instant::now() + Duration::from_millis(30))
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        drop(read);
    }

    #[test]
    fn legacy_input_waits_when_output_shares_its_nonblocking_descriptor() {
        let (read, mut write) = UnixStream::pair().unwrap();
        let mut input = Input::new(
            duplicate(read.as_raw_fd()).unwrap(),
            Duration::from_millis(10),
        );
        input.unbounded();
        let _output = Output::new(duplicate(read.as_raw_fd()).unwrap()).unwrap();
        let sender = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            write.write_all(b"next frame").unwrap();
        });
        let mut bytes = vec![];
        input.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"next frame");
        sender.join().unwrap();
    }

    #[test]
    fn redirected_output_preserves_append_mode_and_the_shared_file_position() {
        use std::io::{Seek, SeekFrom};
        for append in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("diagnostics.log");
            std::fs::write(&path, b"existing\n").unwrap();
            let mut redirected = File::options()
                .write(true)
                .append(append)
                .open(&path)
                .unwrap();
            redirected.seek(SeekFrom::End(0)).unwrap();
            let flags = unsafe { libc::fcntl(redirected.as_raw_fd(), libc::F_GETFL) };
            let mut output = Output::open(redirected.as_raw_fd()).unwrap();
            output.write_all(b"first\n").unwrap();
            redirected.write_all(b"second\n").unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"existing\nfirst\nsecond\n");
            assert_eq!(
                unsafe { libc::fcntl(redirected.as_raw_fd(), libc::F_GETFL) },
                flags
            );
        }
    }
}
