//! Read older Codex prompts in bounded pages without resuming the live thread,
//! claiming its writer lease, starting inference or changing tool permissions.
use crate::{Event, Launch, Session, protocol::clip};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
enum Call {
    Initialize,
    Metadata,
    Page,
}

/// A read-only cursor over the saved conversation. The desktop retains recalled
/// prompts locally; the normal bounded transcript/host IPC do not grow with it.
pub struct Reader {
    launch: Launch,
    session: Option<Session>,
    thread: String,
    pending: Option<(u64, Call)>,
    serial: u64,
    seek_pages: usize,
    known: Vec<String>,
    cursor: Option<String>,
    cursors: VecDeque<String>,
    ready: Vec<String>,
    want: bool,
    anchored: bool,
    done: bool,
    deadline: Instant,
    error: Option<String>,
}

impl Reader {
    /// `known` is the recent suffix already available in the composer, including
    /// locally accepted prompts which might have failed before reaching Codex.
    /// Match entries in order without deduplicating identical submissions.
    /// The live thread can remain active; hold new local sends only until the
    /// first turn page anchors the cursor.
    pub fn open(
        launch: Launch,
        thread: &str,
        known: Vec<String>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        ensure!(
            !launch.config.provider.is_acp(),
            "This agent does not provide read-only prompt history pagination"
        );
        ensure!(
            !thread.is_empty() && thread.len() <= 256,
            "Invalid conversation ID"
        );
        let session = Session::history(launch.clone(), wake)?;
        let mut reader = Self {
            launch,
            session: Some(session),
            thread: thread.into(),
            pending: None,
            serial: 0,
            seek_pages: 0,
            known,
            cursor: None,
            cursors: VecDeque::new(),
            ready: vec![],
            want: true,
            anchored: false,
            done: false,
            deadline: Instant::now(),
            error: None,
        };
        reader.call("initialize", json!({"clientInfo":{"name":"rode","title":"RODE prompt history","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}), Call::Initialize)?;
        Ok(reader)
    }

    /// Only bootstrap briefly holds new submissions; later pages are anchored
    /// before the live thread's subsequently appended turns.
    pub fn anchoring(&self) -> bool {
        !self.anchored && !self.done
    }
    /// There may be older prompts, even if intervening turns have no user item.
    pub fn more(&self) -> bool {
        !self.done
    }
    /// Whether host I/O currently needs repaint/polling.
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    /// Ask for the next older prompt; repeated arrows coalesce while loading.
    pub fn load_more(&mut self) -> Result<()> {
        self.want = true;
        if self.pending.is_none() && !self.done {
            self.seek_pages = 0;
            self.deadline = Instant::now() + Duration::from_secs(60);
            self.page()?;
        }
        Ok(())
    }
    /// Chronological prompts older than the composer's known suffix.
    pub fn take_prompts(&mut self) -> Vec<String> {
        std::mem::take(&mut self.ready)
    }
    /// Report a read failure once, preserving the live chat and known recall.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// Drain only bounded metadata events. Unsolicited tool/permission calls
    /// receive errors and cannot run through this connection.
    pub fn poll(&mut self) {
        for _ in 0..64 {
            let Some(event) = self.session.as_ref().and_then(Session::poll) else {
                break;
            };
            let result = match event {
                Event::Message(value) => self.receive(value),
                Event::Error(error) => Err(anyhow::anyhow!(error)),
                Event::Exited(_) => Err(anyhow::anyhow!("Prompt history connection closed")),
                Event::Started | Event::Stderr(_) => Ok(()),
            };
            if let Err(error) = result {
                self.fail(error);
                break;
            }
        }
        if self.pending.is_some() && Instant::now() > self.deadline {
            self.fail(anyhow::anyhow!("Prompt history request timed out"));
        }
    }

    fn send(&self, value: Value) -> Result<()> {
        self.session
            .as_ref()
            .context("Prompt history connection closed")?
            .send(&value)
    }
    fn call(&mut self, method: &str, params: Value, kind: Call) -> Result<()> {
        self.serial += 1;
        self.send(json!({"id":self.serial,"method":method,"params":params}))?;
        self.pending = Some((self.serial, kind));
        if !matches!(kind, Call::Page) {
            self.deadline = Instant::now() + Duration::from_secs(60);
        }
        Ok(())
    }
    fn page(&mut self) -> Result<()> {
        // Bound one seek across empty/tool-only turns, not the conversation's
        // age. Every successful older-prompt request gets its own budget.
        ensure!(
            self.seek_pages < 512,
            "Prompt history seek exceeded 512 pages without finding an earlier prompt"
        );
        self.seek_pages += 1;
        let mut params =
            json!({"threadId":self.thread,"limit":1,"sortDirection":"desc","itemsView":"summary"});
        if let Some(cursor) = &self.cursor {
            params["cursor"] = cursor.clone().into();
        }
        self.call("thread/turns/list", params, Call::Page)
    }
    fn receive(&mut self, value: Value) -> Result<()> {
        if value.get("method").is_some() {
            if let Some(id) = value.get("id") {
                self.send(json!({"id":id,"error":{"code":-32601,"message":"Prompt history is read-only and provides no executable tools"}}))?;
            }
            return Ok(());
        }
        let Some((id, kind)) = self.pending else {
            return Ok(());
        };
        if value["id"].as_u64() != Some(id) {
            return Ok(());
        }
        self.pending = None;
        if let Some(error) = value.get("error") {
            bail!("{}", clip(&error.to_string(), 2048));
        }
        let result = &value["result"];
        match kind {
            Call::Initialize => {
                self.send(json!({"method":"initialized","params":{}}))?;
                self.call(
                    "thread/read",
                    json!({"threadId":self.thread,"includeTurns":false}),
                    Call::Metadata,
                )?;
            }
            Call::Metadata => {
                ensure!(
                    result["thread"]["id"] == self.thread
                        && crate::history::in_project(&result["thread"], &self.launch, false),
                    "Prompt history belongs to another project or provider"
                );
                // Active threads are readable; never resume or acquire ownership.
                self.page()?;
            }
            Call::Page => {
                let turns = result["data"]
                    .as_array()
                    .context("Missing prompt history page")?;
                ensure!(
                    turns.len() <= 1,
                    "Prompt history page exceeds its requested size"
                );
                self.anchored = true;
                for turn in turns {
                    for prompt in prompts(turn).into_iter().rev() {
                        if let Some(index) = self
                            .known
                            .iter()
                            .rposition(|known| known == &prompt)
                            .or_else(|| {
                                self.known.iter().rposition(|known| {
                                    known.strip_prefix(&prompt).is_some_and(|suffix| {
                                        suffix.starts_with("\n\nAttachments: ")
                                            || suffix.starts_with("\nAttached document: ")
                                            || suffix.starts_with("\nAttached image: ")
                                    }) || prompt == "Attached files."
                                        && known.starts_with("Attachments: ")
                                })
                            })
                        {
                            // A later local submission may never have reached
                            // the backend. Keep it in the composer, but do not
                            // skip an additional real prompt on its behalf.
                            self.known.truncate(index);
                        } else if !self.known.is_empty() {
                            continue;
                        } else {
                            self.ready.insert(0, prompt);
                        }
                    }
                }
                self.cursor = match result["nextCursor"].as_str() {
                    Some(cursor) if !cursor.is_empty() => {
                        ensure!(
                            cursor.len() <= 4096
                                && !cursor.chars().any(char::is_control)
                                && !self.cursors.iter().any(|old| old == cursor),
                            "Invalid or repeated prompt history cursor"
                        );
                        self.cursors.push_back(cursor.into());
                        if self.cursors.len() > 32 {
                            self.cursors.pop_front();
                        }
                        Some(cursor.into())
                    }
                    _ => None,
                };
                if self.cursor.is_none() {
                    self.done = true;
                    self.session = None;
                }
                if !self.ready.is_empty() {
                    self.want = false;
                }
                if !self.done && (!self.known.is_empty() || self.want) {
                    self.page()?;
                }
            }
        }
        Ok(())
    }
    fn fail(&mut self, error: anyhow::Error) {
        self.error = Some(format!("Cannot read earlier prompts: {error:#}"));
        self.pending = None;
        self.session = None;
        self.done = true;
    }
}

/// Ignore tools and response-style blocks, while retaining duplicate user items
/// and explicitly marking oversized original prompts just like recent recall.
fn prompts(turn: &Value) -> Vec<String> {
    turn["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "userMessage")
        .filter_map(|item| {
            let text = item["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| block["text"].as_str())
                .map(crate::personality::preview_text)
                // RODE sends the original typed prompt first. Following text
                // blocks contain attachments or response-style instructions,
                // which must not become part of a recalled input.
                .find(|text| !text.is_empty())
                .unwrap_or_default();
            let text = if text.is_empty() {
                item["text"].as_str().unwrap_or_default()
            } else {
                text
            };
            let text = crate::personality::preview_text(text);
            (!text.trim().is_empty()).then(|| clip(text, crate::PROMPT_LIMIT))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recall_contains_typed_prompts_without_attachment_bodies_or_style_instructions() {
        let turn = json!({"items":[
            {"type":"userMessage","content":[
                {"type":"text","text":"Read these notes"},
                {"type":"text","text":"Attached document: notes.txt\n\nDocument contents"},
                {"type":"text","text":"<rode_response_style name=\"default\">style</rode_response_style>"}
            ]},
            {"type":"commandExecution","aggregatedOutput":"not a user prompt"},
            {"type":"userMessage","text":"Another question\n\n<rode_response_style name=\"default\">style</rode_response_style>"}
        ]});
        assert_eq!(prompts(&turn), ["Read these notes", "Another question"]);
    }
}
