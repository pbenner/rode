//! Host-owned notebook queues and debugger controllers with exclusive UI leases.
use super::*;
use rode_runtime_core::{
    debugger, notebook,
    service::{
        self, Cell, Command as Action, DebugAction, Envelope, Info, Kind, Spec, Start,
        State as View, Update,
    },
};
use serde_json::Value;
use std::sync::Condvar;

const FRAME_LIMIT: usize = 16 * 1024 * 1024;
const NOTEBOOK_LIMIT: usize = 8 * 1024 * 1024;
#[derive(Default)]
pub(super) struct Store {
    jobs: Arc<Mutex<HashMap<String, Arc<Job>>>>,
}
struct Shared {
    view: View,
    cells: BTreeMap<String, Cell>,
    revision: u64,
    terminals: BTreeMap<u64, (String, vt100::Parser, bool)>,
    events: VecDeque<(u64, debugger::Event)>,
    event_sequence: u64,
    ever_attached: bool,
    finished: bool,
}
struct Job {
    shared: Mutex<Shared>,
    changed: Condvar,
    input: mpsc::SyncSender<Envelope>,
    stop: AtomicBool,
    discard: AtomicBool,
    completed: AtomicBool,
    created: Instant,
}
impl Store {
    pub fn live(&self) -> bool {
        !self.jobs.lock().unwrap().is_empty()
    }
    pub fn list(&self) -> Vec<Info> {
        let mut infos: Vec<_> = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .map(|j| j.shared.lock().unwrap().view.info.clone())
            .collect();
        infos.sort_by_key(|i| i.created);
        infos
    }
    fn find(&self, id: &str, policy: &str) -> Result<Arc<Job>> {
        ensure!(valid_id(id), "Invalid runtime ID");
        let job = self
            .jobs
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("Runtime is no longer available; refresh the running list")?;
        ensure!(
            job.shared.lock().unwrap().view.info.policy == policy,
            "Open this runtime's original project and sandbox policy before attaching"
        );
        Ok(job)
    }
    pub fn end(&self, id: &str, policy: &str) -> Result<()> {
        let job = self.find(id, policy)?;
        job.discard.store(true, Ordering::Release);
        job.stop.store(true, Ordering::Release);
        job.changed.notify_all();
        if job.completed.load(Ordering::Acquire) {
            self.jobs.lock().unwrap().remove(id);
        }
        Ok(())
    }
    pub fn create(&self, spec: Spec) -> Result<Info> {
        ensure!(
            spec.sources.len() <= 128
                && serde_json::to_vec(&spec.sources)?.len() <= 64 * 1024
                && spec.sources.iter().all(|(path, digest)| path.len() <= 4096
                    && digest.len() == 64
                    && digest.bytes().all(|v| v.is_ascii_hexdigit())),
            "Invalid debugger source digests"
        );
        ensure!(
            spec.python.len() <= 4096
                && !spec.python.contains('\0')
                && spec.target.as_ref().is_none_or(
                    |v| serde_json::to_vec(v).is_ok_and(|bytes| bytes.len() <= 16 * 1024)
                ),
            "Runtime interpreter/target metadata exceeds its limit"
        );
        ensure!(
            valid_id(&spec.id) && valid_name(&spec.name),
            "Invalid runtime ID/name"
        );
        ensure!(
            Path::new(&spec.root).is_absolute()
                && spec.root.len() <= 4096
                && spec.document.len() <= 4096,
            "Invalid runtime project/document path"
        );
        ensure!(
            spec.policy.len() == 64 && spec.policy.bytes().all(|v| v.is_ascii_hexdigit()),
            "Invalid runtime policy fingerprint"
        );
        ensure!(
            !spec.argv.is_empty()
                && spec.argv.len() <= 256
                && spec.argv.iter().all(|v| !v.contains('\0'))
                && spec.argv.iter().map(String::len).sum::<usize>() <= 256 * 1024,
            "Runtime command exceeds its limit"
        );
        ensure!(
            spec.prefix.len() <= 256
                && spec.prefix.iter().map(String::len).sum::<usize>() <= 64 * 1024,
            "Runtime sandbox command exceeds its limit"
        );
        ensure!(
            spec.env
                .iter()
                .all(|(k, v)| !k.is_empty() && !k.contains(['=', '\0']) && !v.contains('\0'))
                && spec
                    .env
                    .iter()
                    .map(|(k, v)| k.len() + v.len())
                    .sum::<usize>()
                    <= 128 * 1024,
            "Invalid runtime environment"
        );
        let (kind, configuration) = match &spec.start {
            Start::Notebook { name } => {
                ensure!(
                    !name.is_empty()
                        && name.len() <= 128
                        && name
                            .chars()
                            .all(|v| v.is_ascii_alphanumeric() || "._-".contains(v)),
                    "Invalid kernelspec"
                );
                (Kind::Notebook, name.clone())
            }
            Start::Debugger { settings, launch } => {
                settings.validate()?;
                ensure!(
                    settings.config == launch.config,
                    "Debugger launch/settings mismatch"
                );
                (Kind::Debugger, settings.config.backend.label().into())
            }
        };
        let mut jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.get(&spec.id) {
            let shared = job.shared.lock().unwrap();
            ensure!(
                shared.view.info.policy == spec.policy,
                "Runtime ID belongs to another policy"
            );
            return Ok(shared.view.info.clone());
        }
        ensure!(
            jobs.len() < 32,
            "At most 32 retained notebook/debugger runtimes; end an old runtime first"
        );
        let info = Info {
            sources: spec.sources.clone(),
            id: spec.id.clone(),
            name: spec.name.clone(),
            kind,
            root: spec.root.clone(),
            policy: spec.policy.clone(),
            mode: spec.mode.clone(),
            document: spec.document.clone(),
            target: spec.target.clone(),
            python: spec.python.clone(),
            configuration,
            alive: true,
            busy: true,
            status: "Starting…".into(),
            attached: false,
            persistent: spec.persistent,
            created: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        };
        let (input, commands) = mpsc::sync_channel(64);
        let job = Arc::new(Job {
            shared: Mutex::new(Shared {
                view: View {
                    acknowledgement: String::new(),
                    acknowledgements: vec![],
                    info: info.clone(),
                    runs: vec![],
                    language: Value::Null,
                    python: spec.python.clone(),
                    execution: spec
                        .target
                        .as_ref()
                        .and_then(|t| t["id"].as_str())
                        .map(|id| format!("Slurm job {id}"))
                        .unwrap_or_else(|| "Project host".into()),
                    debugger: None,
                    error: String::new(),
                },
                cells: BTreeMap::new(),
                revision: 0,
                terminals: BTreeMap::new(),
                events: VecDeque::new(),
                event_sequence: 0,
                ever_attached: false,
                finished: false,
            }),
            changed: Condvar::new(),
            input,
            stop: AtomicBool::new(false),
            discard: AtomicBool::new(false),
            completed: AtomicBool::new(false),
            created: Instant::now(),
        });
        jobs.insert(spec.id.clone(), job.clone());
        let jobs = Arc::downgrade(&self.jobs);
        std::thread::spawn(move || {
            let id = spec.id.clone();
            run(job.clone(), commands, spec);
            job.completed.store(true, Ordering::Release);
            let shared = job.shared.lock().unwrap();
            let remove = job.discard.load(Ordering::Acquire)
                || (!shared.view.info.persistent && !shared.view.info.attached);
            drop(shared);
            if remove && let Some(jobs) = jobs.upgrade() {
                jobs.lock().unwrap().remove(&id);
            }
        });
        Ok(info)
    }
    pub fn attach(&self, mut stream: UnixStream, id: &str, policy: &str) -> Result<()> {
        let job = self.find(id, policy)?;
        {
            let mut shared = job.shared.lock().unwrap();
            ensure!(
                !shared.view.info.attached,
                "This runtime already has a controlling RODE window; detach it there first"
            );
            shared.view.info.attached = true;
            shared.ever_attached = true;
            shared.revision += 1;
        }
        struct Lease(Arc<Job>, Arc<Mutex<HashMap<String, Arc<Job>>>>, String);
        impl Drop for Lease {
            fn drop(&mut self) {
                let mut s = self.0.shared.lock().unwrap();
                s.view.info.attached = false;
                s.revision += 1;
                let remove = self.0.completed.load(Ordering::Acquire) && !s.view.info.persistent;
                drop(s);
                if remove {
                    self.1.lock().unwrap().remove(&self.2);
                }
                self.0.changed.notify_all();
            }
        }
        let _lease = Lease(job.clone(), self.jobs.clone(), id.into());
        stream.set_read_timeout(None)?;
        let mut writer = stream.try_clone()?;
        let writing = job.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::spawn(move || {
            let mut revision = u64::MAX;
            let mut cells = BTreeMap::new();
            let mut sequence = 0;
            let result = (|| -> Result<()> {
                loop {
                    let shared = writing.shared.lock().unwrap();
                    let shared = writing
                        .changed
                        .wait_timeout_while(shared, Duration::from_millis(200), |s| {
                            s.revision == revision && !stop.load(Ordering::Acquire)
                        })
                        .unwrap()
                        .0;
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    if revision == shared.revision {
                        continue;
                    }
                    let first = revision == u64::MAX;
                    revision = shared.revision;
                    let state = shared.view.clone();
                    let updated: Vec<_> = shared
                        .cells
                        .iter()
                        .filter(|(key, cell)| cells.get(*key) != Some(&cell.revision))
                        .map(|(key, cell)| (key.clone(), cell.clone()))
                        .collect();
                    let mut events = vec![];
                    if first {
                        for (id, (title, parser, alive)) in &shared.terminals {
                            events.push(debugger::Event::Terminal {
                                id: *id,
                                title: title.clone(),
                            });
                            use base64::{Engine, engine::general_purpose::STANDARD};
                            events.push(debugger::Event::Data {
                                id: *id,
                                data: STANDARD.encode(parser.screen().contents_formatted()),
                            });
                            if !alive {
                                events.push(debugger::Event::TerminalExited { id: *id });
                            }
                        }
                    } else {
                        ensure!(
                            shared
                                .events
                                .front()
                                .is_none_or(|(n, _)| *n <= sequence + 1),
                            "Debug terminal replay overflow; reattach to recover its current screen"
                        );
                        events.extend(
                            shared
                                .events
                                .iter()
                                .filter(|(n, _)| *n > sequence)
                                .map(|(_, e)| e.clone()),
                        );
                    }
                    sequence = shared.event_sequence;
                    drop(shared);
                    send(&mut writer, &Update::State(Box::new(state)))?;
                    for (key, cell) in updated {
                        cells.insert(key, cell.revision);
                        send(&mut writer, &Update::Cell(cell))?;
                    }
                    for event in events {
                        send(&mut writer, &Update::Terminal(event))?;
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                let _ = send(&mut writer, &Update::Error(error.to_string()));
            }
            let _ = writer.shutdown(std::net::Shutdown::Both);
        });
        while let Ok(action) = receive::<Envelope>(&mut stream) {
            if !valid_id(&action.id) {
                let _ = send(
                    &mut stream,
                    &Update::Error("Invalid runtime command ID".into()),
                );
                break;
            }
            if job.input.try_send(action).is_err() {
                let _ = send(
                    &mut stream,
                    &Update::Error(
                        "Runtime command queue is full; reattach before retrying".into(),
                    ),
                );
                break;
            }
        }
        stopped.store(true, Ordering::Release);
        job.changed.notify_all();
        let _ = stream.shutdown(std::net::Shutdown::Both);
        Ok(())
    }
}
enum Controller {
    Notebook(Box<notebook::Kernel>),
    Debugger(Box<debugger::protocol::Client>),
}
fn launch(spec: &Spec) -> Result<Controller> {
    let argv: Vec<_> = spec.prefix.iter().chain(&spec.argv).cloned().collect();
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(&spec.root)
        .env_clear()
        .envs(&spec.env)
        .env("NO_COLOR", "1");
    let session = rode_runtime_core::process::spawn(command)?;
    match &spec.start {
        Start::Notebook { name } => Ok(Controller::Notebook(Box::new(
            notebook::Kernel::from_session(
                session,
                spec.python.clone(),
                name.clone(),
                spec.target
                    .as_ref()
                    .and_then(|target| target["id"].as_str())
                    .map(|id| format!("Slurm job {id}"))
                    .unwrap_or_else(|| "Project host".into()),
                Duration::from_secs(60),
            )?,
        ))),
        Start::Debugger { launch, settings } => Ok(Controller::Debugger(Box::new(
            debugger::protocol::Client::from_session(
                session,
                launch.as_ref().clone(),
                settings.as_ref().clone(),
            )?,
        ))),
    }
}
fn run(job: Arc<Job>, input: mpsc::Receiver<Envelope>, spec: Spec) {
    let mut controller = match launch(&spec) {
        Ok(c) => c,
        Err(error) => {
            let mut s = job.shared.lock().unwrap();
            s.view.error = format!("{error:#}");
            s.view.info.alive = false;
            s.view.info.busy = false;
            s.finished = true;
            s.revision += 1;
            job.changed.notify_all();
            return;
        }
    };
    loop {
        let mut changed = false;
        let shared = job.shared.lock().unwrap();
        if !shared.view.info.persistent
            && !shared.view.info.attached
            && (shared.ever_attached || job.created.elapsed() > Duration::from_secs(30))
        {
            job.stop.store(true, Ordering::Release);
        }
        let stopping = job.stop.load(Ordering::Acquire);
        drop(shared);
        if stopping {
            break;
        }
        for _ in 0..32 {
            let Ok(envelope) = input.try_recv() else {
                break;
            };
            if job
                .shared
                .lock()
                .unwrap()
                .view
                .acknowledgements
                .contains(&envelope.id)
            {
                continue;
            }
            changed = true;
            let result = apply(&mut controller, &job, envelope.command);
            let mut shared = job.shared.lock().unwrap();
            shared.view.acknowledgement = envelope.id;
            let acknowledgement = shared.view.acknowledgement.clone();
            shared.view.acknowledgements.push(acknowledgement);
            if shared.view.acknowledgements.len() > 256 {
                shared.view.acknowledgements.remove(0);
            }
            shared.view.error = result.err().map(|e| format!("{e:#}")).unwrap_or_default();
            shared.revision += 1;
        }
        let mut shared = job.shared.lock().unwrap();
        match &mut controller {
            Controller::Notebook(kernel) => {
                for event in kernel.poll() {
                    changed = true;
                    if let Err(error) = notebook_event(&mut shared, event) {
                        shared.view.error = format!("{error:#}");
                        kernel.stop();
                        shared.finished = true;
                    }
                }
                shared.view.info.busy = !shared.finished && kernel.busy();
                shared.view.info.status = kernel.status.clone();
                if changed {
                    shared.view.runs = kernel.executions().map(service::RunRef::from).collect();
                }
                shared.view.python = kernel.python.clone();
                shared.view.execution = kernel.execution.clone();
            }
            Controller::Debugger(client) => {
                changed |= client.poll();
                while let Some(event) = client.terminal_events.pop_front() {
                    terminal_event(&mut shared, event);
                }
                shared.view.info.busy = client.phase == debugger::protocol::Phase::Starting;
                client.status = client.status.chars().take(4096).collect();
                client.error = client.error.chars().take(8192).collect();
                shared.view.info.status = client.status.chars().take(512).collect();
                if !client.error.is_empty() {
                    shared.view.error = client.error.clone();
                }
                if changed {
                    shared.view.debugger = Some(client.view());
                }
                shared.finished =
                    client.phase == debugger::protocol::Phase::Ended && !client.is_stopping();
            }
        }
        shared.view.info.alive = !shared.finished;
        if changed || shared.finished {
            shared.revision += 1;
        }
        let finished = shared.finished;
        drop(shared);
        if changed || finished {
            job.changed.notify_all();
        }
        if finished {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    // Do not acknowledge removal before the supervisor has reaped owned children.
    match &mut controller {
        Controller::Notebook(kernel) => {
            kernel.stop();
            while !kernel.is_finished() {
                kernel.poll();
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        Controller::Debugger(client) => {
            client.stop();
            while client.is_stopping() {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    let mut shared = job.shared.lock().unwrap();
    if let Controller::Debugger(client) = &controller {
        shared.view.debugger = Some(client.view());
    }
    shared.finished = true;
    shared.view.info.alive = false;
    shared.view.info.busy = false;
    shared.view.runs.clear();
    shared.revision += 1;
    drop(shared);
    job.changed.notify_all();
}
fn apply(controller: &mut Controller, job: &Job, action: Action) -> Result<()> {
    match action {
        Action::Document { path } => {
            ensure!(
                matches!(controller, Controller::Notebook(_))
                    && path.len() <= 4096
                    && !path.contains('\0')
                    && Path::new(&path).is_absolute(),
                "Invalid notebook document path"
            );
            job.shared.lock().unwrap().view.info.document = path;
            Ok(())
        }
        Action::ClearOutputs { cell } => {
            ensure!(
                matches!(controller, Controller::Notebook(_)),
                "This is not a notebook kernel"
            );
            let mut shared = job.shared.lock().unwrap();
            for (id, result) in &mut shared.cells {
                if cell.as_ref().is_none_or(|wanted| wanted == id) {
                    result.outputs.clear();
                    result.count = None;
                    result.revision += 1;
                }
            }
            Ok(())
        }
        Action::Persistence { persistent } => {
            let mut s = job.shared.lock().unwrap();
            s.view.info.persistent = persistent;
            s.revision += 1;
            Ok(())
        }
        Action::Stop => {
            job.stop.store(true, Ordering::Release);
            Ok(())
        }
        Action::Run { runs } => {
            let Controller::Notebook(kernel) = controller else {
                bail!("This is not a notebook kernel")
            };
            ensure!(
                serde_json::to_vec(&runs)?.len() <= 8 * 1024 * 1024,
                "Notebook queue exceeds 8 MiB encoded size"
            );
            kernel.run(runs)
        }
        Action::Retain { cells } => {
            let Controller::Notebook(kernel) = controller else {
                bail!("This is not a notebook kernel")
            };
            ensure!(cells.len() <= 1024, "Too many notebook cells");
            kernel.retain_queued(|run| {
                cells
                    .iter()
                    .any(|(id, source)| id == &run.cell && source == &run.code)
            });
            Ok(())
        }
        Action::Interrupt => {
            let Controller::Notebook(kernel) = controller else {
                bail!("This is not a notebook kernel")
            };
            kernel.interrupt()
        }
        Action::Debug { epoch, operation } => {
            let Controller::Debugger(client) = controller else {
                bail!("This is not a debugger")
            };
            if !matches!(
                operation.as_ref(),
                DebugAction::Configure { .. } | DebugAction::Terminal { .. }
            ) {
                ensure!(
                    epoch == client.inspection_epoch(),
                    "Debugger stop changed; review its current state before using an inspection handle"
                );
            }
            match *operation {
                DebugAction::Control { command } => client.control(&command),
                DebugAction::Thread { id } => client.select_thread(id),
                DebugAction::Frame { id } => client.select_frame(id),
                DebugAction::Variables { reference } => {
                    ensure!(
                        client.variables.contains_key(&reference) || client.variables.len() < 32,
                        "Persistent debugger variable budget reached; select the frame again before expanding more references"
                    );
                    client.load_variables(reference)
                }
                DebugAction::Evaluate { expression } => client.evaluate(&expression),
                DebugAction::Configure {
                    settings,
                    paths,
                    watches,
                } => {
                    settings.validate()?;
                    ensure!(
                        settings.config == client.settings.config,
                        "Changing debugger launch settings requires a new session"
                    );
                    ensure!(paths.len() <= 128, "Too many breakpoint sources");
                    client.settings = settings;
                    for path in paths {
                        client.sync_breakpoints(&path)?;
                    }
                    if watches {
                        client.refresh_watches()?;
                    }
                    Ok(())
                }
                DebugAction::Terminal { command } => {
                    ensure!(
                        matches!(
                            command,
                            debugger::Command::Input { .. }
                                | debugger::Command::Resize { .. }
                                | debugger::Command::EndTerminal { .. }
                        ),
                        "Only terminal controls are accepted"
                    );
                    client
                        .writer()
                        .context("Debugger coordinator ended")?
                        .send(&serde_json::to_value(command)?)
                }
            }
        }
    }
}
fn notebook_event(shared: &mut Shared, event: notebook::Event) -> Result<()> {
    use notebook::Event;
    if let Event::Output {
        output,
        update: true,
        display_id,
        ..
    } = &event
    {
        if let Some(id) = display_id {
            update_notebook_displays(&mut shared.cells, id, output)?;
        }
        return Ok(());
    }
    if let Event::Ready(language) = event {
        shared.view.language = language;
        return Ok(());
    }
    if let Event::Error(error) = event {
        shared.view.error = error;
        shared.finished = true;
        return Ok(());
    }
    let run = match &event {
        Event::Output { run, .. }
        | Event::Clear(run)
        | Event::Count(run, _)
        | Event::Done(run, _) => run.clone(),
        _ => unreachable!(),
    };
    let key = if run.cell.is_empty() {
        format!("uid:{}", run.uid)
    } else {
        run.cell.clone()
    };
    let previous = shared.cells.get(&key);
    let mut cell = previous.cloned().unwrap_or_else(|| Cell {
        run: run.clone(),
        outputs: vec![],
        count: None,
        done: None,
        revision: 0,
    });
    if cell.run.uid != run.uid
        || cell.run.version != run.version
        || cell.run.code != run.code
        || cell.done.is_some()
    {
        cell = Cell {
            run: run.clone(),
            outputs: vec![],
            count: None,
            done: None,
            revision: cell.revision,
        };
    }
    cell.revision += 1;
    match event {
        Event::Output {
            output, display_id, ..
        } => {
            ensure!(cell.outputs.len() < 256, "Cell output exceeded 256 records");
            cell.outputs.push(service::Output {
                value: output,
                display: display_id,
            });
        }
        Event::Clear(_) => cell.outputs.clear(),
        Event::Count(_, count) => cell.count = Some(count),
        Event::Done(_, status) => cell.done = Some(status),
        _ => {}
    }
    let size = json_size(&shared.cells)?;
    let size = if let Some(previous) = previous {
        size - json_size(previous)? + json_size(&cell)?
    } else {
        // Map braces already exist; a new entry adds its key, colon and
        // (except in an empty map) the comma separating it from its neighbour.
        size + json_size(&key)? + 1 + json_size(&cell)? + usize::from(!shared.cells.is_empty())
    };
    check_notebook_budget(shared.cells.len() + usize::from(previous.is_none()), size)?;
    shared.cells.insert(key, cell);
    Ok(())
}
fn update_notebook_displays(
    cells: &mut BTreeMap<String, Cell>,
    id: &str,
    output: &Value,
) -> Result<()> {
    // Validate the entire fan-out before cloning any replacement. One
    // display identity can belong to many outputs in different cells.
    let mut size = json_size(&cells)?;
    let replacement = json_size(output)?;
    for cell in cells.values() {
        let mut changed = false;
        for old in &cell.outputs {
            if old.display.as_deref() == Some(id) {
                size = size - json_size(&old.value)? + replacement;
                changed = true;
            }
        }
        if changed {
            size = size - json_size(&cell.revision)? + json_size(&(cell.revision + 1))?;
        }
    }
    check_notebook_budget(cells.len(), size)?;
    for cell in cells.values_mut() {
        let mut changed = false;
        for old in &mut cell.outputs {
            if old.display.as_deref() == Some(id) {
                old.value = output.clone();
                changed = true;
            }
        }
        if changed {
            cell.revision += 1;
        }
    }
    Ok(())
}
fn check_notebook_budget(cells: usize, size: usize) -> Result<()> {
    ensure!(
        cells <= 1024 && size <= NOTEBOOK_LIMIT,
        "Retained notebook outputs exceed 8 MiB; shut down this kernel and clear outputs"
    );
    Ok(())
}
fn json_size(value: &impl serde::Serialize) -> Result<usize> {
    // Count encoded bytes without allocating another copy of retained outputs.
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

fn terminal_event(shared: &mut Shared, event: debugger::Event) {
    use base64::{Engine, engine::general_purpose::STANDARD};
    match &event {
        debugger::Event::Terminal { id, title } => {
            if shared.terminals.len() < 4 {
                shared
                    .terminals
                    .insert(*id, (title.clone(), vt100::Parser::new(24, 80, 1000), true));
            }
        }
        debugger::Event::Data { id, data } => {
            if let Some((_, parser, _)) = shared.terminals.get_mut(id)
                && let Ok(bytes) = STANDARD.decode(data)
            {
                parser.process(&bytes);
            }
        }
        debugger::Event::TerminalExited { id } => {
            if let Some((_, _, alive)) = shared.terminals.get_mut(id) {
                *alive = false;
            }
        }
        _ => {}
    }
    shared.event_sequence += 1;
    shared.events.push_back((shared.event_sequence, event));
    while shared.events.len() > 128 {
        shared.events.pop_front();
    }
}
fn send(stream: &mut UnixStream, value: &Update) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() < FRAME_LIMIT, "Runtime update exceeds 16 MiB");
    stream.write_all(&((bytes.len() + 1) as u32).to_be_bytes())?;
    stream.write_all(b"J")?;
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(())
}
fn receive<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    let n = u32::from_be_bytes(len) as usize;
    ensure!(
        (2..=FRAME_LIMIT).contains(&n),
        "Runtime frame exceeds 16 MiB"
    );
    let mut bytes = vec![0; n];
    stream.read_exact(&mut bytes)?;
    ensure!(bytes[0] == b'J', "Invalid runtime frame");
    Ok(serde_json::from_slice(&bytes[1..])?)
}

pub(super) fn bridge(id: &str, policy: &str) -> Result<()> {
    let launcher = Launcher {
        executable: current_executable()?,
        prefix: vec![],
    };
    let dir=service_directories().into_iter().find(|dir|matches!(request_once(&Request::Runtimes,&launcher,dir),Ok(Response::Runtimes(infos)) if infos.iter().any(|i|i.id==id))).context("Runtime is no longer available; refresh the running list")?;
    let mut stream = connect_at(&dir)?;
    send_json(
        &mut stream,
        &Request::AttachRuntime {
            id: id.into(),
            policy: policy.into(),
        },
    )?;
    stream.set_read_timeout(None)?;
    let mut sender = stream.try_clone()?;
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(std::io::stdin());
        use std::io::BufRead;
        loop {
            let mut bytes = vec![];
            if !matches!((&mut reader).take(FRAME_LIMIT as u64+1).read_until(b'\n',&mut bytes),Ok(n) if n>0&&n<FRAME_LIMIT)
            {
                break;
            }
            let Ok(value) = serde_json::from_slice::<Envelope>(&bytes) else {
                break;
            };
            let data = serde_json::to_vec(&value).unwrap();
            if sender
                .write_all(&((data.len() + 1) as u32).to_be_bytes())
                .and_then(|_| sender.write_all(b"J"))
                .and_then(|_| sender.write_all(&data))
                .is_err()
            {
                break;
            }
        }
        let _ = sender.shutdown(std::net::Shutdown::Both);
    });
    loop {
        let value: Value = receive(&mut stream)?;
        if value["kind"] == "error" {
            bail!(
                "{}",
                value["data"]
                    .as_str()
                    .unwrap_or("Runtime attachment failed")
            );
        }
        serde_json::to_writer(std::io::stdout().lock(), &value)?;
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn display_cells() -> BTreeMap<String, Cell> {
        (0..20)
            .map(|uid| {
                let id = format!("cell-{uid}");
                (
                    id.clone(),
                    Cell {
                        run: notebook::Run {
                            uid,
                            cell: id,
                            version: 0,
                            code: "display(value, display_id='shared')".into(),
                        },
                        outputs: vec![service::Output {
                            value: json!({"output_type":"display_data", "data":{"text/plain":"seed"}}),
                            display: Some("shared".into()),
                        }],
                        count: Some(uid + 1),
                        done: Some("ok".into()),
                        revision: 9,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn oversized_display_fan_out_preserves_every_previous_output_and_revision() {
        let mut cells = display_cells();
        let previous = serde_json::to_vec(&cells).unwrap();
        let output =
            json!({"output_type":"display_data", "data":{"text/plain":"x".repeat(512*1024)}});
        let error = update_notebook_displays(&mut cells, "shared", &output).unwrap_err();
        assert!(error.to_string().contains("8 MiB"));
        assert_eq!(serde_json::to_vec(&cells).unwrap(), previous);
    }

    #[test]
    fn permitted_display_updates_replace_matching_outputs_across_cells() {
        let mut cells = display_cells();
        cells.get_mut("cell-0").unwrap().outputs[0].display = Some("other".into());
        let output = json!({"output_type":"display_data", "data":{"text/plain":"updated"}});
        update_notebook_displays(&mut cells, "shared", &output).unwrap();
        assert_eq!(
            cells["cell-0"].outputs[0].value["data"]["text/plain"],
            "seed"
        );
        assert_eq!(cells["cell-0"].revision, 9);
        for cell in cells.values().filter(|c| c.run.cell != "cell-0") {
            assert_eq!(cell.outputs[0].value, output);
            assert_eq!(cell.revision, 10);
            assert_eq!(cell.done.as_deref(), Some("ok"));
        }
        assert!(json_size(&cells).unwrap() <= NOTEBOOK_LIMIT);
    }
}
