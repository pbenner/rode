//! Connection-owned native host gateway. Persistent jobs remain in the session service.
use anyhow::{Context, Result, bail, ensure};
use rode_host_core::{environment, files, project, sandbox};
use rode_host_protocol::{self as wire, framing};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{BufRead, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Service composition excludes all desktop decoded results.
pub type FileRequest = wire::files::Request<
    project::Request<crate::workspace::Operation, crate::workspace::Helper>,
    sandbox::Options,
>;
/// Typed native gateway command.
pub type Operation = wire::Operation<FileRequest, sandbox::Options, environment::Settings>;

/// JSON-compatible desktop reply without any rendering dependencies.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct FileResponse {
    /// Encoded filesystem result; decoding remains in the desktop.
    #[serde(flatten)]
    pub file: wire::files::Response,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Actual prepared policy and opaque context identity.
    pub project: Option<sandbox::Plan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Shared project or persistent-service result.
    pub tool: Option<project::Response<crate::workspace::Reply>>,
}

/// Typed response correlated with a gateway request.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    /// Newly prepared project and initial listing.
    Prepared {
        /// Prepared policy and file listing.
        response: FileResponse,
    },
    /// Captured project policy with a new environment generation.
    Derived {
        /// Opaque derived context identity.
        context: String,
    },
    /// Acknowledged filesystem or service operation.
    Completed {
        /// Result of this operation only.
        response: FileResponse,
    },
}

fn local(request: &FileRequest, cancelled: &AtomicBool) -> Result<FileResponse> {
    match request {
        FileRequest::Prepare { .. } => bail!("Prepare is a host administration operation"),
        FileRequest::Tool { request } => {
            let reply = match request {
                project::Request::Sessions {
                    operation,
                    directory,
                    ..
                } => {
                    let mut command = std::process::Command::new(std::env::current_exe()?);
                    command.arg("--workspace-request");
                    if let Some(directory) = directory {
                        ensure!(
                            std::path::Path::new(directory).is_absolute()
                                && directory.len() <= 4096
                                && !directory.contains(['\0', '\n', '\r']),
                            "Invalid session runtime directory"
                        );
                        command.env("RODE_SESSION_DIR", directory);
                    }
                    let text = project::run_controlled(
                        command,
                        serde_json::to_vec(operation)?,
                        Duration::from_secs(30),
                        cancelled,
                        false,
                    )?
                    .checked()?;
                    let mut reply: crate::workspace::Reply = serde_json::from_str(&text)?;
                    if let Some(directory) = directory {
                        reply.prefix =
                            vec![format!("RODE_SESSION_DIR={directory}"), reply.executable];
                        reply.executable = "env".into();
                    }
                    project::Response::Sessions(reply)
                }
                _ => project::local(request)?,
            };
            Ok(FileResponse {
                tool: Some(reply),
                ..Default::default()
            })
        }
        _ => Ok(FileResponse {
            file: files::dispatch(request)?,
            ..Default::default()
        }),
    }
}

/// One-shot host request for explicitly launched local helpers. Gateway workers
/// keep their narrower admission checks and captured policy independently.
pub fn file_request() -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(framing::MAX_MESSAGE as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= framing::MAX_MESSAGE,
        "File request exceeds its limit"
    );
    let request: FileRequest = serde_json::from_slice(&bytes)?;
    let response = if let FileRequest::Prepare {
        path,
        sandbox: options,
    } = &request
    {
        let plan = sandbox::prepare_native(path, options)?;
        FileResponse {
            file: files::local(&files::Request::List {
                path: plan.root.clone(),
            })?,
            project: Some(plan),
            ..Default::default()
        }
    } else {
        local(&request, &AtomicBool::new(false))?
    };
    serde_json::to_writer(std::io::stdout().lock(), &response)?;
    Ok(())
}

/// A transient worker is started inside its captured policy before creating threads.
/// The trailing input channel is a lifetime lease, not another source of requests.
pub fn worker() -> Result<()> {
    let mut input = std::io::BufReader::new(std::io::stdin());
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(framing::MAX_MESSAGE as u64 + 1)
        .read_until(b'\n', &mut bytes)?;
    ensure!(
        bytes.len() <= framing::MAX_MESSAGE && bytes.last() == Some(&b'\n'),
        "Invalid worker request"
    );
    let request: FileRequest = serde_json::from_slice(&bytes)?;
    ensure!(
        !matches!(
            request,
            FileRequest::Prepare { .. }
                | FileRequest::Tool {
                    request: project::Request::Sessions { .. } | project::Request::Monitor { .. }
                }
        ),
        "Host administration is unavailable in a project worker"
    );
    std::thread::spawn(move || {
        let _ = input.read(&mut [0]);
        rode_host_core::command::CANCELLED.store(true, Ordering::Release);
    });
    let response = local(&request, &rode_host_core::command::CANCELLED)?;
    serde_json::to_writer(std::io::stdout().lock(), &response)?;
    Ok(())
}

