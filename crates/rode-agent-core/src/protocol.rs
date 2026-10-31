//! Small Codex app-server / ACP v1 clients, sharing one bounded chat model.
use super::{Access, Approval, Config, Event, Launch, PROMPT_LIMIT, Provider, Session, controls};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

/// Presentation role for one bounded in-memory chat item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// A prompt submitted by the user.
    User,
    /// An agent response, including streamed text.
    Assistant,
    /// An agent tool-call/result item.
    Tool,
    /// A local lifecycle or protocol explanation.
    Notice,
}
/// One in-memory transcript item; never persisted as preferences.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    /// Protocol/stable item identifier used to merge streamed updates.
    pub id: String,
    /// Presentation role and styling.
    pub role: Role,
    /// Short speaker/tool/status label.
    pub title: String,
    /// Bounded accumulated body text.
    pub text: String,
}
/// One protocol-supplied permission response choice.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Choice {
    /// Human-readable decision label.
    pub label: String,
    /// JSON result sent when this choice is selected.
    pub result: Value,
}
/// An agent question awaiting an explicit user answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Question {
    /// Protocol question identifier.
    pub id: String,
    /// Human-readable question text.
    pub text: String,
    /// Suggested answers supplied by the agent.
    pub options: Vec<String>,
    /// Current unsubmitted answer draft.
    pub answer: String,
    /// Whether the answer field should obscure its text.
    pub secret: bool,
}
/// Queued permission/question card; responses cannot widen the outer project sandbox.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prompt {
    /// Original JSON-RPC request identifier.
    pub id: Value,
    /// Short prompt heading.
    pub title: String,
    /// Compact description for the approval card.
    pub summary: String,
    /// Bounded full request details for inspection.
    pub detail: String,
    /// Available explicit permission decisions.
    pub choices: Vec<Choice>,
    /// Questions to include in the reply.
    pub questions: Vec<Question>,
    cancel: Value,
}
#[derive(Clone)]
enum Call {
    Initialize,
    Conversation,
    Turn,
    Interrupt,
    Models,
    Configure {
        id: String,
        value: Value,
        legacy: bool,
    },
}

/// Lazy Codex/ACP conversation controller with bounded messages, prompts and pending calls.
/// Processes start only on submission and belong to the current host/project context.
#[derive(Default)]
pub struct Chat {
    /// Bounded in-memory transcript in display order.
    pub messages: VecDeque<Message>,
    /// Bounded queue of unanswered permission/question cards.
    pub prompts: VecDeque<Prompt>,
    /// Current human-readable session/turn status.
    pub status: String,
    /// Whether a turn or initial pending message is active.
    pub busy: bool,
    /// Completed-turn counter used to trigger clean-buffer refresh.
    pub completed: u64,
    /// Whether the last turn finished successfully, without cancellation.
    pub last_turn_succeeded: bool,
    /// Whether displayed chat content changed since the UI last observed it.
    pub changed: bool,
    /// Models advertised by the current Codex process; empty before the first message.
    pub models: Vec<controls::Model>,
    /// Capability-driven ACP controls and their acknowledged backend values.
    pub controls: Vec<controls::Control>,
    session: Option<Session>,
    config: Config,
    root: String,
    read_only: bool,
    external: bool,
    network: bool,
    conversation: String,
    turn: String,
    next_id: u64,
    calls: HashMap<u64, Call>,
    pending: Option<String>,
    cancelling: bool,
    deadline: Option<Instant>,
    last_stderr: String,
    serial: u64,
    model_cursors: Vec<String>,
    settings_attempts: usize,
    /// A restored/styled context needs an explicit normal-tone hint when Default is selected.
    response_style_context: bool,
}

