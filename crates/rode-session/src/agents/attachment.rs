//! Atomic agent attachment ownership. Replacing a lease closes only its old
//! stream. Old readers and destructors cannot mutate or detach the successor.
use super::*;
use rode_agent_core::service::{Controller, ViewOwner};

/// The socket belongs to a lease, never to the host-owned controller process.
pub(super) struct Connection {
    id: String,
    stream: UnixStream,
}
fn owns(shared: &Shared, id: &str) -> bool {
    shared.controller.as_ref().is_some_and(|c| c.id == id)
}
struct Release {
    job: Arc<Job>,
    id: String,
}
impl Drop for Release {
    fn drop(&mut self) {
        let mut shared = self.job.shared.lock().unwrap();
        if !owns(&shared, &self.id) {
            return;
        }
        shared.controller = None;
        shared.state.info.attached = false;
        shared.state.info.controller = None;
        shared.revision += 1;
        drop(shared);
        self.job.changed.notify_all();
        let _ = self.job.input.try_send(Work::Detached);
    }
}
impl Store {
    /// Reserve or replace a controlling lease under the same lock that admits
    /// input. A stale confirmation fails even if another window has detached.
    pub(crate) fn attach(
        &self,
        mut stream: UnixStream,
        id: &str,
        policy: &str,
        owner: ViewOwner,
        expected: Option<&str>,
    ) -> Result<()> {
        owner.validate()?;
        let job = self.find(id, policy)?;
        let lease = new_id()?;
        let socket = stream.try_clone()?;
        {
            let mut shared = job.shared.lock().unwrap();
            ensure!(!shared.finished, "This agent has already stopped");
            if let Some(expected) = expected {
                ensure!(
                    owns(&shared, expected),
                    "The controlling window changed. Refresh Running agents and confirm again."
                );
            } else {
                ensure!(
                    shared.controller.is_none(),
                    "This agent already has a controlling RODE window. Use Take over in Running agents to replace it after confirmation."
                );
            }
            let previous = shared.controller.replace(Connection {
                id: lease.clone(),
                stream: socket,
            });
            shared.ever_attached = true;
            shared.state.info.attached = true;
            shared.state.info.controller = Some(Controller {
                id: lease.clone(),
                owner,
            });
            shared.revision += 1;
            if let Some(previous) = previous {
                // Install the successor first: Auto never sees a detached gap.
                let _ = previous.stream.shutdown(std::net::Shutdown::Both);
            }
        }
        job.changed.notify_all();
        let _lease = Release {
            job: job.clone(),
            id: lease.clone(),
        };
        let _ = job.input.try_send(Work::Wake);
        stream.set_read_timeout(None)?;
        let mut writer = stream.try_clone()?;
        let writing = job.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let writer_lease = lease.clone();
        std::thread::spawn(move || {
            let mut revision = u64::MAX;
            let mut previous = HashMap::<String, rode_agent_core::protocol::Message>::new();
            let mut previous_input_history = None;
            loop {
                let shared = writing.shared.lock().unwrap();
                let shared = writing
                    .changed
                    .wait_timeout_while(shared, Duration::from_secs(1), |s| {
                        s.revision == revision && !stop.load(Ordering::Acquire)
                    })
                    .unwrap()
                    .0;
                if stop.load(Ordering::Acquire) || !owns(&shared, &writer_lease) {
                    break;
                }
                if shared.revision == revision {
                    continue;
                }
                revision = shared.revision;
                let mut state = shared.state.clone();
                let finished = shared.finished;
                drop(shared);
                let order = state.chat.messages.iter().map(|m| m.id.clone()).collect();
                // Compare the complete item, including tool lifecycle. An empty
                // command can finish without changing either its title or body.
                let changed: VecDeque<_> = state
                    .chat
                    .messages
                    .iter()
                    .filter(|m| previous.get(&m.id).is_none_or(|old| old != *m))
                    .cloned()
                    .collect();
                previous = state
                    .chat
                    .messages
                    .iter()
                    .map(|m| (m.id.clone(), m.clone()))
                    .collect();
                state.chat.messages = changed;
                // Prompts change only on submission/replay; streaming output
                // should not retransmit the whole recall buffer each frame.
                if state.chat.input_history == previous_input_history {
                    state.chat.input_history = None;
                } else {
                    previous_input_history = state.chat.input_history.clone();
                }
                if send_json(
                    &mut writer,
                    &rode_agent_core::service::Update { state, order },
                )
                .is_err()
                    || finished
                {
                    break;
                }
            }
            let _ = writer.shutdown(std::net::Shutdown::Both);
        });
        while let Ok(action) = recv_json::<_, Action>(&mut stream) {
            let shared = job.shared.lock().unwrap();
            if !owns(&shared, &lease) {
                break;
            }
            if matches!(action, Action::Heartbeat) {
                continue;
            }
            if job.input.try_send(Work::Action(Box::new(action))).is_err() {
                break;
            }
        }
        stopped.store(true, Ordering::Release);
        job.changed.notify_all();
        let _ = stream.shutdown(std::net::Shutdown::Both);
        Ok(())
    }
}
