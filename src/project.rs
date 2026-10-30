//! Small, bounded project services. The remote peer implements this wire format
//! using only Python's standard library; local services stay native Rust.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;

/// Tagged project-host service requests shared with `remote_project.py`. Calls are explicit and
/// bounded; the caller supplies the active sandbox boundary, except for intentional read-only
/// host telemetry.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "task", rename_all = "snake_case")]
pub enum Request {
    /// Format/check an unsaved TeX buffer without modifying its source file.
    LatexAnalyze {
        /// Working project root on the execution host.
        root: String,
        /// Source path used for language/tool context.
        path: String,
        /// Unsaved source snapshot.
        text: String,
        /// Installed LaTeX formatter/checker configuration.
        settings: crate::latex::Settings,
        /// Request formatted text in the reply.
        format: bool,
        /// Request source diagnostics in the reply.
        check: bool,
    },
    /// Run an explicitly enabled installed adapter.
    Extension {
        /// Working project root on the execution host.
        root: String,
        /// Validated adapter invocation and source snapshot.
        invocation: crate::extensions::Invocation,
    },
    /// Discover or manage native host-owned PTY sessions.
    Sessions {
        /// Project root used by the remote service wrapper.
        root: String,
        /// Session creation/discovery/mutation request.
        operation: crate::sessions::Operation,
        /// Optional checksum-verified native helper deployment payload.
        helper: Option<crate::sessions::Helper>,
        /// Optional explicit remote service runtime directory.
        directory: Option<String>,
    },
    /// Read scheduler allocations or explicitly cancel one owned allocation.
    Slurm {
        /// Working project root on the scheduler host.
        root: String,
        /// Installed `squeue` executable override.
        queue: String,
        /// Installed `scancel` executable override.
        cancel_program: String,
        /// Exact allocation identifier to cancel; `None` requests a snapshot.
        cancel: Option<String>,
    },
    /// Execute an explicit Git argument vector.
    Git {
        /// Repository working directory on the project host.
        root: String,
        /// Literal Git arguments, without shell parsing.
        args: Vec<String>,
        /// Optional UTF-8 stdin, such as a patch or commit message.
        input: Option<String>,
    },
    /// Sample host counters on demand, outside the project's sandbox by explicit policy.
    Monitor {
        /// Filesystem path used for disk-capacity statistics.
        root: String,
    },
    /// Discover available Python environments and inspect the selected interpreter.
    Environments {
        /// Project directory used for conventional environment discovery.
        root: String,
        /// Configured interpreter path or executable name.
        selected: String,
    },
    /// Explicitly create a Python environment without replacing an existing one.
    CreateEnvironment {
        /// Project root used to resolve a relative destination.
        root: String,
        /// Requested new environment directory.
        path: String,
        /// Installed base Python executable.
        python: String,
        /// Whether creation should include pip.
        with_pip: bool,
    },
    /// Format/check an unsaved source snapshot with its registered language tools.
    Analyze {
        /// Working project root on the execution host.
        root: String,
        /// Source filename used for tool/language detection.
        path: String,
        /// Unsaved source snapshot; tools do not write the source file.
        text: String,
        /// Selected Python interpreter for environment-aware tools.
        python: String,
        /// Ruff override, `auto` discovery, or the requested default `uvx ruff` route.
        ruff: String,
        /// Request formatting of the submitted snapshot.
        format: bool,
        /// Request diagnostics for the submitted snapshot.
        check: bool,
        /// Request supported safe fixes; unsafe Ruff fixes stay disabled.
        fix: bool,
    },
}

/// Typed replies to project-service requests.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Response {
    /// Installed adapter reply.
    Extension(crate::extensions::Reply),
    /// Native terminal discovery/control reply.
    Sessions(crate::sessions::Reply),
    /// User-owned scheduler allocation snapshot.
    Slurm(crate::slurm::Snapshot),
    /// Explicit command output, including cancellation commands.
    Command(Output),
    /// Host utilization/capacity sample.
    Monitor(Monitor),
    /// Python environment discovery and interpreter information.
    Environments(Environments),
    /// Formatted buffer text and diagnostics.
    Analysis(Analysis),
}

