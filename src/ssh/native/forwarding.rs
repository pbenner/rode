//! X11 forwarding with a per-connection fake cookie and explicit trust/expiry.
use super::*;

#[derive(Clone)]
pub(super) struct X11 {
    display: String,
    cookie: [u8; 16],
    fake: [u8; 16],
    expires: Option<std::time::Instant>,
    pub screen: u32,
}
fn unhex(value: &str) -> Result<[u8; 16]> {
    ensure!(value.len() == 32, "Invalid X11 cookie length");
    let mut bytes = [0; 16];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[2 * i..2 * i + 2], 16)?;
    }
    Ok(bytes)
}
impl X11 {
    pub fn prepare(config: &config::Config) -> Result<Option<Self>> {
        let Ok(display) = std::env::var("DISPLAY") else {
            return Ok(None);
        };
        if display.is_empty() {
            return Ok(None);
        }
        let result = (|| -> Result<Self> {
            ensure!(
                display.len() <= 512 && !display.chars().any(char::is_control),
                "Invalid X11 display"
            );
            let temporary = tempfile::NamedTempFile::new()?;
            if !config.x11_trusted {
                let mut command = std::process::Command::new(&config.xauth);
                command.arg("-f").arg(temporary.path()).args([
                    "generate",
                    &display,
                    "MIT-MAGIC-COOKIE-1",
                    "untrusted",
                    "timeout",
                    &config.x11_timeout.to_string(),
                ]);
                rode_host_core::command::run_bounded_limited(
                    command,
                    vec![],
                    Duration::from_secs(5),
                    8192,
                )?;
            }
            let mut command = std::process::Command::new(&config.xauth);
            if !config.x11_trusted {
                command.arg("-f").arg(temporary.path());
            }
            command.args(["list", &display]);
            let output = rode_host_core::command::run_bounded_limited(
                command,
                vec![],
                Duration::from_secs(5),
                8192,
            )?;
            let text = std::str::from_utf8(&output)?;
            let values = text
                .lines()
                .find_map(|line| {
                    let words = line.split_whitespace().collect::<Vec<_>>();
                    (words.len() == 3 && words[1] == "MIT-MAGIC-COOKIE-1").then_some(words)
                })
                .context("No MIT-MAGIC-COOKIE-1 X11 cookie")?;
            let cookie = unhex(values[2])?;
            let mut fake = [0; 16];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut fake)?;
            let screen = display
                .rsplit(':')
                .next()
                .and_then(|s| s.split_once('.'))
                .map(|(_, s)| s.parse())
                .transpose()?
                .unwrap_or(0);
            Ok(Self {
                display,
                cookie,
                fake,
                screen,
                expires: (!config.x11_trusted)
                    .then(|| std::time::Instant::now() + Duration::from_secs(config.x11_timeout)),
            })
        })();
        match result {
            Ok(x11) => Ok(Some(x11)),
            Err(error) => {
                eprintln!("X11 forwarding unavailable: {error:#}. SSH remains usable.");
                Ok(None)
            }
        }
    }
    pub fn fake_hex(&self) -> String {
        self.fake.iter().map(|b| format!("{b:02x}")).collect()
    }
    pub async fn forward<S: AsyncRead + AsyncWrite + Unpin>(&self, mut remote: S) -> Result<()> {
        ensure!(
            self.expires
                .is_none_or(|end| std::time::Instant::now() < end),
            "X11 forwarding expired"
        );
        let handshake = tokio::time::timeout(IO_TIMEOUT, async {
            let mut header = [0; 12];
            remote.read_exact(&mut header).await?;
            let number = |a, b| match header[0] {
                b'l' => Ok(u16::from_le_bytes([a, b]) as usize),
                b'B' => Ok(u16::from_be_bytes([a, b]) as usize),
                _ => bail!("Invalid X11 byte order"),
            };
            let name_length = number(header[6], header[7])?;
            let data_length = number(header[8], header[9])?;
            ensure!(
                name_length == 18 && data_length == 16,
                "Invalid X11 authentication"
            );
            let padded_name = (name_length + 3) & !3;
            let mut auth = vec![0; padded_name + 16];
            remote.read_exact(&mut auth).await?;
            ensure!(
                &auth[..name_length] == b"MIT-MAGIC-COOKIE-1",
                "Unsupported X11 authentication"
            );
            let difference = auth[padded_name..]
                .iter()
                .zip(self.fake)
                .fold(0u8, |acc, (a, b)| acc | (a ^ b));
            ensure!(difference == 0, "Invalid X11 forwarding cookie");
            auth[padded_name..].copy_from_slice(&self.cookie);
            Ok::<_, anyhow::Error>([header.as_slice(), auth.as_slice()].concat())
        })
        .await??;
        let (host, number) = self
            .display
            .rsplit_once(':')
            .context("Invalid X11 display")?;
        let number = number
            .split('.')
            .next()
            .context("Invalid display number")?
            .parse::<u16>()?;
        if host.is_empty() || host == "unix" || host.ends_with("/unix") {
            let path = format!("/tmp/.X11-unix/X{number}");
            let mut local = match UnixStream::connect(&path).await {
                Ok(stream) => stream,
                #[cfg(target_os = "linux")]
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    // Xlib also supports Linux abstract display sockets. Some
                    // desktops expose that endpoint without a filesystem socket.
                    use std::os::linux::net::SocketAddrExt;
                    let address = std::os::unix::net::SocketAddr::from_abstract_name(path)?;
                    let stream = std::os::unix::net::UnixStream::connect_addr(&address)?;
                    stream.set_nonblocking(true)?;
                    UnixStream::from_std(stream)?
                }
                Err(error) => return Err(error.into()),
            };
            local.write_all(&handshake).await?;
            tokio::io::copy_bidirectional(&mut local, &mut remote).await?;
        } else {
            let port = 6000u16.checked_add(number).context("X11 port overflow")?;
            let mut local =
                tokio::time::timeout(IO_TIMEOUT, tokio::net::TcpStream::connect((host, port)))
                    .await??;
            local.write_all(&handshake).await?;
            tokio::io::copy_bidirectional(&mut local, &mut remote).await?;
        }
        Ok(())
    }
}
