//! Codex's file-only tool surface and process-level restrictions.
//!
//! Native host helpers enforce paths and revisions. Codex's own tools stay
//! read-only; disabling shell tools alone would still leave configured MCP servers.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

/// Client response or thread-start continuation belonging to a single process.
pub(crate) enum Target {
    Verify(Value),
    Tool { id: Value, name: String },
}

struct Job {
    target: Option<Target>,
    cancel: Arc<AtomicBool>,
    result: mpsc::Receiver<std::result::Result<Value, String>>,
    worker: std::thread::JoinHandle<()>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// Keep native I/O off the controller thread and retain cancelled workers until
/// their helper processes have exited. Late results never enter another turn.
#[derive(Default)]
pub(crate) struct Jobs {
    active: Vec<Job>,
    retired: Vec<Job>,
}
impl Jobs {
    /// Start a bounded native tool worker with a process-specific reply target. Active and retiring
    /// workers share the concurrency limit; spawn/limit errors leave no admitted job.
    pub fn start(
        &mut self,
        factory: Arc<dyn crate::Factory>,
        tool: &str,
        arguments: Value,
        wake: Arc<dyn Fn() + Send + Sync>,
        target: Target,
    ) -> Result<()> {
        self.retired.retain(|job| !job.worker.is_finished());
        ensure!(
            self.active.len() + self.retired.len() < 4,
            "File tools are busy; wait for the current operations to finish"
        );
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = cancel.clone();
        let tool = tool.to_owned();
        let (send, result) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("rode-agent-files".into())
            .spawn(move || {
                let value = if tool == "rode_run_command" {
                    factory.command_tool(&arguments, &stopped)
                } else {
                    factory.file_tool(&tool, &arguments, &stopped)
                }
                .map_err(|e| format!("{e:#}"));
                let _ = send.send(value);
                wake();
            })?;
        self.active.push(Job {
            target: Some(target),
            cancel,
            result,
            worker,
        });
        Ok(())
    }

    /// Collect completed active replies without blocking. Cancelled workers are only reaped from
    /// retirement; their late results are never delivered to another turn.
    pub fn poll(&mut self) -> Vec<(Target, std::result::Result<Value, String>)> {
        self.retired.retain(|job| !job.worker.is_finished());
        let mut ready = vec![];
        let mut index = 0;
        while index < self.active.len() {
            let result = match self.active[index].result.try_recv() {
                Ok(result) => result,
                Err(mpsc::TryRecvError::Empty) => {
                    index += 1;
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    Err("Native file tool worker stopped unexpectedly".into())
                }
            };
            let mut job = self.active.remove(index);
            ready.push((job.target.take().expect("active file operation"), result));
            // A worker can have sent its result just before returning.
            if !job.worker.is_finished() {
                self.retired.push(job);
            }
        }
        ready
    }

    /// Signal cancellation for every active worker and return its reply targets. Retain the workers
    /// until they finish, so cancellation is not mistaken for acknowledged cleanup.
    pub fn cancel(&mut self) -> Vec<Target> {
        let mut targets = vec![];
        for mut job in self.active.drain(..) {
            job.cancel.store(true, Ordering::Release);
            targets.push(job.target.take().expect("active file operation"));
            self.retired.push(job);
        }
        targets
    }