/// Bounded UTF-8 command output and an exit status.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Output {
    /// Exit code; `-1` represents termination without a conventional exit code.
    pub code: i32,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}
impl Output {
    /// Return whether the command exited with code zero.
    pub fn success(&self) -> bool {
        self.code == 0
    }
    /// Return stdout on success or combine diagnostic output into an error on failure.
    pub fn checked(self) -> Result<String> {
        if self.success() {
            Ok(self.stdout)
        } else {
            bail!("{}", self.stderr.trim().to_owned() + &self.stdout)
        }
    }
}

/// Cumulative Linux CPU counters; utilization requires differences between samples.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Cpu {
    /// Counter name, including aggregate `cpu` or an individual core.
    pub name: String,
    /// Total CPU ticks from the first eight `/proc/stat` counters.
    pub total: u64,
    /// Idle plus I/O-wait ticks from `/proc/stat`.
    pub idle: u64,
}
/// Optional GPU measurements; unavailable values remain `None`, not fabricated zeroes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Gpu {
    /// GPU index/name label.
    pub name: String,
    /// GPU utilization percentage.
    pub usage: Option<f32>,
    /// Used device memory in bytes.
    pub memory_used: Option<u64>,
    /// Total device memory in bytes.
    pub memory_total: Option<u64>,
    /// Temperature in degrees Celsius.
    pub temperature: Option<f32>,
    /// Power draw in watts.
    pub power: Option<f32>,
}
/// One bounded `ps` row; values remain display text from the host tool.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Process {
    /// Host process identifier.
    pub pid: String,
    /// CPU percentage as formatted by `ps`.
    pub cpu: String,
    /// Memory percentage as formatted by `ps`.
    pub memory: String,
    /// Host command name/arguments for display.
    pub name: String,
}
/// Read-only host snapshot; cumulative byte/tick counters are converted to rates by the UI.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Monitor {
    /// Execution host's reported hostname.
    pub hostname: String,
    /// Aggregate and per-core cumulative CPU counters.
    pub cpus: Vec<Cpu>,
    /// Physical memory capacity in bytes.
    pub memory_total: u64,
    /// Currently available physical memory in bytes.
    pub memory_available: u64,
    /// Swap capacity in bytes.
    pub swap_total: u64,
    /// Unused swap in bytes.
    pub swap_free: u64,
    /// Host load averages, normally the 1/5/15-minute values.
    pub load: Vec<f32>,
    /// Host uptime in seconds.
    pub uptime: f64,
    /// Cumulative received network bytes, excluding loopback.
    pub net_received: u64,
    /// Cumulative sent network bytes, excluding loopback.
    pub net_sent: u64,
    /// Capacity of the sampled filesystem in bytes.
    pub disk_total: u64,
    /// Available space on the sampled filesystem in bytes.
    pub disk_free: u64,
    /// Measurements from detected NVIDIA/DRM devices.
    pub gpus: Vec<Gpu>,
    /// Explanation when GPU metrics are unavailable or incomplete.
    pub gpu_note: String,
    /// Bounded high-CPU process list.
    pub processes: Vec<Process>,
}

/// One discovered project-host Python interpreter.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Environment {
    /// Display name for the environment candidate.
    pub name: String,
    /// Interpreter executable path on the project host.
    pub python: String,
}
/// Environment candidates plus information from the selected interpreter.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Environments {
    /// Bounded detected conventional project environments.
    pub candidates: Vec<Environment>,
    /// Resolved selected Python executable.
    pub executable: String,
    /// Selected interpreter's reported Python version.
    pub version: String,
    /// Selected interpreter's `sys.prefix`.
    pub prefix: String,
    /// Selected interpreter's `sys.base_prefix`, for virtual-environment detection.
    pub base_prefix: String,
}
/// Normalized source diagnostic. Rows and character columns are one-based; end positions are
/// tool-derived and may equal the start for point diagnostics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Tool/rule identifier.
    pub code: String,
    /// Human-readable issue description.
    pub message: String,
    /// One-based start line.
    pub row: usize,
    /// One-based start character column after tool-specific normalization.
    pub column: usize,
    /// One-based end line.
    pub end_row: usize,
    /// One-based end character column.
    pub end_column: usize,
    /// Severity label, such as `error` or `warning`.
    pub severity: String,
}
/// Result for the exact submitted buffer snapshot; the UI must version-check before applying
/// it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Analysis {
    /// Resulting source text, unchanged when formatting/fixing was not requested.
    pub text: String,
    /// Bounded normalized source issues.
    pub diagnostics: Vec<Diagnostic>,
    /// Missing-tool and other nonfatal explanations; editing remains available.
    pub warnings: Vec<String>,
}

