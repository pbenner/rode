//! Native host-owned agent controllers. One controlling view prevents conflicting
//! prompt answers; persistence keeps the controller and bounded state in RAM after
//! that view disconnects. No transcript or credentials are written to disk.
use super::*;
use rode_agent_core::{
    history::Browser,
    protocol::Chat,
    service::{Command as Action, Info, Spec, State},
};
use serde_json::Value;
use std::sync::{Condvar, atomic::AtomicUsize};
mod transport;

#[derive(Default)]
pub(super) struct Store {
    jobs: Mutex<HashMap<String, Arc<Job>>>,
}
struct Shared {
    state: State,
    revision: u64,
    attached: bool,
    ever_attached: bool,
    finished: bool,
    changed: Instant,
}
struct Job {
    shared: Mutex<Shared>,
    changed: Condvar,
    input: mpsc::SyncSender<Work>,
    created: Instant,
}
enum Work {
    Action(Box<Action>),
    Wake,
    Detached,
}
impl Store {
    pub fn live(&self) -> bool {
        self.jobs
            .lock()
            .unwrap()
            .values()
            .any(|job| !job.shared.lock().unwrap().finished)
    }
    pub fn list(&self) -> Vec<Info> {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.retain(|_, job| {
            !job.shared.lock().unwrap().finished || job.created.elapsed() < Duration::from_secs(5)
        });
        let mut infos: Vec<_> = jobs
            .values()
            .map(|job| job.shared.lock().unwrap().state.info.clone())
            .collect();
        infos.sort_by_key(|info| info.created);
        infos
    }
    fn find(&self, id: &str, policy: &str) -> Result<Arc<Job>> {
        let job = self
            .jobs
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("Background agent is no longer available")?;
        ensure!(
            job.shared.lock().unwrap().state.info.policy == policy,
            "Open the agent's original project and sandbox policy before attaching"
        );
        Ok(job)
    }
    pub fn end(&self, id: &str, policy: &str) -> Result<()> {
        let job = self.find(id, policy)?;
        if job.shared.lock().unwrap().finished {
            return Ok(());
        }
        job.input
            .try_send(Work::Action(Box::new(Action::End)))
            .context("Background agent command queue is full or disconnected")
    }
    pub fn create(&self, spec: Spec) -> Result<Info> {
        ensure!(
            valid_id(&spec.id) && valid_name(&spec.name),
            "Invalid background agent ID/name"
        );
        ensure!(
            spec.policy.len() == 64 && spec.policy.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid agent policy fingerprint"
        );
        ensure!(
            Path::new(&spec.plan.root).is_absolute() && spec.plan.root.len() <= 4096,
            "Invalid agent working folder"
        );
        ensure!(
            !spec.plan.active || !spec.plan.prefix.is_empty(),
            "Active agent sandbox has no launch policy"
        );
        ensure!(
            !spec.executable.is_empty()
                && spec.executable.len() <= 4096
                && !spec.executable.contains('\0'),
            "Invalid agent executable"
        );
        ensure!(
            spec.plan.prefix.len() <= 256
                && spec.plan.prefix.iter().map(String::len).sum::<usize>() <= 64 * 1024,
            "Agent sandbox command exceeds its size limit"
        );
        ensure!(
            spec.env
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>()
                <= 64 * 1024,
            "Agent environment exceeds 64 KiB"
        );
        spec.config.arguments(spec.text.is_some())?;
        if let Some(text) = &spec.text {
            ensure!(
                !text.trim().is_empty() && text.len() <= rode_agent_core::PROMPT_LIMIT,
                "Agent prompt must contain 1–32768 bytes"
            );
        }
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get(&spec.id) {
            let state = job.shared.lock().unwrap();
            ensure!(
                state.state.info.policy == spec.policy,
                "Agent ID belongs to another policy"
            );
            return Ok(state.state.info.clone());
        }
        jobs.retain(|_, job| !job.shared.lock().unwrap().finished);
        ensure!(
            jobs.len() < 16,
            "At most 16 background agents may run on this host"
        );
        let (input, commands) = mpsc::sync_channel(32);
        let info = Info {
            id: spec.id.clone(),
            name: spec.name.clone(),
            root: spec.plan.root.clone(),
            policy: spec.policy.clone(),
            mode: spec.mode.clone(),
            provider: spec.config.provider,
            alive: true,
            busy: true,
            status: "Starting agent…".into(),
            attached: false,
            persistent: spec.persistent,
            created: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        };
        let job = Arc::new(Job {
            shared: Mutex::new(Shared {
                state: State {
                    info: info.clone(),
                    chat: rode_agent_core::service::ChatView {
                        busy: spec.text.is_some(),
                        status: "Starting agent…".into(),
                        config: spec.config.clone(),
                        ..Default::default()
                    },
                    history: spec
                        .text
                        .is_none()
                        .then(|| rode_agent_core::service::HistoryView {
                            busy: true,
                            status: "Loading saved conversations…".into(),
                            ..Default::default()
                        }),
                    error: String::new(),
                },
                revision: 0,
                attached: false,
                ever_attached: false,
                finished: false,
                changed: Instant::now(),
            }),
            changed: Condvar::new(),
            input,
            created: Instant::now(),
        });
        jobs.insert(spec.id.clone(), job.clone());
        std::thread::spawn(move || run(job, commands, spec));
        Ok(info)
    }
    pub fn attach(&self, mut stream: UnixStream, id: &str, policy: &str) -> Result<()> {
        let job = self.find(id, policy)?;
        {
            let mut shared = job.shared.lock().unwrap();
            ensure!(
                !shared.attached,
                "This agent already has a controlling RODE window. Detach it there before attaching here."
            );
            shared.attached = true;
            shared.ever_attached = true;
            shared.state.info.attached = true;
        }
        struct Lease(Arc<Job>);
        impl Drop for Lease {
            fn drop(&mut self) {
                let mut shared = self.0.shared.lock().unwrap();
                shared.attached = false;
                shared.state.info.attached = false;
                drop(shared);
                let _ = self.0.input.try_send(Work::Detached);
            }
        }
        let _lease = Lease(job.clone());
        let _ = job.input.try_send(Work::Wake);
        stream.set_read_timeout(None)?;
        let mut writer = stream.try_clone()?;
        let writing = job.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::spawn(move || {
            let mut revision = u64::MAX;
            let mut previous = HashMap::<String, rode_agent_core::protocol::Message>::new();
            loop {
                let shared = writing.shared.lock().unwrap();
                let shared = writing
                    .changed
                    .wait_timeout_while(shared, Duration::from_secs(1), |s| {
                        s.revision == revision && !stop.load(Ordering::Acquire)
                    })
                    .unwrap()
                    .0;
                if stop.load(Ordering::Acquire) {
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
                let changed: VecDeque<_> = state
                    .chat
                    .messages
                    .iter()
                    .filter(|m| {
                        previous.get(&m.id).is_none_or(|old| {
                            old.text != m.text || old.title != m.title || old.role != m.role
                        })
                    })
                    .cloned()
                    .collect();
                previous = state
                    .chat
                    .messages
                    .iter()
                    .map(|m| (m.id.clone(), m.clone()))
                    .collect();
                state.chat.messages = changed;
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
fn run(job: Arc<Job>, input: mpsc::Receiver<Work>, spec: Spec) {
    let active = Arc::new(AtomicUsize::new(0));
    let factory = Arc::new(transport::Factory {
        spec: spec.clone(),
        active: active.clone(),
    });
    let launch = rode_agent_core::Launch {
        config: spec.config.clone(),
        plan: spec.plan.clone(),
        remote: false,
        python: spec.python.clone(),
        factory,
    };
    let sender = job.input.clone();
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = sender.try_send(Work::Wake);
    });
    let mut chat = Chat::default();
    let mut browser = None;
    let result = if let Some(text) = spec.text {
        chat.submit(launch.clone(), text, wake.clone())
    } else {
        Browser::open(launch.clone(), wake.clone()).map(|value| browser = Some(value))
    };
    let mut error = result.err().map(|e| format!("{e:#}")).unwrap_or_default();
    if !error.is_empty() {
        chat.notice(&error);
    }
    let mut changed = true;
    let mut stopping = false;
    let mut last_view = Instant::now() - Duration::from_secs(1);
    loop {
        chat.poll();
        if let Some(history) = &mut browser {
            history.poll();
            if let Some(restored) = history.take_restored() {
                chat.restore(restored);
                browser = None;
                changed = true;
            }
        }
        let mut shared = job.shared.lock().unwrap();
        let view = shared.attached;
        if !shared.state.info.persistent
            && !view
            && (shared.ever_attached || job.created.elapsed() >= Duration::from_secs(30))
        {
            stopping = true;
        }
        if !error.is_empty() && !chat.busy && !chat.connected() && browser.is_none() {
            stopping = true;
        }
        if browser.is_none()
            && !chat.busy
            && !chat.connected()
            && active.load(Ordering::Acquire) == 0
        {
            stopping = true;
        }
        if stopping {
            chat.disconnect();
            browser = None;
        }
        let finished = stopping && active.load(Ordering::Acquire) == 0;
        shared.state.info.busy = chat.busy || browser.as_ref().is_some_and(|b: &Browser| b.busy);
        shared.state.info.status = browser
            .as_ref()
            .map(|b| b.status.clone())
            .unwrap_or_else(|| chat.status.clone());
        shared.state.info.alive = !finished;
        if (changed && view && last_view.elapsed() >= Duration::from_millis(200)) || finished {
            shared.state.chat = chat.view();
            shared.state.history = browser.as_ref().map(Browser::view);
            shared.state.error = std::mem::take(&mut error);
            shared.revision = shared.revision.wrapping_add(1);
            shared.changed = Instant::now();
            job.changed.notify_all();
            changed = false;
            last_view = Instant::now();
        }
        shared.finished = finished;
        drop(shared);
        if finished {
            break;
        }
        let timeout = if stopping {
            Duration::from_millis(50)
        } else if changed && view {
            Duration::from_millis(200).saturating_sub(last_view.elapsed())
        } else if chat.has_deadline() || browser.as_ref().is_some_and(|b| b.busy) {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(30)
        };
        match input.recv_timeout(timeout) {
            Ok(Work::Action(action)) => {
                changed = true;
                error.clear();
                let result = match *action {
                    Action::Submit { text, config } => {
                        if !launch.config.same_process(&config) {
                            Err(anyhow::anyhow!(
                                "Backend executable/provider changes require a new agent session"
                            ))
                        } else {
                            let mut launch = launch.clone();
                            launch.config = config;
                            chat.submit(launch, text, wake.clone())
                        }
                    }
                    Action::Configure { config } => chat.configure_idle(&config),
                    Action::Answer { id, result } => {
                        if chat.prompts.front().is_some_and(|p| p.id == id) {
                            chat.answer(result)
                        } else {
                            Err(anyhow::anyhow!(
                                "The permission/question card changed; review the current request before answering"
                            ))
                        }
                    }
                    Action::Cancel => {
                        chat.cancel();
                        Ok(())
                    }
                    Action::End => {
                        stopping = true;
                        Ok(())
                    }
                    Action::Persistence { persistent } => {
                        job.shared.lock().unwrap().state.info.persistent = persistent;
                        Ok(())
                    }
                    Action::HistoryMore => browser
                        .as_mut()
                        .context("History browser is closed")
                        .and_then(Browser::load_more),
                    Action::HistoryResume { id } => browser
                        .as_mut()
                        .context("History browser is closed")
                        .and_then(|b| b.resume(&id)),
                };
                if let Err(failure) = result {
                    error = format!("{failure:#}");
                }
            }
            Ok(Work::Wake | Work::Detached) => changed = true,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if chat.has_deadline() || browser.as_ref().is_some_and(|b| b.busy) {
                    changed = true;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => stopping = true,
        }
    }
}

pub(super) fn bridge(id: &str, policy: &str) -> Result<()> {
    let launcher = Launcher {
        executable: current_executable()?,
        prefix: vec![],
    };
    let dir = service_directories().into_iter().find(|dir| matches!(request_once(&Request::Agents, &launcher, dir), Ok(Response::Agents(infos)) if infos.iter().any(|info| info.id == id))).context("Background agent is no longer available; refresh the agent list")?;
    let mut stream = connect_at(&dir)?;
    send_json(
        &mut stream,
        &Request::AttachAgent {
            id: id.into(),
            policy: policy.into(),
        },
    )?;
    stream.set_read_timeout(None)?;
    let mut sender = stream.try_clone()?;
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(std::io::stdin());
        use std::io::BufRead;
        loop {
            let mut bytes = vec![];
            let read = (&mut reader)
                .take(256 * 1024 + 1)
                .read_until(b'\n', &mut bytes);
            if !matches!(read, Ok(n) if n > 0 && n <= 256 * 1024) {
                break;
            }
            let Ok(action) = serde_json::from_slice::<Action>(&bytes) else {
                break;
            };
            if send_json(&mut sender, &action).is_err() {
                break;
            }
        }
        let _ = sender.shutdown(std::net::Shutdown::Both);
    });
    loop {
        let value = recv_json::<_, Value>(&mut stream)?;
        if value["kind"] == "error" {
            bail!(
                "{}",
                value["data"]
                    .as_str()
                    .unwrap_or("Cannot attach background agent")
            );
        }
        let update: rode_agent_core::service::Update = serde_json::from_value(value)?;
        serde_json::to_writer(std::io::stdout().lock(), &update)?;
        println!();
        if !update.state.info.alive {
            break;
        }
    }
    Ok(())
}
