//! Embedded SSH client process and authenticated, bounded local channel transport.
//!
//! One login PTY owns authentication. Channel subprocesses only attach to that
//! connection; they never resolve configuration or retry a login. The existing
//! host helper remains responsible for project execution and sandbox policy.
use anyhow::{Context, Result, bail, ensure};
use russh::{ChannelMsg, client};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixStream,
};
mod auth;
mod config;
mod forwarding;
mod gssapi;

const MAX_PACKET: usize = 128 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(15);
type Session = client::Handle<auth::Handler>;

pub(super) fn host_token(profile: &super::Profile) -> Result<String> {
    Ok(config::Config::resolve(profile, &profile.host, true)?.host_token())
}

#[derive(Serialize, Deserialize)]
enum Request {
    Check,
    Stop,
    Exec {
        command: String,
        pty: bool,
        size: (u16, u16),
        x11: bool,
    },
}

pub(super) fn cli(args: &[String]) -> Result<i32> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = runtime.block_on(async {
        match args {
            [mode, socket, digest] if mode == "--ssh-client-master" => {
                let profile = read_profile(Path::new(socket), digest)?;
                profile.validate()?;
                master(Path::new(socket), profile).await?;
                Ok(0)
            }
            [mode, socket, pty, x11, command] if mode == "--ssh-client-channel" => {
                ensure!(
                    command.len() < MAX_PACKET - 4096,
                    "SSH command exceeds limit"
                );
                channel_client(Path::new(socket), command, pty == "pty", x11 == "x11").await
            }
            _ => bail!("Invalid internal SSH client invocation"),
        }
    });
    // Tokio's stdin reader can be blocked on a terminal. It must not keep a
    // completed channel process alive after remote EOF or disconnect.
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}

fn read_profile(socket: &Path, digest: &str) -> Result<super::Profile> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let directory = socket.parent().context("Missing SSH runtime directory")?;
    let metadata = std::fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0,
        "SSH runtime directory must be private"
    );
    let path = directory.join("profile.json");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0
            && metadata.len() <= MAX_PACKET as u64,
        "Invalid SSH profile file"
    );
    std::fs::remove_file(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_PACKET as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_PACKET, "SSH profile exceeds limit");
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == digest,
        "SSH launch profile changed before authentication"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

fn local_connect(socket: &Path, timeout: Duration) -> Result<std::os::unix::net::UnixStream> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    #[cfg(target_os = "linux")]
    rode_session::control_auth::authenticate(
        &mut stream,
        socket.parent().context("Missing SSH runtime directory")?,
    )?;
    Ok(stream)
}

pub(super) fn connected(socket: &Path) -> bool {
    (|| -> Result<()> {
        let mut stream = local_connect(socket, Duration::from_millis(500))?;
        let body = serde_json::to_vec(&Request::Check)?;
        stream.write_all(&((body.len() + 1) as u32).to_be_bytes())?;
        stream.write_all(b"J")?;
        stream.write_all(&body)?;
        let mut reply = [0; 5];
        stream.read_exact(&mut reply)?;
        ensure!(reply == [0, 0, 0, 1, b'S'], "SSH connection is offline");
        Ok(())
    })()
    .is_ok()
}

pub(super) fn stop(socket: &Path) {
    // Capture the endpoint before Remote removes its name. Cleanup and token
    // authentication run off the GUI thread, including after a killed master.
    let stream = std::os::unix::net::UnixStream::connect(socket);
    let directory = socket.parent().map(Path::to_owned);
    std::thread::spawn(move || {
        let Some(directory) = directory else {
            return;
        };
        let _cleanup = RuntimeDirectory(directory.clone());
        if let Ok(mut stream) = stream {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            #[cfg(target_os = "linux")]
            if rode_session::control_auth::authenticate(&mut stream, &directory).is_err() {
                return;
            }
            let body = serde_json::to_vec(&Request::Stop).unwrap_or_default();
            let _ = stream.write_all(&((body.len() + 1) as u32).to_be_bytes());
            let _ = stream.write_all(b"J");
            let _ = stream.write_all(&body);
        }
    });
}

