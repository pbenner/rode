//! Native display lifetime, independent of repainting, focus and the GUI thread.
//!
//! A disconnected display is a lost desktop, even if its event loop keeps running.
//! Ending that frontend closes its input writers: host-owned work then follows its
//! existing persistence policy. The monitor never reads from the toolkit's socket
//! and never treats a quiet or minimized window as a failed connection.

use anyhow::{Context, Result};
use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    os::unix::net::UnixStream,
};

/// Cancels display monitoring when orderly desktop shutdown drops the guard.
pub struct Guard(UnixStream);

impl Guard {
    /// Watch the connection actually used by eframe, including non-default X
    /// displays and Wayland. The callback runs once on an independent thread.
    /// Unsupported display backends return `None` instead of guessing a socket.
    pub fn watch(
        cc: &eframe::CreationContext<'_>,
        disconnected: impl FnOnce() + Send + 'static,
    ) -> Result<Option<Self>> {
        let display = cc
            .display_handle()
            .context("Cannot inspect native display")?;
        // Resolve the socket while the toolkit's display object is alive on the
        // GUI thread. Only an owned descriptor crosses into the monitor thread.
        let fd = unsafe {
            match display.as_raw() {
                RawDisplayHandle::Xlib(handle) => {
                    let Some(display) = handle.display else {
                        return Ok(None);
                    };
                    connection_fd("libX11.so.6", b"XConnectionNumber\0", display.as_ptr())?
                }
                RawDisplayHandle::Wayland(handle) => connection_fd(
                    "libwayland-client.so.0",
                    b"wl_display_get_fd\0",
                    handle.display.as_ptr(),
                )?,
                _ => return Ok(None),
            }
        };
        Self::start(fd, disconnected).map(Some)
    }

    fn start(fd: OwnedFd, disconnected: impl FnOnce() + Send + 'static) -> Result<Self> {
        let (cancel, wake) = UnixStream::pair()?;
        std::thread::Builder::new()
            .name("display-lifetime".into())
            .spawn(move || {
                let mut fds = [
                    libc::pollfd {
                        fd: fd.as_raw_fd(),
                        events: libc::POLLRDHUP,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: wake.as_raw_fd(),
                        events: 0,
                        revents: 0,
                    },
                ];
                loop {
                    // No POLLIN: normal X/Wayland events belong to winit. A quiet
                    // display can remain connected indefinitely without a timeout.
                    let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
                    if result < 0 {
                        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                        {
                            continue;
                        }
                        eprintln!(
                            "Display monitor failed: {}",
                            std::io::Error::last_os_error()
                        );
                        return;
                    }
                    if fds[1].revents != 0 {
                        return;
                    }
                    if fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLRDHUP) != 0 {
                        disconnected();
                        return;
                    }
                }
            })?;
        Ok(Self(cancel))
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

/// Duplicate, without consuming, a descriptor owned by the native window system.
/// The pointer and function name must match the live toolkit display backend.
unsafe fn connection_fd(
    library: &str,
    symbol: &[u8],
    display: *mut std::ffi::c_void,
) -> Result<OwnedFd> {
    let library = unsafe { libloading::Library::new(library) }?;
    let get_fd = unsafe {
        library.get::<unsafe extern "C" fn(*mut std::ffi::c_void) -> libc::c_int>(symbol)
    }?;
    let fd = unsafe { libc::fcntl(get_fd(display), libc::F_DUPFD_CLOEXEC, 0) };
    anyhow::ensure!(
        fd >= 0,
        "Cannot duplicate display connection: {}",
        std::io::Error::last_os_error()
    );
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Informational identity for live-agent ownership. It never grants authority;
/// the authenticated host socket and original policy remain the security boundary.
pub fn owner() -> rode_agent_core::service::ViewOwner {
    let mut hostname = [0u8; 256];
    let host = if unsafe { libc::gethostname(hostname.as_mut_ptr().cast(), hostname.len()) } == 0 {
        String::from_utf8_lossy(
            &hostname[..hostname
                .iter()
                .position(|b| *b == 0)
                .unwrap_or(hostname.len())],
        )
        .into_owned()
    } else {
        String::new()
    };
    let display = std::env::var("WAYLAND_DISPLAY")
        .or_else(|_| std::env::var("DISPLAY"))
        .unwrap_or_default();
    let clean = |s: String| s.chars().filter(|c| !c.is_control()).take(128).collect();
    rode_agent_core::service::ViewOwner {
        host: clean(host),
        display: clean(display),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::mpsc, time::Duration};

    #[test]
    fn quiet_connections_and_unread_events_are_healthy_but_peer_loss_is_final() {
        let (display, mut server) = UnixStream::pair().unwrap();
        let (tx, rx) = mpsc::channel();
        let _guard = Guard::start(display.into(), move || tx.send(()).unwrap()).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        server.write_all(b"unread toolkit events").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        drop(server);
        rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(rx.recv().is_err(), "loss callback must run exactly once");
    }

    #[test]
    fn orderly_shutdown_cancels_the_monitor() {
        let (display, _server) = UnixStream::pair().unwrap();
        let (tx, rx) = mpsc::channel();
        let guard = Guard::start(display.into(), move || tx.send(()).unwrap()).unwrap();
        drop(guard);
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_err());
    }
}
