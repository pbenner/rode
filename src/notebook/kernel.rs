//! On-demand stateful kernels over core workspace process transport.
use crate::{process, sandbox, ssh};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

/// One versioned cell execution snapshot.
#[derive(Clone)]
pub struct Run {
    /// Stable cell identity at submission time.
    pub uid: u64,
    /// Cell source version at submission time.
    pub version: u64,
    /// Code snapshot submitted to the owned kernel.
    pub code: String,
}
/// Normalized Jupyter events tagged with their originating execution snapshot.
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
    started: Instant,
    /// Current human-readable bridge/kernel status.
    pub status: String,
    /// Project-host interpreter running the Jupyter bridge.
    pub python: String,
    /// Installed kernelspec name used for this session.
    pub name: String,
}
impl Kernel {
    /// Start an installed kernelspec through the selected project-host interpreter and actual
    /// sandbox; startup errors arrive asynchronously and no packages are installed.
    pub fn start(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        python: String,
        name: String,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        ensure!(
            !name.is_empty()
                && name.len() <= 128
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
            "Invalid kernelspec name."
        );
        let session = process::Session::start(
            process::Launch {
                native_debug: false,
                argv: vec![
                    "python3".into(),
                    "-u".into(),
                    "-c".into(),
                    include_str!("../notebook_kernel.py").into(),
                ],
                plan,
                remote,
                python: if python == "off" {
                    "python3".into()
                } else {
                    python.clone()
                },
                python_program: true,
                line_limit: 2 * 1024 * 1024,
            },
            wake,
        )?;
        session.send(&json!({"cmd":"start","kernel":name}))?;
        Ok(Self {
            session,
            queue: VecDeque::new(),
            pending: None,
            next: 1,
            ready: false,
            started: Instant::now(),
            status: "Starting kernel…".into(),
            python,
            name,
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
        self.pending.is_some() || !self.queue.is_empty() || !self.ready
    }
    /// Validate and enqueue bounded cell snapshots, preserving sequential execution.
    pub fn run(&mut self, runs: Vec<Run>) -> Result<()> {
        ensure!(
            self.pending.is_none() && self.queue.is_empty(),
            "A notebook run is already in progress."
        );
        ensure!(
            runs.len() <= super::MAX_CELLS
                && runs.iter().map(|r| r.code.len()).sum::<usize>() <= super::MAX_BYTES,
            "Too many queued cells."
        );
        ensure!(
            runs.iter().all(|r| r.code.len() <= super::MAX_CODE),
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
    /// Drain available bridge events without blocking, advancing the sequential run queue.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        for _ in 0..32 {
            let Some(event) = self.session.poll() else {
                break;
            };
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
                            events.push(Event::Ready(message["language_info"].clone()));
                            if let Err(e) = self.next_run() {
                                events.push(Event::Error(e.to_string()));
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
                                    events.push(Event::Error(e.to_string()));
                                }
                            }
                        }
                        "error" => events.push(Event::Error(
                            message["message"]
                                .as_str()
                                .unwrap_or("Notebook kernel error")
                                .into(),
                        )),
                        _ => events.push(Event::Error("Invalid notebook kernel message.".into())),
                    }
                }
                process::Event::Error(error) => events.push(Event::Error(error)),
                process::Event::Exited(code) => events.push(Event::Error(format!(
                    "Notebook kernel connection ended ({code:?})."
                ))),
                process::Event::Stderr(text) => {
                    if self.ready {
                        continue;
                    }
                    self.status = text.chars().take(512).collect();
                }
                process::Event::Started => {}
            }
        }
        if !self.ready && self.started.elapsed() > Duration::from_secs(30) {
            events.push(Event::Error("Kernel startup exceeded 30 seconds.".into()));
        }
        events
    }
}