struct Gateway {
    digest: String,
    contexts: Mutex<HashMap<String, Arc<sandbox::Plan>>>,
    cancelled: AtomicBool,
}
impl Gateway {
    fn context(&self, id: &str) -> Result<Arc<sandbox::Plan>> {
        self.contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("Host context lock failed"))?
            .get(id)
            .cloned()
            .context("Unknown or expired project context; reopen the project")
    }
    fn insert(&self, mut plan: sandbox::Plan) -> Result<sandbox::Plan> {
        let mut contexts = self
            .contexts
            .lock()
            .map_err(|_| anyhow::anyhow!("Host context lock failed"))?;
        ensure!(
            contexts.len() < 128,
            "Connection context limit reached; reconnect to prepare another project"
        );
        let id = crate::new_id()?;
        plan.host_context = Some(id.clone());
        contexts.insert(id, Arc::new(plan.clone()));
        Ok(plan)
    }
    fn execute(&self, operation: Operation) -> Result<Response> {
        ensure!(
            !self.cancelled.load(Ordering::Acquire),
            "Host connection closed"
        );
        match operation {
            Operation::Prepare { path, options } => {
                let plan = self.insert(sandbox::prepare_native(&path, &options)?)?;
                let response = FileResponse {
                    file: files::local(&files::Request::List {
                        path: plan.root.clone(),
                    })?,
                    project: Some(plan),
                    ..Default::default()
                };
                Ok(Response::Prepared { response })
            }
            Operation::Derive {
                context,
                environment,
            } => {
                environment.validate()?;
                let mut plan = (*self.context(&context)?).clone();
                plan.environment = environment;
                let plan = self.insert(plan)?;
                Ok(Response::Derived {
                    context: plan.host_context.context("Missing context identity")?,
                })
            }
            Operation::Run {
                context: None,
                request,
            } => {
                // The unscoped interface is deliberately narrow: browsing before opening a
                // project, explicit host telemetry, and persistent-service administration.
                ensure!(
                    matches!(
                        &request,
                        FileRequest::List { .. }
                            | FileRequest::Tool {
                                request: project::Request::Sessions { .. }
                                    | project::Request::Monitor { .. }
                            }
                    ),
                    "This operation requires a prepared project context"
                );
                if let FileRequest::Tool {
                    request:
                        project::Request::Sessions {
                            helper: Some(helper),
                            ..
                        },
                } = &request
                {
                    ensure!(
                        helper.digest == self.digest
                            && helper.architecture == std::env::consts::ARCH,
                        "This connection uses a fixed native helper; per-request replacement is refused. Reconnect with a matching RODE_HOST_HELPER."
                    );
                }
                Ok(Response::Completed {
                    response: local(&request, &self.cancelled)?,
                })
            }
            Operation::Run {
                context: Some(context),
                request,
            } => {
                ensure!(
                    !matches!(
                        &request,
                        FileRequest::Prepare { .. }
                            | FileRequest::Tool {
                                request: project::Request::Sessions { .. }
                                    | project::Request::Monitor { .. }
                            }
                    ),
                    "Use the explicit host administration operation"
                );
                let mut plan = (*self.context(&context)?).clone();
                if !matches!(&request, FileRequest::Tool { request } if !matches!(request, project::Request::Mise { .. }))
                {
                    plan.environment.mise = false;
                }
                let (command, _lease) = plan.coordinator_command("--host-worker")?;
                let mut input = serde_json::to_vec(&request)?;
                input.push(b'\n');
                let output = project::run_controlled_limited(
                    command,
                    input,
                    Duration::from_secs(150),
                    &self.cancelled,
                    true,
                    framing::MAX_MESSAGE as u64,
                    Duration::ZERO,
                )?
                .checked()?;
                Ok(Response::Completed {
                    response: serde_json::from_str(&output)
                        .context("Invalid native worker response")?,
                })
            }
        }
    }
}

