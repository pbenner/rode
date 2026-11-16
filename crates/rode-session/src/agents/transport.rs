//! Host-side stdio transport. The spawning thread remains alive until the
//! owned process exits, including bubblewrap's --die-with-parent lifetime.
use super::*;
use rode_runtime_core::process::ProcessGroup;
use std::{
    io::BufReader,
    os::unix::process::CommandExt,
    process::{Command as ProcessCommand, Stdio},
    sync::atomic::AtomicUsize,
};

/// Captured host launch policy/executable/environment and a count of active owned
/// process workers; spawning threads outlive the pipes to preserve sandbox parent lifetime.
pub(super) struct Factory {
    pub spec: Spec,
    pub active: Arc<AtomicUsize>,
}
impl rode_agent_core::Factory for Factory {
    fn command_tool(&self, arguments: &Value, cancel: &AtomicBool) -> Result<Value> {
        let plan: rode_host_core::sandbox::Plan = serde_json::from_value(
            self.spec
                .file_tool_plan
                .clone()
                .context("This controller lacks offline tool support; start a new chat")?,
        )?;
        ensure!(
            plan.root == self.spec.plan.root
                && plan.active == self.spec.plan.active
                && plan.project_read_only == self.spec.plan.project_read_only,
            "Offline command tools do not match the captured project policy"
        );
        rode_host_core::agent_commands::run(&plan, arguments, &self.spec.env, cancel)
    }
    fn file_tool(&self, tool: &str, arguments: &Value, cancel: &AtomicBool) -> Result<Value> {
        let mut plan: rode_host_core::sandbox::Plan = serde_json::from_value(
            self.spec
                .file_tool_plan
                .clone()
                .context("This controller lacks file-only support; start a new chat")?,
        )?;
        ensure!(
            plan.root == self.spec.plan.root
                && plan.active == self.spec.plan.active
                && plan.project_read_only == self.spec.plan.project_read_only,
            "Native file tools do not match the agent's captured project policy"
        );
        plan.native_helper = Some(crate::current_executable()?.to_string_lossy().into_owned());
        plan.environment.mise = false;
        let (mut command, _lease) = plan.coordinator_command("--agent-file-tool")?;
        command
            .env_clear()
            .envs(&self.spec.env)
            .current_dir(&plan.root);
        let request = rode_host_core::agent_files::Request {
            root: plan.root.clone(),
            tool: tool.into(),
            arguments: arguments.clone(),
            read_only: plan.project_read_only,
            protected_configuration: rode_host_core::agent_files::configuration_paths(
                std::path::Path::new(&plan.root),
                self.spec.env.get("CODEX_HOME").map(String::as_str),
                self.spec.env.get("HOME").map(String::as_str),
                &plan.prefix,
            )?,
            protected_paths: std::iter::once(self.spec.executable.clone())
                .chain(std::iter::once(
                    rode_host_core::command::sandbox_host_path(
                        std::path::Path::new(&self.spec.executable),
                        &plan.prefix,
                    )
                    .to_string_lossy()
                    .into_owned(),
                ))
                .collect(),
        };
        let output = rode_host_core::command::run_controlled(
            command,
            serde_json::to_vec(&request)?,
            Duration::from_secs(10),
            cancel,
            false,
        )?
        .checked()?;
        rode_host_core::agent_files::response(&output)
    }
    fn start(
        &self,
        launch: &rode_agent_core::Launch,
        history: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<rode_agent_core::Session> {
        let mut argv = launch.config.arguments(!history)?;
        argv[0] = self.spec.executable.clone();
        // A normal-mode plan may still carry an optional environment launcher.
        let mut argv_with_policy = self.spec.plan.controller_prefix(launch.config.network)?;
        argv_with_policy.extend(argv);
        let mut command = rode_host_core::command::from_argv(&argv_with_policy)?;
        command
            .current_dir(&self.spec.plan.root)
            .env_clear()
            .envs(&self.spec.env)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        rode_host_core::private_tmp::command(&mut command)?;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let (input, incoming) = mpsc::sync_channel::<Vec<u8>>(32);
        let (output, events) = rode_agent_core::io::event_channel();
        let control = Arc::new(ProcessGroup::default());
        let pipe = Pipe {
            input,
            events,
            control: control.clone(),
        };
        let active = self.active.clone();
        active.fetch_add(1, Ordering::AcqRel);
        std::thread::spawn(move || {
            struct Running(Arc<AtomicUsize>);
            impl Drop for Running {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::AcqRel);
                }
            }
            let running = Running(active);
            let result = run(
                command,
                incoming,
                output.clone(),
                control.clone(),
                wake.clone(),
            );
            if let Err(error) = result {
                emit(
                    &output,
                    &wake,
                    rode_agent_core::Event::Error(format!("Cannot start agent: {error:#}")),
                );
            }
            control.finish();
            drop(running);
            wake();
        });
        Ok(rode_agent_core::Session::new(pipe))
    }
}
/// Bounded agent input/event channels; the spawning worker retains actual child ownership.
struct Pipe {
    input: mpsc::SyncSender<Vec<u8>>,
    events: rode_agent_core::io::EventReceiver,
    control: Arc<ProcessGroup>,
}
impl rode_agent_core::Transport for Pipe {
    fn send(&self, message: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(message)?;
        ensure!(
            bytes.len() < rode_agent_core::LINE_LIMIT,
            "Agent input exceeds 4 MiB"
        );
        bytes.push(b'\n');
        self.input
            .try_send(bytes)
            .context("Agent input queue is full or disconnected")
    }
    fn poll(&self) -> Option<rode_agent_core::Event> {
        self.events.poll()
    }
    fn stop(&self) {
        self.control.stop();
    }
}
fn emit(
    output: &rode_agent_core::io::EventSender,
    wake: &Arc<dyn Fn() + Send + Sync>,
    event: rode_agent_core::Event,
) -> bool {
    wake();
    let sent = output.send(event);
    wake();
    sent
}
fn run(
    mut command: ProcessCommand,
    input: mpsc::Receiver<Vec<u8>>,
    output: rode_agent_core::io::EventSender,
    control: Arc<ProcessGroup>,
    wake: Arc<dyn Fn() + Send + Sync>,
) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(1);
    let mut child = loop {
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(10))
            }
            result => break result?,
        }
    };
    control.started(child.id());
    let mut stdin = child.stdin.take().context("Missing agent stdin")?;
    std::thread::spawn(move || {
        for bytes in input {
            if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
                break;
            }
        }
    });
    let stdout = child.stdout.take().context("Missing agent stdout")?;
    let stderr = child.stderr.take().context("Missing agent stderr")?;
    let mut readers = vec![];
    for (reader, rpc) in [
        (Box::new(stdout) as Box<dyn Read + Send>, true),
        (Box::new(stderr) as Box<dyn Read + Send>, false),
    ] {
        let output = output.clone();
        let wake = wake.clone();
        let control = control.clone();
        readers.push(std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            let send = |event| emit(&output, &wake, event);
            let result = if rpc {
                rode_agent_core::io::read_messages(
                    &mut reader,
                    rode_agent_core::OUTPUT_LINE_LIMIT,
                    "Agent stdout JSON-RPC",
                    send,
                )
            } else {
                rode_agent_core::io::read_diagnostics(&mut reader, send)
            };
            if let Err(error) = result {
                emit(
                    &output,
                    &wake,
                    rode_agent_core::Event::Error(format!("{error:#}")),
                );
                control.stop();
            }
        }));
    }
    emit(&output, &wake, rode_agent_core::Event::Started);
    let status = control.wait(&mut child)?;
    for reader in readers {
        let _ = reader.join();
    }
    emit(
        &output,
        &wake,
        rode_agent_core::Event::Exited(status.code()),
    );
    Ok(())
}
