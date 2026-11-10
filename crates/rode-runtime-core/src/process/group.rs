//! Shared signal/stop state for a single, separately created process group.
//! The transport worker retains the actual child; this owner orders its final
//! signals before reaping so a released PID cannot address an unrelated group.
use std::{
    io,
    process::{Child, ExitStatus},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const STOP_GRACE: Duration = Duration::from_millis(1500);

#[derive(Default)]
struct State {
    pid: Option<u32>,
    escalating: bool,
}

/// Single-use process-group lifetime shared by desktop, agent and runtime transports.
///
/// The spawning worker must create a separate process group/session before publishing
/// its leader with [`Self::started`]. Use [`Self::wait`] to wait for that leader,
/// then [`Self::finish`] once transport cleanup has completed.
/// Do not reap the child through another waiter or a SIGCHLD handler.
/// None of these operations owns a terminal, SSH authentication master or host policy.
#[derive(Default)]
pub struct ProcessGroup {
    // Publication, invalidation and timer signals share one short lock. A stop
    // racing spawn cannot miss both the cancellation flag and the live PID.
    state: Mutex<State>,
    stopped: AtomicBool,
    finished: AtomicBool,
}

impl ProcessGroup {
    /// Borrow the cancellation flag for bounded executable preparation and spawn retries.
    /// Callers must only read it; cancellation requests go through [`Self::stop`].
    pub fn cancelled(&self) -> &AtomicBool {
        &self.stopped
    }

    /// Publish the leader of this transport's freshly created process group.
    /// A stop requested during preparation is honoured, including timed escalation.
    pub fn started(self: &Arc<Self>, pid: u32) {
        assert!(
            pid > 0 && pid <= i32::MAX as u32,
            "Invalid child process ID"
        );
        let mut state = self.state.lock().unwrap();
        state.pid = Some(pid);
        if self.stopped.load(Ordering::Acquire) {
            self.terminate(&mut state);
        }
    }

    /// Signal only the published, still-owned group. Before spawn and after reaping,
    /// this is a no-op; PID zero must never address the caller's process group.
    fn signal(&self, signal: i32) {
        let state = self.state.lock().unwrap();
        if let Some(pid) = state.pid {
            // SAFETY: the spawning worker published a positive group leader;
            // a negative PID addresses only that separately created group.
            unsafe {
                libc::kill(-(pid as i32), signal);
            }
        }
    }

    /// Request idempotent SIGTERM and a bounded SIGKILL escalation.
    /// The grace period starts only when a child actually exists, so slow startup
    /// cannot exhaust escalation before the process has been published.
    pub fn stop(self: &Arc<Self>) {
        self.stopped.store(true, Ordering::Release);
        self.terminate(&mut self.state.lock().unwrap());
    }

    fn terminate(self: &Arc<Self>, state: &mut State) {
        if let Some(pid) = state.pid
            && !state.escalating
        {
            // SAFETY: the worker published a positive, independently owned group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            state.escalating = true;
            let group = self.clone();
            std::thread::spawn(move || {
                std::thread::sleep(STOP_GRACE);
                group.signal(libc::SIGKILL);
            });
        }
    }

    /// Wait for the leader and clean up its descendants before joining pipe readers.
    /// Closes any remaining stdin handle, as [`Child::wait`] does. Exit observation
    /// retains the leader's PID until all group signalling has been invalidated.
    /// The child must match the leader published through [`Self::started`].
    /// The worker must retain exclusive ownership of this child's wait status;
    /// lost ownership returns an error and disables further signals.
    pub fn wait(&self, child: &mut Child) -> io::Result<ExitStatus> {
        drop(child.stdin.take());
        self.finish_signalling(child)?;
        child.wait()
    }

    /// Check for leader exit without reaping or releasing its PID.
    /// Use this instead of [`Child::try_wait`] while supervising deadlines or
    /// input leases, and finish with [`Self::wait`] before collecting pipe readers.
    /// Kernel observation errors disable signalling and propagate to the worker;
    /// an unpublished child is rejected without changing the published owner.
    pub fn exited(&self, child: &Child) -> io::Result<bool> {
        self.observe_exit(child, false)
    }

    /// Observe exit without reaping, then finish signals while the PID is reserved.
    fn finish_signalling(&self, child: &Child) -> io::Result<()> {
        self.observe_exit(child, true)?;
        let mut state = self.state.lock().unwrap();
        if let Some(pid) = state.pid.take() {
            // SAFETY: the leader remains unreaped. Invalidation shares the
            // timer/stop lock and precedes the wait that releases this ID.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        Ok(())
    }

    fn observe_exit(&self, child: &Child, blocking: bool) -> io::Result<bool> {
        if self.state.lock().unwrap().pid != Some(child.id()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Child is not the published process-group leader",
            ));
        }
        let result = if blocking {
            super::wait::wait_for_exit(child).map(|()| true)
        } else {
            super::wait::exited(child)
        };
        if result.is_err() {
            // The kernel could not establish ownership. Do not trust a cached
            // Child status or allow timer/Stop signals to use a released PID.
            self.state.lock().unwrap().pid = None;
        }
        result
    }

    /// Publish completed supervision, including startup failures with no child.
    /// A started child must already have been reaped by its owning worker.
    pub fn finish(&self) {
        self.state.lock().unwrap().pid = None;
        self.finished.store(true, Ordering::Release);
    }

    /// Whether supervision has completed, including an early startup failure.
    /// A stop request alone does not establish completion.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Read,
        os::unix::process::CommandExt,
        process::{Command, Stdio},
        time::Instant,
    };

    struct Owned(Child);
    impl Owned {
        fn new(ignore_term: bool) -> Self {
            let script = if ignore_term {
                "trap '' TERM; printf x; exec /bin/sleep 30"
            } else {
                "printf x; exec /bin/sleep 30"
            };
            let mut child = Self(
                Command::new("/bin/sh")
                    .args(["-c", script])
                    .process_group(0)
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
            child.0.stdout.take().unwrap().read_exact(&mut [0]).unwrap();
            child
        }
        fn wait(&mut self, group: &ProcessGroup) -> ExitStatus {
            let until = Instant::now() + STOP_GRACE + Duration::from_secs(3);
            loop {
                if group.exited(&self.0).unwrap() {
                    return group.wait(&mut self.0).unwrap();
                }
                assert!(Instant::now() < until, "Owned process did not stop");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn a_stop_during_slow_startup_still_escalates_after_the_child_starts() {
        let group = Arc::new(ProcessGroup::default());
        group.stop();
        assert!(!group.is_finished());
        std::thread::sleep(STOP_GRACE + Duration::from_millis(100));
        let mut owned = Owned::new(true);
        group.started(owned.0.id());
        assert!(!owned.wait(&group).success());
        group.finish();
        assert!(group.is_finished());
    }

    #[test]
    fn repeated_stop_and_completed_cleanup_leave_an_independent_group_alive() {
        let group = Arc::new(ProcessGroup::default());
        let mut independent = Owned::new(false);
        let mut owned = Owned::new(false);
        group.started(owned.0.id());
        group.stop();
        group.stop();
        assert!(group.cancelled().load(Ordering::Acquire));
        assert!(!owned.wait(&group).success());
        group.finish();
        group.stop();
        assert!(group.is_finished());
        assert!(independent.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn concurrent_spawn_publication_and_stop_cannot_lose_cancellation() {
        for _ in 0..16 {
            let group = Arc::new(ProcessGroup::default());
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let stopping = group.clone();
            let ready = barrier.clone();
            let mut owned = Owned::new(false);
            let worker = std::thread::spawn(move || {
                ready.wait();
                stopping.stop();
            });
            barrier.wait();
            group.started(owned.0.id());
            worker.join().unwrap();
            assert!(!owned.wait(&group).success());
            group.finish();
        }
    }

    #[test]
    fn a_previously_reaped_leader_invalidates_signalling_and_reports_lost_ownership() {
        let group = Arc::new(ProcessGroup::default());
        let mut owned = Owned::new(false);
        group.started(owned.0.id());
        assert!(!group.exited(&owned.0).unwrap());
        owned.0.kill().unwrap();
        owned.0.wait().unwrap();
        // Child::wait caches status. It must not stand in for kernel ownership
        // before a group signal: the numeric ID has already been released.
        assert_eq!(
            group.wait(&mut owned.0).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert!(group.state.lock().unwrap().pid.is_none());
    }

    #[test]
    fn final_signals_end_while_the_leader_is_still_waitable() {
        let group = Arc::new(ProcessGroup::default());
        let mut owned = Owned::new(false);
        group.started(owned.0.id());
        owned.0.kill().unwrap();
        group.finish_signalling(&owned.0).unwrap();
        // Ask the kernel, not Child's status cache, whether the PID is retained.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    owned.0.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            },
            0,
            "{}",
            io::Error::last_os_error()
        );
        assert_eq!(unsafe { info.si_pid() }, owned.0.id() as i32);
        assert!(group.state.lock().unwrap().pid.is_none());
        group.stop();
        owned.0.wait().unwrap();
    }

    #[test]
    fn nonblocking_observation_also_refuses_a_reaped_leader() {
        let group = Arc::new(ProcessGroup::default());
        let mut owned = Owned::new(false);
        group.started(owned.0.id());
        owned.0.kill().unwrap();
        owned.0.wait().unwrap();
        assert_eq!(
            group.exited(&owned.0).unwrap_err().raw_os_error(),
            Some(libc::ECHILD)
        );
        assert!(group.state.lock().unwrap().pid.is_none());
    }

    #[test]
    fn an_unpublished_child_cannot_replace_the_owned_group() {
        let group = Arc::new(ProcessGroup::default());
        let mut owned = Owned::new(false);
        let mut independent = Owned::new(false);
        group.started(owned.0.id());
        assert_eq!(
            group.wait(&mut independent.0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(group.state.lock().unwrap().pid, Some(owned.0.id()));
        group.stop();
        assert!(!owned.wait(&group).success());
        assert!(independent.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn natural_exit_preserves_status_and_closes_descendant_output() {
        let group = Arc::new(ProcessGroup::default());
        let mut owned = Owned(
            Command::new("/bin/sh")
                // The three-second sleep bounds this fixture even on failure.
                .args(["-c", "/bin/sleep 3 & printf 'final output'; exit 7"])
                .process_group(0)
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        group.started(owned.0.id());
        let mut stdout = owned.0.stdout.take().unwrap();
        let (sent, received) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut output = String::new();
            stdout.read_to_string(&mut output).unwrap();
            let _ = sent.send(output);
        });
        assert_eq!(group.wait(&mut owned.0).unwrap().code(), Some(7));
        assert_eq!(
            received.recv_timeout(Duration::from_secs(1)).unwrap(),
            "final output"
        );
        reader.join().unwrap();
        group.finish();
    }
}
