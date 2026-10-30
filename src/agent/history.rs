//! Project-host history discovery and resumable agent-owned transcripts.
//!
//! # Design decisions
//!
//! A short-lived, separately supervised connection leaves the current chat intact
//! while listing or attempting a restore. Only an explicit History action starts
//! it. No model prompt is sent and no second transcript database is created.
//! All operations use the captured host and current outer sandbox. A successful
//! restore transfers the initialized process to Chat; errors/cancellation drop
//! only this browser's process. Metadata is filtered again on the client, and
//! restored Codex permissions are overridden before any later turn can run.
//! Pages, messages, bytes, replay time and cursors are bounded. History transport
//! allows 2 MiB responses; the retained view keeps Chat's 128-message/512-KiB cap.

use super::{
    Access, Event, Launch, Provider, Session, controls,
    protocol::{Message, Role, clip},
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    path::Path,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// A host/project-scoped saved conversation returned by the agent.
pub struct Entry {
    /// Opaque backend session/thread identity.
    pub id: String,
    /// Human-readable title, bounded and safe for native text widgets.
    pub title: String,
    /// Short saved-message preview when supplied by the agent.
    pub preview: String,
    /// Relative recency or backend-supplied metadata.
    pub updated: String,
    /// Whether the backend reports an already-active turn.
    pub active: bool,
}

/// A fully acknowledged restored session, transferred exactly once into Chat.
pub struct Restored {
    pub(crate) session: Session,
    pub(crate) launch: Launch,
    pub(crate) id: String,
    pub(crate) messages: VecDeque<Message>,
    pub(crate) controls: Vec<controls::Control>,
    pub(crate) next_id: u64,
    pub(crate) limited: bool,
}

#[derive(Clone, Copy)]
enum Call {
    Initialize,
    List,
    Read,
    Resume,
    Turns,
    Load,
}

/// An explicit, bounded history-browser connection. Drop cancels owned work.
pub struct Browser {
    /// Saved conversations for this project; never persisted by RODE.
    pub entries: Vec<Entry>,
    /// Human-readable load/support/error status.
    pub status: String,
    /// Whether startup, listing or restoring is pending.
    pub busy: bool,
    /// Whether this agent can replay/resume listed sessions.
    pub can_resume: bool,
    /// Whether another metadata page can be requested.
    pub more: bool,
    launch: Launch,
    session: Option<Session>,
    next_id: u64,
    pending: Option<(u64, Call)>,
    cursor: Option<String>,
    cursors: BTreeSet<String>,
    selected: String,
    pages: Vec<Vec<Message>>,
    replay: VecDeque<Message>,
    replay_controls: Vec<controls::Control>,
    replay_serial: u64,
    limited: bool,
    deadline: Instant,
    stderr: String,
    restored: Option<Restored>,
}

impl Browser {
    /// Start only protocol initialization on the selected host; no thread/turn is created.
    pub fn open(launch: Launch, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let session = Session::history(launch.clone(), wake)?;
        Ok(Self {
            entries: vec![],
            status: "Loading saved conversations…".into(),
            busy: true,
            can_resume: false,
            more: false,
            launch,
            session: Some(session),
            next_id: 1000,
            pending: None,
            cursor: None,
            cursors: BTreeSet::new(),
            selected: String::new(),
            pages: vec![],
            replay: VecDeque::new(),
            replay_controls: vec![],
            replay_serial: 0,
            limited: false,
            deadline: Instant::now() + Duration::from_secs(420),
            stderr: String::new(),
            restored: None,
        })
    }
    /// Request another bounded metadata page without polling in the background.
    pub fn load_more(&mut self) -> Result<()> {
        ensure!(
            !self.busy && self.more,
            "No further history page is available"
        );
        if let Err(error) = self.list() {
            self.fail(&error.to_string());
            return Err(error);
        }
        Ok(())
    }
    /// Restore only a listed conversation, retaining the current chat until success.
    pub fn resume(&mut self, id: &str) -> Result<()> {
        ensure!(
            !self.busy && self.can_resume,
            "This agent cannot resume history right now"
        );
        ensure!(
            self.entries.iter().any(|e| e.id == id && !e.active),
            "Select an idle conversation from this project"
        );
        self.selected = id.into();
        self.pages.clear();
        self.replay.clear();
        self.replay_controls.clear();
        self.replay_serial = 0;
        self.limited = false;
        self.cursors.clear();
        self.busy = true;
        self.deadline = Instant::now() + Duration::from_secs(60);
        self.status = "Restoring conversation…".into();
        let result = if self.launch.config.provider == Provider::Acp {
            self.call(
                "session/load",
                json!({"sessionId":id,"cwd":self.launch.plan.root,"mcpServers":[]}),
                Call::Load,
            )
        } else {
            self.call(
                "thread/read",
                json!({"threadId":id,"includeTurns":false}),
                Call::Read,
            )
        };
        if let Err(error) = result {
            self.fail(&error.to_string());
            return Err(error);
        }
        Ok(())
    }
    /// Drain available process events with a per-frame budget and a total operation deadline.
    pub fn poll(&mut self) {
        for _ in 0..64 {
            let Some(event) = self.session.as_ref().and_then(Session::poll) else {
                break;
            };
            let result = match event {
                Event::Started => self.call("initialize",if self.launch.config.provider == Provider::Acp {
                    json!({"protocolVersion":1,"clientInfo":{"name":"rode","title":"RODE","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false,"session":{"configOptions":{"boolean":true}}}})
                } else { json!({"clientInfo":{"name":"rode","title":"RODE","version":env!("CARGO_PKG_VERSION")}}) },Call::Initialize),
                Event::Message(value) => self.receive(value),
                Event::Stderr(text) => { self.stderr=clip(&format!("{}\n{text}",self.stderr),8192); Ok(()) },
                Event::Error(error) => Err(anyhow::anyhow!(error)),
                Event::Exited(code) => { if self.busy {Err(anyhow::anyhow!("History agent exited ({code:?})"))} else {self.session=None; Ok(())} },
            };
            if let Err(error) = result {
                self.fail(&format!("{error:#}"));
                break;
            }
            if self.restored.is_some() {
                break;
            }
        }
        if self.busy && Instant::now() >= self.deadline {
            self.fail("History operation timed out. Retry or check the host connection.");
        }
    }
    /// Transfer the resumed process and bounded replay after the backend acknowledges it.
    pub fn take_restored(&mut self) -> Option<Restored> {
        self.restored.take()
    }

    /// Check the captured project-host interpreter before adopting a loaded process.
    /// Selecting a different environment invalidates pending history work.
    pub fn uses_interpreter(&self, interpreter: &str) -> bool {
        self.launch.python == interpreter
    }

    fn call(&mut self, method: &str, params: Value, call: Call) -> Result<()> {
        ensure!(
            self.pending.is_none(),
            "A history request is already pending"
        );
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":self.next_id,"method":method,"params":params}))?;
        self.pending = Some((self.next_id, call));
        Ok(())
    }
    fn send(&self, value: Value) -> Result<()> {
        self.session
            .as_ref()
            .context("History connection is closed; refresh it")?
            .send(&value)
    }
    fn list(&mut self) -> Result<()> {
        self.busy = true;
        self.deadline = Instant::now() + Duration::from_secs(60);
        self.status = "Loading saved conversations…".into();
        let acp = self.launch.config.provider == Provider::Acp;
        let mut params = json!({"cwd":self.launch.plan.root});
        if let Some(cursor) = &self.cursor {
            params["cursor"] = cursor.clone().into();
        }
        if !acp {
            params["limit"] = 32.into();
            params["sortKey"] = "updated_at".into();
            params["sourceKinds"] = json!(["cli", "vscode", "appServer", "exec", "unknown"]);
            params["modelProviders"] = if self.launch.config.provider == Provider::Local {
                json!(["rode_local"])
            } else {
                json!([])
            };
        }
        self.call(
            if acp { "session/list" } else { "thread/list" },
            params,
            Call::List,
        )
    }
    fn receive(&mut self, value: Value) -> Result<()> {
        if let Some(method) = value["method"].as_str() {
            if let Some(id) = value.get("id") {
                // Browsing/replay never grants tool access or answers questions.
                let result = match method {
                    "session/request_permission" => {
                        Some(json!({"outcome":{"outcome":"cancelled"}}))
                    }
                    "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                        Some(json!({"decision":"cancel"}))
                    }
                    "item/permissions/requestApproval" => {
                        Some(json!({"permissions":{},"scope":"turn"}))
                    }
                    "item/tool/requestUserInput" => Some(json!({"answers":{}})),
                    "mcpServer/elicitation/request" => Some(json!({"action":"cancel"})),
                    _ => None,
                };
                return self.send(if let Some(result)=result {json!({"jsonrpc":"2.0","id":id,"result":result})} else {json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"History browsing does not provide executable client tools"}})});
            }
            if method == "session/update"
                && !self.selected.is_empty()
                && value["params"]["sessionId"] == self.selected
            {
                let update = &value["params"]["update"];
                if update["sessionUpdate"] == "config_option_update" {
                    self.replay_controls = controls::acp(update);
                } else {
                    self.limited |= acp_message(&mut self.replay, update, &mut self.replay_serial);
                }
            }
            return Ok(());
        }
        let Some((id, call)) = self.pending else {
            return Ok(());
        };
        if value["id"].as_u64() != Some(id) {
            return Ok(());
        }
        self.pending = None;
        if let Some(error) = value.get("error") {
            bail!(
                "Agent history: {}",
                error["message"].as_str().unwrap_or("request failed")
            );
        }
        let result = &value["result"];
        match call {
            Call::Initialize => {
                if self.launch.config.provider == Provider::Acp {
                    ensure!(
                        result["protocolVersion"] == 1,
                        "Agent does not support ACP v1"
                    );
                    self.can_resume = result["agentCapabilities"]["loadSession"] == true;
                    if !result["agentCapabilities"]["sessionCapabilities"]["list"].is_object() {
                        self.busy = false;
                        self.status = "This ACP agent does not support a history list.".into();
                        self.session = None;
                        return Ok(());
                    }
                } else {
                    self.can_resume = true;
                    self.send(json!({"method":"initialized","params":{}}))?;
                }
                self.list()?;
            }
            Call::List => {
                let acp = self.launch.config.provider == Provider::Acp;
                let list = if acp {
                    &result["sessions"]
                } else {
                    &result["data"]
                };
                for row in list.as_array().into_iter().flatten().take(128) {
                    if self.entries.len() >= 256 {
                        break;
                    }
                    if !in_project(row, &self.launch, acp) {
                        continue;
                    }
                    let Some(id) = row[if acp { "sessionId" } else { "id" }]
                        .as_str()
                        .filter(|s| controls::identifier(s, false))
                    else {
                        continue;
                    };
                    if self.entries.iter().any(|e| e.id == id) {
                        continue;
                    }
                    let preview = clip(row["preview"].as_str().unwrap_or_default(), 512);
                    let title = clip(
                        row[if acp { "title" } else { "name" }]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .unwrap_or(if preview.is_empty() {
                                "Untitled conversation"
                            } else {
                                &preview
                            }),
                        256,
                    );
                    self.entries.push(Entry {
                        id: id.into(),
                        title,
                        preview,
                        updated: if let Some(date) = row["updatedAt"].as_str() {
                            clip(date, 64)
                        } else {
                            recency(row["updatedAt"].as_u64())
                        },
                        active: row["status"]["type"] == "active",
                    });
                }
                let next = cursor(result);
                self.more = self.entries.len() < 256
                    && self.cursors.len() < 32
                    && next.as_ref().is_some_and(|s| !self.cursors.contains(s));
                if let Some(next) = &next {
                    self.cursors.insert(next.clone());
                }
                self.cursor = next;
                self.busy = false;
                self.status = if !self.can_resume {
                    "This agent lists history but does not support replay/resume.".into()
                } else if self.entries.is_empty() {
                    "No saved conversations for this project.".into()
                } else {
                    format!("{} saved conversations", self.entries.len())
                };
                if self.cursor.is_some() && (self.entries.len() >= 256 || self.cursors.len() >= 32)
                {
                    self.status.push_str(" · browse limit reached");
                }
            }
            Call::Read => {
                let thread = &result["thread"];
                ensure!(
                    thread["id"] == self.selected && in_project(thread, &self.launch, false),
                    "Saved conversation does not belong to this project/provider"
                );
                ensure!(
                    thread["status"]["type"] != "active",
                    "This conversation has an active turn; finish it before resuming"
                );
                let access = if self.launch.plan.project_read_only {
                    Access::ReadOnly
                } else {
                    self.launch.config.access()
                };
                let mut params = json!({"threadId":self.selected,"cwd":self.launch.plan.root,"excludeTurns":true,"sandbox":access.codex(),"approvalPolicy":if access == Access::ReadOnly {"never"} else {self.launch.config.approval.codex()}});
                if !self.launch.config.model().is_empty() {
                    params["model"] = self.launch.config.model().into();
                }
                if self.launch.config.provider == Provider::Local {
                    params["modelProvider"] = "rode_local".into();
                }
                self.call("thread/resume", params, Call::Resume)?;
            }
            Call::Resume => {
                ensure!(
                    result["thread"]["id"] == self.selected
                        && in_project(&result["thread"], &self.launch, false),
                    "Agent resumed a different project or conversation"
                );
                self.cursors.clear();
                self.turns(None)?;
            }
            Call::Turns => {
                let mut page = vec![];
                for turn in result["data"].as_array().into_iter().flatten().take(1) {
                    page.extend(codex_messages(turn));
                }
                self.pages.push(page);
                let next = cursor(result);
                let count: usize = self.pages.iter().map(Vec::len).sum();
                let bytes: usize = self.pages.iter().flatten().map(|m| m.text.len()).sum();
                if let Some(next) = next.filter(|s| !self.cursors.contains(s)) {
                    if self.pages.len() < 32 && count < 128 && bytes < 512 * 1024 {
                        self.cursors.insert(next.clone());
                        self.turns(Some(next))?;
                    } else {
                        self.limited = true;
                        self.finish(vec![])?;
                    }
                } else {
                    self.finish(vec![])?;
                }
            }
            Call::Load => {
                ensure!(
                    result
                        .get("sessionId")
                        .is_none_or(|id| id == &self.selected),
                    "Agent loaded a different conversation"
                );
                self.finish(controls::acp(result))?;
            }
        }
        Ok(())
    }
    fn turns(&mut self, cursor: Option<String>) -> Result<()> {
        self.status = format!("Loading recent history… {} turns", self.pages.len());
        let mut params = json!({"threadId":self.selected,"limit":1,"sortDirection":"desc","itemsView":"summary"});
        if let Some(cursor) = cursor {
            params["cursor"] = cursor.into();
        }
        self.call("thread/turns/list", params, Call::Turns)
    }
    fn finish(&mut self, mut controls: Vec<controls::Control>) -> Result<()> {
        if self.launch.config.provider != Provider::Acp {
            for page in std::mem::take(&mut self.pages).into_iter().rev() {
                self.replay.extend(page);
            }
        } else if controls.is_empty() {
            controls = std::mem::take(&mut self.replay_controls);
        }
        self.limited |= trim(&mut self.replay);
        self.restored = Some(Restored {
            session: self.session.take().context("History connection closed")?,
            launch: self.launch.clone(),
            id: self.selected.clone(),
            messages: std::mem::take(&mut self.replay),
            controls,
            next_id: self.next_id,
            limited: self.limited,
        });
        self.busy = false;
        self.status = "Conversation restored".into();
        Ok(())
    }
    fn fail(&mut self, error: &str) {
        self.status = clip(
            &format!(
                "{error}{}",
                if self.stderr.is_empty() {
                    String::new()
                } else {
                    format!("\n{}", self.stderr)
                }
            ),
            16384,
        );
        self.busy = false;
        self.more = false;
        self.can_resume = false;
        self.pending = None;
        self.session = None;
        self.pages.clear();
        self.replay.clear();
    }
}