async fn packet<W: AsyncWrite + Unpin>(writer: &mut W, kind: u8, bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() < MAX_PACKET, "SSH local packet exceeds limit");
    writer.write_u32((bytes.len() + 1) as u32).await?;
    writer.write_u8(kind).await?;
    writer.write_all(bytes).await?;
    Ok(())
}
async fn read_packet<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(u8, Vec<u8>)> {
    let length = reader.read_u32().await? as usize;
    ensure!(
        length > 0 && length <= MAX_PACKET,
        "Invalid SSH local packet size"
    );
    let kind = reader.read_u8().await?;
    let mut bytes = vec![0; length - 1];
    reader.read_exact(&mut bytes).await?;
    Ok((kind, bytes))
}

struct RuntimeDirectory(PathBuf);
impl Drop for RuntimeDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.0.join("ssh"));
        let _ = std::fs::remove_file(self.0.join("control.capability"));
        let _ = std::fs::remove_file(self.0.join("profile.json"));
        let _ = std::fs::remove_dir(&self.0);
    }
}

async fn master(socket: &Path, profile: super::Profile) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let directory = socket.parent().context("Missing SSH runtime directory")?;
    let metadata = std::fs::symlink_metadata(directory)?;
    ensure!(
        metadata.is_dir()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o077 == 0,
        "SSH runtime directory must be private"
    );
    let _directory = RuntimeDirectory(directory.into());
    let config = config::Config::resolve(&profile, &profile.host, true)?;
    let x11 = if profile.x11_forwarding {
        forwarding::X11::prepare(&config)?
    } else {
        None
    };
    let (session, jumps) = auth::connect(&profile, config.clone(), x11.clone()).await?;
    let session = Arc::new(session);
    #[cfg(target_os = "linux")]
    let capability = Arc::new(rode_session::control_auth::Token::publish(directory)?);
    let listener = tokio::net::UnixListener::bind(socket)?;
    eprintln!(
        "Connected to {}:{} as {} using Rust SSH.",
        config.host, config.port, config.user
    );
    let (stop, mut stopping) = tokio::sync::mpsc::channel::<()>(1);
    let slots = Arc::new(tokio::sync::Semaphore::new(64));
    let mut tasks = tokio::task::JoinSet::new();
    let mut health = tokio::time::interval(Duration::from_secs(1));
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    loop {
        tokio::select! {
            _ = stopping.recv() => break,
            _ = terminate.recv() => break,
            _ = hangup.recv() => break,
            _ = health.tick() => {
                if session.is_closed() || jumps.iter().any(Session::is_closed) { bail!("SSH connection lost; reconnect to continue"); }
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = listener.accept() => {
                let (stream,_) = accepted?;
                let peer = stream.peer_cred()?;
                if peer.uid() != unsafe { libc::geteuid() } { continue; }
                #[cfg(target_os = "linux")]
                if peer.pid().is_none_or(|pid| {
                    let peer_namespace = std::fs::read_link(format!("/proc/{pid}/ns/mnt"));
                    let own_namespace = std::fs::read_link("/proc/self/ns/mnt");
                    !matches!((peer_namespace, own_namespace), (Ok(peer), Ok(own)) if peer == own)
                }) { continue; }
                let Ok(slot) = slots.clone().try_acquire_owned() else { continue; };
                let session = session.clone(); let config = config.clone(); let x11 = x11.clone(); let stop = stop.clone();
                #[cfg(target_os = "linux")]
                let capability = capability.clone();
                tasks.spawn(async move {
                    let _slot = slot;
                    let result = async {
                        #[cfg(target_os = "linux")]
                        let stream = {
                            let mut stream = stream.into_std()?;
                            tokio::task::spawn_blocking(move || -> Result<std::os::unix::net::UnixStream> {
                                stream.set_nonblocking(false)?;
                                stream.set_read_timeout(Some(IO_TIMEOUT))?;
                                stream.set_write_timeout(Some(IO_TIMEOUT))?;
                                capability.verify(&mut stream)?;
                                let reply = serde_json::to_vec(&rode_session::Response::Ok)?;
                                stream.write_all(&((reply.len()+1) as u32).to_be_bytes())?;
                                stream.write_all(b"J")?; stream.write_all(&reply)?;
                                stream.set_nonblocking(true)?;
                                Ok(stream)
                            }).await??
                        };
                        #[cfg(target_os = "linux")]
                        let mut stream = UnixStream::from_std(stream)?;
                        #[cfg(not(target_os = "linux"))]
                        let mut stream = stream;
                        let (kind,bytes) = tokio::time::timeout(IO_TIMEOUT,read_packet(&mut stream)).await??;
                        ensure!(kind == b'J', "Expected SSH channel request");
                        match serde_json::from_slice::<Request>(&bytes)? {
                            Request::Check => { ensure!(!session.is_closed(), "SSH connection closed"); packet(&mut stream,b'S',&[]).await?; }
                            Request::Stop => { let _ = stop.try_send(()); }
                            Request::Exec {command,pty,size,x11: display} => {
                                let result = relay(&mut stream,&session,&config,if display { x11.as_ref() } else {None},&command,pty,size).await;
                                if let Err(error) = result {
                                    let message = format!("SSH channel: {error:#}\n");
                                    let _ = tokio::time::timeout(Duration::from_secs(1), async {
                                        packet(&mut stream,b'E',message.as_bytes()).await?;
                                        packet(&mut stream,b'X',&255u32.to_be_bytes()).await
                                    }).await;
                                }
                            }
                        }
                        Ok::<_,anyhow::Error>(())
                    }.await;
                    // A failed local attachment has no authority to tear down other channels.
                    let _ = result;
                });
            }
        }
    }
    tasks.abort_all();
    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "RODE disconnected", "en")
        .await;
    for jump in jumps.iter().rev() {
        let _ = jump
            .disconnect(russh::Disconnect::ByApplication, "RODE disconnected", "en")
            .await;
    }
    Ok(())
}