/// Execute a blocking project service on this host; callers enforce sandbox policy and keep
/// this off the UI thread.
pub fn local(request: &Request) -> Result<Response> {
    match request {
        Request::LatexAnalyze {
            root,
            path,
            text,
            settings,
            format,
            check,
        } => Ok(Response::Analysis(crate::latex::analyze(
            &crate::files::expand_home(root),
            path,
            text,
            settings,
            *format,
            *check,
        )?)),
        Request::Extension { root, invocation } => Ok(Response::Extension(crate::extensions::run(
            root, invocation,
        )?)),
        Request::Sessions { operation, .. } => {
            Ok(Response::Sessions(crate::sessions::local(operation)?))
        }
        Request::Slurm {
            root,
            queue,
            cancel_program,
            cancel,
        } => {
            let root = crate::files::expand_home(root);
            if let Some(id) = cancel {
                Ok(Response::Command(crate::slurm::cancel(
                    &root,
                    queue,
                    cancel_program,
                    id,
                )?))
            } else {
                Ok(Response::Slurm(crate::slurm::snapshot(&root, queue)?))
            }
        }
        Request::Git { root, args, input } => {
            if args.is_empty()
                || args.len() > 128
                || args.iter().map(String::len).sum::<usize>() > 64 * 1024
            {
                bail!("Invalid Git command size.");
            }
            if input
                .as_ref()
                .is_some_and(|s| s.len() as u64 > OUTPUT_LIMIT)
            {
                bail!("Git input exceeds 8 MiB.");
            }
            let mut command = Command::new("git");
            command
                .current_dir(crate::files::expand_home(root))
                .args([
                    "--no-pager",
                    "--literal-pathspecs",
                    "-c",
                    "color.ui=false",
                    "-c",
                    "core.quotePath=false",
                ])
                .args(args)
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_EDITOR", "true")
                .env("GIT_SEQUENCE_EDITOR", "true")
                .env("LC_ALL", "C.UTF-8");
            Ok(Response::Command(run(
                command,
                input.clone().unwrap_or_default().into_bytes(),
                Duration::from_secs(120),
            )?))
        }
        Request::Monitor { root } => Ok(Response::Monitor(monitor(root)?)),
        Request::Environments { root, selected } => {
            Ok(Response::Environments(environments(root, selected)?))
        }
        Request::CreateEnvironment {
            root,
            path,
            python,
            with_pip,
        } => {
            let root = crate::files::expand_home(root).canonicalize()?;
            let path = resolve(&root, path);
            if path.exists() {
                bail!(
                    "The environment destination already exists. Select it, or choose a new folder."
                );
            }
            let parent = path
                .parent()
                .context("Environment needs a parent directory")?
                .canonicalize()?;
            if !parent.starts_with(&root) {
                bail!("Create environments inside the project folder.");
            }
            let path = parent.join(path.file_name().context("Missing environment name")?);
            std::fs::create_dir(&path)
                .context("Cannot reserve the environment folder; it may already exist.")?;
            let mut command = Command::new(python_path(&root, python));
            command.current_dir(&root).args(["-m", "venv"]).arg(&path);
            if !with_pip {
                command.arg("--without-pip");
            }
            Ok(Response::Command(run(
                command,
                vec![],
                Duration::from_secs(120),
            )?))
        }
        Request::Analyze {
            root,
            path,
            text,
            python,
            ruff,
            format,
            check,
            fix,
        } => {
            if text.len() as u64 > crate::files::MAX_FILE_BYTES {
                bail!("Buffer exceeds the tool limit.");
            }
            Ok(Response::Analysis(analyze(
                &crate::files::expand_home(root),
                path,
                text,
                python,
                ruff,
                *format,
                *check,
                *fix,
            )?))
        }
    }
}

