//! Shared signal/stop state for a single, separately created process group.
//! The transport worker retains the actual child and performs wait/reaping.
use std::{
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
/// its leader with [`Self::started`]. Call [`Self::reaped`] immediately after waiting
/// for that leader, then [`Self::finish`] once transport cleanup has completed.
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

    /// Clean up descendants after the leader was reaped, then invalidate signalling.
    /// Call this before joining pipe readers so surviving descendants cannot keep
    /// stdout/stderr open indefinitely after their parent exits.
    pub fn reaped(&self) {
        let mut state = self.state.lock().unwrap();
        if let Some(pid) = state.pid.take() {
            // SAFETY: finish the separately owned group before releasing its ID.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
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
        process::{Child, Command, ExitStatus, Stdio},
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
        fn wait(&mut self) -> ExitStatus {
            let until = Instant::now() + STOP_GRACE + Duration::from_secs(3);
            loop {
                if let Some(status) = self.0.try_wait().unwrap() {
                    return status;
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
        assert!(!owned.wait().success());
        group.reaped();
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
        assert!(!owned.wait().success());
        group.reaped();
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
            assert!(!owned.wait().success());
            group.reaped();
            group.finish();
        }
    }
}
