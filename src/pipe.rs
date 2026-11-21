//! Cancellation-aware nonblocking pipes. Protocol owners choose idle and fixed
//! deadlines: copies expire when stalled, while idle mounts remain connected.
use std::{
    io::{self, Read, Write},
    os::fd::AsRawFd,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// One owned descriptor with bounded polling and caller-selected timeout policy.
pub(crate) struct Pipe<T> {
    inner: T,
    cancel: Arc<AtomicBool>,
    touched: Instant,
    idle: Option<Duration>,
    deadline: Option<Instant>,
}
impl<T: AsRawFd> Pipe<T> {
    /// Inactivity is allowed unless the caller explicitly sets an idle timeout.
    pub fn new(inner: T, cancel: Arc<AtomicBool>) -> io::Result<Self> {
        let fd = inner.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            inner,
            cancel,
            touched: Instant::now(),
            idle: None,
            deadline: None,
        })
    }

    /// Successful reads/writes renew this timeout, independently of fixed deadlines.
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle = Some(timeout);
        self
    }

    /// A fixed operation budget is never renewed by successful I/O.
    pub fn deadline(&mut self, deadline: Instant) {
        self.deadline = Some(deadline);
    }

    /// Cleanup ignores the job's cancellation but retains one absolute budget.
    pub fn cleanup_until(&mut self, deadline: Instant) {
        self.cancel = Arc::default();
        self.touched = Instant::now();
        self.deadline(deadline);
    }

    fn ready(&self, events: libc::c_short) -> io::Result<()> {
        loop {
            if self.cancel.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "Channel cancelled",
                ));
            }
            if self.deadline.is_some_and(|end| Instant::now() >= end) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Channel deadline elapsed",
                ));
            }
            if self
                .idle
                .is_some_and(|timeout| self.touched.elapsed() > timeout)
            {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "Channel stalled"));
            }
            let mut fd = libc::pollfd {
                fd: self.inner.as_raw_fd(),
                events,
                revents: 0,
            };
            match unsafe { libc::poll(&mut fd, 1, 100) } {
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() != io::ErrorKind::Interrupted {
                        return Err(error);
                    }
                }
                0 => {}
                _ => return Ok(()),
            }
        }
    }
}

impl<T: Read + AsRawFd> Read for Pipe<T> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            self.ready(libc::POLLIN)?;
            match self.inner.read(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                result => {
                    if matches!(result, Ok(n) if n > 0) {
                        self.touched = Instant::now();
                    }
                    return result;
                }
            }
        }
    }
}
impl<T: Write + AsRawFd> Write for Pipe<T> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            self.ready(libc::POLLOUT)?;
            match self.inner.write(bytes) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                result => {
                    if matches!(result, Ok(n) if n > 0) {
                        self.touched = Instant::now();
                    }
                    return result;
                }
            }
        }
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
    fn idle_policy_is_explicit_and_cancellation_and_eof_remain_observable() {
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut pipe = Pipe::new(stream, cancel.clone()).unwrap();
        pipe.touched = Instant::now() - Duration::from_secs(120);
        peer.write_all(b"x").unwrap();
        pipe.read_exact(&mut [0]).unwrap();
        pipe = pipe.with_idle_timeout(Duration::from_secs(60));
        pipe.touched = Instant::now() - Duration::from_secs(120);
        assert_eq!(
            pipe.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        cancel.store(true, Ordering::Release);
        assert_eq!(
            pipe.write(b"x").unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
        pipe.cleanup_until(Instant::now() + Duration::from_secs(1));
        pipe.write_all(b"cleanup").unwrap();
        let mut bytes = [0; 7];
        peer.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"cleanup");
        drop(peer);
        assert_eq!(pipe.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn fixed_deadlines_are_not_renewed_by_successful_io() {
        let (stream, mut peer) = UnixStream::pair().unwrap();
        let mut pipe = Pipe::new(stream, Arc::default()).unwrap();
        let (start, ready) = std::sync::mpsc::sync_channel(0);
        let writer = std::thread::spawn(move || {
            peer.write_all(b"x").unwrap();
            ready.recv().unwrap();
            for _ in 0..100 {
                if peer.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        pipe.read_exact(&mut [0]).unwrap();
        pipe.cleanup_until(Instant::now() + Duration::from_millis(150));
        start.send(()).unwrap();
        let error = pipe.read_exact(&mut [0; 200]).unwrap_err();
        drop(pipe);
        writer.join().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn stalled_writes_observe_the_fixed_budget_and_waiting_reads_can_be_cancelled() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let size: libc::c_int = 4096;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    std::ptr::from_ref(&size).cast(),
                    std::mem::size_of_val(&size) as libc::socklen_t,
                )
            },
            0
        );
        let mut pipe = Pipe::new(stream, Arc::default()).unwrap();
        pipe.deadline(Instant::now() + Duration::from_millis(100));
        assert_eq!(
            pipe.write_all(&vec![0; 2 * 1024 * 1024])
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        let (stream, _peer) = UnixStream::pair().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut pipe = Pipe::new(stream, cancel.clone()).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            tx.send(pipe.read(&mut [0]).unwrap_err().kind()).unwrap();
        });
        cancel.store(true, Ordering::Release);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            io::ErrorKind::ConnectionAborted
        );
        worker.join().unwrap();
    }
}
