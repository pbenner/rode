//! Small Codex app-server / ACP v1 clients, sharing one bounded chat model.
use super::{Config, Event, Launch, PROMPT_LIMIT, Provider, Session};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

/// Presentation role for one bounded in-memory chat item.
#[derive(Clone, Copy, PartialEq, Eq)]
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
pub struct Choice {
    /// Human-readable decision label.
    pub label: String,
    /// JSON result sent when this choice is selected.
    pub result: Value,
}
/// An agent question awaiting an explicit user answer.
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
#[derive(Clone, Copy)]
enum Call {
    Initialize,
    Conversation,
    Turn,
    Interrupt,
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
    /// Whether displayed chat content changed since the UI last observed it.
    pub changed: bool,
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
}

impl Chat {
    /// Return whether a transport and initialized conversation are present.
    pub fn connected(&self) -> bool {
        self.session.is_some() && !self.conversation.is_empty()
    }
    /// Return whether cancellation is awaiting completion or escalation.
    pub fn stopping(&self) -> bool {
        self.cancelling
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
        launch.config.argv()?;
        if self.session.is_some() && self.config != launch.config {
            self.disconnect();
            self.notice("Agent settings changed · starting a new conversation");
        }
        if self.session.is_none() {
            self.root = launch.plan.root.clone();
            self.read_only = launch.plan.project_read_only || launch.config.read_only;
            self.external = launch.plan.active && !self.read_only;
            self.network = launch.plan.prefix.iter().any(|arg| arg == "--share-net");
            self.config = launch.config.clone();
            self.session = Some(Session::start(launch, wake)?);
            self.status = "Starting agent…".into();
            self.deadline = Some(Instant::now() + Duration::from_secs(60));
        }
        self.serial += 1;
        self.push(
            format!("user-{}", self.serial),
            Role::User,
            "You".into(),
            text.clone(),
        );
        self.pending = Some(text);
        self.busy = true;
        self.cancelling = false;
        if !self.conversation.is_empty() {
            self.begin_turn()?;
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
        self.conversation.clear();
        self.turn.clear();
        self.calls.clear();
        self.pending = None;
        self.prompts.clear();
        self.deadline = None;
        self.cancelling = false;
        self.last_stderr.clear();
    }

    fn initialize(&mut self) -> Result<()> {
        let params = if self.config.provider == Provider::Acp {
            json!({"protocolVersion":1,"clientInfo":{"name":"rode","title":"RODE","version":env!("CARGO_PKG_VERSION")},
                "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}})
        } else {
            json!({"clientInfo":{"name":"rode","title":"RODE","version":env!("CARGO_PKG_VERSION")}})
        };
        self.call("initialize", params, Call::Initialize)
    }
    fn begin_turn(&mut self) -> Result<()> {
        let Some(text) = self.pending.take() else {
            return Ok(());
        };
        self.status = "Working…".into();
        self.deadline = None;
        self.turn.clear();
        if self.config.provider == Provider::Acp {
            self.call(
                "session/prompt",
                json!({"sessionId":self.conversation,"prompt":[{"type":"text","text":text}]}),
                Call::Turn,
            )
        } else {
            let mut params =
                json!({"threadId":self.conversation,"input":[{"type":"text","text":text}]});
            if self.external {
                // bwrap already enforces the project policy for this process
                // and every child. Tell Codex to use that external sandbox.
                params["sandboxPolicy"] = json!({"type":"externalSandbox","networkAccess":if self.network {"enabled"} else {"restricted"}});
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
                    let mut params = json!({"cwd":self.root,"approvalPolicy":"on-request","sandbox":if self.read_only {"read-only"} else {"workspace-write"}});
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
