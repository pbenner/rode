//! Host-side stdio transport. The spawning thread remains alive until the
//! owned process exits, including bubblewrap's --die-with-parent lifetime.
use super::*;
use std::{
    io::{BufRead, BufReader},
    os::unix::process::CommandExt,
    process::{Command as ProcessCommand, Stdio},
    sync::atomic::{AtomicU32, AtomicUsize},
};

pub(super) struct Factory {
    pub spec: Spec,
    pub active: Arc<AtomicUsize>,
}
impl rode_agent_core::Factory for Factory {
    fn start(
        &self,
        launch: &rode_agent_core::Launch,
        history: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<rode_agent_core::Session> {
        let mut argv = launch.config.arguments(!history)?;
        argv[0] = self.spec.executable.clone();
        let mut argv_with_policy = if self.spec.plan.active {
            self.spec.plan.prefix.clone()
        } else {
            vec![]
        };
        argv_with_policy.extend(argv);
        let mut command = ProcessCommand::new(&argv_with_policy[0]);
        command
            .args(&argv_with_policy[1..])
            .current_dir(&self.spec.plan.root)
            .env_clear()
            .envs(&self.spec.env)
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let (input, incoming) = mpsc::sync_channel::<Vec<u8>>(32);
        let (output, events) = mpsc::sync_channel::<rode_agent_core::Event>(16);
        let control = Arc::new(Control {
            pid: AtomicU32::new(0),
            alive: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        });
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
                if history {
                    2 * 1024 * 1024
                } else {
                    rode_agent_core::LINE_LIMIT
                },
            );
            if let Err(error) = result {
                emit(
                    &output,
                    &wake,
                    rode_agent_core::Event::Error(format!("Cannot start agent: {error:#}")),
                );
            }
            control.alive.store(false, Ordering::Release);
            drop(running);
            wake();
        });
        Ok(rode_agent_core::Session::new(pipe))
    }
}
struct Control {
    pid: AtomicU32,
    alive: AtomicBool,
    stopped: AtomicBool,
}
impl Control {
    fn kill(&self, signal: i32) {
        let pid = self.pid.load(Ordering::Acquire);
        if pid != 0 && self.alive.load(Ordering::Acquire) {
            unsafe {
                libc::kill(-(pid as i32), signal);
            }
        }
    }
    fn stop(self: &Arc<Self>) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        self.kill(libc::SIGTERM);
        let control = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            control.kill(libc::SIGKILL);
        });
    }
}
struct Pipe {
    input: mpsc::SyncSender<Vec<u8>>,
    events: mpsc::Receiver<rode_agent_core::Event>,
    control: Arc<Control>,
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
        self.events.try_recv().ok()
    }
    fn stop(&self) {
        self.control.stop();
    }
}
fn emit(
    output: &mpsc::SyncSender<rode_agent_core::Event>,
    wake: &Arc<dyn Fn() + Send + Sync>,
    event: rode_agent_core::Event,
) -> bool {
    wake();
    let sent = output.send(event).is_ok();
    wake();
    sent
}
fn run(
    mut command: ProcessCommand,
    input: mpsc::Receiver<Vec<u8>>,
    output: mpsc::SyncSender<rode_agent_core::Event>,
    control: Arc<Control>,
    wake: Arc<dyn Fn() + Send + Sync>,
    limit: usize,
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
    control.pid.store(child.id(), Ordering::Release);
    control.alive.store(true, Ordering::Release);
    if control.stopped.load(Ordering::Acquire) {
        control.kill(libc::SIGTERM);
    }
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
            loop {
                let mut bytes = vec![];
                let bound = if rpc { limit } else { 8192 };
                match (&mut reader).take((bound + 1) as u64).read_until(b'\n', &mut bytes) {
                    Ok(0) => break,
                    Ok(_) if bytes.len() > bound => {
                        emit(&output, &wake, rode_agent_core::Event::Error("Agent output line exceeds its size limit".into())); control.stop(); break;
                    }
                    Ok(_) => {
                        let event = if rpc {
                            if bytes.iter().all(u8::is_ascii_whitespace) { continue; }
                            match serde_json::from_slice(&bytes) { Ok(value) => rode_agent_core::Event::Message(value), Err(_) => { emit(&output, &wake, rode_agent_core::Event::Error("Agent stdout is not JSON-RPC. Check the selected protocol and arguments.".into())); control.stop(); break; } }
                        } else { rode_agent_core::Event::Stderr(String::from_utf8_lossy(&bytes).trim_end().to_owned()) };
                        if !emit(&output, &wake, event) { break; }
                    }
                    Err(error) => { emit(&output, &wake, rode_agent_core::Event::Error(error.to_string())); control.stop(); break; }
                }
            }
        }));
    }
    emit(&output, &wake, rode_agent_core::Event::Started);
    let status = child.wait()?;
    control.kill(libc::SIGKILL);
    control.alive.store(false, Ordering::Release);
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