    /// Whether active or retiring workers still prevent controller shutdown.
    pub fn pending(&self) -> bool {
        !self.active.is_empty() || self.stopping()
    }
    /// Whether a worker still awaits exit after delivering its reply or receiving cancellation.
    pub fn stopping(&self) -> bool {
        self.retired.iter().any(|job| !job.worker.is_finished())
    }
}

/// Process-level overrides apply before any thread, hook or tool can start.
pub const LAUNCH_OVERRIDES: &[&str] = &[
    concat!(
        "features={shell_tool=false,shell_snapshot=false,shell_snapshot_v2=false,",
        "hooks=false,plugins=false,apps=false,multi_agent=false,multi_agent_v2=false,",
        "code_mode=false,code_mode_host=false,code_mode_only=false,computer_use=false,",
        "browser_use=false,browser_use_external=false,skill_mcp_dependency_install=false,",
        "view_image=false,memories=false,tool_suggest=false,recommended_plugins=false,goals=false}"
    ),
    "web_search=\"disabled\"",
    "notify=[]",
    "shell_environment_policy.experimental_use_profile=false",
    "skills.bundled.enabled=false",
];

/// Guidance describes the supplied tools; host checks enforce their restrictions.
pub const INSTRUCTIONS: &str = "RODE file-only mode: use only the supplied rode_ file tools for project files. Paths must be project-relative. Read files first and pass their revision when replacing or deleting them; use revision=null only to create a new file. Create missing parent directories with rode_create_directory. No shell, process, terminal, build, test, installation, Git command, MCP or plugin execution is available. Ask the user to run any required commands. Never use built-in apply_patch; all edits go through the RODE tools. Files under .git and .codex, symlinks and special files are unavailable.";

/// Disable each configured MCP server explicitly. Empty tables merge with inherited
/// configuration and therefore do not remove servers. Nothing is written to config files.
pub fn thread_config(config: &Value) -> Result<Value> {
    ensure!(
        config["web_search"] == "disabled",
        "Codex did not confirm that web search is disabled; confined native tools cannot start"
    );
    for feature in [
        "shell_tool",
        "hooks",
        "plugins",
        "apps",
        "multi_agent",
        "multi_agent_v2",
        "code_mode",
        "code_mode_host",
        "code_mode_only",
        "computer_use",
        "browser_use",
        "browser_use_external",
        "view_image",
        "memories",
        "skill_mcp_dependency_install",
    ] {
        ensure!(
            config["features"][feature] == false,
            "Codex did not confirm that {feature} is disabled; confined native tools cannot start"
        );
    }
    ensure!(
        config["notify"].is_null() || config["notify"].as_array().is_some_and(Vec::is_empty),
        "Codex notification commands must be disabled in file-only mode"
    );
    let mut overrides = serde_json::Map::new();
    ensure!(
        config["mcp_servers"].is_object(),
        "Codex did not report its MCP configuration; file-only mode cannot start"
    );
    if let Some(servers) = config["mcp_servers"].as_object() {
        ensure!(
            servers.len() <= 128,
            "Too many configured MCP servers for file-only mode"
        );
        for name in servers.keys() {
            ensure!(
                !name.is_empty()
                    && name.len() <= 256
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')),
                "Unsupported MCP server name in file-only mode"
            );
            overrides.insert(format!("mcp_servers.{name}.enabled"), false.into());
        }
    }
    Ok(overrides.into())
}

/// Only these client-owned tool names may dispatch native filesystem operations.
pub fn supported(name: &str) -> bool {
    matches!(
        name,
        "rode_list_files"
            | "rode_read_file"
            | "rode_search_files"
            | "rode_write_file"
            | "rode_delete_file"
            | "rode_create_directory"
    )
}

/// Experimental app-server dynamic tools. These schemas contain no command or
/// executable fields; the host also rejects unknown fields and operation names.
pub fn tools() -> Value {
    let mut tools = vec![];
    for (name, description, properties, required) in [
        (
            "rode_list_files",
            "List immediate children of a project-relative directory. Use path='.' for the project root.",
            json!({"path":{"type":"string"}}),
            vec!["path"],
        ),
        (
            "rode_read_file",
            "Read a bounded UTF-8 project file and its revision. Symlinks and protected paths are rejected.",
            json!({"path":{"type":"string"}}),
            vec!["path"],
        ),
        (
            "rode_search_files",
            "Search literal text recursively in bounded project source files. Returns matching paths and line numbers; never runs a search program.",
            json!({"path":{"type":"string"},"text":{"type":"string"}}),
            vec!["path", "text"],
        ),
        (
            "rode_write_file",
            "Atomically create or replace a UTF-8 project file. Use the revision from rode_read_file, or null to create without overwriting. Existing parent directories are required.",
            json!({"path":{"type":"string"},"text":{"type":"string"},"revision":{"type":["string","null"]}}),
            vec!["path", "text", "revision"],
        ),
        (
            "rode_delete_file",
            "Delete a regular project file only if its revision still matches the last read.",
            json!({"path":{"type":"string"},"revision":{"type":"string"}}),
            vec!["path", "revision"],
        ),
        (
            "rode_create_directory",
            "Create one project directory. Its parent must already exist. No permissions or executable arguments are accepted.",
            json!({"path":{"type":"string"}}),
            vec!["path"],
        ),
    ] {
        tools.push(json!({"type":"function","name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}));
    }
    tools.into()
}
