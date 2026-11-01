//! On-demand stateful kernels over core workspace process transport.
use crate::process;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

/// One versioned cell execution snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    /// Stable cell identity at submission time.
    pub uid: u64,
    /// Stable nbformat cell ID for reattachment after reopening a document.
    #[serde(default)]
    pub cell: String,
    /// Cell source version at submission time.
    pub version: u64,
    /// Code snapshot submitted to the owned kernel.
    pub code: String,
}

/// Normalized Jupyter events tagged with their originating execution snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Event {
    /// Kernel startup information received from the bridge.
    Ready(Value),
    /// A bounded notebook MIME/stream/error output.
    Output {
        /// Execution snapshot that owns this output.
        run: Run,
        /// nbformat-compatible output JSON.
        output: Value,
        /// Whether this updates an existing display rather than appending output.
        update: bool,
        /// Optional Jupyter display identifier used for later display updates.
        display_id: Option<String>,
    },
    /// Clear outputs for this execution snapshot.
    Clear(Run),
    /// Execution count reported for the submitted cell.
    Count(Run, u64),
    /// Execution completed with the kernel's status label.
    Done(Run, String),
    /// Bridge/startup/protocol failure for display.
    Error(String),
}
/// On-demand, project-scoped kernel and bounded sequential execution queue. Dropping the core
/// transport stops the owned kernel; it is separate from persistent terminals.
pub struct Kernel {
    session: process::Session,
    queue: VecDeque<Run>,
    pending: Option<(u64, Run)>,
    next: u64,
    ready: bool,
    failed: bool,
    stderr: String,
    started: Instant,
    startup_timeout: Duration,
    /// Current human-readable bridge/kernel status.
    pub status: String,
    /// Project-host interpreter running the Jupyter bridge.
    pub python: String,
    /// Installed kernelspec name used for this session.
    pub name: String,
    /// Reported execution host and allocation; never inferred GPU visibility.
    pub execution: String,
}
impl Kernel {
    /// Whether initialization has completed.
    pub fn ready(&self) -> bool {
        self.ready
    }
    /// Execution snapshots that still belong to the host queue.
    pub fn runs(&self) -> Vec<Run> {
        self.pending
            .iter()
            .map(|(_, run)| run.clone())
            .chain(self.queue.iter().cloned())
            .collect()
    }
    /// Borrow the active/queued snapshots without copying their source text.
    pub fn executions(&self) -> impl Iterator<Item = &Run> {
        self.pending
            .iter()
            .map(|(_, run)| run)
            .chain(self.queue.iter())
    }

