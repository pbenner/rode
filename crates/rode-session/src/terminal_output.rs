//! Drain a PTY before publishing shell exit, without waiting forever for descendants.
//!
//! A drain request changes the reader's poll timeout, not a supervisor deadline:
//! the supervisor cannot cut off a descheduled reader after its first 200 ms.
//! Explicit stop remains immediate. Byte and time budgets bound descendants
//! that keep producing output after their parent exits.
use std::{
    fs::File,
    io::Read,
    os::fd::AsRawFd,
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

const DRAIN_LIMIT: usize = 4 * 1024 * 1024;
const DRAIN_TIME: Duration = Duration::from_secs(5);

/// Read a nonblocking PTY until EOF, explicit stop, or the bounded exit drain.
pub(super) fn read(mut reader: File, mut control: UnixStream, mut emit: impl FnMut(&[u8])) {
    let mut data = [0; 8192];
    let mut draining = None::<Instant>;
    let mut remaining = DRAIN_LIMIT;
    loop {
        let mut fds = [
            libc::pollfd {
                fd: reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: control.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let polled = unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                2,
                if draining.is_some() { 200 } else { -1 },
            )
        };
        if polled < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if polled == 0 {
            break;
        }
        if fds[1].revents != 0 {
            let mut command = [0];
            if !matches!(control.read(&mut command), Ok(1)) || command[0] != b'D' {
                break;
            }
            draining.get_or_insert_with(Instant::now);
        }
        if fds[0].revents == 0 {
            continue;
        }
        let maximum = if draining.is_some() {
            data.len().min(remaining)
        } else {
            data.len()
        };
        let count = match reader.read(&mut data[..maximum]) {
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        emit(&data[..count]);
        if let Some(started) = draining {
            remaining -= count;
            if remaining == 0 || started.elapsed() >= DRAIN_TIME {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        os::fd::{FromRawFd, IntoRawFd},
        sync::mpsc,
        time::Duration,
    };

    #[test]
    fn drain_keeps_queued_tail_despite_a_slow_reader_and_an_open_descendant() {
        let (input, mut output) = UnixStream::pair().unwrap();
        input.set_nonblocking(true).unwrap();
        let input = unsafe { File::from_raw_fd(input.into_raw_fd()) };
        let (control, mut signal) = UnixStream::pair().unwrap();
        let payload = vec![b'x'; 32 * 1024];
        output.write_all(&payload).unwrap();
        signal.write_all(b"D").unwrap();
        let (done, observed) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = vec![];
            read(input, control, |chunk| {
                std::thread::sleep(Duration::from_millis(60));
                bytes.extend_from_slice(chunk);
            });
            done.send(bytes).unwrap();
        });
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(5)).unwrap(),
            payload
        );
        drop(output);
    }

    #[test]
    fn drain_is_bounded_when_a_descendant_writes_continuously() {
        let (input, mut output) = UnixStream::pair().unwrap();
        input.set_nonblocking(true).unwrap();
        let input = unsafe { File::from_raw_fd(input.into_raw_fd()) };
        let (control, mut signal) = UnixStream::pair().unwrap();
        signal.write_all(b"D").unwrap();
        let writer = std::thread::spawn(move || output.write_all(&vec![b'x'; DRAIN_LIMIT * 2]));
        let (done, observed) = mpsc::channel();
        std::thread::spawn(move || {
            let mut count = 0;
            read(input, control, |chunk| count += chunk.len());
            done.send(count).unwrap();
        });
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(5)).unwrap(),
            DRAIN_LIMIT
        );
        assert!(writer.join().unwrap().is_err());
    }
}