impl Chat {
    /// Copy the bounded presentation state, never process handles or credentials.
    pub fn view(&self) -> crate::service::ChatView {
        crate::service::ChatView {
            messages: self.messages.clone(),
            prompts: self.prompts.clone(),
            status: self.status.clone(),
            busy: self.busy,
            completed: self.completed,
            last_turn_succeeded: self.last_turn_succeeded,
            models: self.models.clone(),
            controls: self.controls.clone(),
            connected: self.connected(),
            stopping: self.stopping(),
            config: self.config.clone(),
        }
    }
    /// Update a remote view without creating a process. Preserve unsubmitted
    /// question drafts when another streamed update arrives for the same card.
    pub fn apply_view(&mut self, mut view: crate::service::ChatView) {
        for prompt in &mut view.prompts {
            if let Some(old) = self.prompts.iter().find(|old| old.id == prompt.id) {
                for question in &mut prompt.questions {
                    if let Some(previous) = old.questions.iter().find(|q| q.id == question.id) {
                        question.answer = previous.answer.clone();
                    }
                }
            }
        }
        self.messages = view.messages;
        self.prompts = view.prompts;
        self.status = view.status;
        self.busy = view.busy;
        self.completed = view.completed;
        self.last_turn_succeeded = view.last_turn_succeeded;
        self.models = view.models;
        self.controls = view.controls;
        self.config = view.config;
        self.changed = true;
    }
    /// Adopt an acknowledged history restore, replacing only this chat's process.
    /// Current project policy was captured before the backend resumed the thread.
    pub fn restore(&mut self, restored: super::history::Restored) {
        self.disconnect();
        self.config = restored.launch.config;
        self.root = restored.launch.plan.root;
        self.read_only = restored.launch.plan.project_read_only
            || (self.config.provider != Provider::Acp && self.config.access() == Access::ReadOnly);
        self.external = restored.launch.plan.active && !self.read_only;
        self.network = restored
            .launch
            .plan
            .prefix
            .iter()
            .any(|arg| arg == "--share-net");
        self.conversation = restored.id;
        self.messages = restored.messages;
        self.controls = restored.controls;
        self.next_id = restored.next_id;
        self.session = Some(restored.session);
        self.response_style_context = true;
        self.status = "Ready · restored conversation".into();
        self.changed = true;
        if restored.limited {
            self.notice(
                "Showing recent saved messages; the agent retains the full conversation context.",
            );
        }
        if self.config.provider == Provider::Codex {
            let _ = self.call(
                "model/list",
                json!({"limit":32,"includeHidden":false}),
                Call::Models,
            );
        }
    }
    /// Return whether a transport and initialized conversation are present.
    pub fn connected(&self) -> bool {
        self.session.is_some() && !self.conversation.is_empty()
    }
    /// Return whether cancellation is awaiting completion or escalation.
    pub fn stopping(&self) -> bool {
        self.cancelling
    }

    /// Whether startup, setting acknowledgement or cancellation needs a timed repaint.
    pub fn has_deadline(&self) -> bool {
        self.deadline.is_some()
    }

    /// Whether cached capabilities belong to the currently selected executable/provider.
    pub fn controls_match(&self, config: &Config) -> bool {
        self.config.same_process(config)
    }

    /// Apply advertised ACP choices to an idle session without submitting a prompt.
    /// New processes are still started only by an explicit chat message.
    pub fn configure_idle(&mut self, config: &Config) -> Result<()> {
        if config.provider != Provider::Acp
            || !self.connected()
            || self.busy
            || !self.config.same_process(config)
        {
            return Ok(());
        }
        self.config = config.clone();
        self.settings_attempts = 0;
        self.busy = true;
        if let Err(error) = self.begin_turn() {
            self.disconnect();
            return Err(error);
        }
        Ok(())
    }