fn resolve(root: &Path, value: &str) -> PathBuf {
    let path = crate::files::expand_home(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}
/// Resolve an interpreter or environment directory relative to the project, honoring explicit
/// executable overrides.
pub fn python_path(root: &Path, value: &str) -> PathBuf {
    if value == "off" {
        return PathBuf::new();
    }
    if value == "auto" {
        return [".venv", "venv", "env", ".env"]
            .into_iter()
            .map(|name| root.join(name).join("bin/python"))
            .find(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("python3"));
    }
    if value.trim().is_empty() {
        return PathBuf::from("python3");
    }
    let path = if value.contains('/') || value.starts_with('~') {
        resolve(root, value)
    } else {
        PathBuf::from(value)
    };
    if path.is_dir() {
        path.join("bin/python")
    } else {
        path
    }
}

/// Build environment variables for new project commands from the selected interpreter; existing
/// terminals are not modified.
pub fn environment_variables(python: &str) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let path = Path::new(python);
    if path.is_absolute()
        && let Some(bin) = path.parent()
    {
        env.insert(
            "PATH".into(),
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into())
            ),
        );
        if let Some(prefix) = bin.parent()
            && prefix.join("pyvenv.cfg").exists()
        {
            env.insert("VIRTUAL_ENV".into(), prefix.to_string_lossy().into());
        }
    }
    env
}

