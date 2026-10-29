//! Bounded, event-driven DAP client. Variable references and responses belong to
//! one stopped epoch; late adapter replies can never repopulate a running view.
use super::{Command, Event, Settings, Start};
use crate::{process, sandbox, ssh};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// High-level debugger execution state; inspection handles are valid only while paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Adapter initialization/configuration is in progress.
    Starting,
    /// The debuggee is executing; paused-state references have been invalidated.
    Running,
    /// The debuggee stopped and current-epoch inspection is available.
    Paused,
    /// The adapter/debuggee session ended.
    Ended,
}
#[derive(Clone)]
enum Pending {
    Initialize,
    Launch,
    Configure,
    Breakpoints(String, Vec<usize>, bool),
    Resume,
    Pause,
    ConfigurationDone,
    Threads(u64),
    PauseThreads(u64),
    Stack(u64),
    Scopes(u64),
    Variables(u64, i64),
    Evaluate(u64, Option<usize>),
}
impl Pending {
    fn epoch(&self) -> Option<u64> {
        match self {
            Self::PauseThreads(e)
            | Self::Threads(e)
            | Self::Stack(e)
            | Self::Scopes(e)
            | Self::Variables(e, _)
            | Self::Evaluate(e, _) => Some(*e),
            _ => None,
        }
    }
}
/// Adapter verification result corresponding to one requested source breakpoint.
#[derive(Clone, Debug)]
pub struct Verified {
    /// One-based adapter-resolved source line.
    pub line: usize,
    /// Whether the adapter accepted the breakpoint.
    pub verified: bool,
    /// Adapter explanation, including unresolved breakpoint reasons.
    pub message: String,
    /// Optional adapter-owned breakpoint identifier.
    pub id: Option<i64>,
}
const MAX_REQUESTS: usize = super::REQUEST_LIMIT;
struct Outgoing {
    sequence: u64,
    bytes: Vec<u8>,
}
/// Bounded event-driven DAP state machine and owned host coordinator. Paused queries carry an
/// epoch, queued writes retain encoded bytes across backpressure, and drop starts scoped
/// shutdown.
pub struct Client {
    session: Option<process::Session>,
    closing: Arc<AtomicBool>,
    queue: VecDeque<Outgoing>,
    breakpoint_requests: BTreeMap<String, u64>,
    pending: BTreeMap<u64, (Pending, Instant)>,
    next: u64,
    epoch: u64,
    started: Instant,
    terminated: Option<Instant>,
    configuring: usize,
    initialized: bool,
    /// Current execution phase.
    pub phase: Phase,
    /// Human-readable lifecycle/stop status.
    pub status: String,
    /// Most recent user-visible failure.
    pub error: String,
    /// Adapter capabilities returned during initialization.
    pub capabilities: Value,
    /// Bounded current-epoch DAP thread objects.
    pub threads: Vec<Value>,
    /// Bounded frames for the selected thread.
    pub frames: Vec<Value>,
    /// Scopes for the selected paused frame.
    pub scopes: Vec<Value>,
    /// Lazily loaded children keyed by current-epoch DAP variable reference.
    pub variables: BTreeMap<i64, Vec<Value>>,
    /// Selected adapter thread identifier.
    pub thread: Option<i64>,
    /// Selected adapter frame identifier.
    pub frame: Option<i64>,
    /// Bounded debugger console transcript.
    pub output: String,
    /// Displayed evaluation results corresponding to saved watches.
    pub watch_values: Vec<String>,
    /// Verified breakpoint state keyed by project-host source path.
    pub breakpoints: BTreeMap<String, Vec<Verified>>,
    /// Bounded coordinator terminal notifications drained by the UI terminal service.
    pub terminal_events: VecDeque<Event>,
    /// Validated per-project configuration, breakpoints and watches.
    pub settings: Settings,
    arguments: Value,
    /// Pending source jump as host path, one-based line and one-based column.
    pub navigation: Option<(String, usize, usize)>,
}
impl Client {
    /// Start a project-host adapter/coordinator under the prepared plan; sandboxed attach is
    /// rejected. Validation is synchronous, while startup and DAP work progress through
    /// [`Self::poll`].
    pub fn start(
        plan: sandbox::Plan,
        remote: Option<ssh::Remote>,
        file: String,
        python: String,
        settings: Settings,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        settings.validate()?;
        ensure!(
            !plan.active || !settings.config.attach,
            "Attach is available in normal mode. An existing process cannot be moved into a project sandbox."
        );
        let start = Start {
            root: plan.root.clone(),
            file,
            python: python.clone(),
            config: settings.config.clone(),
        };
        let session = process::Session::start(
            process::Launch {
                argv: vec![
                    "python3".into(),
                    "-u".into(),
                    "-c".into(),
                    include_str!("../remote_debugger.py").into(),
                ],
                plan,
                remote,
                python,
                python_program: false,
                line_limit: 2 * 1024 * 1024,
                native_debug: true,
            },
            wake,
        )?;
        Self::from_session(session, start, settings)
    }
    /// Inject an already host-scoped transport (also used by native/peer tests).
    pub fn from_session(
        session: process::Session,
        start: Start,
        settings: Settings,
    ) -> Result<Self> {
        settings.validate()?;
        session.send(&serde_json::to_value(Command::Start { launch: start })?)?;
        Ok(Self::new(Some(session), settings))
    }
    fn new(session: Option<process::Session>, settings: Settings) -> Self {
        Self {
            session,
            breakpoint_requests: BTreeMap::new(),
            closing: Arc::new(AtomicBool::new(false)),
            queue: VecDeque::new(),
            pending: BTreeMap::new(),
            next: 1,
            epoch: 0,
            started: Instant::now(),
            terminated: None,
            configuring: 0,
            initialized: false,
            phase: Phase::Starting,
            status: "Starting adapter…".into(),
            error: String::new(),
            capabilities: json!({}),
            threads: vec![],
            frames: vec![],
            scopes: vec![],
            variables: BTreeMap::new(),
            thread: None,
            frame: None,
            output: String::new(),
            watch_values: vec![String::new(); settings.watches.len()],
            breakpoints: BTreeMap::new(),
            terminal_events: VecDeque::new(),
            settings,
            arguments: Value::Null,
            navigation: None,
        }
    }
    /// Return the epoch that owns current paused stack/scope/variable references.
    pub fn inspection_epoch(&self) -> u64 {
        self.epoch
    }
    /// Clone the flag indicating asynchronous scoped cleanup is in progress.
    pub fn cleanup(&self) -> Arc<AtomicBool> {
        self.closing.clone()
    }
    /// Return whether the client has begun shutdown.
    pub fn is_stopping(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }
    /// Return whether protocol requests or queued writes still need progress.
    pub fn pending(&self) -> bool {
        !self.queue.is_empty() || !self.pending.is_empty() || self.terminated.is_some()
    }
    /// Clone a bounded input handle while the coordinator is present.
    pub fn writer(&self) -> Option<process::Writer> {
        self.session.as_ref().map(process::Session::writer)
    }
    fn sequence(&mut self) -> u64 {
        let seq = self.next;
        self.next += 1;
        seq
    }
    fn request(&mut self, command: &str, arguments: Value, pending: Pending) -> Result<()> {
        ensure!(
            self.pending.len() < MAX_REQUESTS && self.queue.len() < MAX_REQUESTS,
            "Too many pending debugger requests."
        );
        let seq = self.sequence();
        let message = json!({"seq":seq,"type":"request","command":command,"arguments":arguments});
        let bytes = process::encode_request(&serde_json::to_value(Command::Dap { message })?)?;
        self.queue.push_back(Outgoing {
            sequence: seq,
            bytes,
        });
        self.pending.insert(seq, (pending, Instant::now()));
        Ok(())
    }
    /// Read a boolean adapter capability by its DAP key.
    pub fn supports(&self, cap: &str) -> bool {
        self.capabilities[cap].as_bool().unwrap_or(false)
    }
    /// Append text to the bounded debugger console.
    pub fn log(&mut self, text: &str) {
        self.output.push_str(text);
        if self.output.len() > 256 * 1024 {
            let mut end = self.output.len() - 192 * 1024;
            while !self.output.is_char_boundary(end) {
                end += 1;
            }
            self.output.drain(..end);
        }
    }
    fn cancel_requests(&mut self, filter: impl Fn(&Pending) -> bool) {
        self.queue.retain(|item| {
            !self
                .pending
                .get(&item.sequence)
                .is_some_and(|(pending, _)| filter(pending))
        });
        self.pending.retain(|_, (pending, _)| !filter(pending));
    }
    fn invalidate_inspection(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.cancel_requests(|pending| pending.epoch().is_some());
        self.scopes.clear();
        self.variables.clear();
        self.frame = None;
        self.navigation = None;
        self.watch_values.fill(String::new());
    }
    fn clear_pause(&mut self) {
        self.invalidate_inspection();
        self.frames.clear();
    }
    /// Send a supported pause/continue/step control request with phase/capability guards.
    pub fn control(&mut self, command: &str) -> Result<()> {
        ensure!(
            matches!(
                command,
                "continue" | "pause" | "next" | "stepIn" | "stepOut"
            ),
            "Unsupported debugger control: {command}"
        );
        ensure!(
            self.phase == Phase::Paused || command == "pause" && self.phase == Phase::Running,
            "The program is not ready for this control."
        );
        if command == "pause" && self.thread.is_none() {
            return self.request("threads", json!({}), Pending::PauseThreads(self.epoch));
        }
        let thread = self
            .thread
            .ok_or_else(|| anyhow::anyhow!("No thread selected."))?;
        self.request(
            command,
            json!({"threadId":thread}),
            if command == "pause" {
                Pending::Pause
            } else {
                Pending::Resume
            },
        )?;
        if command != "pause" {
            self.clear_pause();
            self.phase = Phase::Running;
            self.status = "Running".into();
        }
        Ok(())
    }
    /// Select a paused thread and request its current-epoch stack.
    pub fn select_thread(&mut self, thread: i64) -> Result<()> {
        ensure!(self.phase == Phase::Paused, "Pause the program first.");
        self.clear_pause();
        self.thread = Some(thread);
        self.request(
            "stackTrace",
            json!({"threadId":thread,"startFrame":0,"levels":100}),
            Pending::Stack(self.epoch),
        )
    }
    /// Select a paused frame, invalidating superseded scopes/variables/watch requests.
    pub fn select_frame(&mut self, id: i64) -> Result<()> {
        ensure!(self.phase == Phase::Paused, "Pause the program first.");
        let navigation = self
            .frames
            .iter()
            .find(|v| v["id"].as_i64() == Some(id))
            .ok_or_else(|| anyhow::anyhow!("Unknown frame."))?
            .clone();
        self.invalidate_inspection();
        self.frame = Some(id);
        let frame = &navigation;
        if let Some(path) = frame["source"]["path"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 4096)
        {
            self.navigation = Some((
                path.into(),
                frame["line"].as_u64().unwrap_or(1).max(1) as usize,
                frame["column"].as_u64().unwrap_or(1).max(1) as usize,
            ));
        } else {
            self.navigation = None;
            self.log("This frame has no project-host source path.\n");
        }
        self.request("scopes", json!({"frameId":id}), Pending::Scopes(self.epoch))?;
        self.refresh_watches()
    }
    /// Request lazy children for a variable reference valid in the current paused epoch.
    pub fn load_variables(&mut self, reference: i64) -> Result<()> {
        if reference <= 0 || self.variables.contains_key(&reference) {
            return Ok(());
        }
        ensure!(
            self.variables.len() < 128,
            "Variable expansion limit reached; select the frame again."
        );
        let args = json!({"variablesReference":reference,"start":0,"count":200});
        self.request("variables", args, Pending::Variables(self.epoch, reference))?;
        self.variables.insert(reference, vec![]);
        Ok(())
    }
    /// Evaluate an explicit expression in the selected paused frame.
    pub fn evaluate(&mut self, expression: &str) -> Result<()> {
        ensure!(
            self.phase == Phase::Paused,
            "Pause the program to evaluate an expression."
        );
        ensure!(
            !expression.trim().is_empty() && expression.len() <= 4096,
            "Enter an expression up to 4 KiB."
        );
        self.log(&format!("> {expression}\n"));
        self.request(
            "evaluate",
            json!({"expression":expression,"frameId":self.frame,"context":"repl"}),
            Pending::Evaluate(self.epoch, None),
        )
    }
    /// Invalidate and refresh saved watches in the selected paused frame.
    pub fn refresh_watches(&mut self) -> Result<()> {
        self.cancel_requests(|pending| matches!(pending, Pending::Evaluate(_, Some(_))));
        self.watch_values.fill(String::new());
        self.watch_values
            .resize(self.settings.watches.len(), String::new());
        if self.phase != Phase::Paused || self.frame.is_none() {
            return Ok(());
        }
        for (i, expression) in self.settings.watches.clone().into_iter().enumerate() {
            self.request(
                "evaluate",
                json!({"expression":expression,"frameId":self.frame,"context":"watch"}),
                Pending::Evaluate(self.epoch, Some(i)),
            )?;
        }
        Ok(())
    }
    /// Synchronize one source's enabled breakpoints, superseding obsolete replies for that
    /// source.
    pub fn sync_breakpoints(&mut self, path: &str) -> Result<()> {
        self.set_breakpoints(path, false)
    }
    fn set_breakpoints(&mut self, path: &str, configuring: bool) -> Result<()> {
        if !self.initialized || self.phase == Phase::Ended {
            return Ok(());
        }
        let enabled: Vec<_> = self
            .settings
            .breakpoints
            .get(path)
            .into_iter()
            .flatten()
            .filter(|b| b.enabled)
            .collect();
        let values: Vec<_> = enabled
            .iter()
            .map(|bp| {
                let mut v = json!({"line":bp.line});
                for (key, text, cap) in [
                    ("condition", &bp.condition, "supportsConditionalBreakpoints"),
                    (
                        "hitCondition",
                        &bp.hit_condition,
                        "supportsHitConditionalBreakpoints",
                    ),
                    ("logMessage", &bp.log_message, "supportsLogPoints"),
                ] {
                    if !text.is_empty() {
                        if !self.supports(cap) {
                            return Err(anyhow::anyhow!("Adapter does not support {key}."));
                        }
                        v[key] = json!(text);
                    }
                }
                Ok(v)
            })
            .collect::<Result<_>>()?;
        let lines = enabled.iter().map(|b| b.line).collect();
        self.cancel_requests(
            |pending| matches!(pending,Pending::Breakpoints(source,_,false) if source==path),
        );
        let sequence = self.next;
        self.request(
            "setBreakpoints",
            json!({"source":{"path":path},"breakpoints":values,"sourceModified":false}),
            Pending::Breakpoints(path.into(), lines, configuring),
        )?;
        self.breakpoint_requests.insert(path.into(), sequence);
        self.breakpoints.remove(path);
        Ok(())
    }
    fn configured(&mut self) -> Result<()> {
        if self.configuring > 0 {
            self.configuring -= 1;
            if self.configuring > 0 {
                return Ok(());
            }
        }
        if self.supports("supportsConfigurationDoneRequest") {
            self.request("configurationDone", json!({}), Pending::ConfigurationDone)?;
        }
        Ok(())
    }
    fn handle(&mut self, message: Value) -> Result<()> {
        match message["type"].as_str() {
            Some("response") => self.handle_response(&message),
            Some("event") => self.handle_event(&message),
            _ => Ok(()),
        }
    }
    fn handle_response(&mut self, message: &Value) -> Result<()> {
        let sequence = message["request_seq"].as_u64().unwrap_or(0);
        let Some((pending, _)) = self.pending.remove(&sequence) else {
            return Ok(());
        };
        if pending.epoch().is_some_and(|e| e != self.epoch) {
            return Ok(());
        }
        let success = message["success"] == true;
        let body = &message["body"];
        if !success {
            let error = message["message"]
                .as_str()
                .unwrap_or("Debugger request failed");
            self.log(&format!("{error}\n"));
            self.error = error.chars().take(4096).collect();
            match pending {
                Pending::Initialize | Pending::Launch | Pending::ConfigurationDone => {
                    self.stop();
                }
                Pending::Evaluate(_, Some(i)) => {
                    if let Some(v) = self.watch_values.get_mut(i) {
                        *v = error.chars().take(4096).collect();
                    }
                }
                Pending::Breakpoints(path, lines, configuring) => {
                    if self.breakpoint_requests.get(&path) == Some(&sequence) {
                        self.breakpoints.insert(
                            path,
                            lines
                                .into_iter()
                                .map(|line| Verified {
                                    line,
                                    verified: false,
                                    message: error.into(),
                                    id: None,
                                })
                                .collect(),
                        );
                    }
                    if configuring {
                        self.configured()?;
                    }
                }
                Pending::Configure => self.configured()?,
                // A refused step leaves the target stopped; ask for its state again.
                Pending::Resume if self.phase == Phase::Running => {
                    self.phase = Phase::Paused;
                    self.status = "Paused · step rejected".into();
                    self.status = "Paused · step rejected".into();
                    self.request("threads", json!({}), Pending::Threads(self.epoch))?;
                }
                _ => {}
            }
            return Ok(());
        }
        match pending {
            Pending::Initialize => {
                self.capabilities = body.clone();
                let command = if self.settings.config.attach {
                    "attach"
                } else {
                    "launch"
                };
                self.request(command, self.arguments.clone(), Pending::Launch)?;
            }
            Pending::Launch => {
                if self.phase == Phase::Starting {
                    self.phase = Phase::Running;
                    self.status = "Running".into();
                }
            }
            Pending::Configure => self.configured()?,
            Pending::Breakpoints(path, lines, configuring) => {
                let replies = body["breakpoints"].as_array();
                let items: Vec<_> = lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, line)| {
                        let b = replies.and_then(|a| a.get(i));
                        Verified {
                            line: b.and_then(|v| v["line"].as_u64()).unwrap_or(line as u64)
                                as usize,
                            verified: b.is_some_and(|v| v["verified"] == true),
                            message: b
                                .and_then(|v| v["message"].as_str())
                                .unwrap_or("")
                                .chars()
                                .take(4096)
                                .collect(),
                            id: b.and_then(|v| v["id"].as_i64()),
                        }
                    })
                    .collect();
                if self.breakpoint_requests.get(&path) == Some(&sequence) {
                    if items.is_empty() {
                        self.breakpoints.remove(&path);
                        self.breakpoint_requests.remove(&path);
                    } else {
                        self.breakpoints.insert(path, items);
                    }
                }
                if configuring {
                    self.configured()?;
                }
            }
            Pending::PauseThreads(_) => {
                if let Some(id) = array(body, "threads", 128)
                    .first()
                    .and_then(|t| t["id"].as_i64())
                {
                    self.thread = Some(id);
                    self.request("pause", json!({"threadId":id}), Pending::Pause)?;
                }
            }
            Pending::Threads(_) => {
                self.threads = array(body, "threads", 128);
                if let Some(id) = self
                    .thread
                    .filter(|id| self.threads.iter().any(|t| t["id"].as_i64() == Some(*id)))
                    .or_else(|| self.threads.first().and_then(|v| v["id"].as_i64()))
                {
                    self.thread = Some(id);
                    self.request(
                        "stackTrace",
                        json!({"threadId":id,"startFrame":0,"levels":100}),
                        Pending::Stack(self.epoch),
                    )?;
                }
            }
            Pending::Stack(_) => {
                self.frames = array(body, "stackFrames", 100);
                if let Some(id) = self.frames.first().and_then(|v| v["id"].as_i64()) {
                    self.select_frame(id)?;
                }
            }
            Pending::Scopes(_) => {
                self.scopes = array(body, "scopes", 32);
                let references: Vec<_> = self
                    .scopes
                    .iter()
                    .filter(|s| s["expensive"] != true)
                    .filter_map(|s| s["variablesReference"].as_i64())
                    .take(4)
                    .collect();
                for id in references {
                    self.load_variables(id)?;
                }
            }
            Pending::Variables(_, id) => {
                self.variables.insert(id, array(body, "variables", 200));
            }
            Pending::Evaluate(_, index) => {
                let result = body["result"].as_str().unwrap_or("");
                if let Some(i) = index {
                    if let Some(v) = self.watch_values.get_mut(i) {
                        *v = result.chars().take(4096).collect();
                    }
                } else {
                    self.log(&format!("{result}\n"));
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn handle_event(&mut self, message: &Value) -> Result<()> {
        let body = &message["body"];
        match message["event"].as_str().unwrap_or("") {
            "rodeReady" => {
                self.arguments = body["arguments"].clone();
                let capabilities = json!({
                    "clientID":"rode",
                    "clientName":"RODE",
                    "adapterID":"rode",
                    "linesStartAt1":true,
                    "columnsStartAt1":true,
                    "pathFormat":"path",
                    "supportsVariableType":true,
                    "supportsVariablePaging":true,
                    "supportsRunInTerminalRequest":true,
                    "supportsArgsCanBeInterpretedByShell":false,
                    "supportsProgressReporting":false,
                    "supportsStartDebuggingRequest":false
                });
                self.request("initialize", capabilities, Pending::Initialize)?;
            }
            "rodeReverseResponse" => {
                let mut reply = body.clone();
                reply["seq"] = json!(self.sequence());
                ensure!(self.queue.len() < MAX_REQUESTS, "Debugger queue is full.");
                let sequence = reply["seq"].as_u64().unwrap();
                self.queue.push_back(Outgoing {
                    sequence,
                    bytes: process::encode_request(&serde_json::to_value(Command::Dap {
                        message: reply,
                    })?)?,
                });
            }
            "initialized" => {
                self.initialized = true;
                let paths: Vec<_> = self.settings.breakpoints.keys().cloned().collect();
                self.configuring = paths.len() + 1;
                for path in paths {
                    self.set_breakpoints(&path, true)?;
                }
                let filters: Vec<_> = self
                    .settings
                    .exceptions
                    .iter()
                    .filter(|id| body_filter(&self.capabilities, id))
                    .cloned()
                    .collect();
                self.request(
                    "setExceptionBreakpoints",
                    json!({"filters":filters}),
                    Pending::Configure,
                )?;
            }
            "stopped" => {
                self.clear_pause();
                self.phase = Phase::Paused;
                self.status = format!(
                    "Paused · {}",
                    body["description"]
                        .as_str()
                        .or_else(|| body["reason"].as_str())
                        .unwrap_or("breakpoint")
                );
                self.thread = body["threadId"].as_i64();
                self.request("threads", json!({}), Pending::Threads(self.epoch))?;
            }
            "continued" => {
                self.clear_pause();
                self.phase = Phase::Running;
                self.status = "Running".into();
            }
            "debugpyWaitingForServer" => {
                self.status = format!(
                    "Waiting for debug server · {}:{}",
                    body["host"].as_str().unwrap_or("host"),
                    body["port"]
                );
            }
            "thread" => {
                if self.thread.is_none() {
                    self.thread = body["threadId"].as_i64();
                }
            }
            "output" => {
                if let Some(text) = body["output"].as_str() {
                    self.log(text);
                }
            }
            "terminated" => {
                self.status = "Debug session ended".into();
                self.phase = Phase::Ended;
                self.clear_pause();
                self.pending.clear();
                self.terminated = Some(Instant::now());
            }
            "exited" => {
                self.log(&format!("Program exited ({})\n", body["exitCode"]));
            }
            "capabilities" => {
                if let (Some(old), Some(new)) = (
                    self.capabilities.as_object_mut(),
                    body["capabilities"].as_object(),
                ) {
                    old.extend(new.clone());
                }
            }
            "breakpoint" => {
                let bp = &body["breakpoint"];
                for values in self.breakpoints.values_mut() {
                    for value in values {
                        if value.id.is_some() && value.id == bp["id"].as_i64() {
                            value.verified = bp["verified"] == true;
                            if let Some(line) = bp["line"].as_u64() {
                                value.line = line as usize;
                            }
                            value.message = bp["message"]
                                .as_str()
                                .unwrap_or("")
                                .chars()
                                .take(4096)
                                .collect();
                        }
                    }
                }
            }
            "invalidated" if self.phase == Phase::Paused => {
                if let Some(id) = self.thread {
                    self.select_thread(id)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Drain bounded coordinator/DAP work, retry queued writes and handle protocol deadlines
    /// without blocking.
    pub fn poll(&mut self) {
        if self
            .terminated
            .is_some_and(|t| t.elapsed() > Duration::from_millis(300))
        {
            self.stop();
        }
        for _ in 0..64 {
            let Some(event) = self.session.as_ref().and_then(process::Session::poll) else {
                break;
            };
            let result = match event {
                process::Event::Message(v) => match serde_json::from_value::<Event>(v) {
                    Ok(Event::Dap { message }) => self.handle(message),
                    Ok(Event::Error { message }) => Err(anyhow::anyhow!(message)),
                    Ok(event) => {
                        if self.terminal_events.len() < 128 {
                            self.terminal_events.push_back(event);
                        } else {
                            self.error = "Debug terminal output overflow".into();
                        }
                        Ok(())
                    }
                    Err(e) => Err(e.into()),
                },
                process::Event::Stderr(text) => {
                    self.log(&text);
                    Ok(())
                }
                process::Event::Error(e) => Err(anyhow::anyhow!(e)),
                process::Event::Exited(code) => {
                    if self.phase != Phase::Ended {
                        self.log(&format!("Adapter closed ({code:?}).\n"));
                    }
                    self.stop();
                    Ok(())
                }
                process::Event::Started => Ok(()),
            };
            if let Err(e) = result {
                self.error = format!("{e:#}");
                self.log(&format!("{}\n", self.error.clone()));
                self.stop();
                break;
            }
        }
        if self.phase == Phase::Starting && self.started.elapsed() > Duration::from_secs(90) {
            self.error =
                "Debugger startup timed out. Check the installed adapter and configuration.".into();
            self.stop();
        }
        if let Some(session) = &self.session {
            for _ in 0..32 {
                let Some(message) = self.queue.front_mut() else {
                    break;
                };
                match session.writer().try_send_buffer(&mut message.bytes) {
                    Ok(true) => {
                        self.queue.pop_front();
                    }
                    Ok(false) => break,
                    Err(e) => {
                        self.error = e.to_string();
                        self.stop();
                        break;
                    }
                }
            }
        }
        if self
            .pending
            .values()
            .any(|(p, t)| !matches!(p, Pending::Launch) && t.elapsed() > Duration::from_secs(60))
        {
            self.error = "Debugger request timed out.".into();
            self.stop();
        }
    }
    /// Begin idempotent asynchronous disconnect: launched targets stop, attached targets
    /// detach, and owned coordinator descendants are cleaned up.
    pub fn stop(&mut self) {
        if self.phase != Phase::Ended {
            self.status = if self.settings.config.attach {
                "Detached from program"
            } else {
                "Debug session stopped"
            }
            .into();
        }
        self.terminated = None;
        self.phase = Phase::Ended;
        self.clear_pause();
        self.pending.clear();
        self.queue.clear();
        if let Some(session) = self.session.take() {
            let closing = self.closing.clone();
            closing.store(true, Ordering::Release);
            let seq = self.sequence();
            let terminate = !self.settings.config.attach;
            super::shutdown::start(session, seq, terminate, closing);
        }
    }
}
impl Default for Client {
    fn default() -> Self {
        let mut client = Self::new(None, Settings::default());
        client.phase = Phase::Ended;
        client.status = "Ready".into();
        client
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stop();
    }
}
fn array(body: &Value, key: &str, limit: usize) -> Vec<Value> {
    fn bound(v: &Value, depth: usize) -> Value {
        if depth > 8 {
            return Value::Null;
        }
        match v {
            Value::String(s) => json!(s.chars().take(4096).collect::<String>()),
            Value::Array(a) => {
                Value::Array(a.iter().take(32).map(|v| bound(v, depth + 1)).collect())
            }
            Value::Object(o) => Value::Object(
                o.iter()
                    .take(32)
                    .map(|(k, v)| (k.chars().take(128).collect(), bound(v, depth + 1)))
                    .collect(),
            ),
            v => v.clone(),
        }
    }
    let mut out = vec![];
    let mut bytes = 0;
    for v in body[key].as_array().into_iter().flatten().take(limit) {
        let value = bound(v, 0);
        bytes += serde_json::to_vec(&value).map_or(0, |v| v.len());
        if bytes > 128 * 1024 {
            break;
        }
        out.push(value);
    }
    out
}
fn body_filter(cap: &Value, id: &str) -> bool {
    cap["exceptionBreakpointFilters"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v["filter"] == id))
}

#[cfg(test)]
mod tests;