    /// Validate and submit a bounded user message, starting the configured host-scoped process
    /// only when needed. Changing configuration ends the previous conversation.
    pub fn submit(
        &mut self,
        launch: Launch,
        text: String,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<()> {
        ensure!(
            !self.busy,
            "The agent is still working. Stop it or wait for its reply."
        );
        ensure!(!text.trim().is_empty(), "Enter a message.");
        ensure!(
            text.len() <= PROMPT_LIMIT,
            "Agent messages are limited to 32 KiB."
        );
        let bundled = launch.config.argv()?[0] == crate::AUTO_CODEX;
        let reset_defaults = (self.config.model() != launch.config.model()
            && launch.config.model().is_empty())
            || (self.config.reasoning_effort != launch.config.reasoning_effort
                && launch.config.reasoning_effort.is_empty());
        if self.session.is_some() && (!self.config.same_process(&launch.config) || reset_defaults) {
            self.disconnect();
            self.notice("Agent settings changed · starting a new conversation");
        }
        self.read_only = launch.plan.project_read_only
            || (launch.config.provider != Provider::Acp
                && launch.config.access() == Access::ReadOnly);
        self.external = launch.plan.active && !self.read_only;
        self.network = launch.plan.prefix.iter().any(|arg| arg == "--share-net");
        self.config = launch.config.clone();
        if self.session.is_none() {
            self.root = launch.plan.root.clone();
            // A first SSH start may stream a large native runtime. Keep this
            // bounded separately from the ordinary protocol handshake; Stop
            // still cancels preparation through the owned process transport.
            let preparing_remote_bundle = bundled && launch.remote;
            self.session = Some(Session::start(launch, wake)?);
            self.status = if preparing_remote_bundle {
                "Preparing Codex on SSH host…"
            } else {
                "Starting agent…"
            }
            .into();
            self.deadline = Some(
                Instant::now()
                    + Duration::from_secs(if preparing_remote_bundle { 420 } else { 60 }),
            );
        }
        self.serial += 1;
        self.push(
            format!("user-{}", self.serial),
            Role::User,
            "You".into(),
            text.clone(),
        );
        self.pending = Some(text);
        self.settings_attempts = 0;
        self.busy = true;
        self.cancelling = false;
        self.last_turn_succeeded = false;
        if !self.conversation.is_empty()
            && let Err(error) = self.begin_turn()
        {
            self.disconnect();
            return Err(error);
        }
        Ok(())
    }

    /// Process a bounded batch of available transport events and deadlines without blocking the
    /// UI.
    pub fn poll(&mut self) {
        for _ in 0..64 {
            let Some(event) = self.session.as_ref().and_then(Session::poll) else {
                break;
            };
            let result = match event {
                Event::Started => self.initialize(),
                Event::Message(value) => self.receive(value),
                Event::Stderr(line) => {
                    append_bounded(&mut self.last_stderr, &(line + "\n"), 8192);
                    Ok(())
                }
                Event::Error(error) => {
                    self.fail(&error);
                    Ok(())
                }
                Event::Exited(code) => {
                    let working = self.busy;
                    self.completed += u64::from(working);
                    let detail = self.last_stderr.trim().to_owned();
                    self.disconnect();
                    self.status = "Disconnected".into();
                    if working || code != Some(0) {
                        self.notice(&format!(
                            "Agent exited{}{}",
                            code.map(|c| format!(" ({c})")).unwrap_or_default(),
                            if detail.is_empty() {
                                String::new()
                            } else {
                                format!(": {detail}")
                            }
                        ));
                    }
                    Ok(())
                }
            };
            if let Err(error) = result {
                self.fail(&format!("{error:#}"));
            }
        }
        if self.deadline.is_some_and(|time| Instant::now() >= time) {
            let message = if self.cancelling {
                "Agent did not stop in time; its process was closed."
            } else {
                "Agent startup timed out. Check its protocol, executable, and authentication on this host."
            };
            self.fail(message);
        }
    }

    /// Reply to the oldest outstanding agent prompt with an explicit protocol result.
    pub fn answer(&mut self, result: Value) -> Result<()> {
        if let Some(prompt) = self.prompts.front() {
            self.send(json!({"jsonrpc":"2.0", "id":prompt.id, "result":result}))?;
            self.prompts.pop_front();
            self.status = if self.prompts.is_empty() {
                "Working…".into()
            } else {
                "Waiting for you".into()
            };
        }
        Ok(())
    }

    /// Request cancellation of the current turn; timeout handling may stop the owned process.
    pub fn cancel(&mut self) {
        if !self.busy {
            return;
        }
        if self
            .calls
            .values()
            .any(|call| matches!(call, Call::Configure { .. }))
        {
            self.disconnect();
            self.status = "Stopped".into();
            return;
        }
        self.cancelling = true;
        self.status = "Stopping…".into();
        self.deadline = Some(Instant::now() + Duration::from_secs(3));
        let prompts = std::mem::take(&mut self.prompts);
        for prompt in prompts {
            let _ = self.send(json!({"jsonrpc":"2.0", "id":prompt.id, "result":prompt.cancel}));
        }
        let result = if self.conversation.is_empty() {
            self.disconnect();
            self.status = "Stopped".into();
            Ok(())
        } else if self.config.provider == Provider::Acp {
            self.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":self.conversation}}))
        } else if !self.turn.is_empty() {
            self.interrupt()
        } else {
            Ok(())
        };
        if let Err(error) = result {
            self.fail(&error.to_string());
        }
    }

    /// Stop the owned agent process and reset protocol state while retaining the bounded
    /// transcript.
    pub fn disconnect(&mut self) {
        self.session = None;
        self.busy = false;
        self.last_turn_succeeded = false;
        self.conversation.clear();
        self.turn.clear();
        self.calls.clear();
        self.pending = None;
        self.prompts.clear();
        self.deadline = None;
        self.cancelling = false;
        self.last_stderr.clear();
        self.models.clear();
        self.controls.clear();
        self.model_cursors.clear();
        self.response_style_context = false;
    }

    fn initialize(&mut self) -> Result<()> {
        self.deadline = Some(Instant::now() + Duration::from_secs(60));
        let params = if self.config.provider == Provider::Acp {
            json!({"protocolVersion":1,"clientInfo":{"name":"rode","title":"RODE","version":env!("CARGO_PKG_VERSION")},
                "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false,"session":{"configOptions":{"boolean":true}}}})
        } else {
            json!({"clientInfo":{"name":"rode","title":"RODE","version":env!("CARGO_PKG_VERSION")}})
        };
        self.call("initialize", params, Call::Initialize)
    }
    fn begin_turn(&mut self) -> Result<()> {
        if self.cancelling {
            return Ok(());
        }
        if self.config.provider == Provider::Acp {
            ensure!(
                self.config
                    .acp_options
                    .keys()
                    .all(|id| self.controls.iter().any(|c| c.id == *id)),
                "Agent no longer exposes a saved option. Reset agent controls in settings before sending."
            );
            // Model changes may alter reasoning options. Acknowledge them first,
            // then validate dependent choices against the returned capabilities.
            for control in self
                .controls
                .iter()
                .filter(|c| c.category == "model")
                .chain(self.controls.iter().filter(|c| c.category != "model"))
            {
                let id = &control.id;
                let Some(value) = self.config.acp_options.get(id) else {
                    continue;
                };
                ensure!(
                    control.accepts(value),
                    "Saved agent option {id} is unavailable. Reset agent controls in settings."
                );
                if control.current != *value {
                    ensure!(
                        self.settings_attempts < controls::CONTROL_LIMIT * 2,
                        "Agent kept changing the selected settings. No prompt was sent."
                    );
                    self.settings_attempts += 1;
                    let mut params = json!({"sessionId":self.conversation});
                    let legacy = control.legacy;
                    let method = if legacy {
                        params["modeId"] = value.clone();
                        "session/set_mode"
                    } else {
                        params["configId"] = id.clone().into();
                        params["value"] = value.clone();
                        if control.boolean {
                            params["type"] = "boolean".into();
                        }
                        "session/set_config_option"
                    };
                    self.status = "Applying agent controls…".into();
                    self.deadline = Some(Instant::now() + Duration::from_secs(30));
                    return self.call(
                        method,
                        params,
                        Call::Configure {
                            id: id.clone(),
                            value: value.clone(),
                            legacy,
                        },
                    );
                }
            }
        }
        let Some(text) = self.pending.take() else {
            self.busy = false;
            self.deadline = None;
            self.status = "Ready".into();
            return Ok(());
        };
        self.status = "Working…".into();
        self.deadline = None;
        self.turn.clear();
        let mut input = vec![json!({"type":"text","text":text})];
        if let Some(hint) = self.config.personality.hint(self.response_style_context) {
            input.push(json!({"type":"text","text":format!("\n\n{hint}")}));
            self.response_style_context = true;
        }
        if self.config.provider == Provider::Acp {
            self.call(
                "session/prompt",
                json!({"sessionId":self.conversation,"prompt":input}),
                Call::Turn,
            )
        } else {
            let mut params = json!({"threadId":self.conversation,"input":input});
            params["approvalPolicy"] = if self.read_only {
                "never"
            } else {
                self.config.approval.codex()
            }
            .into();
            if !self.config.model().is_empty() {
                params["model"] = self.config.model().into();
            }
            if !self.config.reasoning_effort.is_empty() {
                params["effort"] = self.config.reasoning_effort.clone().into();
            }
            if self.external {
                // bwrap already enforces the project policy for this process
                // and every child. Tell Codex to use that external sandbox.
                params["sandboxPolicy"] = json!({"type":"externalSandbox","networkAccess":if self.network {"enabled"} else {"restricted"}});
            } else {
                params["sandboxPolicy"] = if self.read_only {
                    json!({"type":"readOnly"})
                } else if self.config.access() == Access::Full {
                    json!({"type":"dangerFullAccess"})
                } else {
                    json!({"type":"workspaceWrite","writableRoots":[self.root],"networkAccess":false})
                };
            }
            self.call("turn/start", params, Call::Turn)
        }
    }
    fn interrupt(&mut self) -> Result<()> {
        self.call(
            "turn/interrupt",
            json!({"threadId":self.conversation,"turnId":self.turn}),
            Call::Interrupt,
        )
    }
    fn call(&mut self, method: &str, params: Value, kind: Call) -> Result<()> {
        ensure!(
            self.calls.len() < 64,
            "Agent has too many unanswered requests."
        );
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        self.calls.insert(id, kind);
        Ok(())
    }
    fn send(&self, value: Value) -> Result<()> {
        self.session
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Agent is disconnected"))?
            .send(&value)
    }

    fn receive(&mut self, value: Value) -> Result<()> {
        if let Some(method) = value["method"].as_str() {
            let params = &value["params"];
            if let Some(id) = value.get("id") {
                return self.request(method, id.clone(), params);
            }
            if self.config.provider == Provider::Acp {
                if method == "session/update"
                    && params["sessionId"].as_str() == Some(&self.conversation)
                {
                    self.acp_update(&params["update"]);
                }
            } else if params
                .get("threadId")
                .and_then(Value::as_str)
                .is_none_or(|id| id == self.conversation)
            {
                self.codex_update(method, params)?;
            }
            return Ok(());
        }
        let Some(id) = value["id"].as_u64() else {
            return Ok(());
        };
        let Some(call) = self.calls.remove(&id) else {
            return Ok(());
        };
        if let Some(error) = value.get("error") {
            if matches!(call, Call::Models) {
                self.notice("This agent does not provide a model catalog. Enter a model ID in Model controls.");
                return Ok(());
            }
            // A completed turn may race its interrupt response. Keep the
            // conversation; the cancellation deadline handles an unresponsive agent.
            if matches!(call, Call::Interrupt) {
                return Ok(());
            }
            anyhow::bail!(
                "Agent: {}",
                error["message"].as_str().unwrap_or("request failed")
            );
        }
        let result = &value["result"];
        match call {
            Call::Initialize => {
                if self.config.provider == Provider::Acp {
                    ensure!(
                        result["protocolVersion"].as_u64() == Some(1),
                        "Agent does not support ACP v1."
                    );
                    self.call(
                        "session/new",
                        json!({"cwd":self.root,"mcpServers":[]}),
                        Call::Conversation,
                    )?;
                } else {
                    self.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
                    if self.config.provider == Provider::Codex {
                        self.call(
                            "model/list",
                            json!({"limit":32,"includeHidden":false}),
                            Call::Models,
                        )?;
                    }
                    let access = if self.read_only {
                        Access::ReadOnly
                    } else {
                        self.config.access()
                    };
                    let mut params = json!({"cwd":self.root,"approvalPolicy":if self.read_only {"never"} else {self.config.approval.codex()},"sandbox":access.codex()});
                    if !self.config.model().is_empty() {
                        params["model"] = self.config.model().into();
                    }
                    if self.config.provider == Provider::Local {
                        params["modelProvider"] = "rode_local".into();
                    }
                    self.call("thread/start", params, Call::Conversation)?;
                }
            }
            Call::Conversation => {
                let id = if self.config.provider == Provider::Acp {
                    &result["sessionId"]
                } else {
                    &result["thread"]["id"]
                };
                self.conversation = id
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 256)
                    .context("Agent returned no conversation ID")?
                    .into();
                if self.config.provider == Provider::Acp {
                    self.controls = controls::acp(result);
                }
                self.begin_turn()?;
            }
            Call::Turn => {
                if self.config.provider == Provider::Acp {
                    self.finish(result["stopReason"].as_str() != Some("cancelled"), None);
                } else if self.busy {
                    self.turn = result["turn"]["id"]
                        .as_str()
                        .context("Agent returned no turn ID")?
                        .into();
                    if self.cancelling {
                        self.interrupt()?;
                    }
                }
            }
            Call::Interrupt => {}
            Call::Models => {
                for model in controls::models(result) {
                    if self.models.len() >= 128 {
                        break;
                    }
                    if !self.models.iter().any(|m| m.value.id == model.value.id) {
                        self.models.push(model);
                    }
                }
                if let Some(cursor) = result["nextCursor"]
                    .as_str()
                    .filter(|s| controls::identifier(s, false))
                    && self.model_cursors.len() < 3
                    && !self.model_cursors.iter().any(|s| s == cursor)
                    && self.models.len() < 128
                {
                    self.model_cursors.push(cursor.into());
                    self.call(
                        "model/list",
                        json!({"limit":32,"includeHidden":false,"cursor":cursor}),
                        Call::Models,
                    )?;
                }
            }
            Call::Configure { id, value, legacy } => {
                if self.cancelling {
                    self.finish(false, None);
                    return Ok(());
                }
                if legacy {
                    if let Some(control) = self.controls.iter_mut().find(|c| c.id == id) {
                        control.current = value.clone();
                    }
                } else {
                    self.controls = controls::acp(result);
                }
                ensure!(
                    self.controls
                        .iter()
                        .any(|c| c.id == id && c.current == value),
                    "Agent did not acknowledge the selected setting. No prompt was sent."
                );
                self.begin_turn()?;
            }
        }
        Ok(())
    }

    fn codex_update(&mut self, method: &str, params: &Value) -> Result<()> {
        let item = &params["item"];
        match method {
            "turn/started" => {
                self.turn = params["turn"]["id"].as_str().unwrap_or_default().into();
            }
            "turn/completed" => {
                let status = params["turn"]["status"].as_str().unwrap_or("failed");
                self.finish(
                    status == "completed",
                    params["turn"]["error"]["message"].as_str(),
                );
            }
            "item/agentMessage/delta" => {
                self.append(
                    params["itemId"].as_str().unwrap_or("reply"),
                    Role::Assistant,
                    "Agent",
                    params["delta"].as_str().unwrap_or_default(),
                );
            }
            "item/commandExecution/outputDelta" => {
                self.append(
                    params["itemId"].as_str().unwrap_or("tool"),
                    Role::Tool,
                    "Command",
                    params["delta"].as_str().unwrap_or_default(),
                );
            }
            "item/started" | "item/completed" => {
                let id = item["id"].as_str().unwrap_or("item");
                match item["type"].as_str().unwrap_or_default() {
                    "agentMessage" => {
                        if let Some(text) = item["text"].as_str() {
                            self.replace(id, Role::Assistant, "Agent", text);
                        }
                    }
                    "commandExecution" => {
                        let title = item["command"].as_str().unwrap_or("Command");
                        let text = item["aggregatedOutput"].as_str().unwrap_or_default();
                        if method == "item/completed" {
                            self.replace(id, Role::Tool, title, text);
                        } else {
                            self.append(id, Role::Tool, title, "");
                        }
                    }
                    "fileChange" => {
                        self.replace(id, Role::Tool, "File changes", &pretty(&item["changes"]));
                        self.changed = true;
                    }
                    "mcpToolCall" | "dynamicToolCall" => self.replace(
                        id,
                        Role::Tool,
                        item["tool"].as_str().unwrap_or("Tool"),
                        &pretty(item),
                    ),
                    _ => {}
                }
            }
            "serverRequest/resolved" => {
                self.prompts
                    .retain(|prompt| prompt.id != params["requestId"]);
            }
            "error" if !params["willRetry"].as_bool().unwrap_or(false) => {
                self.notice(
                    params["error"]["message"]
                        .as_str()
                        .unwrap_or("Agent reported an error"),
                );
            }
            _ => {}
        }
        Ok(())
    }

    fn acp_update(&mut self, update: &Value) {
        match update["sessionUpdate"].as_str().unwrap_or_default() {
            "agent_message_chunk" => {
                if let Some(text) = update["content"]["text"].as_str() {
                    let id = self
                        .messages
                        .back()
                        .filter(|m| m.role == Role::Assistant)
                        .map(|m| m.id.clone())
                        .unwrap_or_else(|| {
                            format!("reply-{}-{}", self.serial, self.messages.len())
                        });
                    self.append(&id, Role::Assistant, "Agent", text);
                }
            }
            "tool_call" | "tool_call_update" => {
                let id = update["toolCallId"].as_str().unwrap_or("tool");
                let title = update["title"].as_str().unwrap_or("Tool");
                self.replace(id, Role::Tool, title, &pretty(update));
                self.changed = true;
            }
            "config_option_update" => {
                self.controls = controls::acp(update);
            }
            "current_mode_update" => {
                if let Some(control) = self.controls.iter_mut().find(|c| c.legacy)
                    && control.accepts(&update["currentModeId"])
                {
                    control.current = update["currentModeId"].clone();
                }
            }
            _ => {}
        }
    }

    fn request(&mut self, method: &str, id: Value, params: &Value) -> Result<()> {
        if let Some(scope) = params
            .get(if self.config.provider == Provider::Acp {
                "sessionId"
            } else {
                "threadId"
            })
            .and_then(Value::as_str)
            && scope != self.conversation
        {
            return self.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Request belongs to another conversation"}}));
        }
        let mut prompt = Prompt {
            id: id.clone(),
            title: "Agent permission".into(),
            summary: clip(
                params["command"]
                    .as_str()
                    .or_else(|| params["toolCall"]["title"].as_str())
                    .or_else(|| params["reason"].as_str())
                    .unwrap_or_default(),
                2048,
            ),
            detail: clip(&pretty(params), 16384),
            choices: vec![],
            questions: vec![],
            cancel: json!({"decision":"cancel"}),
        };
        let permission = matches!(
            method,
            "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "item/permissions/requestApproval"
                | "session/request_permission"
        );
        match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                prompt.title = if method.contains("commandExecution") {
                    "Run command?"
                } else {
                    "Apply file changes?"
                }
                .into();
                prompt.choices = vec![
                    Choice {
                        label: "Allow once".into(),
                        result: json!({"decision":"accept"}),
                    },
                    Choice {
                        label: "Decline".into(),
                        result: json!({"decision":"decline"}),
                    },
                ];
                if let Some(decisions) = params["availableDecisions"].as_array() {
                    prompt
                        .choices
                        .retain(|c| decisions.contains(&c.result["decision"]));
                    if prompt.choices.is_empty() {
                        prompt.choices.push(Choice {
                            label: "Cancel request".into(),
                            result: json!({"decision":"cancel"}),
                        });
                    }
                }
            }
            "item/permissions/requestApproval" => {
                prompt.cancel = json!({"permissions":{},"scope":"turn"});
                prompt.title = "Additional agent permissions?".into();
                prompt.choices = vec![
                    Choice {
                        label: "Allow for this turn".into(),
                        result: json!({"permissions":params["permissions"],"scope":"turn"}),
                    },
                    Choice {
                        label: "Decline".into(),
                        result: json!({"permissions":{},"scope":"turn"}),
                    },
                ];
            }
            "item/tool/requestUserInput" => {
                prompt.cancel = json!({"answers":{}});
                prompt.title = "Agent needs your input".into();
                if let Some(questions) = params["questions"].as_array() {
                    for question in questions.iter().take(8) {
                        prompt.questions.push(Question {
                            id: question["id"].as_str().unwrap_or_default().into(),
                            text: clip(question["question"].as_str().unwrap_or("Answer"), 2048),
                            options: question["options"]
                                .as_array()
                                .map(|v| {
                                    v.iter()
                                        .take(16)
                                        .filter_map(|o| o["label"].as_str().map(str::to_owned))
                                        .collect()
                                })
                                .unwrap_or_default(),
                            answer: String::new(),
                            secret: question["isSecret"].as_bool().unwrap_or(false),
                        });
                    }
                }
            }
            "session/request_permission" => {
                prompt.cancel = json!({"outcome":{"outcome":"cancelled"}});
                if let Some(options) = params["options"].as_array() {
                    for option in options.iter().take(8) {
                        let Some(option_id) = option["optionId"].as_str() else {
                            continue;
                        };
                        let label = match option["kind"].as_str().unwrap_or_default() {
                            "allow_once" => "Allow once",
                            "allow_always" => "Always allow",
                            "reject_once" => "Decline",
                            "reject_always" => "Always decline",
                            _ => continue,
                        };
                        prompt.choices.push(Choice {
                            label: label.into(),
                            result: json!({"outcome":{"outcome":"selected","optionId":option_id}}),
                        });
                    }
                }
            }
            "mcpServer/elicitation/request" => {
                prompt.cancel = json!({"action":"cancel"});
                prompt.title = "Agent requests information from another service".into();
                prompt.choices.push(Choice {
                    label: "Decline".into(),
                    result: json!({"action":"decline"}),
                });
            }
            _ => {
                self.notice(&format!("Agent requested unsupported client method {method}. Use an agent with its own filesystem/terminal tools."));
                return self.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"RODE does not provide client-side filesystem, terminal, authentication, or custom tool RPCs"}}));
            }
        }
        if permission
            && (self.config.approval == Approval::Never
                || self.read_only
                || self.cancelling
                || !self.busy)
        {
            let denied = if method == "session/request_permission" {
                params["options"]
                    .as_array()
                    .and_then(|options| options.iter().find(|o| o["kind"] == "reject_once"))
                    .and_then(|o| o["optionId"].as_str())
                    .map(|id| json!({"outcome":{"outcome":"selected","optionId":id}}))
                    .unwrap_or(prompt.cancel)
            } else if method == "item/permissions/requestApproval" {
                prompt.cancel
            } else {
                json!({"decision":if params["availableDecisions"].as_array().is_some_and(|choices| !choices.contains(&json!("decline"))) {"cancel"} else {"decline"}})
            };
            self.send(json!({"jsonrpc":"2.0","id":id,"result":denied}))?;
            self.notice("Permission request denied by the selected agent access/approval policy.");
            return Ok(());
        }
        ensure!(self.prompts.len() < 16, "Too many pending agent requests.");
        ensure!(
            !prompt.choices.is_empty() || !prompt.questions.is_empty(),
            "Agent sent a request with no supported choices."
        );
        self.prompts.push_back(prompt);
        self.status = "Waiting for you".into();
        Ok(())
    }

    fn finish(&mut self, success: bool, error: Option<&str>) {
        if !self.busy {
            return;
        }
        self.busy = false;
        self.last_turn_succeeded = success && !self.cancelling;
        self.pending = None;
        self.turn.clear();
        self.prompts.clear();
        self.cancelling = false;
        self.deadline = None;
        self.status = if success { "Ready" } else { "Stopped / failed" }.into();
        self.completed += 1;
        if let Some(error) = error {
            self.notice(error);
        }
    }
    fn fail(&mut self, error: &str) {
        self.completed += u64::from(self.busy);
        let detail = self.last_stderr.trim().to_owned();
        self.disconnect();
        self.status = "Disconnected".into();
        self.notice(&clip(
            &format!(
                "{error}{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!("\n{detail}")
                }
            ),
            16384,
        ));
    }
    /// Append a bounded local notice to the transcript.
    pub fn notice(&mut self, text: &str) {
        self.serial += 1;
        self.push(
            format!("notice-{}", self.serial),
            Role::Notice,
            "Status".into(),
            clip(text, 16384),
        );
    }
    fn append(&mut self, id: &str, role: Role, title: &str, text: &str) {
        if let Some(message) = self.messages.iter_mut().find(|m| m.id == id) {
            append_bounded(
                &mut message.text,
                text,
                if role == Role::Tool { 16384 } else { 65536 },
            );
            if title != "Command" && title != "Tool" {
                message.title = clip(title, 256);
            }
        } else {
            self.push(
                id.into(),
                role,
                clip(title, 256),
                clip(text, if role == Role::Tool { 16384 } else { 65536 }),
            );
        }
        self.trim();
    }
    fn replace(&mut self, id: &str, role: Role, title: &str, text: &str) {
        if let Some(message) = self.messages.iter_mut().find(|m| m.id == id) {
            if !text.is_empty() {
                message.text = clip(text, if role == Role::Tool { 16384 } else { 65536 });
            }
            if title != "Tool" {
                message.title = clip(title, 256);
            }
        } else {
            self.push(
                id.into(),
                role,
                clip(title, 256),
                clip(text, if role == Role::Tool { 16384 } else { 65536 }),
            );
        }
        self.trim();
    }
    fn push(&mut self, id: String, role: Role, title: String, text: String) {
        self.messages.push_back(Message {
            id,
            role,
            title,
            text,
        });
        self.trim();
    }
    fn trim(&mut self) {
        while self.messages.len() > 128
            || self
                .messages
                .iter()
                .map(|m| m.text.len() + m.title.len() + m.id.len())
                .sum::<usize>()
                > 512 * 1024
        {
            self.messages.pop_front();
        }
    }
}

