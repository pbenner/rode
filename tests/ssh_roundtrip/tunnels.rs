//! Real OpenSSH relay bindings, using only the isolated SSH fixture's credentials.
use rode_session::{
    Request, Response,
    tunnels::{ListenOn, Spec, Status},
};
use std::{
    fs,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, TcpListener, TcpStream, UdpSocket},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

struct Service {
    directory: tempfile::TempDir,
    child: Child,
}
impl Service {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let log = fs::File::create(directory.path().join("service.log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_rode-sessiond"))
            .arg("serve")
            .env("RODE_SESSION_DIR", directory.path().join("runtime"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let mut service = Self { directory, child };
        let until = Instant::now() + Duration::from_secs(10);
        while service.request(Request::Tunnels).is_err() {
            assert!(
                service.child.try_wait().unwrap().is_none() && Instant::now() < until,
                "{}",
                fs::read_to_string(service.directory.path().join("service.log")).unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        service
    }

    fn request(&self, request: Request) -> anyhow::Result<Response> {
        let mut stream = rode_session::connect_control(&self.directory.path().join("runtime"))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        rode_session::write_stream(&mut stream, b'J', &serde_json::to_vec(&request)?)?;
        let (kind, bytes) = rode_session::read_stream(&mut stream)?;
        anyhow::ensure!(kind == b'J', "Unexpected service frame");
        serde_json::from_slice::<Response>(&bytes)?.checked()
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Echo {
    port: u16,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Echo {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let worker = std::thread::spawn(move || {
            while !cancelled.load(Ordering::Acquire) {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut bytes = [0; 6];
                    if stream.read_exact(&mut bytes).is_ok() {
                        let _ = stream.write_all(&bytes);
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        });
        Self {
            port,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Echo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.worker.take().unwrap().join();
    }
}

fn roundtrip(address: IpAddr, port: u16) {
    let mut stream =
        TcpStream::connect_timeout(&(address, port).into(), Duration::from_secs(3)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.write_all(b"tunnel").unwrap();
    let mut bytes = [0; 6];
    stream.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"tunnel");
}

fn interface_address() -> Option<IpAddr> {
    // UDP connect selects a local route without sending a packet. Hosts with
    // only loopback still exercise OpenSSH's wildcard/IPv6 channel protocol.
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let address = socket.local_addr().ok()?.ip();
    (!address.is_loopback() && !address.is_unspecified()).then_some(address)
}

#[test]
#[ignore = "requires the isolated OpenSSH relay started by scripts/test-ssh.sh"]
fn relay_listener_scope_controls_localhost_and_network_access() {
    let config = std::env::var("RODE_TEST_SSH_CONFIG").expect("Use scripts/test-ssh.sh");
    let directory = Path::new(&config).parent().unwrap();
    let config = fs::read_to_string(&config).unwrap();
    let setting = |key: &str| {
        config
            .lines()
            .find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some(key)).then(|| words.next().unwrap().to_owned())
            })
            .unwrap()
    };
    let echo = Echo::new();
    let service = Service::new();
    let id = rode_session::new_id().unwrap();
    let interface = interface_address();
    for listen_on in [ListenOn::Loopback, ListenOn::AllInterfaces] {
        let reserved = TcpListener::bind("0.0.0.0:0").unwrap();
        let port = reserved.local_addr().unwrap().port();
        drop(reserved);
        let spec = Spec {
            relay_host: "127.0.0.1".into(),
            relay_port: setting("Port").parse().unwrap(),
            relay_user: setting("User"),
            identity: directory.join("client").to_string_lossy().into(),
            relay_key: fs::read_to_string(directory.join("host-known.pub")).unwrap(),
            listen_on,
            listen_port: port,
            ssh_port: echo.port,
            ..Default::default()
        };
        service
            .request(Request::StartTunnel {
                id: id.clone(),
                spec,
            })
            .unwrap();
        let until = Instant::now() + Duration::from_secs(12);
        loop {
            let Response::Tunnels(list) = service.request(Request::Tunnels).unwrap() else {
                panic!()
            };
            let info = &list[0];
            assert_eq!(info.spec.listen_on, listen_on);
            assert!(
                !matches!(info.status, Status::Attention | Status::HostKeyRequired),
                "{info:?}"
            );
            if info.status == Status::Connected {
                break;
            }
            assert!(Instant::now() < until, "{info:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
        roundtrip(Ipv4Addr::LOCALHOST.into(), port);
        if let Some(interface) = interface {
            if listen_on == ListenOn::AllInterfaces {
                roundtrip(interface, port);
                eprintln!("All-interface relay forwards through {interface}:{port}");
            } else {
                assert!(
                    TcpStream::connect_timeout(
                        &(interface, port).into(),
                        Duration::from_millis(500)
                    )
                    .is_err()
                );
            }
        }
        if listen_on == ListenOn::AllInterfaces && TcpListener::bind("[::1]:0").is_ok() {
            roundtrip("::1".parse().unwrap(), port);
        }
        service
            .request(Request::StopTunnel { id: id.clone() })
            .unwrap();
    }
}
