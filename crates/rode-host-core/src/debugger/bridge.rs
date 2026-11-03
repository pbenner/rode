//! One host/sandbox session for an adapter and its interactive PTYs. No TCP
//! listener, SSH forwarding or separate terminal service is needed by RODE.
use super::{Command as Input, Event, Start, framing};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
static STOP: AtomicBool = AtomicBool::new(false);
struct Pty {
    master: Box<dyn MasterPty + Send>,
    input: mpsc::SyncSender<Vec<u8>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    pid: u32,
    start: u64,
}
impl Pty {
    fn stop(&mut self) {
        if process(self.pid).is_some_and(|(_, start)| start == self.start) {
            let _ = self.killer.kill();
        }
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        self.stop();
    }
}
enum Incoming {
    Client(Input),
    Adapter(Value),
    Error(String),
    Eof,
    AdapterEof,
}
type Output = Arc<Mutex<std::io::Stdout>>;
fn emit(output: &Output, event: Event) -> Result<()> {
    ensure!(!STOP.load(Ordering::Acquire), "Debugger stopped.");
    let mut output = output
        .lock()
        .map_err(|_| anyhow::anyhow!("Debugger output lock"))?;
    serde_json::to_writer(&mut *output, &event)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}
fn process(pid: u32) -> Option<(u32, u64)> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, tail) = text.rsplit_once(") ")?;
    let fields: Vec<_> = tail.split_whitespace().collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}
