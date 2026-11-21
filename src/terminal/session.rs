//! Terminal construction and background I/O. Each constructor captures one
//! transport/protocol; rendering and native input handling live in view.rs.
use super::*;

impl Terminal {
    /// A project-host debug PTY, carried over the bounded debugger transport.
    pub fn stream(
        ctx: &egui::Context,
        title: String,
        control: impl Fn(StreamControl) + Send + Sync + 'static,
    ) -> (Self, Stream) {
        let parser = Arc::new(Mutex::new(vt100::Parser::new(24, 80, 4000)));
        let alive = Arc::new(AtomicBool::new(true));
        let visible = Arc::new(AtomicBool::new(true));
        let (output, rx) = mpsc::sync_channel::<Vec<u8>>(64);
        let read_parser = parser.clone();
        let read_ctx = ctx.clone();
        let read_visible = visible.clone();
        std::thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if let Ok(mut parser) = read_parser.lock() {
                    parser.process(&bytes);
                }
                if read_visible.load(Ordering::Acquire) {
                    read_ctx.request_repaint_after(Duration::from_millis(16));
                }
            }
        });
        let control = Arc::new(control);
        let input_control = control.clone();
        let (input, rx) = mpsc::sync_channel::<Vec<u8>>(16);
        std::thread::spawn(move || {
            while let Ok(bytes) = rx.recv() {
                for chunk in bytes.chunks(16384) {
                    input_control(StreamControl::Input(chunk.to_vec()));
                }
            }
        });
        let end_control = control.clone();
        (
            Self {
                parser,
                backend: Backend::Debugger {
                    input,
                    control: Box::new(move |event| end_control(event)),
                },
                alive: alive.clone(),
                size: (24, 80),
                title,
                python: None,
                visible,
                contrast: Default::default(),
                selection: None,
            },
            Stream { output, alive },
        )
    }
    /// Start a directly owned local login shell rooted at the supplied directory.
    pub fn local(ctx: &egui::Context, root: &str) -> Result<Self> {
        Self::local_project(
            ctx,
            &crate::sandbox::Plan {
                root: root.into(),
                ..Default::default()
            },
        )
    }

    /// Start a directly owned local project shell using the prepared sandbox plan.
    pub fn local_project(ctx: &egui::Context, plan: &crate::sandbox::Plan) -> Result<Self> {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut command = if plan.active {
            let mut command = CommandBuilder::new(&plan.prefix[0]);
            command.args(&plan.prefix[1..]);
            command.arg(&shell);
            command
        } else {
            CommandBuilder::new(&shell)
        };
        command.cwd(&plan.root);
        Self::spawn(ctx, command, format!("LOCAL / {}", plan.label()))
    }

    /// Start the selected SSH client authentication master in a real local PTY.
    pub fn remote(ctx: &egui::Context, remote: &Remote) -> Result<Self> {
        let mut command = CommandBuilder::new(remote.client_program());
        command.args(remote.master_args()?);
        Self::spawn(
            ctx,
            command,
            format!("SSH LOGIN / {}", remote.profile.endpoint()),
        )
    }

    /// Start a project shell through the existing authenticated SSH master and sandbox plan.
    pub fn remote_project(
        ctx: &egui::Context,
        remote: &Remote,
        plan: &crate::sandbox::Plan,
    ) -> Result<Self> {
        anyhow::ensure!(
            remote.socket.exists(),
            "SSH connection is offline. Reconnect before starting a terminal."
        );
        let mut command = CommandBuilder::new(remote.client_program());
        command.args(remote.display_channel_args(true));
        command.arg(remote.shell_command(plan));
        Self::spawn(
            ctx,
            command,
            format!("SSH {} / {}", remote.profile.endpoint(), plan.label()),
        )
    }

    /// Spawn a directly owned PTY command with background I/O and scoped cleanup.
    pub fn spawn(ctx: &egui::Context, command: CommandBuilder, title: String) -> Result<Self> {
        Self::spawn_pty(ctx, command, title, Protocol::Raw)
    }
    /// Spawn a command using the native framed attachment mode rather than raw terminal output.
    pub fn spawn_native(
        ctx: &egui::Context,
        command: CommandBuilder,
        title: String,
    ) -> Result<Self> {
        Self::spawn_pty(ctx, command, title, Protocol::framed(true))
    }
    /// Attach a view to an existing native host session without restarting its shell or
    /// replaying startup input.
    pub fn native_session(
        ctx: &egui::Context,
        remote: Option<&Remote>,
        reply: &crate::sessions::Reply,
        info: &rode_session::Info,
    ) -> Result<Self> {
        let mut args = reply.prefix.clone();
        args.extend([
            "attach".into(),
            "--framed".into(),
            "--heartbeat".into(),
            info.id.clone(),
            info.policy.clone(),
        ]);
        let mut command = if let Some(remote) = remote {
            anyhow::ensure!(
                remote.socket.exists(),
                "Reconnect SSH before attaching a terminal"
            );
            let mut command = CommandBuilder::new(remote.client_program());
            command.args(remote.channel_args(false));
            let mut argv = vec![reply.executable.clone()];
            argv.extend(args);
            command.arg(
                argv.iter()
                    .map(|arg| crate::ssh::quote(arg))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            command
        } else {
            let mut command = CommandBuilder::new(&reply.executable);
            command.args(args);
            command
        };
        command.env_remove("TMUX");
        command.env_remove("TMUX_PANE");
        Self::spawn_pty(
            ctx,
            command,
            format!("NATIVE · {} / {}", info.name, info.mode),
            Protocol::framed(info.persistent),
        )
    }
    /// Set visibility so hidden output does not request unnecessary GUI repaints.
    pub fn set_visible(&self, visible: bool) {
        self.visible.store(visible, Ordering::Release);
    }
    /// Return the persistent-service lifetime state observed by this native attachment.
    pub fn persistent(&self) -> bool {
        self.backend.persistent()
    }
    /// Whether this view uses the framed native session protocol.
    pub fn is_native(&self) -> bool {
        self.backend.is_native()
    }
    fn spawn_pty(
        ctx: &egui::Context,
        mut command: CommandBuilder,
        title: String,
        protocol: Protocol,
    ) -> Result<Self> {
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let pair = portable_pty::native_pty_system().openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let native = protocol.is_native();
        if native && let Some(fd) = pair.master.as_raw_fd() {
            let mut attributes: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(fd, &mut attributes) } == 0 {
                unsafe {
                    libc::cfmakeraw(&mut attributes);
                    // portable-pty's master writer sends newline + VEOF on
                    // drop. This PTY carries a native attach protocol: EOF
                    // bytes would be forwarded as shell input, potentially
                    // exiting a persistent shell when its view disconnects.
                    attributes.c_cc[libc::VEOF] = 0;
                    libc::tcsetattr(fd, libc::TCSANOW, &attributes);
                }
            }
        }
        let arguments = rode_host_core::command::compact_pty(&mut command)?;
        let mut child = pair
            .slave
            .spawn_command(command)
            .context("Could not start terminal")?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = BufReader::new(pair.master.try_clone_reader()?);
        let mut writer = pair.master.take_writer()?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(24, 100, 4000)));
        let alive = Arc::new(AtomicBool::new(true));
        let read_protocol = protocol.clone();
        let visible = Arc::new(AtomicBool::new(true));
        let read_visible = visible.clone();
        let read_parser = parser.clone();
        let read_ctx = ctx.clone();
        let (drained_tx, drained_rx) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            let mut native_started = false;
            loop {
                if let Protocol::Framed(read_persistent) = &read_protocol {
                    let result = if native_started {
                        rode_session::read_stream(&mut reader)
                    } else {
                        framing::begin(&mut reader, |bytes| {
                            if !bytes.is_empty() {
                                if let Ok(mut parser) = read_parser.lock() {
                                    parser.process(bytes);
                                }
                                // Release the parser before taking egui's context lock.
                                if read_visible.load(Ordering::Acquire) {
                                    read_ctx.request_repaint_after(Duration::from_millis(16));
                                }
                            }
                        })
                    };
                    let (kind, bytes) = match result {
                        Ok(frame) => frame,
                        Err(error) => {
                            if !native_started && let Ok(mut parser) = read_parser.lock() {
                                parser.process(
                                    format!("\r\n[terminal attachment failed: {error:#}]\r\n")
                                        .as_bytes(),
                                );
                            }
                            break;
                        }
                    };
                    native_started = true;
                    if let Ok(mut parser) = read_parser.lock() {
                        match kind {
                            b'S' if bytes.len() == 4 => {
                                parser.screen_mut().set_size(
                                    u16::from_be_bytes([bytes[0], bytes[1]]).clamp(2, 200),
                                    u16::from_be_bytes([bytes[2], bytes[3]]).clamp(10, 400),
                                );
                            }
                            b'O' | b'X' => parser.process(&bytes),
                            b'P' if bytes.len() == 1 => {
                                read_persistent.store(bytes[0] != 0, Ordering::Release);
                            }
                            _ => {}
                        }
                    }
                } else {
                    let count = match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if let Ok(mut parser) = read_parser.lock() {
                        parser.process(&buffer[..count]);
                    }
                }
                if read_visible.load(Ordering::Acquire) {
                    read_ctx.request_repaint_after(Duration::from_millis(16));
                }
            }
            let _ = drained_tx.send(());
            if read_visible.load(Ordering::Acquire) {
                read_ctx.request_repaint();
            }
        });
        let wait_alive = alive.clone();
        let wait_ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = child.wait();
            drop(arguments);
            // The attachment process may exit while its final PTY frames are
            // still buffered. Publish completion only after the parser drains
            // them; bound the wait for descendants retaining a slave handle.
            let _ = drained_rx.recv_timeout(Duration::from_secs(1));
            wait_alive.store(false, Ordering::Release);
            wait_ctx.request_repaint();
        });
        let (input, rx) = mpsc::sync_channel::<Vec<u8>>(256);
        std::thread::spawn(move || {
            loop {
                let bytes = if native {
                    match rx.recv_timeout(rode_host_protocol::framing::HEARTBEAT_INTERVAL) {
                        Ok(bytes) => bytes,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            let mut frame = vec![];
                            if rode_session::write_stream(&mut frame, b'H', &[]).is_err() {
                                break;
                            }
                            frame
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                } else {
                    match rx.recv() {
                        Ok(bytes) => bytes,
                        Err(_) => break,
                    }
                };
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        Ok(Self {
            parser,
            backend: Backend::process(Process::new(pair.master, killer), input, protocol),
            alive,
            // A native host may have a different size before its first view.
            // Always publish the first layout, even if it matches this PTY.
            size: if native { (0, 0) } else { (24, 100) },
            title,
            python: None,
            visible,
            contrast: Default::default(),
            selection: None,
        })
    }
}