fn in_project(row: &Value, launch: &Launch, acp: bool) -> bool {
    row["cwd"]
        .as_str()
        .is_some_and(|s| Path::new(s) == Path::new(&launch.plan.root))
        && (acp
            || if launch.config.provider == Provider::Local {
                row["modelProvider"] == "rode_local"
            } else {
                row["modelProvider"] != "rode_local"
            })
}
fn cursor(value: &Value) -> Option<String> {
    value["nextCursor"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.chars().any(char::is_control))
        .map(str::to_owned)
}
fn recency(updated: Option<u64>) -> String {
    let Some(updated) = updated else {
        return String::new();
    };
    let age = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(updated);
    if age < 60 {
        "just now".into()
    } else if age < 3600 {
        format!("{}m ago", age / 60)
    } else if age < 86400 {
        format!("{}h ago", age / 3600)
    } else {
        format!("{}d ago", age / 86400)
    }
}
fn message(id: String, role: Role, title: &str, text: &str) -> Message {
    Message {
        id: clip(&id, 512),
        role,
        title: clip(title, 256),
        text: clip(text, if role == Role::Tool { 16384 } else { 65536 }),
    }
}
fn codex_messages(turn: &Value) -> Vec<Message> {
    turn["items"]
        .as_array()
        .into_iter()
        .flatten()
        .take(128)
        .enumerate()
        .filter_map(|(n, item)| {
            let id = format!(
                "history:{}:{}:{n}",
                turn["id"].as_str().unwrap_or("turn"),
                item["id"].as_str().unwrap_or("item")
            );
            match item["type"].as_str()? {
                "userMessage" => {
                    let text = item["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .take(32)
                        .filter_map(|c| c["text"].as_str())
                        .map(|s| clip(s, 65536))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Some(message(id, Role::User, "You", &text))
                }
                "agentMessage" => Some(message(
                    id,
                    Role::Assistant,
                    "Agent",
                    item["text"].as_str().unwrap_or_default(),
                )),
                "commandExecution" => Some(message(
                    id,
                    Role::Tool,
                    item["command"].as_str().unwrap_or("Command"),
                    item["aggregatedOutput"].as_str().unwrap_or("Saved command"),
                )),
                "fileChange" => Some(message(
                    id,
                    Role::Tool,
                    "File changes",
                    &clip(&item["changes"].to_string(), 16384),
                )),
                _ => None,
            }
        })
        .collect()
}
fn acp_message(messages: &mut VecDeque<Message>, update: &Value, serial: &mut u64) -> bool {
    let role = match update["sessionUpdate"].as_str().unwrap_or_default() {
        "user_message_chunk" => Role::User,
        "agent_message_chunk" => Role::Assistant,
        "tool_call" | "tool_call_update" => Role::Tool,
        _ => return false,
    };
    let text = if role == Role::Tool {
        clip(
            update["status"].as_str().unwrap_or("Saved tool call"),
            16384,
        )
    } else {
        clip(
            update["content"]["text"].as_str().unwrap_or_default(),
            65536,
        )
    };
    if text.is_empty() {
        return false;
    }
    let limit = if role == Role::Tool { 16384 } else { 65536 };
    let mut limited = false;
    let id = update["messageId"]
        .as_str()
        .or_else(|| update["toolCallId"].as_str())
        .filter(|id| controls::identifier(id, false));
    if let Some(last) = messages
        .back_mut()
        .filter(|m| m.role == role && id.is_none_or(|id| m.id == format!("history:{id}")))
    {
        limited = last.text.len() + text.len() > limit;
        last.text = clip(&format!("{}{text}", last.text), limit);
    } else {
        *serial += 1;
        messages.push_back(message(
            id.map(|id| format!("history:{id}"))
                .unwrap_or_else(|| format!("history:replay:{serial}")),
            role,
            if role == Role::User {
                "You"
            } else if role == Role::Assistant {
                "Agent"
            } else {
                "Tool"
            },
            &text,
        ));
    }
    trim(messages) || limited
}
fn trim(messages: &mut VecDeque<Message>) -> bool {
    let before = messages.len();
    while messages.len() > 128
        || messages
            .iter()
            .map(|m| m.text.len() + m.title.len() + m.id.len())
            .sum::<usize>()
            > 512 * 1024
    {
        messages.pop_front();
    }
    messages.len() != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_omits_hidden_reasoning_and_bounds_multibyte_messages() {
        let text = "🦀".repeat(100_000);
        let messages = codex_messages(&json!({"id":"turn","items":[
            {"type":"reasoning","summary":[{"text":"private reasoning"}]},
            {"type":"agentMessage","text":text}
        ]}));
        assert_eq!(messages.len(), 1);
        assert!(messages[0].text.len() <= 65536);
        let mut replay = VecDeque::new();
        let mut limited = false;
        let mut serial = 0;
        for n in 0..200 {
            limited |= acp_message(
                &mut replay,
                &json!({
                    "sessionUpdate":"agent_message_chunk","messageId":n.to_string(),
                    "content":{"type":"text","text":text}
                }),
                &mut serial,
            );
        }
        assert!(limited);
        assert!(replay.len() <= 128);
        assert!(
            replay
                .iter()
                .map(|m| m.text.len() + m.id.len() + m.title.len())
                .sum::<usize>()
                <= 512 * 1024
        );

        // ACP may omit message IDs. Evicting old rows must not reuse widget IDs.
        replay.clear();
        for n in 0..200 {
            acp_message(
                &mut replay,
                &json!({
                    "sessionUpdate":if n % 2 == 0 {"user_message_chunk"} else {"agent_message_chunk"},
                    "content":{"type":"text","text":"small message"}
                }),
                &mut serial,
            );
        }
        assert_eq!(replay.len(), 128);
        assert_eq!(
            replay.iter().map(|m| &m.id).collect::<BTreeSet<_>>().len(),
            128
        );
    }
}
