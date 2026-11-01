//! Short-lived model discovery using the captured project-host transport.
use super::{Model, identifier, models};
use crate::{Config, Event, Launch, Provider, Session};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// Model metadata loaded independently of a conversation, without a model turn.
pub struct Catalog {
    /// Bounded visible models and their supported reasoning efforts.
    pub models: Vec<Model>,
    /// Discovery failure, retained so drawing the menu does not retry indefinitely.
    pub error: Option<String>,
    config: Config,
    session: Option<Session>,
    deadline: Instant,
    initialized: bool,
    next_id: u64,
    pending: Option<u64>,
    cursors: Vec<String>,
}

impl Catalog {
    /// Initialize a metadata-only connection on the selected host and sandbox.
    /// Never creates, resumes or submits a conversation.
    pub fn open(launch: Launch, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let config = launch.config.clone();
        let timeout = if launch.remote { 420 } else { 60 };
        let result = (|| -> Result<_> {
            ensure!(
                launch.config.provider == Provider::Codex,
                "Only Codex advertises this model catalog"
            );
            launch.config.arguments(false)?;
            Session::history(launch, wake)
        })();
        let (session, error) = match result {
            Ok(session) => (Some(session), None),
            Err(error) => (
                None,
                Some(crate::protocol::clip(&format!("{error:#}"), 2048)),
            ),
        };
        Self {
            models: vec![],
            error,
            config,
            session,
            deadline: Instant::now() + Duration::from_secs(timeout),
            initialized: false,
            next_id: 0,
            pending: None,
            cursors: vec![],
        }
    }

    /// Whether discovery belongs to the currently selected provider/executable.
    pub fn matches(&self, config: &Config) -> bool {
        self.config.same_process(config)
    }

    /// Whether a connection or model-list response is still pending.
    pub fn busy(&self) -> bool {
        self.session.is_some()
    }

    /// Process available metadata and release the process after the final page.
    pub fn poll(&mut self) {
        for _ in 0..64 {
            let Some(event) = self.session.as_ref().and_then(Session::poll) else {
                break;
            };
            let result = match event {
                Event::Started => {
                    self.deadline = Instant::now() + Duration::from_secs(60);
                    self.call("initialize", json!({"clientInfo":{"name":"rode","title":"RODE","version":env!("CARGO_PKG_VERSION")}}))
                }
                Event::Message(value) => self.receive(value),
                Event::Stderr(_) => Ok(()),
                Event::Error(error) => Err(anyhow::anyhow!(error)),
                Event::Exited(code) => Err(anyhow::anyhow!(
                    "Model discovery exited before completing ({code:?})"
                )),
            };
            if let Err(error) = result {
                self.error = Some(crate::protocol::clip(&format!("{error:#}"), 2048));
                self.session = None;
            }
        }
        if self.busy() && Instant::now() >= self.deadline {
            self.error = Some("Model discovery timed out. Check the agent executable and authentication on this host.".into());
            self.session = None;
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<()> {
        let id = self.next_id;
        self.next_id += 1;
        self.session
            .as_ref()
            .unwrap()
            .send(&json!({"id":id,"method":method,"params":params}))?;
        self.pending = Some(id);
        Ok(())
    }

    fn receive(&mut self, value: Value) -> Result<()> {
        if value.get("method").is_some()
            || self.pending.is_none()
            || value["id"].as_u64() != self.pending
        {
            return Ok(());
        }
        self.pending = None;
        if let Some(error) = value.get("error") {
            anyhow::bail!(
                "{}",
                error["message"]
                    .as_str()
                    .unwrap_or("The agent could not provide a model catalog")
            );
        }
        if !self.initialized {
            self.initialized = true;
            self.session
                .as_ref()
                .unwrap()
                .send(&json!({"method":"initialized","params":{}}))?;
            return self.call("model/list", json!({"limit":32,"includeHidden":false}));
        }
        let result = &value["result"];
        for model in models(result) {
            if self.models.len() >= 128 {
                break;
            }
            if !self.models.iter().any(|m| m.value.id == model.value.id) {
                self.models.push(model);
            }
        }
        if let Some(cursor) = result["nextCursor"]
            .as_str()
            .filter(|s| identifier(s, false))
            && self.cursors.len() < 3
            && self.models.len() < 128
            && !self.cursors.iter().any(|c| c == cursor)
        {
            self.cursors.push(cursor.into());
            self.call(
                "model/list",
                json!({"limit":32,"includeHidden":false,"cursor":cursor}),
            )?;
        } else {
            self.session = None;
        }
        Ok(())
    }
}