/// Serve a bounded set of concurrent operations on one authenticated stdio channel.
/// There is no listener, credential storage, or automatic reconnection.
pub fn serve() -> Result<()> {
    let mut stdin = std::io::stdin().lock();
    let mut magic = [0; 12];
    stdin.read_exact(&mut magic)?;
    ensure!(
        &magic == framing::MAGIC,
        "Incompatible host protocol preamble"
    );
    let hello: wire::Hello = framing::read(&mut stdin)?.context("Missing host handshake")?;
    ensure!(
        hello.version == framing::VERSION,
        "Incompatible host protocol version"
    );
    ensure!(
        hello.architecture == std::env::consts::ARCH,
        "Incompatible host architecture"
    );
    let executable = std::env::current_exe()?;
    let bytes = rode_host_core::persistence::read(&executable, 24 * 1024 * 1024)?
        .context("Missing running host helper")?;
    let digest = files::digest(&bytes);
    ensure!(
        hello.digest == digest,
        "Host helper digest does not match the selected artifact"
    );
    let flags = unsafe { libc::fcntl(1, libc::F_GETFL) };
    ensure!(
        flags >= 0 && unsafe { libc::fcntl(1, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
        "Cannot configure host output"
    );
    let output = Mutex::new(());
    let gateway = Gateway {
        digest: digest.clone(),
        contexts: Mutex::new(HashMap::new()),
        cancelled: AtomicBool::new(false),
    };
    {
        let mut writer = PipeWriter {
            cancelled: &gateway.cancelled,
            until: Instant::now() + Duration::from_secs(15),
        };
        writer.write_all(framing::MAGIC)?;
        framing::write(
            &mut writer,
            &wire::Hello {
                version: framing::VERSION,
                architecture: std::env::consts::ARCH.into(),
                digest,
                capabilities: vec![
                    "files".into(),
                    "project_tools".into(),
                    "contexts".into(),
                    "sessions".into(),
                    "native_landlock".into(),
                    "native_mise".into(),
                    "shared_compute_helper".into(),
                ],
            },
        )?;
    }
    // A fixed pool avoids retaining an unbounded number of completed thread handles.
    // The small queue also covers the race between delivery of a reply and worker reuse.
    std::thread::scope(|scope| -> Result<()> {
        let (send, receive) = std::sync::mpsc::sync_channel::<wire::Request<Operation>>(4);
        let receive = Arc::new(Mutex::new(receive));
        for _ in 0..4 {
            let (gateway, output, receive) = (&gateway, &output, receive.clone());
            scope.spawn(move || {
                loop {
                    let request = match receive
                        .lock()
                        .ok()
                        .and_then(|receiver| receiver.recv().ok())
                    {
                        Some(request) => request,
                        None => break,
                    };
                    let result = gateway
                        .execute(request.operation)
                        .map_err(|e| format!("{e:#}"));
                    if let Ok(_guard) = output.lock() {
                        let mut writer = PipeWriter {
                            cancelled: &gateway.cancelled,
                            until: Instant::now() + Duration::from_secs(45),
                        };
                        if framing::write(
                            &mut writer,
                            &wire::Reply {
                                id: request.id,
                                result,
                            },
                        )
                        .is_err()
                        {
                            gateway.cancelled.store(true, Ordering::Release);
                        }
                    }
                }
            });
        }
        let result = (|| {
            while let Some(request) = framing::read::<wire::Request<Operation>>(&mut stdin)? {
                ensure!(
                    !gateway.cancelled.load(Ordering::Acquire),
                    "Host output channel closed"
                );
                match send.try_send(request) {
                    Ok(()) => {}
                    Err(std::sync::mpsc::TrySendError::Full(request)) => {
                        let _guard = output
                            .lock()
                            .map_err(|_| anyhow::anyhow!("Host output lock failed"))?;
                        let mut writer = PipeWriter {
                            cancelled: &gateway.cancelled,
                            until: Instant::now() + Duration::from_secs(45),
                        };
                        framing::write(
                            &mut writer,
                            &wire::Reply::<Response> {
                                id: request.id,
                                result: Err("Host request queue is full".into()),
                            },
                        )?;
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                        bail!("Host workers disconnected")
                    }
                }
            }
            Ok(())
        })();
        gateway.cancelled.store(true, Ordering::Release);
        drop(send);
        result
    })
}

struct PipeWriter<'a> {
    cancelled: &'a AtomicBool,
    until: Instant,
}
impl Write for PipeWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        loop {
            if self.cancelled.load(Ordering::Acquire) || Instant::now() >= self.until {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Host output closed or stalled",
                ));
            }
            let n = unsafe { libc::write(1, bytes.as_ptr().cast(), bytes.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let error = std::io::Error::last_os_error();
            if !matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
            ) {
                return Err(error);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Dispatch service control in a short-lived process with its captured service directory.
pub fn workspace_request() -> Result<()> {
    let mut bytes = vec![];
    std::io::stdin()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 4 * 1024 * 1024,
        "Workspace request exceeds 4 MiB"
    );
    let operation = serde_json::from_slice(&bytes)?;
    let reply = crate::workspace::local(
        &operation,
        crate::Launcher {
            executable: std::env::current_exe()?,
            prefix: vec![],
        },
    )?;
    serde_json::to_writer(std::io::stdout().lock(), &reply)?;
    Ok(())
}