use anyhow::Context;
fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
/// Truncate UTF-8 text to a byte budget without splitting a character.
pub fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let suffix = "\n[… truncated]";
    let mut end = if limit < suffix.len() {
        limit
    } else {
        limit - suffix.len()
    };
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if limit < suffix.len() {
        text[..end].into()
    } else {
        format!("{}{suffix}", &text[..end])
    }
}
fn append_bounded(target: &mut String, text: &str, limit: usize) {
    if target.len() >= limit {
        return;
    }
    let remaining = limit - target.len();
    target.push_str(&clip(text, remaining));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_successful_uninterrupted_turns_allow_follow_ups() {
        let mut chat = Chat::default();
        for (success, cancelling, expected) in [
            (true, false, true),
            (false, false, false),
            (true, true, false),
        ] {
            chat.busy = true;
            chat.cancelling = cancelling;
            chat.finish(success, None);
            assert_eq!(chat.last_turn_succeeded, expected);
            let mut attached = Chat::default();
            attached.apply_view(chat.view());
            assert_eq!(attached.last_turn_succeeded, expected);
        }
        chat.busy = true;
        chat.finish(true, None);
        chat.disconnect();
        assert!(!chat.last_turn_succeeded);
        let mut old_view = serde_json::to_value(chat.view()).unwrap();
        old_view
            .as_object_mut()
            .unwrap()
            .remove("last_turn_succeeded");
        let view: crate::service::ChatView = serde_json::from_value(old_view).unwrap();
        assert!(!view.last_turn_succeeded);
    }

    #[test]
    fn streamed_unicode_is_bounded_even_at_small_remaining_limits() {
        let text = "🦀é界".repeat(100);
        for limit in 0..200 {
            assert!(clip(&text, limit).len() <= limit);
            let mut output = "x".repeat(limit.saturating_sub(2));
            append_bounded(&mut output, &text, limit);
            assert!(output.len() <= limit);
        }
    }
}
