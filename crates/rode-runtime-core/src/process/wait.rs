//! Observe an owned child's exit without reaping its process ID.
//!
//! Capture guards and asynchronous transports have different signal/state
//! ownership. Both need the same kernel observation: retain the leader's PID
//! until final group signalling ends, then reap through `Child::wait`. These
//! functions send no signals, cache no status and never select another child.
//! Callers must remain the sole reaper and disable their signals if ownership
//! is lost (`ECHILD`); a cached `Child` status cannot establish kernel ownership.
use std::{io, process::Child};

/// Check whether this child exited, leaving its PID and wait status reserved.
/// Returns `false` immediately for a running child; propagates ownership/I/O
/// errors after retrying interruptions. It never calls [`Child::try_wait`].
pub fn exited(child: &Child) -> io::Result<bool> {
    observe(child.id(), libc::WNOHANG)
}

/// Wait until this child exits without reaping it or releasing its PID.
/// Input pipes remain owned by the caller; arrange EOF separately when required.
/// Retrying interruptions preserves blocking-wait behavior. The owner must
/// finish its signals before a subsequent [`Child::wait`] releases the ID.
pub fn wait_for_exit(child: &Child) -> io::Result<()> {
    wait_for_exit_pid(child.id())
}

/// Observe a child owned through another process API, such as a native PTY.
/// The caller must remain its sole reaper, retain the child handle and disable
/// further signals before reaping. A numeric PID alone does not confer ownership.
pub fn wait_for_exit_pid(pid: u32) -> io::Result<()> {
    observe(pid, 0).map(|_| ())
}

fn observe(pid: u32, options: libc::c_int) -> io::Result<bool> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid child PID",
        ));
    }
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: P_PID selects only this positive child ID; info is writable.
        // WNOWAIT retains ownership even when polling observes an exited child.
        if unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &mut info,
                libc::WEXITED | libc::WNOWAIT | options,
            )
        } == 0
        {
            return Ok(unsafe { info.si_pid() } != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn repeated_observation_retains_the_status_until_the_owner_reaps() {
        struct Owned(Child);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut child = Owned(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        assert!(!exited(&child.0).unwrap());
        child.0.kill().unwrap();
        wait_for_exit_pid(child.0.id()).unwrap();
        wait_for_exit(&child.0).unwrap();
        assert!(exited(&child.0).unwrap());
        wait_for_exit(&child.0).unwrap();
        assert!(!child.0.wait().unwrap().success());
        // Child caches its status; the shared kernel observer must not use it.
        assert_eq!(
            exited(&child.0).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert_eq!(
            wait_for_exit(&child.0).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}