    /// Adopt a prepared, owned transport and start its installed kernel.
    pub fn from_session(
        session: process::Session,
        python: String,
        name: String,
        execution: String,
        startup_timeout: Duration,
    ) -> Result<Self> {
        session.send(&json!({"cmd":"start","kernel":name}))?;
        Ok(Self {
            session,
            queue: VecDeque::new(),
            pending: None,
            next: 1,
            ready: false,
            failed: false,
            stderr: String::new(),
            started: Instant::now(),
            startup_timeout,
            status: "Starting kernel…".into(),
            python,
            name,
            execution,
        })
    }
    /// Remove queued snapshots invalidated by cell deletion/source edits; do not mutate the
    /// running execution.
    pub fn retain_queued(&mut self, mut keep: impl FnMut(&Run) -> bool) {
        self.queue.retain(|run| keep(run));
    }
    /// Return the stable identity of the executing cell, if any.
    pub fn running_uid(&self) -> Option<u64> {
        self.pending.as_ref().map(|(_, run)| run.uid)
    }
    /// Return whether startup or an execution is in progress.
    pub fn busy(&self) -> bool {
        !self.failed && (self.pending.is_some() || !self.queue.is_empty() || !self.ready)
    }
    /// Validate and enqueue bounded cell snapshots, preserving sequential execution.
    pub fn run(&mut self, runs: Vec<Run>) -> Result<()> {
        ensure!(
            !self.failed,
            "Restart the failed notebook kernel before running cells."
        );
        ensure!(
            self.pending.is_none() && self.queue.is_empty(),
            "A notebook run is already in progress."
        );
        ensure!(
            runs.len() <= 1024
                && runs.iter().map(|r| r.code.len()).sum::<usize>() <= 8 * 1024 * 1024,
            "Too many queued cells."
        );
        ensure!(
            runs.iter().all(|r| r.code.len() <= 128 * 1024),
            "Code cells are limited to 128 KiB per execution."
        );
        self.queue = runs.into();
        self.next_run()
    }
    fn next_run(&mut self) -> Result<()> {
        if !self.ready || self.pending.is_some() {
            return Ok(());
        }
        if let Some(run) = self.queue.pop_front() {
            let id = self.next;
            self.next += 1;
            self.session
                .send(&json!({"cmd":"execute","id":id,"code":run.code}))?;
            self.pending = Some((id, run));
            self.status = "Running…".into();
        } else {
            self.status = "Idle".into();
        }
        Ok(())
    }
    /// Request interruption of current execution through the owned bridge.
    pub fn interrupt(&mut self) -> Result<()> {
        self.queue.clear();
        self.session.send(&json!({"cmd":"interrupt"}))?;
        self.status = "Interrupting…".into();
        Ok(())
    }
    /// Stop the kernel transport and discard pending executions.
    pub fn stop(&mut self) {
        let _ = self.session.send(&json!({"cmd":"shutdown"}));
        self.session.stop();
        self.queue.clear();
        self.pending = None;
        self.status = "Stopped".into();
    }
    /// Whether the bridge supervisor has reaped its owned process.
    pub fn is_finished(&self) -> bool {
        self.session.is_finished()
    }
    fn fail(&mut self, error: String, events: &mut Vec<Event>) {
        if self.failed {
            return;
        }
        self.failed = true;
        self.queue.clear();
        self.pending = None;
        self.status = "Kernel failed".into();
        self.session.stop();
        events.push(Event::Error(error));
    }
    /// Drain available bridge events without blocking, advancing the sequential run queue.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        for _ in 0..32 {
            let Some(event) = self.session.poll() else {
                break;
            };
            if self.failed {
                continue;
            }
            match event {
                process::Event::Message(message) => {
                    let run = message["id"].as_u64().and_then(|id| {
                        self.pending
                            .as_ref()
                            .filter(|(pending, _)| *pending == id)
                            .map(|(_, run)| run.clone())
                    });
                    match message["kind"].as_str().unwrap_or("") {
                        "ready" => {
                            self.ready = true;
                            self.python = message["python"].as_str().unwrap_or("").into();
                            let host = message["host"].as_str().unwrap_or("unknown host");
                            self.execution = match message["slurm_job"].as_str() {
                                Some(job) => format!("Slurm job {job} · {host}"),
                                None => format!("Project host · {host}"),
                            };
                            events.push(Event::Ready(message["language_info"].clone()));
                            if let Err(e) = self.next_run() {
                                self.fail(e.to_string(), &mut events);
                            }
                        }
                        "output" => {
                            if let Some(run) = run {
                                events.push(Event::Output {
                                    run,
                                    output: message["output"].clone(),
                                    update: message["update"] == true,
                                    display_id: message["display_id"].as_str().map(str::to_owned),
                                });
                            }
                        }
                        "clear" => {
                            if let Some(run) = run {
                                events.push(Event::Clear(run));
                            }
                        }
                        "count" => {
                            if let Some(run) = run
                                && let Some(count) = message["count"].as_u64()
                            {
                                events.push(Event::Count(run, count));
                            }
                        }
                        "done" => {
                            if let Some(run) = run {
                                let status =
                                    message["status"].as_str().unwrap_or("error").to_owned();
                                if let Some(count) = message["count"].as_u64() {
                                    events.push(Event::Count(run.clone(), count));
                                }
                                self.pending = None;
                                if status != "ok" {
                                    self.queue.clear();
                                }
                                events.push(Event::Done(run, status));
                                if let Err(e) = self.next_run() {
                                    self.fail(e.to_string(), &mut events);
                                }
                            }
                        }
                        "error" => self.fail(
                            message["message"]
                                .as_str()
                                .unwrap_or("Notebook kernel error")
                                .into(),
                            &mut events,
                        ),
                        _ => self.fail("Invalid notebook kernel message.".into(), &mut events),
                    }
                }
                process::Event::Error(error) => self.fail(error, &mut events),
                process::Event::Exited(code) => {
                    let reason = code
                        .map(|code| format!("exit status {code}"))
                        .unwrap_or_else(|| "terminated by a signal".into());
                    self.fail(
                        format!(
                            "Notebook kernel connection ended ({reason}) on {} using {}. {}",
                            self.execution,
                            self.python,
                            if self.stderr.is_empty() {
                                "The process or allocation ended."
                            } else {
                                self.stderr.trim()
                            }
                        ),
                        &mut events,
                    );
                }
                process::Event::Stderr(text) => {
                    if self.ready {
                        continue;
                    }
                    self.status = text.chars().take(512).collect();
                    self.stderr.push_str(&text);
                    self.stderr.push('\n');
                    self.stderr = self
                        .stderr
                        .chars()
                        .rev()
                        .take(4096)
                        .collect::<String>()
                        .chars()
                        .rev()
                        .collect();
                }
                process::Event::Started => {}
            }
        }
        if !self.ready && !self.failed && self.started.elapsed() > self.startup_timeout {
            self.fail(
                format!(
                    "Kernel startup exceeded {} seconds on {} using {}.",
                    self.startup_timeout.as_secs(),
                    self.execution,
                    self.python
                ),
                &mut events,
            );
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    struct Input;
    impl process::Sink for Input {
        fn try_send_buffer(&self, bytes: &mut Vec<u8>) -> Result<bool> {
            bytes.clear();
            Ok(true)
        }
    }
    struct Transport {
        events: mpsc::Receiver<process::Event>,
        stopped: Arc<AtomicBool>,
    }
    impl process::Transport for Transport {
        fn writer(&self) -> process::Writer {
            process::Writer::new(Input)
        }
        fn poll(&self) -> Option<process::Event> {
            self.events.try_recv().ok()
        }
        fn stop(&self) {
            self.stopped.store(true, Ordering::Release);
        }
        fn is_finished(&self) -> bool {
            self.stopped.load(Ordering::Acquire)
        }
    }
    fn kernel(timeout: Duration) -> (Kernel, mpsc::Sender<process::Event>, Arc<AtomicBool>) {
        let (send, events) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let session = process::Session::new(Transport {
            events,
            stopped: stopped.clone(),
        });
        (
            Kernel::from_session(
                session,
                "/project/.venv/bin/python".into(),
                "python3".into(),
                "Project host".into(),
                timeout,
            )
            .unwrap(),
            send,
            stopped,
        )
    }

    #[test]
    fn startup_error_survives_exit_in_the_same_or_a_later_poll() {
        for split in [false, true] {
            let (mut kernel, send, stopped) = kernel(Duration::from_secs(30));
            kernel
                .run(vec![Run {
                    uid: 1,
                    cell: "cell".into(),
                    version: 0,
                    code: "print(42)".into(),
                }])
                .unwrap();
            send.send(process::Event::Message(json!({
                "kind": "error", "message": "Install jupyter_client and ipykernel"
            })))
            .unwrap();
            let mut events = if split { kernel.poll() } else { vec![] };
            send.send(process::Event::Stderr(
                "Additional shutdown diagnostic".into(),
            ))
            .unwrap();
            send.send(process::Event::Exited(Some(1))).unwrap();
            events.extend(kernel.poll());
            assert!(
                matches!(events.as_slice(), [Event::Error(error)] if error == "Install jupyter_client and ipykernel")
            );
            assert!(!kernel.busy());
            assert!(kernel.runs().is_empty());
            assert_eq!(kernel.status, "Kernel failed");
            assert!(kernel.run(vec![]).is_err());
            assert!(stopped.load(Ordering::Acquire));
        }
    }

    #[test]
    fn exit_without_a_protocol_error_includes_startup_stderr_and_interpreter() {
        let (mut kernel, send, _) = kernel(Duration::from_secs(30));
        send.send(process::Event::Stderr("Traceback:".into()))
            .unwrap();
        send.send(process::Event::Stderr(
            "ImportError: broken dependency".into(),
        ))
        .unwrap();
        send.send(process::Event::Exited(Some(1))).unwrap();
        let events = kernel.poll();
        let [Event::Error(error)] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert!(error.contains("exit status 1"));
        assert!(error.contains("Project host using /project/.venv/bin/python"));
        assert!(error.contains("Traceback:\nImportError: broken dependency"));
        assert!(!error.contains("Starting kernel"));
    }

    #[test]
    fn startup_timeout_stops_the_transport_and_cannot_be_revived_by_a_late_ready() {
        let (mut kernel, send, stopped) = kernel(Duration::from_secs(30));
        kernel.started = Instant::now() - Duration::from_secs(31);
        assert!(
            matches!(kernel.poll().as_slice(), [Event::Error(error)] if error.contains("startup exceeded"))
        );
        assert!(stopped.load(Ordering::Acquire));
        send.send(process::Event::Message(json!({"kind": "ready"})))
            .unwrap();
        send.send(process::Event::Exited(Some(0))).unwrap();
        assert!(kernel.poll().is_empty());
        assert!(!kernel.ready());
        assert!(!kernel.busy());
    }
}