fn environments(root: &str, selected: &str) -> Result<Environments> {
    let root = crate::files::expand_home(root);
    let mut candidates = vec![Environment {
        name: "System Python".into(),
        python: String::new(),
    }];
    for name in [".venv", "venv", "env", ".env"] {
        let path = root.join(name).join("bin/python");
        if path.is_file() {
            candidates.push(Environment {
                name: name.into(),
                python: path.to_string_lossy().into(),
            });
        }
    }
    let python = python_path(&root, selected);
    let mut command = Command::new(&python);
    command.current_dir(root).args(["-c", "import json,sys;print(json.dumps(dict(executable=sys.executable,version=sys.version.split()[0],prefix=sys.prefix,base_prefix=sys.base_prefix)))"]);
    let mut result: Environments =
        serde_json::from_str(&run(command, vec![], Duration::from_secs(10))?.checked()?)?;
    result.candidates = candidates;
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn analyze(
    root: &Path,
    path: &str,
    text: &str,
    python: &str,
    ruff: &str,
    format: bool,
    check: bool,
    fix: bool,
) -> Result<Analysis> {
    let mut result = Analysis {
        text: text.into(),
        ..Default::default()
    };
    let language =
        crate::languages::detect(Some(path)).context("No code tools for this file type.")?;
    if language.id == "latex" {
        anyhow::ensure!(!fix, "Ruff safe fixes apply only to Python files.");
        return crate::latex::analyze(root, path, text, &Default::default(), format, check);
    }
    if language.id != "python" {
        if fix {
            bail!("Ruff safe fixes apply only to Python files.");
        }
        let tool = language
            .formatter
            .as_deref()
            .context("No formatter for this language. Use the project terminal for other tools.")?;
        if !format && !language.check {
            bail!(
                "{tool} provides formatting only; use the project terminal for compiler/linter checks."
            );
        }
        let mut command = Command::new(crate::languages::Language::executable(root, tool));
        command
            .current_dir(root)
            .args(language.formatter_args(path))
            .env("NO_COLOR", "1");
        let output = run(command, text.as_bytes().to_vec(), Duration::from_secs(20))
            .with_context(|| format!("{tool} is unavailable. Install it on the project host and make it available on PATH."))?;
        if output.success() {
            if format {
                result.text = output.stdout;
            }
        } else {
            result.diagnostics = crate::languages::syntax_diagnostics(&output.stderr, tool);
            crate::languages::normalize_columns(&mut result.diagnostics, text, tool);
            if result.diagnostics.is_empty() {
                bail!("{tool}: {}", output.stderr.trim());
            }
            if format {
                result.warnings.push(format!(
                    "{tool} reported syntax errors; the buffer was kept unchanged."
                ));
            }
        }
        return Ok(result);
    }
    let python = python_path(root, python);
    let ruff = ruff.trim();
    let uvx = ruff.is_empty() || ruff == "uvx ruff";
    let mut tool = if uvx {
        use std::os::unix::fs::PermissionsExt;
        let local = crate::files::expand_home("~/.local/bin/uvx");
        let program = crate::sandbox::find_program("uvx")
            .or_else(|| {
                local
                    .metadata()
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                    .then_some(local)
            })
            .unwrap_or_else(|| PathBuf::from("uvx"));
        vec![program.to_string_lossy().into(), "ruff".into()]
    } else if ruff != "auto" {
        vec![resolve_program(root, ruff)]
    } else {
        let candidate = python
            .parent()
            .map(|p| p.join("ruff"))
            .filter(|p| p.is_file());
        if let Some(candidate) = candidate {
            vec![candidate.to_string_lossy().into()]
        } else {
            let mut probe = Command::new(&python);
            probe.current_dir(root).args(["-c", "import importlib.util;raise SystemExit(0 if importlib.util.find_spec('ruff') else 1)"]);
            if run(probe, vec![], Duration::from_secs(5)).is_ok_and(|o| o.success()) {
                vec![python.to_string_lossy().into(), "-m".into(), "ruff".into()]
            } else {
                vec!["ruff".into()]
            }
        }
    };
    let program = tool.remove(0);
    let execute = |args: &[&str], input: &str| -> Result<Output> {
        let mut command = Command::new(&program);
        command
            .current_dir(root)
            .args(&tool)
            .args(args)
            .envs(environment_variables(&python.to_string_lossy()))
            .env_remove("RUFF_OUTPUT_FILE");
        // uvx also consults its installed-tool registry. Keep it in the shared
        // cache so read-only ~/.local mounts do not prevent tool execution.
        if uvx && std::env::var_os("UV_TOOL_DIR").is_none() {
            let cache = std::env::var_os("UV_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| crate::files::expand_home("~/.cache/uv"));
            command.env("UV_TOOL_DIR", cache.join("tools"));
        }
        run(command, input.as_bytes().to_vec(), Duration::from_secs(20)).context("Ruff is unavailable. Install uv on the project host for uvx ruff, or choose auto / an explicit Ruff executable in Project settings")
    };
    if fix {
        let output = execute(
            &[
                "check",
                "--fix",
                "--no-unsafe-fixes",
                "--exit-zero",
                "--stdin-filename",
                path,
                "-",
            ],
            &result.text,
        )?;
        if output.success() {
            result.text = output.stdout;
        } else {
            bail!("{}", output.stderr);
        }
    }
    if format {
        let output = execute(&["format", "--stdin-filename", path, "-"], &result.text)?;
        if output.success() {
            result.text = output.stdout;
        } else if check {
            result.warnings.push(output.stderr);
        } else {
            bail!("{}", output.stderr);
        }
    }
    if check {
        let output = execute(
            &[
                "check",
                "--no-fix",
                "--output-format=json",
                "--stdin-filename",
                path,
                "-",
            ],
            &result.text,
        )?;
        if ![0, 1].contains(&output.code) {
            bail!("{}", output.stderr);
        }
        let diagnostics: Vec<serde_json::Value> =
            serde_json::from_str(&output.stdout).context("Ruff returned invalid diagnostics")?;
        result.diagnostics = diagnostics
            .into_iter()
            .take(2000)
            .map(|d| Diagnostic {
                code: d["code"].as_str().unwrap_or("syntax").into(),
                message: d["message"].as_str().unwrap_or("Issue").into(),
                row: d["location"]["row"].as_u64().unwrap_or(1) as usize,
                column: d["location"]["column"].as_u64().unwrap_or(1) as usize,
                end_row: d["end_location"]["row"].as_u64().unwrap_or(1) as usize,
                end_column: d["end_location"]["column"].as_u64().unwrap_or(1) as usize,
                severity: if d["code"].as_str().is_none_or(|c| {
                    c == "invalid-syntax" || c.starts_with('E') || c.starts_with('F')
                }) {
                    "error"
                } else {
                    "warning"
                }
                .into(),
            })
            .collect();
    }
    Ok(result)
}
fn resolve_program(root: &Path, value: &str) -> String {
    if value.contains('/') || value.starts_with('~') {
        resolve(root, value).to_string_lossy().into()
    } else {
        value.into()
    }
}

/// Drain both pipes concurrently, cap allocations, and kill the whole command
/// group on timeout/overflow (including hooks and children).
pub fn run(command: Command, input: Vec<u8>, timeout: Duration) -> Result<Output> {
    run_controlled(command, input, timeout, &AtomicBool::new(false), false)
}

pub(crate) fn run_controlled(
    command: Command,
    input: Vec<u8>,
    timeout: Duration,
    cancel: &AtomicBool,
    lifeline: bool,
) -> Result<Output> {
    run_controlled_limited(
        command,
        input,
        timeout,
        cancel,
        lifeline,
        OUTPUT_LIMIT,
        Duration::ZERO,
    )
}
pub(crate) fn run_controlled_limited(
    command: Command,
    input: Vec<u8>,
    timeout: Duration,
    cancel: &AtomicBool,
    lifeline: bool,
    output_limit: u64,
    terminate_grace: Duration,
) -> Result<Output> {
    run_controlled_stream(
        command,
        std::io::Cursor::new(input),
        timeout,
        cancel,
        lifeline,
        output_limit,
        terminate_grace,
    )
}

/// Stream an owned input without retaining large executable transfers in memory.
/// The same cancellation, output bounds and process-group cleanup apply as for tools.
pub(crate) fn run_controlled_stream(
    mut command: Command,
    mut input: impl Read + Send + 'static,
    timeout: Duration,
    cancel: &AtomicBool,
    lifeline: bool,
    output_limit: u64,
    terminate_grace: Duration,
) -> Result<Output> {
    anyhow::ensure!(!cancel.load(Ordering::Acquire), "Extension task cancelled.");
    let description = command.get_program().to_string_lossy().into_owned();
    crate::process::new_session(&mut command);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Cannot run {description}"))?;
    let mut stdin = child.stdin.take().context("Missing command stdin")?;
    let stdout = child.stdout.take().context("Missing command stdout")?;
    let stderr = child.stderr.take().context("Missing command stderr")?;
    let (input_tx, input_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut input, &mut stdin);
        if lifeline {
            let _ = input_tx.send(stdin);
        }
    });
    let mut input_rx = Some(input_rx);
    let overflow = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    for (is_out, pipe) in [
        (true, Box::new(stdout) as Box<dyn Read + Send>),
        (false, Box::new(stderr) as Box<dyn Read + Send>),
    ] {
        let tx = tx.clone();
        let overflow = overflow.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.take(output_limit + 1).read_to_end(&mut bytes);
            if bytes.len() as u64 > output_limit {
                overflow.store(true, Ordering::Release);
            }
            let _ = tx.send((is_out, bytes, result));
        });
    }
    let kill = |pid: u32| unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    };
    let stop = |child: &mut std::process::Child| {
        if !terminate_grace.is_zero() {
            // Marp/Puppeteer closes its independently grouped browser on SIGINT.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGINT);
            }
            let until = Instant::now() + terminate_grace;
            while Instant::now() < until && child.try_wait().ok().flatten().is_none() {
                std::thread::sleep(Duration::from_millis(15));
            }
        }
        kill(child.id());
        let _ = child.wait();
    };
    let start = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Acquire) {
            // Closing the coordinator's input first lets it terminate the adapter
            // process group on either host, before the transport is torn down.
            input_rx.take();
            if lifeline {
                let grace = Instant::now();
                while grace.elapsed() < Duration::from_secs(1).max(terminate_grace) {
                    if child.try_wait()?.is_some() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
            }
            stop(&mut child);
            bail!("Extension task cancelled.");
        }
        if overflow.load(Ordering::Acquire) || start.elapsed() > timeout {
            input_rx.take();
            stop(&mut child);
            bail!(
                "{description} exceeded its time or {} MiB output limit.",
                output_limit / 1024 / 1024
            );
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let mut output = Output {
        code: status.code().unwrap_or(-1),
        ..Default::default()
    };
    for _ in 0..2 {
        let (is_out, bytes, result) = match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(value) => value,
            Err(error) => {
                kill(child.id());
                return Err(error).context("Command output pipes did not close");
            }
        };
        result?;
        if bytes.len() as u64 > output_limit {
            bail!(
                "Command output exceeded {} MiB.",
                output_limit / 1024 / 1024
            );
        }
        let value = String::from_utf8(bytes).context("Tool output is not UTF-8")?;
        if is_out {
            output.stdout = value;
        } else {
            output.stderr = value;
        }
    }
    Ok(output)
}

