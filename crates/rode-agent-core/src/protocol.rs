//! Codex app-server / ACP v1 conversation lifecycle and one bounded chat model.
//!
//! The controller owns transport, deadlines, pending calls and transcript state.
//! `incoming` normalizes backend responses, notifications and permission requests;
//! `questions` and `recovery` retain their existing acknowledgement/lifetime guards.
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
mod incoming;
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
/// Normalized tool lifecycle, independent of display titles or output text.
/// Backend notifications set individual states; turn completion or disconnect
/// settles any tool still marked Running without rewriting earlier outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolState {
    /// Pending or executing; keep visible even in the compact transcript.
    Running,
    /// Finished successfully; the UI may collapse its presentation.
    Completed,
    /// Failed, declined or interrupted; keep available without opening a group.
    Failed,
}

impl ToolState {
    /// Classify a terminal Codex item, shared by completion notifications and
    /// saved command/file records. A nonzero exit code or explicit failure
    /// takes precedence over a generic "completed" status. Do not call on start.
    pub(super) fn codex_completed(item: &Value) -> Self {
        if matches!(item["status"].as_str(), Some("failed" | "declined"))
            || item["exitCode"].as_i64().is_some_and(|code| code != 0)
            || item["success"] == false
        {
            Self::Failed
        } else {
            Self::Completed
        }
    }

    /// Decode an ACP status supplied by a tool update. Missing or unrecognized
    /// values have no replacement state; partial updates must retain the last
    /// known outcome rather than silently turn a failed tool into a success.
    pub(super) fn acp(status: &str) -> Option<Self> {
        match status {
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "pending" | "in_progress" => Some(Self::Running),
            _ => None,
        }
    }
}

/// One in-memory transcript item; never persisted as preferences.
/// Equality covers lifecycle metadata as well as text: native view deltas must
/// deliver completion-only changes even when the command produced no output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Protocol/stable item identifier used to merge streamed updates.
    pub id: String,
    /// Presentation role and styling.
    pub role: Role,
    /// Short speaker/tool/status label.
    pub title: String,
    /// Bounded accumulated body text.
    pub text: String,
    /// Tool-only lifecycle, absent for other roles and older helpers/history.
    /// An omitted wire field means unknown, never an implied success. Missing
    /// lifecycle data must not hide potentially active tools during a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_state: Option<ToolState>,
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
    undo: crate::undo::Journal,
}

impl Chat {
    /// Recorded file edits from the current/last turn, independent of transcript eviction.
    pub fn undo_patch(&self) -> anyhow::Result<String> {
        self.undo.patch()
    }
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
            undo: None,
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
        self.undo = Default::default();
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
        for (target, mut result) in self.file_jobs.poll() {
            if let Ok(value) = &mut result
                && let Some(object) = value.as_object_mut()
            {
                if object.remove("_rode_undo_failed").is_some() {
                    self.undo.invalidate();
                }
                if let Some(edit) = object.remove("_rode_undo")
                    && let Some(path) = edit["path"].as_str()
                {
                    self.undo
                        .file(path, edit["before"].as_str(), edit["after"].as_str());
                }
            }
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
                    super::io::append_diagnostic(&mut self.last_stderr, &line);
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
        self.finish_tools(ToolState::Failed);
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
        let params = crate::initialization::parameters(
            self.config.provider,
            crate::initialization::Role::Conversation,
        );
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
        if self.config.provider.is_acp() {
            // Compatibility fallback: this client does not yet negotiate ACP's
            // optional session systemPrompt support. Keep the user-role reminder
            // separate from the original task and attachment blocks.
            if let Some(hint) = self.config.personality.hint(self.response_style_context) {
                input.push(json!({"type":"text","text":format!("\n\n{hint}")}));
                self.response_style_context = true;
            }
        }
        input
    }

    fn codex_turn(&mut self, input: Vec<Value>, call: Call) -> Result<()> {
        let mut params = json!({"threadId":self.conversation,"input":input});
        if let Some(hint) = self.config.personality.hint(self.response_style_context) {
            // Codex delivers application context as developer instructions.
            // User-message hints alone compete with the backend's own voice.
            // Use one stable source so follow-ups replace the current style;
            // only built-in prompts belong here, never user or project text.
            params["additionalContext"] = json!({
                "rode_response_personality":{"kind":"application","value":hint}
            });
            self.response_style_context = true;
        }
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
        self.tool_state(
            &format!("file-tool-{}", self.serial),
            if success {
                ToolState::Completed
            } else {
                ToolState::Failed
            },
        );
        Ok(())
    }

    fn finish(&mut self, success: bool, error: Option<&str>) {
        if !self.busy {
            return;
        }
        self.cancel_file_work();
        self.finish_tools(if success && !self.cancelling {
            ToolState::Completed
        } else {
            ToolState::Failed
        });
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
            tool_state: (role == Role::Tool).then_some(ToolState::Running),
        });
        self.trim();
    }

    /// Update an existing tool by protocol identity and publish status-only
    /// changes. Other roles and unknown identities are intentionally untouched.
    fn tool_state(&mut self, id: &str, state: ToolState) {
        if let Some(message) = self
            .messages
            .iter_mut()
            .find(|m| m.id == id && m.role == Role::Tool)
        {
            message.tool_state = Some(state);
            self.changed = true;
        }
    }

    /// Settle unfinished activity at turn/process exit. Already completed or
    /// failed tools retain their own results, including those from older turns.
    fn finish_tools(&mut self, state: ToolState) {
        for message in &mut self.messages {
            if message.tool_state == Some(ToolState::Running) {
                message.tool_state = Some(state);
                self.changed = true;
            }
        }
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
    // This is display text, never a protocol reply. Stop serialization at the
    // tool/detail budget instead of allocating a full multi-megabyte preview
    // only to clip it in replace() or the permission card afterwards.
    const LIMIT: usize = 16384;
    let mut bytes = [0; LIMIT + 1];
    let mut output = std::io::Cursor::new(bytes.as_mut_slice());
    let truncated = serde_json::to_writer_pretty(&mut output, value).is_err();
    let len = output.position() as usize;
    let bytes = &bytes[..len];
    let text = std::str::from_utf8(bytes)
        .unwrap_or_else(|error| std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap());
    if truncated {
        clip(&format!("{text}{TRUNCATED_SUFFIX}"), LIMIT)
    } else {
        clip(text, LIMIT)
    }
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
mod tests;