async fn relay(
    stream: &mut UnixStream,
    session: &Session,
    config: &config::Config,
    x11: Option<&forwarding::X11>,
    command: &str,
    pty: bool,
    size: (u16, u16),
) -> Result<()> {
    let mut channel = tokio::time::timeout(IO_TIMEOUT, session.channel_open_session()).await??;
    let setup = async {
        if pty {
            channel
                .request_pty(
                    true,
                    "xterm-256color",
                    size.1.into(),
                    size.0.into(),
                    0,
                    0,
                    &[],
                )
                .await?;
            expect_success(&mut channel).await?;
        }
        if config.forward_agent {
            channel.agent_forward(false).await?;
        }
        if let Some(x11) = x11 {
            channel
                .request_x11(
                    true,
                    false,
                    "MIT-MAGIC-COOKIE-1",
                    x11.fake_hex(),
                    x11.screen,
                )
                .await?;
            if expect_success(&mut channel).await.is_err() {
                eprintln!("SSH server declined X11 forwarding.");
            }
        }
        for (name, value) in &config.environment {
            channel.set_env(false, name, value).await?;
        }
        channel.exec(true, command).await?;
        expect_success(&mut channel).await
    };
    let result = tokio::time::timeout(IO_TIMEOUT, setup).await;
    if !matches!(&result, Ok(Ok(()))) {
        let _ = channel.close().await;
        result??;
    }
    let (mut output, input) = channel.split();
    let (mut local_input, mut local_output) = stream.split();
    let send = async {
        loop {
            let (kind, bytes) = read_packet(&mut local_input).await?;
            match kind {
                // Preserve bounded owned frames through russh's credit-aware
                // send path instead of creating an AsyncWrite for every frame.
                b'D' => input.data_bytes(bytes).await?,
                b'F' if bytes.is_empty() => {
                    input.eof().await?;
                }
                b'W' if bytes.len() == 4 => {
                    input
                        .window_change(
                            u16::from_be_bytes([bytes[2], bytes[3]]).into(),
                            u16::from_be_bytes([bytes[0], bytes[1]]).into(),
                            0,
                            0,
                        )
                        .await?
                }
                _ => bail!("Invalid SSH channel input"),
            }
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    };
    let receive = async {
        let mut status = 255;
        while let Some(message) = output.wait().await {
            match message {
                ChannelMsg::Data { data } => {
                    for bytes in data.chunks(32768) {
                        packet(&mut local_output, b'D', bytes).await?;
                    }
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    for bytes in data.chunks(32768) {
                        packet(&mut local_output, b'E', bytes).await?;
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => status = exit_status,
                ChannelMsg::ExitSignal { .. } => status = 255,
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        packet(&mut local_output, b'X', &status.to_be_bytes()).await?;
        Ok::<_, anyhow::Error>(())
    };
    let result = tokio::select! { r = send => r, r = receive => r };
    let _ = input.close().await;
    result
}
async fn expect_success(channel: &mut russh::Channel<client::Msg>) -> Result<()> {
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::WindowAdjusted { .. }) => {}
            other => bail!("SSH server rejected channel request ({other:?})"),
        }
    }
}

fn terminal_size() -> (u16, u16) {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(0, libc::TIOCGWINSZ, &mut size) } == 0
        && size.ws_row > 0
        && size.ws_col > 0
    {
        (size.ws_row, size.ws_col)
    } else {
        (24, 100)
    }
}
async fn channel_client(socket: &Path, command: &str, pty: bool, x11: bool) -> Result<i32> {
    let socket = socket.to_owned();
    let stream = tokio::task::spawn_blocking(move || local_connect(&socket, IO_TIMEOUT)).await??;
    stream.set_nonblocking(true)?;
    let mut stream = UnixStream::from_std(stream)?;
    let request = Request::Exec {
        command: command.into(),
        pty,
        size: terminal_size(),
        x11,
    };
    packet(&mut stream, b'J', &serde_json::to_vec(&request)?).await?;
    let _raw = if pty {
        Some(auth::TerminalMode::raw()?)
    } else {
        None
    };
    let (mut read, mut write) = stream.split();
    let send = async {
        let mut stdin = tokio::io::stdin();
        let mut bytes = [0; 32768];
        let mut resize =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;
        let mut eof = false;
        loop {
            tokio::select! {
                n = stdin.read(&mut bytes), if !eof => {
                    let n = n?;
                    if n == 0 { eof = true; packet(&mut write,b'F',&[]).await?; }
                    else { packet(&mut write,b'D',&bytes[..n]).await?; }
                }
                _ = resize.recv(), if pty => {
                    let (rows,cols) = terminal_size();
                    packet(&mut write,b'W',&[rows.to_be_bytes(),cols.to_be_bytes()].concat()).await?;
                }
                else => std::future::pending::<()>().await,
            }
        }
        #[allow(unreachable_code)]
        Ok::<i32, anyhow::Error>(255)
    };
    let receive = async {
        let mut stdout = tokio::io::stdout();
        let mut stderr = tokio::io::stderr();
        loop {
            let (kind, bytes) = read_packet(&mut read)
                .await
                .context("SSH channel disconnected")?;
            match kind {
                b'D' => {
                    stdout.write_all(&bytes).await?;
                    stdout.flush().await?;
                }
                b'E' => {
                    stderr.write_all(&bytes).await?;
                    stderr.flush().await?;
                }
                b'X' if bytes.len() == 4 => {
                    return Ok(u32::from_be_bytes(
                        bytes
                            .try_into()
                            .map_err(|_| anyhow::anyhow!("Invalid SSH exit status"))?,
                    )
                    .min(255) as i32);
                }
                _ => bail!("Invalid SSH channel output"),
            }
        }
    };
    tokio::pin!(send, receive);
    tokio::select! {
        result = &mut receive => result,
        result = &mut send => {
            // Fast commands may close stdin while stdout and exit status are
            // still buffered. An EPIPE on EOF must not discard that reply.
            match result {
                Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                    matches!(error.kind(), std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset)
                }) => tokio::time::timeout(IO_TIMEOUT, &mut receive).await.context("SSH output did not close after input shutdown")?,
                other => other,
            }
        }
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn login_profile_is_private_consumed_and_absent_from_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let remote = crate::ssh::Remote::new(crate::ssh::Profile {
            host: "private-login-host".into(),
            ..Default::default()
        })
        .unwrap();
        let args = remote.master_args().unwrap();
        assert_eq!(args.len(), 3);
        assert!(!args.iter().any(|arg| arg.contains("private-login-host")));
        let path = remote.socket.parent().unwrap().join("profile.json");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            read_profile(&remote.socket, &args[2]).unwrap().host,
            "private-login-host"
        );
        assert!(!path.exists());
        let args = remote.master_args().unwrap();
        std::fs::write(&path, br#"{"host":"substituted-host"}"#).unwrap();
        assert!(
            read_profile(&remote.socket, &args[2])
                .unwrap_err()
                .to_string()
                .contains("changed before authentication")
        );
        let other = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(other.path(), &path).unwrap();
        assert!(read_profile(&remote.socket, &args[2]).is_err());
        assert!(other.path().exists());
        remote.cleanup();
    }
}