fn read(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap_or_default()
}
fn number(path: impl AsRef<Path>) -> Option<u64> {
    read(path).trim().parse().ok()
}
fn monitor(root: &str) -> Result<Monitor> {
    let mut result = Monitor {
        hostname: read("/proc/sys/kernel/hostname").trim().into(),
        ..Default::default()
    };
    for line in read("/proc/stat").lines().filter(|l| l.starts_with("cpu")) {
        let mut words = line.split_whitespace();
        let name = words.next().unwrap_or_default().to_owned();
        let values: Vec<u64> = words.take(8).filter_map(|s| s.parse().ok()).collect();
        result.cpus.push(Cpu {
            name,
            total: values.iter().sum(),
            idle: values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0),
        });
    }
    if result.cpus.is_empty() {
        bail!("Host monitoring currently requires Linux /proc.");
    }
    let memory: BTreeMap<String, u64> = read("/proc/meminfo")
        .lines()
        .filter_map(|line| {
            let (key, rest) = line.split_once(':')?;
            Some((
                key.to_owned(),
                rest.split_whitespace().next()?.parse::<u64>().ok()? * 1024,
            ))
        })
        .collect();
    result.memory_total = *memory.get("MemTotal").unwrap_or(&0);
    result.memory_available = *memory.get("MemAvailable").unwrap_or(&0);
    result.swap_total = *memory.get("SwapTotal").unwrap_or(&0);
    result.swap_free = *memory.get("SwapFree").unwrap_or(&0);
    result.load = read("/proc/loadavg")
        .split_whitespace()
        .take(3)
        .filter_map(|s| s.parse().ok())
        .collect();
    result.uptime = read("/proc/uptime")
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.0);
    for line in read("/proc/net/dev").lines() {
        if let Some((name, fields)) = line.split_once(':')
            && name.trim() != "lo"
        {
            let fields: Vec<u64> = fields
                .split_whitespace()
                .filter_map(|s| s.parse().ok())
                .collect();
            result.net_received += fields.first().copied().unwrap_or(0);
            result.net_sent += fields.get(8).copied().unwrap_or(0);
        }
    }
    if let Ok(path) =
        std::ffi::CString::new(crate::files::expand_home(root).to_string_lossy().as_bytes())
    {
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } == 0 {
            let stats = unsafe { stats.assume_init() };
            result.disk_total = stats.f_blocks * stats.f_frsize;
            result.disk_free = stats.f_bavail * stats.f_frsize;
        }
    }
    let mut nvidia = Command::new("nvidia-smi");
    nvidia.args(["--query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw", "--format=csv,noheader,nounits"]);
    match run(nvidia, vec![], Duration::from_secs(3)) {
        Ok(output) if output.success() => result.gpus = parse_nvidia(&output.stdout),
        Ok(output) => result.gpu_note = output.stderr.trim().chars().take(256).collect(),
        Err(_) => {}
    }
    if let Ok(cards) = fs::read_dir("/sys/class/drm") {
        for card in cards.flatten() {
            let name = card.file_name().to_string_lossy().into_owned();
            if !name
                .strip_prefix("card")
                .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
            {
                continue;
            }
            let device = card.path().join("device");
            let vendor = read(device.join("vendor"));
            if vendor.trim() == "0x10de" {
                continue;
            }
            let brand = match vendor.trim() {
                "0x1002" => "AMD",
                "0x8086" => "Intel",
                _ => "DRM",
            };
            let temperature = fs::read_dir(device.join("hwmon"))
                .ok()
                .and_then(|dirs| {
                    dirs.flatten()
                        .find_map(|d| number(d.path().join("temp1_input")))
                })
                .map(|n| n as f32 / 1000.0);
            result.gpus.push(Gpu {
                name: format!("{brand} {name}"),
                usage: number(device.join("gpu_busy_percent")).map(|n| n as f32),
                memory_used: number(device.join("mem_info_vram_used")),
                memory_total: number(device.join("mem_info_vram_total")),
                temperature,
                power: None,
            });
        }
    }
    let mut ps = Command::new("ps");
    ps.args(["-eo", "pid,pcpu,pmem,comm", "--sort=-pcpu"]);
    if let Ok(output) = run(ps, vec![], Duration::from_secs(2)) {
        result.processes = output
            .stdout
            .lines()
            .skip(1)
            .take(12)
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                Some(Process {
                    pid: parts.next()?.into(),
                    cpu: parts.next()?.into(),
                    memory: parts.next()?.into(),
                    name: parts.collect::<Vec<_>>().join(" "),
                })
            })
            .collect();
    }
    Ok(result)
}