fn descendants() -> Vec<(u32, u64)> {
    let root = std::process::id();
    let mut all = vec![];
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten().take(65536) {
            if let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>()
                && let Some((parent, start)) = process(pid)
            {
                all.push((pid, parent, start));
            }
        }
    }
    let mut owned = BTreeSet::from([root]);
    for _ in 0..128 {
        let before = owned.len();
        for (pid, parent, _) in &all {
            if owned.contains(parent) {
                owned.insert(*pid);
            }
        }
        if before == owned.len() {
            break;
        }
    }
    all.into_iter()
        .filter(|(pid, _, _)| *pid != root && owned.contains(pid))
        .map(|(pid, _, start)| (pid, start))
        .collect()
}
fn cleanup() {
    // The helper is a subreaper: detached grandchildren remain attributable to
    // this one session. Never enumerate/kill descendants of the desktop/sshd.
    for signal in [libc::SIGTERM, libc::SIGKILL, libc::SIGKILL] {
        for (pid, start) in descendants().into_iter().rev() {
            if process(pid).is_some_and(|(_, now)| now == start) {
                unsafe {
                    libc::kill(pid as i32, signal);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(40));
        while unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) } > 0 {}
    }
}
fn resolve(root: &std::path::Path, value: &str) -> String {
    let path = crate::files::expand_home(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
    .to_string_lossy()
    .into()
}
fn prepare(start: &Start) -> Result<(Vec<String>, Value)> {
    super::Settings {
        config: start.config.clone(),
        ..Default::default()
    }
    .validate()?;
    let root = crate::files::expand_home(&start.root).canonicalize()?;
    let python = crate::project::python_path(
        &root,
        if start.python == "off" {
            ""
        } else {
            &start.python
        },
    )
    .to_string_lossy()
    .into_owned();
    let file = if start.file.is_empty() {
        String::new()
    } else {
        resolve(&root, &start.file)
    };
    let parent = std::path::Path::new(&file)
        .parent()
        .unwrap_or(&root)
        .to_string_lossy();
    let stem = std::path::Path::new(&file)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let root_text = root.to_string_lossy();
    let values = [
        ("root", root_text.as_ref()),
        ("file", file.as_str()),
        ("dir", parent.as_ref()),
        ("stem", stem.as_ref()),
        ("python", python.as_str()),
    ];
    let cfg = &start.config;
    let adapter = if cfg.adapter.trim().is_empty() {
        match cfg.backend {
            super::Backend::Python => python.clone(),
            super::Backend::Lldb => "lldb-dap".into(),
            super::Backend::Custom => bail!("Choose a DAP adapter executable."),
        }
    } else {
        super::replace(&cfg.adapter, &values)
    };
    let mut argv = vec![adapter];
    if cfg.adapter_arguments.is_empty() && cfg.backend == super::Backend::Python {
        argv.extend(["-m".into(), "debugpy.adapter".into()]);
    } else {
        argv.extend(
            cfg.adapter_arguments
                .iter()
                .map(|a| super::replace(a, &values)),
        );
    }
    let cwd = resolve(&root, &super::replace(&cfg.cwd, &values));
    let mut args = json!({"cwd":cwd,"args":cfg.arguments.iter().map(|a|super::replace(a,&values)).collect::<Vec<_>>(),"stopOnEntry":cfg.stop_on_entry});
    let program = super::replace(&cfg.program, &values);
    if !program.is_empty() {
        args["program"] = resolve(&root, &program).into();
    }
    if cfg.backend == super::Backend::Python {
        args["python"] = json!([python]);
        args["console"] = "integratedTerminal".into();
        args["justMyCode"] = false.into();
        args["subProcess"] = false.into();
    } else if cfg.backend == super::Backend::Lldb {
        args["console"] = "integratedTerminal".into();
        args["runInTerminal"] = true.into();
        args["disableASLR"] = false.into();
    }
    for (key, value) in super::expand(&cfg.extra, &values, 0)?.as_object().unwrap() {
        args[key] = value.clone();
    }
    ensure!(
        cfg.attach
            || args
                .get("program")
                .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
            || args.get("module").is_some()
            || args.get("launchCommands").is_some(),
        "Choose a saved program/script or module before debugging."
    );
    Ok((argv, args))
}
fn terminal(
    body: &Value,
    id: u64,
    output: &Output,
    environment: &BTreeMap<String, String>,
) -> Result<Pty> {
    ensure!(
        body["kind"].as_str().unwrap_or("integrated") == "integrated",
        "Only an integrated project terminal is supported."
    );
    ensure!(
        body["argsCanBeInterpretedByShell"] != true,
        "Shell-interpreted debugger arguments are unsupported."
    );
    let args = body["args"]
        .as_array()
        .context("Missing terminal arguments")?;
    ensure!(
        !args.is_empty() && args.len() <= 128 && serde_json::to_vec(body)?.len() <= 128 * 1024,
        "Invalid debug terminal request."
    );
    let mut command = CommandBuilder::new(args[0].as_str().context("Invalid executable")?);
    for (key, value) in environment {
        command.env(key, value);
    }
    for arg in &args[1..] {
        command.arg(arg.as_str().context("Invalid terminal argument")?);
    }
    if let Some(cwd) = body["cwd"].as_str().filter(|s| !s.is_empty()) {
        command.cwd(cwd);
    }
    if let Some(env) = body["env"].as_object() {
        ensure!(
            env.len() <= 256,
            "Debug terminal environment exceeds its limit."
        );
        for (key, value) in env {
            ensure!(
                !key.is_empty() && !key.contains(['=', '\0']),
                "Invalid environment key."
            );
            if value.is_null() {
                command.env_remove(key);
            } else {
                command.env(key, value.as_str().context("Invalid environment value")?);
            }
        }
    }
    let pair = native_pty_system().openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let pid = child.process_id().context("Missing debug terminal PID")?;
    let stamp = process(pid).map_or(0, |(_, start)| start);
    let killer = child.clone_killer();
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let (input, receive) = mpsc::sync_channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        for bytes in receive {
            if writer
                .write_all(&bytes)
                .and_then(|_| writer.flush())
                .is_err()
            {
                break;
            }
        }
    });
    emit(
        output,
        Event::Terminal {
            id,
            title: body["title"]
                .as_str()
                .unwrap_or("Debug program")
                .chars()
                .take(120)
                .collect(),
        },
    )?;
    let out = output.clone();
    std::thread::spawn(move || {
        let mut buffer = [0; 16384];
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 {
                break;
            }
            if emit(
                &out,
                Event::Data {
                    id,
                    data: STANDARD.encode(&buffer[..n]),
                },
            )
            .is_err()
            {
                break;
            }
        }
    });
    let out = output.clone();
    std::thread::spawn(move || {
        let _ = child.wait();
        let _ = emit(&out, Event::TerminalExited { id });
    });
    Ok(Pty {
        master: pair.master,
        input,
        killer,
        pid,
        start: stamp,
    })
}
/// Serve the native `--debug-helper` coordinator. The caller applies the project sandbox before
/// entry; adapter and reverse `runInTerminal` processes share that namespace and are cleaned up
/// on shutdown/input loss.
pub fn coordinator() -> Result<()> {
    extern "C" fn stop(_: libc::c_int) {
        STOP.store(true, Ordering::Release);
    }
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = stop as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
    let result = run();
    cleanup();
    result
}
fn run() -> Result<()> {
    let mut input = BufReader::new(std::io::stdin());
    let mut line = String::new();
    input.by_ref().take(256 * 1024 + 1).read_line(&mut line)?;
    ensure!(
        line.len() <= 256 * 1024,
        "Debugger launch exceeds its limit."
    );
    let Input::Start { launch } = serde_json::from_str(&line)? else {
        bail!("Expected a debugger launch.");
    };
    let (argv, arguments) = prepare(&launch)?;
    let output = Arc::new(Mutex::new(std::io::stdout()));
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(&launch.root)
        .process_group(0);
    if argv[0].contains('/') && !std::path::Path::new(&argv[0]).is_absolute() {
        command = Command::new(resolve(std::path::Path::new(&launch.root), &argv[0]));
        command
            .args(&argv[1..])
            .current_dir(&launch.root)
            .process_group(0);
    }
    let python = crate::project::python_path(std::path::Path::new(&launch.root), &launch.python);
    let environment = crate::project::environment_variables(&python.to_string_lossy());
    command.envs(&environment);
    let mut adapter = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Cannot start DAP adapter; install/configure it on the project host")?;
    let mut adapter_in = adapter.stdin.take().unwrap();
    let adapter_out = adapter.stdout.take().unwrap();
    let adapter_err = adapter.stderr.take().unwrap();
    let (incoming, rx) = mpsc::sync_channel::<Incoming>(32);
    let tx = incoming.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(adapter_out);
        loop {
            match framing::read(&mut reader) {
                Ok(Some(message)) => {
                    if tx.send(Incoming::Adapter(message)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = tx.send(Incoming::AdapterEof);
                    break;
                }
                Err(e) => {
                    let _ = tx.send(Incoming::Error(format!("{e:#}")));
                    break;
                }
            }
        }
    });
    let tx = incoming.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(adapter_err);
        loop {
            let mut line = String::new();
            match reader.by_ref().take(8193).read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if line.len() > 8192 {
                        let _ =
                            tx.send(Incoming::Error("Adapter stderr line exceeds 8 KiB".into()));
                        break;
                    }
                    let message = json!({"type":"event","event":"output","body":{"category":"stderr","output":line}});
                    if tx.send(Incoming::Adapter(message)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let tx = incoming.clone();
    std::thread::spawn(move || {
        loop {
            let mut line = String::new();
            match input.by_ref().take(256 * 1024 + 1).read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if line.len() > 256 * 1024 {
                        break;
                    }
                    match serde_json::from_str(&line) {
                        Ok(message) => {
                            if tx.send(Incoming::Client(message)).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = tx.send(Incoming::Error(error.to_string()));
                            return;
                        }
                    }
                }
            }
        }
        let _ = tx.send(Incoming::Eof);
    });
    let (send, write_rx) = mpsc::sync_channel::<Value>(32);
    let tx = incoming.clone();
    std::thread::spawn(move || {
        for message in write_rx {
            if let Err(error) = framing::write(&mut adapter_in, &message) {
                let _ = tx.send(Incoming::Error(error.to_string()));
                break;
            }
        }
    });
    // The resolved launch payload goes back to the UI, which owns DAP sequence
    // numbers and initialize/configuration ordering.
    emit(
        &output,
        Event::Dap {
            message: json!({"type":"event","event":"rodeReady","body":{"arguments":arguments}}),
        },
    )?;
    let mut ptys = BTreeMap::<u64, Pty>::new();
    let mut next = 1u64;
    let mut pending = VecDeque::<Value>::new();
    while !STOP.load(Ordering::Acquire) {
        while let Some(message) = pending.pop_front() {
            match send.try_send(message) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(message)) => {
                    pending.push_front(message);
                    break;
                }
                Err(mpsc::TrySendError::Disconnected(_)) => bail!("Debugger input disconnected."),
            }
        }
        let message = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(v) => v,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        match message {
            Incoming::Client(Input::Dap { message }) => {
                ensure!(
                    pending.len() < super::REQUEST_LIMIT,
                    "Too many queued debugger requests."
                );
                pending.push_back(message);
            }
            Incoming::Client(Input::Input { id, data }) => {
                ensure!(data.len() <= 32768, "Terminal input exceeds its limit.");
                if let Some(pty) = ptys.get_mut(&id) {
                    match pty.input.try_send(STANDARD.decode(data)?) {
                        Ok(()) => {}
                        Err(mpsc::TrySendError::Full(_)) => emit(
                            &output,
                            Event::Dap {
                                message: json!({"type":"event","event":"output","body":{"category":"stderr","output":"Debug terminal input is full; some input was not sent. Retry after the program reads stdin.\n"}}),
                            },
                        )?,
                        Err(mpsc::TrySendError::Disconnected(_)) => {}
                    }
                }
            }
            Incoming::Client(Input::Resize { id, rows, columns }) => {
                if let Some(pty) = ptys.get(&id) {
                    pty.master.resize(PtySize {
                        rows: rows.clamp(1, 500),
                        cols: columns.clamp(1, 1000),
                        pixel_width: 0,
                        pixel_height: 0,
                    })?;
                }
            }
            Incoming::Client(Input::EndTerminal { id }) => {
                ptys.remove(&id);
            }
            Incoming::Client(Input::Shutdown) | Incoming::Eof | Incoming::AdapterEof => break,
            Incoming::Client(Input::Start { .. }) => bail!("Debugger is already started."),
            Incoming::Error(error) => bail!(error),
            Incoming::Adapter(message) if message["type"] == "request" => {
                let command = message["command"].as_str().unwrap_or_default();
                let reply = if command == "runInTerminal" {
                    if ptys.len() >= 4 {
                        Err(anyhow::anyhow!("At most four debug terminals."))
                    } else {
                        terminal(&message["arguments"], next, &output, &environment).map(|pty| {
                            let pid = pty.pid;
                            ptys.insert(next, pty);
                            next += 1;
                            json!({"processId":pid,"shellProcessId":pid})
                        })
                    }
                } else {
                    Err(anyhow::anyhow!("Unsupported debugger request: {command}"))
                };
                let response = match reply {
                    Ok(body) => {
                        json!({"seq":0,"type":"response","request_seq":message["seq"],"command":command,"success":true,"body":body})
                    }
                    Err(error) => {
                        json!({"seq":0,"type":"response","request_seq":message["seq"],"command":command,"success":false,"message":format!("{error:#}")})
                    }
                };
                // The UI assigns sequence numbers before returning reverse replies.
                emit(
                    &output,
                    Event::Dap {
                        message: json!({"type":"event","event":"rodeReverseResponse","body":response}),
                    },
                )?;
            }
            Incoming::Adapter(message) => emit(&output, Event::Dap { message })?,
        }
    }
    drop(ptys);
    Ok(())
}
