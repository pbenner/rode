//! Small Codex app-server / ACP v1 clients, sharing one bounded chat model.
use super::{Access, Approval, Config, Event, Launch, Provider, Session, controls};
use crate::attachment::{self, Attachment};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};
mod questions;
mod recovery;

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
    /// Short agent-supplied heading.
    pub header: String,
    /// Suggested answers supplied by the agent.
    pub options: Vec<String>,
    /// Explanations aligned with `options`.
    pub descriptions: Vec<String>,
    /// Current unsubmitted answer draft.
    pub answer: String,
    /// Already acknowledged by the backend; retained to deduplicate repeated notifications.
    pub answered: bool,
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
    /// Whether the agent is waiting rather than continuing independent work.
    pub blocking: bool,
    /// An asynchronous message answer is awaiting a backend acknowledgement.
    pub sending: bool,
    /// Delivery errors stay with the form so its answer drafts remain usable.
    pub error: String,
    /// Whether this form accepts subsets of message-based questions.
    pub partial_answers: bool,
    async_message: bool,
    cancel: Value,
}

impl Prompt {
    /// Message-based questions use ordinary user input rather than an RPC reply.
    pub fn is_async_message(&self) -> bool {
        self.async_message
    }

    /// Construct a reply from entered, unacknowledged answers. Blocking requests
    /// require the complete form; message-based forms accept subsets.
    /// Free-form text and suggested labels use the same app-server answer format.
    pub fn answers(&self) -> Option<Value> {
        if !self.partial_answers
            && self
                .questions
                .iter()
                .any(|q| !q.answered && q.answer.trim().is_empty())
        {
            return None;
        }
        let answers: serde_json::Map<String, Value> = self
            .questions
            .iter()
            .filter(|q| !q.answered && !q.answer.trim().is_empty())
            .map(|q| (q.id.clone(), json!({"answers":[q.answer]})))
            .collect();
        (!answers.is_empty()).then(|| json!({"answers":answers}))
    }
}
#[derive(Clone)]
enum Call {
    Initialize,
    FilePolicy,
    Conversation,
    Turn,
    Interrupt,
    Models,
    Answer {
        id: Value,
        text: String,
        answers: Vec<String>,
        turn: Option<String>,
        deadline: Option<Instant>,
    },
    Configure {
        id: String,
        value: Value,
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
    /// Actual controller policy, including read-only chats and model-only access.
    controller_offline: bool,
    conversation: String,
    turn: String,
    next_id: u64,
    calls: HashMap<u64, Call>,
    pending: Option<(String, Vec<Attachment>)>,
    image_support: bool,
    cancelling: bool,
    deadline: Option<Instant>,
    last_stderr: String,
    serial: u64,
    model_pages: controls::ModelPages,
    settings_attempts: usize,
    /// A restored/styled context needs an explicit normal-tone hint when Default is selected.
    response_style_context: bool,
    file_factory: Option<Arc<dyn super::Factory>>,
    file_wake: Option<Arc<dyn Fn() + Send + Sync>>,
    file_jobs: super::file_only::Jobs,
    file_calls: usize,
    recovery: Option<String>,
    fresh_recovery: bool,
    input_history: super::input_history::History,
    question_messages: VecDeque<String>,
}

impl Chat {
    /// Copy the bounded presentation state, never process handles or credentials.
    pub fn view(&self) -> crate::service::ChatView {
        crate::service::ChatView {
            conversation: self.conversation_id().unwrap_or_default().into(),
            recoverable: self.recoverable(),
            messages: self.messages.clone(),
            input_history: Some(self.input_history.texts().map(str::to_owned).collect()),
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
    /// Update a remote view without creating a process. Preserve drafts only for
    /// unchanged, unanswered questions; IDs can be reused by a refreshed form.
    pub fn apply_view(&mut self, mut view: crate::service::ChatView) {
        super::personality::clean_history(&mut view.messages);
        self.input_history = super::input_history::History::from_texts(
            view.input_history.take().unwrap_or_default().into_iter(),
        );
        for prompt in &mut view.prompts {
            if let Some(old) = self.prompts.iter().find(|old| old.id == prompt.id) {
                for question in &mut prompt.questions {
                    if !question.answered
                        && let Some(previous) = old.questions.iter().find(|q| {
                            !q.answered
                                && q.id == question.id
                                && q.text == question.text
                                && q.secret == question.secret
                        })
                    {
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
        self.conversation = view.conversation;
        self.recovery = view.recoverable.then(|| self.conversation.clone());
        self.changed = true;
    }
    /// Backend identity of the displayed conversation, absent before initialization.
    pub fn conversation_id(&self) -> Option<&str> {
        (!self.conversation.is_empty())
            .then_some(self.conversation.as_str())
            .or_else(|| self.recovery.as_deref().filter(|id| !id.is_empty()))
    }
    /// Recent accepted/replayed prompts, retained separately from displayed tool output.
    pub fn input_history(&self) -> impl Iterator<Item = &str> {
        self.input_history.texts()
    }
    /// Adopt an acknowledged history restore, replacing only this chat's process.
    /// Current project policy was captured before the backend resumed the thread.
    pub fn restore(&mut self, restored: super::history::Restored) {
        self.disconnect();
        self.controller_offline = restored
            .launch
            .plan
            .controller_network_disabled(restored.launch.config.network);
        self.config = restored.launch.config;
        self.root = restored.launch.plan.root;
        self.read_only = restored.launch.plan.project_read_only
            || (!self.config.provider.is_acp() && self.config.access == Access::ReadOnly);
        self.external = restored.launch.plan.active && !self.read_only;
        self.network = restored
            .launch
            .plan
            .prefix
            .iter()
            .any(|arg| arg == "--share-net");
        self.conversation = restored.id;
        self.messages = restored.messages;
        self.input_history = restored.input_history;
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
        self.cancelling || self.file_jobs.stopping()
    }

    /// Native operations retain their controller until cancelled helper cleanup ends.
    pub fn file_work_pending(&self) -> bool {
        self.file_jobs.pending()
    }

    /// Whether startup, settings, cancellation or native I/O needs a timed repaint.
    pub fn has_deadline(&self) -> bool {
        self.deadline.is_some()
            || self.file_jobs.pending()
            || self.calls.values().any(|call| {
                matches!(
                    call,
                    Call::Answer {
                        deadline: Some(_),
                        ..
                    }
                )
            })
    }

    /// Whether cached capabilities belong to the currently selected executable/provider.
    pub fn controls_match(&self, config: &Config) -> bool {
        self.config.same_process(config)
    }

    /// Apply advertised ACP choices to an idle session without submitting a prompt.
    /// New processes are still started only by an explicit chat message.
    pub fn configure_idle(&mut self, config: &Config) -> Result<()> {
        if !config.provider.is_acp()
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
        self.submit_with_attachments(launch, text, vec![], wake)
    }

    /// Submit immutable image/document contents with a bounded user prompt.
    pub fn submit_with_attachments(
        &mut self,
        launch: Launch,
        text: String,
        attachments: Vec<Attachment>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<()> {
        ensure!(
            !self.busy && !self.file_jobs.pending(),
            "The agent is still working. Stop it or wait for its reply."
        );
        ensure!(
            !self.recoverable(),
            "Recover the agent before sending another prompt."
        );
        ensure!(
            self.prompts.is_empty(),
            "Answer or dismiss the pending agent question first."
        );
        attachment::validate(&text, &attachments)?;
        let reset_defaults = (self.config.model() != launch.config.model()
            && launch.config.model().is_empty())
            || (self.config.reasoning_effort != launch.config.reasoning_effort
                && launch.config.reasoning_effort.is_empty());
        if self.session.is_some() && (!self.config.same_process(&launch.config) || reset_defaults) {
            self.disconnect();
            self.notice("Agent settings changed · starting a new conversation");
        }
        self.read_only = launch.plan.project_read_only
            || (!launch.config.provider.is_acp()
                && matches!(launch.config.access, Access::ReadOnly | Access::FilesOnly));
        self.external = launch.plan.active && !self.read_only;
        self.network = launch.plan.prefix.iter().any(|arg| arg == "--share-net");
        self.config = launch.config.clone();
        if self.session.is_none() {
            self.start_session(launch, wake)?;
        }
        self.serial += 1;
        self.input_history
            .remember(&format!("user-{}", self.serial), &text);
        self.push(
            format!("user-{}", self.serial),
            Role::User,
            "You".into(),
            attachment::summary(&text, &attachments),
        );
        self.pending = Some((text, attachments));
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
        for (target, result) in self.file_jobs.poll() {
            let result = match target {
                super::file_only::Target::Verify(config) => result
                    .map_err(anyhow::Error::msg)
                    .context("Cannot start confined project file tools")
                    .and_then(|_| self.start_file_thread(config)),
                super::file_only::Target::Tool { id, name } => self.file_reply(id, &name, result),
            };
            if let Err(error) = result {
                self.fail(&format!("{error:#}"));
                break;
            }
        }
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
                    self.fail(&format!(
                        "Agent exited{}",
                        code.map(|c| format!(" ({c})")).unwrap_or_default()
                    ));
                    Ok(())
                }
            };
            if let Err(error) = result {
                self.fail(&format!("{error:#}"));
            }
        }
        self.poll_answers();
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
            if prompt.async_message {
                let id = prompt.id.clone();
                let outcome = self.answer_message(result);
                if let Err(error) = &outcome
                    && let Some(prompt) = self.prompts.iter_mut().find(|prompt| prompt.id == id)
                    && !prompt.sending
                {
                    prompt.error = clip(&format!("Could not send answers: {error:#}"), 2048);
                    self.changed = true;
                }
                return outcome;
            }
            self.send(json!({"jsonrpc":"2.0", "id":prompt.id, "result":result}))?;
            self.prompts.pop_front();
            self.request_status();
        }
        Ok(())
    }

    /// Request cancellation of the current turn; timeout handling may stop the owned process.
    pub fn cancel(&mut self) {
        if !self.busy {
            if self.prompts.front().is_some_and(|prompt| {
                prompt.async_message && (!prompt.sending || !prompt.error.is_empty())
            }) {
                self.prompts.pop_front();
                self.changed = true;
                self.request_status();
            }
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
        self.cancel_file_work();
        self.status = "Stopping…".into();
        self.deadline = Some(Instant::now() + Duration::from_secs(3));
        let prompts = std::mem::take(&mut self.prompts);
        for prompt in prompts {
            if !prompt.async_message {
                let _ = self.send(json!({"jsonrpc":"2.0", "id":prompt.id, "result":prompt.cancel}));
            }
        }
        let result = if self.conversation.is_empty() {
            self.disconnect();
            self.status = "Stopped".into();
            Ok(())
        } else if self.config.provider.is_acp() {
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
        self.file_jobs.cancel();
        self.session = None;
        self.busy = false;
        self.last_turn_succeeded = false;
        self.conversation.clear();
        self.turn.clear();
        self.calls.clear();
        self.pending = None;
        self.prompts.clear();
        self.question_messages.clear();
        self.deadline = None;
        self.cancelling = false;
        self.last_stderr.clear();
        self.models.clear();
        self.controls.clear();
        self.model_pages.clear();
        self.response_style_context = false;
        self.file_factory = None;
        self.file_wake = None;
        self.file_calls = 0;
        self.recovery = None;
        self.fresh_recovery = false;
    }

    fn initialize(&mut self) -> Result<()> {
        self.deadline = Some(Instant::now() + Duration::from_secs(60));
        let mut params = if self.config.provider.is_acp() {
            json!({"protocolVersion":1,"clientInfo":{"name":"rode","title":"RODE","version":env!("CARGO_PKG_VERSION")},
                "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false,"session":{"configOptions":{"boolean":true}}}})
        } else {
            json!({"clientInfo":{"name":"rode","title":"RODE","version":env!("CARGO_PKG_VERSION")}})
        };
        if !self.config.provider.is_acp() {
            params["capabilities"] = json!({"experimentalApi":true});
        }
        self.call("initialize", params, Call::Initialize)
    }
    fn begin_turn(&mut self) -> Result<()> {
        if self.cancelling {
            return Ok(());
        }
        if self.config.provider.is_acp() {
            ensure!(
                self.config
                    .acp_options()
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
                let Some(value) = self.config.acp_options().get(id) else {
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
                    params["configId"] = id.clone().into();
                    params["value"] = value.clone();
                    if control.boolean {
                        params["type"] = "boolean".into();
                    }
                    self.status = "Applying agent controls…".into();
                    self.deadline = Some(Instant::now() + Duration::from_secs(30));
                    return self.call(
                        "session/set_config_option",
                        params,
                        Call::Configure {
                            id: id.clone(),
                            value: value.clone(),
                        },
                    );
                }
            }
        }
        if self.config.provider.is_acp() && !self.image_support {
            ensure!(
                !self
                    .pending
                    .as_ref()
                    .is_some_and(|(_, files)| files.iter().any(Attachment::is_image)),
                "This ACP agent does not advertise image input support. Remove the image or choose another agent."
            );
        }
        let Some((text, attachments)) = self.pending.take() else {
            self.busy = false;
            self.deadline = None;
            self.status = "Ready".into();
            return Ok(());
        };
        self.status = "Working…".into();
        self.deadline = None;
        self.turn.clear();
        self.file_calls = 0;
        let input = self.prompt_input(text, attachments);
        if self.config.provider.is_acp() {
            self.call(
                "session/prompt",
                json!({"sessionId":self.conversation,"prompt":input}),
                Call::Turn,
            )
        } else {
            self.codex_turn(input, Call::Turn)
        }
    }

    fn prompt_input(&mut self, text: String, attachments: Vec<Attachment>) -> Vec<Value> {
        let mut input = vec![
            json!({"type":"text","text":if text.trim().is_empty() { "Attached files.".into() } else { text }}),
        ];
        for attachment in attachments {
            input.extend(attachment.blocks(self.config.provider.is_acp()));
        }
        if let Some(hint) = self.config.personality.hint(self.response_style_context) {
            input.push(json!({"type":"text","text":format!("\n\n{hint}")}));
            self.response_style_context = true;
        }
        input
    }

    fn codex_turn(&mut self, input: Vec<Value>, call: Call) -> Result<()> {
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
        if self.config.native_tools() {
            // Built-in tools stay read-only even though the trusted model
            // controller has a network exception. Dynamic tools use RODE's
            // original offline policy and never this inner sandbox.
            params["sandboxPolicy"] = json!({"type":"readOnly"});
            params["approvalPolicy"] = "never".into();
        } else if self.external {
            // The outer sandbox enforces the project policy for this process
            // and every child. Tell Codex to use that external sandbox.
            params["sandboxPolicy"] = json!({"type":"externalSandbox","networkAccess":if self.network {"enabled"} else {"restricted"}});
        } else {
            params["sandboxPolicy"] = if self.read_only {
                json!({"type":"readOnly"})
            } else if self.config.access == Access::Full {
                json!({"type":"dangerFullAccess"})
            } else {
                json!({"type":"workspaceWrite","writableRoots":[self.root],"networkAccess":false})
            };
        }
        self.call("turn/start", params, call)
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
            if self.config.provider.is_acp() {
                // session/load may replay saved messages before its response.
                // Recovery retains our transcript; only accept live updates once
                // the returned conversation ID has been validated and adopted.
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
            if let Call::Answer { id, turn, .. } = call {
                self.answer_failed(
                    &id,
                    error["message"]
                        .as_str()
                        .unwrap_or("Answer was not accepted"),
                    turn.is_none(),
                );
                return Ok(());
            }
            if matches!(call, Call::Models) {
                let message = error["message"].as_str().unwrap_or("request failed");
                let explanation = super::network::explain_error(
                    self.config.provider,
                    self.controller_offline,
                    message,
                    16384,
                );
                if explanation != message {
                    self.notice(&explanation);
                    return Ok(());
                }
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
            Call::Answer {
                id,
                text,
                answers,
                turn,
                ..
            } => self.answer_accepted(id, text, answers, turn, result)?,
            Call::Initialize => {
                if self.config.provider.is_acp() {
                    ensure!(
                        result["protocolVersion"].as_u64() == Some(1),
                        "Agent does not support ACP v1."
                    );
                    self.image_support = result["agentCapabilities"]["promptCapabilities"]["image"]
                        .as_bool()
                        .unwrap_or(false);
                    if let Some(id) = self.resume_id().map(str::to_owned) {
                        ensure!(
                            result["agentCapabilities"]["loadSession"].as_bool() == Some(true),
                            "This ACP agent cannot reconnect saved sessions. Use Start new context to recover while keeping queued prompts."
                        );
                        self.call(
                            "session/load",
                            json!({"sessionId":id,"cwd":self.root,"mcpServers":[]}),
                            Call::Conversation,
                        )?;
                    } else {
                        self.call(
                            "session/new",
                            json!({"cwd":self.root,"mcpServers":[]}),
                            Call::Conversation,
                        )?;
                    }
                } else {
                    self.send(json!({"jsonrpc":"2.0","method":"initialized","params":{}}))?;
                    if self.config.provider == Provider::Codex {
                        self.call(
                            "model/list",
                            json!({"limit":32,"includeHidden":false}),
                            Call::Models,
                        )?;
                    }
                    if self.config.native_tools() {
                        self.call(
                            "config/read",
                            json!({"includeLayers":false}),
                            Call::FilePolicy,
                        )?;
                        return Ok(());
                    }
                    let access = if self.read_only {
                        Access::ReadOnly
                    } else {
                        self.config.access
                    };
                    let mut params = json!({"cwd":self.root,"approvalPolicy":if self.read_only {"never"} else {self.config.approval.codex()},"sandbox":access.codex()});
                    if !self.config.model().is_empty() {
                        params["model"] = self.config.model().into();
                    }
                    if self.config.provider == Provider::Local {
                        params["modelProvider"] = "rode_local".into();
                    }
                    self.start_conversation(params)?;
                }
            }
            Call::FilePolicy => {
                let config = super::file_only::thread_config(&result["config"])?;
                self.start_file_job(
                    "rode_list_files",
                    json!({"path":"."}),
                    super::file_only::Target::Verify(config),
                )?;
            }
            Call::Conversation => {
                let expected = self.resume_id().map(str::to_owned);
                let id = if self.config.provider.is_acp() {
                    result["sessionId"].as_str().or(expected.as_deref())
                } else {
                    result["thread"]["id"].as_str()
                }
                .filter(|id| !id.is_empty() && id.len() <= 256)
                .context("Agent returned no conversation ID")?;
                ensure!(
                    expected.as_deref().is_none_or(|expected| expected == id),
                    "Agent reconnected a different conversation; no prompt was sent"
                );
                self.conversation = id.into();
                if self.config.provider.is_acp() {
                    self.controls = controls::acp(result);
                }
                if self.recovery.take().is_some() {
                    if expected.is_none() {
                        // New processes may reuse item IDs. Retained messages
                        // must stay separate from output in the fresh context.
                        for (index, message) in self.messages.iter_mut().enumerate() {
                            message.id = format!("retained-{}-{index}", self.serial);
                        }
                    }
                    self.response_style_context = expected.is_some();
                    self.notice(if expected.is_some() {
                        "Agent reconnected to the saved conversation. The interrupted prompt was not repeated. Queued prompts remain paused."
                    } else {
                        "Agent restarted with a new context. Earlier messages remain visible, but are not in the new agent context. Queued prompts are kept and remain paused."
                    });
                    self.fresh_recovery = false;
                }
                self.begin_turn()?;
            }
            Call::Turn => {
                if self.config.provider.is_acp() {
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
                if let Some(cursor) = self.model_pages.append(&mut self.models, result) {
                    self.call(
                        "model/list",
                        json!({"limit":32,"includeHidden":false,"cursor":cursor}),
                        Call::Models,
                    )?;
                }
            }
            Call::Configure { id, value } => {
                if self.cancelling {
                    self.finish(false, None);
                    return Ok(());
                }
                self.controls = controls::acp(result);
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
                        self.message_questions(item);
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
                if params["threadId"]
                    .as_str()
                    .is_none_or(|id| id == self.conversation)
                {
                    let before = self.prompts.len();
                    self.prompts
                        .retain(|prompt| prompt.id != params["requestId"]);
                    if before != self.prompts.len() && self.busy && !self.cancelling {
                        self.request_status();
                    }
                }
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
            _ => {}
        }
    }

    fn request(&mut self, method: &str, id: Value, params: &Value) -> Result<()> {
        if method == "item/tool/call" {
            return self.file_request(id, params);
        }
        if let Some(scope) = params
            .get(if self.config.provider.is_acp() {
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
            blocking: true,
            sending: false,
            error: String::new(),
            partial_answers: false,
            async_message: false,
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
                prompt.blocking = params["isBlocking"].as_bool().unwrap_or(true);
                if let Some(questions) = params["questions"].as_array() {
                    for question in questions.iter().take(8) {
                        prompt.questions.push(Question {
                            id: question["id"].as_str().unwrap_or_default().into(),
                            text: clip(question["question"].as_str().unwrap_or("Answer"), 2048),
                            header: clip(question["header"].as_str().unwrap_or_default(), 256),
                            options: question["options"]
                                .as_array()
                                .map(|v| {
                                    v.iter()
                                        .take(16)
                                        .filter_map(|o| o["label"].as_str().map(|s| clip(s, 1024)))
                                        .collect()
                                })
                                .unwrap_or_default(),
                            descriptions: question["options"]
                                .as_array()
                                .map(|options| {
                                    options
                                        .iter()
                                        .take(16)
                                        .filter(|o| o["label"].is_string())
                                        .map(|o| {
                                            clip(
                                                o["description"].as_str().unwrap_or_default(),
                                                2048,
                                            )
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                            answer: String::new(),
                            answered: false,
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
                || self.config.native_tools()
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
        self.request_status();
        Ok(())
    }

    /// Pending requests take precedence over the generic turn-running status.
    fn request_status(&mut self) {
        self.status = if self.prompts.iter().any(|prompt| prompt.blocking) {
            "Waiting for you"
        } else if !self.prompts.is_empty() && self.busy {
            "Question awaiting your answer · agent working"
        } else if !self.prompts.is_empty() {
            "Question awaiting your answer"
        } else if self.busy {
            "Working…"
        } else {
            "Ready"
        }
        .into();
    }

    fn file_request(&mut self, id: Value, params: &Value) -> Result<()> {
        let tool = params["tool"].as_str().unwrap_or_default();
        let allowed = self.config.native_tools()
            && self.busy
            && !self.cancelling
            && !self.turn.is_empty()
            && params["threadId"] == self.conversation
            && params["turnId"] == self.turn
            && params.get("namespace").is_none_or(Value::is_null)
            && (super::file_only::supported(tool)
                || (tool == "rode_run_command"
                    && self.config.offline_commands()
                    && !self.read_only))
            // File-only makes Codex's own tools read-only, while its native
            // file helper independently enforces the project's write policy.
            && (!self.read_only || self.config.files_only()
                || !matches!(
                    tool,
                    "rode_write_file" | "rode_delete_file" | "rode_create_directory"
                ))
            && self.file_calls < 128;
        let result = if allowed {
            self.file_calls += 1;
            self.start_file_job(
                tool,
                params["arguments"].clone(),
                super::file_only::Target::Tool {
                    id: id.clone(),
                    name: tool.into(),
                },
            )
        } else {
            Err(anyhow::anyhow!(
                "Native tool denied: only permitted tools in the current active confined turn may run"
            ))
        };
        match result {
            Ok(()) => Ok(()),
            Err(error) => self.file_reply(id, tool, Err(format!("{error:#}"))),
        }
    }

    fn start_file_job(
        &mut self,
        tool: &str,
        arguments: Value,
        target: super::file_only::Target,
    ) -> Result<()> {
        self.file_jobs.start(
            self.file_factory
                .clone()
                .context("File tool host is unavailable")?,
            tool,
            arguments,
            self.file_wake
                .clone()
                .context("File tool wake is unavailable")?,
            target,
        )
    }

    fn start_file_thread(&mut self, config: Value) -> Result<()> {
        let mut params = json!({
            "cwd":self.root,"approvalPolicy":"never","sandbox":"read-only",
            "config":config,"dynamicTools":super::network::tools(&self.config, self.read_only),
            "developerInstructions":if self.config.network == super::network::Network::ModelOnly {super::network::INSTRUCTIONS} else {super::file_only::INSTRUCTIONS},
        });
        if !self.config.model().is_empty() {
            params["model"] = self.config.model().into();
        }
        if self.config.provider == Provider::Local {
            params["modelProvider"] = "rode_local".into();
        }
        self.call("thread/start", params, Call::Conversation)?;
        Ok(())
    }

    fn cancel_file_work(&mut self) {
        for target in self.file_jobs.cancel() {
            if let super::file_only::Target::Tool { id, name } = target {
                let _ = self.file_reply(id, &name, Err("File operation cancelled".into()));
            }
        }
    }

    fn file_reply(
        &mut self,
        id: Value,
        tool: &str,
        result: std::result::Result<Value, String>,
    ) -> Result<()> {
        let (mut success, mut text) = match result {
            Ok(value) => (true, serde_json::to_string(&value)?),
            Err(error) => (false, format!("{error:#}")),
        };
        // JSON-in-JSON escaping can make even a 1 MiB file exceed the transport
        // limit. Reject the response as a tool error, keeping the chat connected.
        if serde_json::to_vec(&text)?.len() > super::LINE_LIMIT - 8192 {
            success = false;
            text = "File tool response exceeds the protocol limit; use a smaller file or narrower search".into();
        }
        self.send(json!({"jsonrpc":"2.0","id":id,"result":{"success":success,"contentItems":[{"type":"inputText","text":text}]}}))?;
        self.serial += 1;
        self.push(
            format!("file-tool-{}", self.serial),
            Role::Tool,
            clip(tool, 256),
            clip(&text, 4096),
        );
        Ok(())
    }

    fn finish(&mut self, success: bool, error: Option<&str>) {
        if !self.busy {
            return;
        }
        self.cancel_file_work();
        self.busy = false;
        self.last_turn_succeeded = success && !self.cancelling;
        self.pending = None;
        self.turn.clear();
        if self.cancelling {
            self.prompts.clear();
        } else {
            self.prompts.retain(|prompt| prompt.async_message);
        }
        self.cancelling = false;
        self.deadline = None;
        self.status = if success { "Ready" } else { "Stopped / failed" }.into();
        if !self.prompts.is_empty() {
            self.request_status();
        }
        self.completed += 1;
        if let Some(error) = error {
            self.notice(&super::network::explain_error(
                self.config.provider,
                self.controller_offline,
                error,
                16384,
            ));
        }
    }
    fn fail(&mut self, error: &str) {
        let detail = self.last_stderr.trim().to_owned();
        let error = super::network::explain_error(
            self.config.provider,
            self.controller_offline,
            error,
            16384_usize.saturating_sub(detail.len() + usize::from(!detail.is_empty())),
        );
        let conversation = self.conversation_id().unwrap_or_default().to_owned();
        self.completed += u64::from(self.busy);
        self.disconnect();
        self.recovery = Some(conversation);
        self.status = "Disconnected · recovery available".into();
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
    let suffix = TRUNCATED_SUFFIX;
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
pub(crate) const TRUNCATED_SUFFIX: &str = "\n[… truncated]";
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
    fn offline_guidance_covers_failed_turns_and_timeouts_for_every_provider() {
        for provider in [
            Provider::Codex,
            Provider::Local,
            Provider::Claude,
            Provider::Acp,
        ] {
            for offline in [false, true] {
                let mut chat = Chat {
                    config: Config {
                        provider,
                        ..Default::default()
                    },
                    controller_offline: offline,
                    busy: true,
                    conversation: "saved-thread".into(),
                    ..Default::default()
                };
                chat.finish(false, Some("Unable to connect to model"));
                assert!(!chat.busy);
                assert!(!chat.last_turn_succeeded);
                assert!(!chat.recoverable());
                assert_eq!(chat.conversation_id(), Some("saved-thread"));
                let text = &chat.messages.back().unwrap().text;
                assert!(text.starts_with("Unable to connect to model"));
                assert_eq!(text.contains("sandbox blocks network"), offline);

                chat.busy = true;
                chat.deadline = Some(Instant::now());
                chat.poll();
                assert!(chat.recoverable());
                assert!(!chat.busy);
                let text = &chat.messages.back().unwrap().text;
                assert!(text.starts_with("Agent startup timed out."));
                assert_eq!(text.contains("sandbox blocks network"), offline);
            }
        }
    }

    #[test]
    fn routing_errors_keep_recovery_and_explain_catalog_failures_without_ending_the_chat() {
        for offline in [false, true] {
            let mut chat = Chat {
                controller_offline: offline,
                busy: true,
                conversation: "saved-thread".into(),
                last_stderr: "backend diagnostic".into(),
                ..Default::default()
            };
            chat.calls.insert(1, Call::Models);
            chat.receive(json!({"id":1,"error":{"message":"workspace routing discovery failed"}}))
                .unwrap();
            assert!(chat.busy);
            assert!(!chat.recoverable());
            assert!(
                !chat
                    .messages
                    .back()
                    .unwrap()
                    .text
                    .contains("does not provide a model catalog")
            );

            chat.calls.insert(2, Call::Conversation);
            let error = chat
                .receive(json!({"id":2,"error":{"message":"workspace routing discovery failed"}}))
                .unwrap_err();
            chat.fail(&error.to_string());
            assert!(!chat.busy);
            assert!(chat.recoverable());
            assert_eq!(chat.conversation_id(), Some("saved-thread"));
            let text = &chat.messages.back().unwrap().text;
            assert!(text.starts_with("Agent: workspace routing discovery failed"));
            assert!(text.ends_with("backend diagnostic"));
            assert_eq!(text.contains("sandbox blocks network"), offline);
            assert_eq!(chat.config.network, super::super::network::Network::Project);
            let mut view = Chat::default();
            view.apply_view(chat.view());
            assert_eq!(view.messages.back().unwrap().text, *text);
        }
    }

    #[test]
    fn questions_need_all_answers_and_preserve_request_lifetimes() {
        let mut chat = Chat {
            busy: true,
            conversation: "thread".into(),
            ..Default::default()
        };
        for (id, blocking) in [(401, false), (402, true)] {
            chat.receive(
                json!({"id":id,"method":"item/tool/requestUserInput","params":{
                    "threadId":"thread", "isBlocking":blocking, "questions":[
                        {"id":"scope","header":"Scope","question":"Which tests?","options":[
                            {"label":"All","description":"Run everything"}]},
                        {"id":"note","header":"Note","question":"Anything else?","isSecret":true}
                    ]
                }}),
            )
            .unwrap();
        }
        assert_eq!(chat.status, "Waiting for you");
        let prompt = &mut chat.prompts[0];
        assert!(prompt.answers().is_none());
        prompt.questions[0].answer = "All".into();
        prompt.questions[1].answer = "  ".into();
        assert!(prompt.answers().is_none());
        prompt.questions[1].answer = "Keep this exact text\nplease".into();
        assert_eq!(
            prompt.answers().unwrap(),
            json!({"answers":{
                "scope":{"answers":["All"]}, "note":{"answers":["Keep this exact text\nplease"]}
            }})
        );
        let mut incomplete = serde_json::to_value(&*prompt).unwrap();
        incomplete.as_object_mut().unwrap().remove("blocking");
        assert!(serde_json::from_value::<Prompt>(incomplete).is_err());
        chat.codex_update(
            "serverRequest/resolved",
            &json!({"threadId":"other","requestId":402}),
        )
        .unwrap();
        assert_eq!(chat.prompts.len(), 2);
        chat.codex_update(
            "serverRequest/resolved",
            &json!({"threadId":"thread","requestId":402}),
        )
        .unwrap();
        assert_eq!(chat.prompts.len(), 1);
        assert_eq!(chat.status, "Question awaiting your answer · agent working");
        chat.codex_update(
            "serverRequest/resolved",
            &json!({"threadId":"thread","requestId":401}),
        )
        .unwrap();
        assert!(chat.prompts.is_empty());
        assert_eq!(chat.status, "Working…");
        assert!(
            chat.busy,
            "Resolving a question must not complete the turn or release its queue"
        );
    }

    #[test]
    fn escaped_prompt_history_keeps_snapshots_within_the_desktop_line_limit() {
        let mut chat = Chat::default();
        let text = "\0".repeat(crate::PROMPT_LIMIT);
        for n in 0..32 {
            let id = format!("user-{n}");
            chat.input_history.remember(&id, &text);
            chat.push(id, Role::User, "You".into(), text.clone());
        }
        assert!(serde_json::to_vec(&chat.view()).unwrap().len() < crate::LINE_LIMIT);
    }

    #[test]
    fn stopping_a_slow_file_tool_does_not_block_or_publish_a_late_result() {
        use std::sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc,
        };
        struct Slow {
            started: mpsc::Sender<()>,
            cancelled: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }
        impl crate::Factory for Slow {
            fn start(
                &self,
                _: &Launch,
                _: bool,
                _: Arc<dyn Fn() + Send + Sync>,
            ) -> Result<Session> {
                unreachable!()
            }
            fn file_tool(&self, _: &str, _: &Value, cancel: &AtomicBool) -> Result<Value> {
                self.started.send(())?;
                let until = Instant::now() + Duration::from_secs(5);
                while !cancel.load(Ordering::Acquire) {
                    ensure!(Instant::now() < until, "Cancellation was not delivered");
                    std::thread::yield_now();
                }
                self.cancelled.send(())?;
                self.release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))?;
                Ok(json!({"late":true}))
            }
        }
        struct Capture(mpsc::Sender<Value>);
        impl crate::Transport for Capture {
            fn send(&self, message: &Value) -> Result<()> {
                self.0.send(message.clone())?;
                Ok(())
            }
            fn poll(&self) -> Option<Event> {
                None
            }
            fn stop(&self) {}
        }
        let (started, ready) = mpsc::channel();
        let (cancelled, stopped) = mpsc::channel();
        let (release, cleanup) = mpsc::channel();
        let (sent, received) = mpsc::channel();
        let mut chat = Chat {
            busy: true,
            conversation: "thread".into(),
            turn: "turn".into(),
            config: Config {
                access: Access::FilesOnly,
                ..Default::default()
            },
            session: Some(Session::new(Capture(sent))),
            file_factory: Some(Arc::new(Slow {
                started,
                cancelled,
                release: Mutex::new(cleanup),
            })),
            file_wake: Some(Arc::new(|| {})),
            ..Default::default()
        };
        for id in 10..15 {
            chat.file_request(json!(id), &json!({"tool":"rode_read_file","threadId":"thread","turnId":"turn","arguments":{"path":"slow"}})).unwrap();
        }
        for _ in 0..4 {
            ready.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        chat.cancel();
        for _ in 0..4 {
            stopped.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        assert!(chat.file_work_pending());
        assert!(chat.stopping());
        chat.disconnect();
        for _ in 0..4 {
            release.send(()).unwrap();
        }
        let until = Instant::now() + Duration::from_secs(5);
        while chat.file_work_pending() {
            chat.poll();
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        chat.poll();
        let replies: Vec<_> = received
            .try_iter()
            .filter(|v| (10..15).any(|id| v["id"] == id))
            .collect();
        assert_eq!(replies.len(), 5);
        assert!(
            replies
                .iter()
                .all(|reply| reply["result"]["success"] == false)
        );
        assert!(!chat.messages.iter().any(|m| m.text.contains("late")));
    }

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
        let mut incomplete = serde_json::to_value(chat.view()).unwrap();
        incomplete
            .as_object_mut()
            .unwrap()
            .remove("last_turn_succeeded");
        assert!(serde_json::from_value::<crate::service::ChatView>(incomplete).is_err());
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
