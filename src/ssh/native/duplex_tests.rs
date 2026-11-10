//! A small transport buffer must not turn a full-duplex echo into a window deadlock.
use super::*;
use russh::server;

struct Echo;
impl server::Handler for Echo {
    type Error = anyhow::Error;

    async fn auth_none(&mut self, _: &str) -> Result<server::Auth> {
        Ok(server::Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<()> {
        reply.accept().await;
        tokio::spawn(async move {
            let (mut read, write) = channel.split();
            while let Some(message) = read.wait().await {
                match message {
                    ChannelMsg::Data { data } => write.data_bytes(data).await?,
                    ChannelMsg::Eof => {
                        write.exit_status(37).await?;
                        write.eof().await?;
                        write.close().await?;
                        break;
                    }
                    _ => {}
                }
            }
            Ok::<_, anyhow::Error>(())
        });
        Ok(())
    }
}

struct TrustFixture;
impl client::Handler for TrustFixture {
    type Error = anyhow::Error;
    async fn check_server_key(&mut self, _: &russh::keys::PublicKeyOrCertificate) -> Result<bool> {
        Ok(true)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn echo_larger_than_the_receive_window_finishes_on_a_bounded_transport() {
    bounded_echo(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rekey_preserves_partially_written_packets_on_a_bounded_transport() {
    bounded_echo(Some(russh::Limits::new(
        256 * 1024,
        256 * 1024,
        Duration::from_secs(3600),
    )))
    .await;
}

async fn bounded_echo(limits: Option<russh::Limits>) {
    let (server_stream, client_stream) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        let config = Arc::new(server::Config {
            channel_buffer_size: 1,
            keys: vec![
                russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                    .unwrap(),
            ],
            ..Default::default()
        });
        server::run_stream(config, server_stream, Echo)
            .await?
            .await?;
        Ok::<_, anyhow::Error>(())
    });
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        let config = Arc::new(client::Config {
            channel_buffer_size: 16,
            window_size: 1024 * 1024,
            maximum_packet_size: 32768,
            limits: limits.unwrap_or_default(),
            ..Default::default()
        });
        let mut session = client::connect_stream(config, client_stream, TrustFixture).await?;
        ensure!(session.authenticate_none("fixture").await?.success());
        let channel = session.channel_open_session().await?;
        let (mut read, write) = channel.split();
        let payload: Vec<u8> = (0..2 * 1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect();
        let send = async {
            write.data_bytes(payload.clone()).await?;
            write.eof().await?;
            Ok::<_, anyhow::Error>(())
        };
        let receive = async {
            let mut output = Vec::new();
            let mut status = None;
            while let Some(message) = read.wait().await {
                match message {
                    ChannelMsg::Data { data } => output.extend_from_slice(&data),
                    ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                    ChannelMsg::Close => break,
                    _ => {}
                }
            }
            ensure!(output == payload, "Echoed bytes changed");
            ensure!(status == Some(37), "Exit status was lost: {status:?}");
            Ok::<_, anyhow::Error>(())
        };
        tokio::try_join!(send, receive)?;
        session
            .disconnect(russh::Disconnect::ByApplication, "done", "en")
            .await?;
        Ok::<_, anyhow::Error>(())
    })
    .await;
    server.abort();
    result.expect("Full-duplex SSH echo stalled").unwrap();
}