/// Parse bounded NVIDIA CSV readings, converting MiB to bytes and retaining unavailable values.
pub fn parse_nvidia(output: &str) -> Vec<Gpu> {
    output
        .lines()
        .filter_map(|line| {
            let parts: Vec<_> = line.split(',').map(str::trim).collect();
            if parts.len() != 7 {
                return None;
            }
            let num = |i: usize| parts[i].parse::<f32>().ok();
            Some(Gpu {
                name: format!("GPU {} · {}", parts[0], parts[1].trim_matches('"')),
                usage: num(2),
                memory_used: num(3).map(|n| (n as f64 * 1048576.0) as u64),
                memory_total: num(4).map(|n| (n as f64 * 1048576.0) as u64),
                temperature: num(5),
                power: num(6),
            })
        })
        .collect()
}

/// Porcelain v1 -z is independent of Git's path quoting and relative-path settings.
#[derive(Clone, Debug, PartialEq)]
pub struct GitFile {
    /// Porcelain index status character.
    pub index: char,
    /// Porcelain worktree status character.
    pub worktree: char,
    /// Repository-relative current path from NUL-delimited status output.
    pub path: String,
    /// Original repository-relative path for a rename/copy entry.
    pub original: Option<String>,
}
impl GitFile {
    /// Return whether porcelain status indicates an unresolved merge conflict.
    pub fn conflict(&self) -> bool {
        self.index == 'U'
            || self.worktree == 'U'
            || matches!((self.index, self.worktree), ('A', 'A') | ('D', 'D'))
    }
    /// Return whether the entry has staged changes rather than only untracked/ignored state.
    pub fn staged(&self) -> bool {
        ![' ', '?', '!'].contains(&self.index)
    }
}
/// Parse `git status --porcelain=v1 -z --branch`, preserving unquoted filenames and rename
/// sources.
pub fn parse_status(output: &str) -> (String, Vec<GitFile>) {
    let mut chunks = output.split('\0');
    let mut branch = String::new();
    let mut files = vec![];
    while let Some(chunk) = chunks.next() {
        if let Some(header) = chunk.strip_prefix("## ") {
            branch = header.into();
            continue;
        }
        let bytes = chunk.as_bytes();
        if bytes.len() < 4 {
            continue;
        }
        let index = bytes[0] as char;
        let worktree = bytes[1] as char;
        let original = if ['R', 'C'].contains(&index) || ['R', 'C'].contains(&worktree) {
            chunks.next().map(str::to_owned)
        } else {
            None
        };
        files.push(GitFile {
            index,
            worktree,
            path: chunk[3..].into(),
            original,
        });
    }
    (branch, files)
}

/// Split a unified diff into independently applicable hunk patches and display labels.
pub fn hunk_patches(diff: &str) -> Vec<(String, String)> {
    let mut header = String::new();
    let mut current = String::new();
    let mut label = String::new();
    let mut result = vec![];
    for line in diff.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            if !current.is_empty() {
                result.push((label.clone(), header.clone() + &current));
                current.clear();
            }
            header.clear();
            header.push_str(line);
        } else if line.starts_with("@@ ") {
            if !current.is_empty() {
                result.push((label.clone(), header.clone() + &current));
                current.clear();
            }
            label = line.trim().into();
            current.push_str(line);
        } else if current.is_empty() {
            header.push_str(line);
        } else {
            current.push_str(line);
        }
    }
    if !current.is_empty() {
        result.push((label, header + &current));
    }
    result
}
