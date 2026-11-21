//! Last-turn undo ownership stays with the host controller across detach/reattach.
//! Only a recorded patch and exact post-turn versions can authorize restoration.
use super::*;
use rode_host_core::agent_undo::{Request, Saved};

#[derive(Default)]
pub(super) struct Undo {
    saved: Option<Saved>,
    pub view: rode_agent_core::undo::View,
}
impl Undo {
    pub fn clear(&mut self) {
        self.saved = None;
        self.view.files.clear();
        self.view.reason.clear();
    }
    pub fn completed(&mut self, spec: &Spec, chat: &Chat) {
        self.clear();
        self.view.turn = chat.completed;
        let result = (|| -> Result<Saved> {
            ensure!(
                !chat.stopping() && !chat.file_work_pending(),
                "File operations were still active when the prompt finished; no safe undo record is available"
            );
            ensure!(!spec.plan.project_read_only, "The project is read only");
            let patch = chat.undo_patch()?;
            ensure!(
                !patch.trim().is_empty(),
                "No reversible file edits were reported for this prompt"
            );
            invoke(
                spec,
                Request::Prepare {
                    root: spec.plan.root.clone(),
                    patch,
                    protected_paths: vec![],
                },
            )
        })();
        match result {
            Ok(saved) => {
                self.view.files = saved.files.iter().map(|(p, _)| p.clone()).collect();
                self.saved = Some(saved);
            }
            Err(error) => {
                self.view.reason = rode_agent_core::protocol::clip(&format!("{error:#}"), 4096)
            }
        }
    }
    pub fn apply(&mut self, spec: &Spec, turn: u64) -> Result<()> {
        ensure!(
            turn == self.view.turn,
            "The last agent prompt changed; review its current file changes"
        );
        let saved = self
            .saved
            .as_ref()
            .context("No reversible file edits are available")?
            .clone();
        invoke(
            spec,
            Request::Apply {
                root: spec.plan.root.clone(),
                saved,
                read_only: spec.plan.project_read_only,
                protected_paths: vec![],
            },
        )?;
        self.clear();
        self.view.reason = "The last prompt's file edits were undone".into();
        Ok(())
    }
}
fn invoke(spec: &Spec, mut request: Request) -> Result<Saved> {
    let mut plan: rode_host_core::sandbox::Plan = serde_json::from_value(
        spec.file_tool_plan
            .clone()
            .context("This agent lacks captured undo permissions; start a new chat")?,
    )?;
    ensure!(
        plan.root == spec.plan.root
            && plan.active == spec.plan.active
            && plan.project_read_only == spec.plan.project_read_only,
        "Undo does not match the captured project policy"
    );
    plan.native_helper = Some(crate::current_executable()?.to_string_lossy().into_owned());
    plan.environment.mise = false;
    let protected_paths = match &mut request {
        Request::Prepare {
            protected_paths, ..
        }
        | Request::Apply {
            protected_paths, ..
        } => protected_paths,
    };
    *protected_paths = rode_host_core::agent_files::configuration_paths(
        std::path::Path::new(&plan.root),
        spec.env.get("CODEX_HOME").map(String::as_str),
        spec.env.get("HOME").map(String::as_str),
        &plan.prefix,
    )?;
    for executable in [&spec.executable, plan.native_helper.as_ref().unwrap()] {
        let path = std::path::Path::new(executable);
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            std::path::Path::new(&plan.root).join(path)
        };
        protected_paths.push(path.to_string_lossy().into_owned());
        protected_paths.push(
            rode_host_core::command::sandbox_host_path(&path, &plan.prefix)
                .to_string_lossy()
                .into_owned(),
        );
    }
    let (mut command, _lease) = plan.coordinator_command("--agent-undo")?;
    command.env_clear().envs(&spec.env).current_dir(&plan.root);
    let bytes = rode_host_core::command::run_bounded_limited(
        command,
        serde_json::to_vec(&request)?,
        Duration::from_secs(30),
        2 * 1024 * 1024,
    )?;
    let reply: std::result::Result<Saved, String> = serde_json::from_slice(&bytes)?;
    reply.map_err(anyhow::Error::msg)
}
