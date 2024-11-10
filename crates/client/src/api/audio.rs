use bytes::Bytes;
use crabmic_model::{
    CRABMIC_ALPN, HELLO_SIGNAL, HELLO_SIGNAL_SIZE, ProtocolVersion, RoomAudioPacket, RoomUserId,
    cancellable,
};
use opus::Encoder;
use quinn::{
    ClientConfig, Endpoint, Incoming, RecvStream, SendStream, ServerConfig,
    crypto::rustls::QuicClientConfig,
    rustls::{self, pki_types::CertificateDer},
};
use std::{net::SocketAddr, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use tracing::instrument;

use crate::{ClientError, audio_io::AudioIoChannelNetworkSide};
pub struct AudioNetworkService {
    remote_addr: SocketAddr,
    server_name: String,
    channel: AudioIoChannelNetworkSide,
    certs: Vec<CertificateDer<'static>>,
    ct: CancellationToken,
}

impl AudioNetworkService {
    pub fn new(
        remote_addr: SocketAddr,
        server_name: String,
        channel: AudioIoChannelNetworkSide,
        certs: Vec<CertificateDer<'static>>,
        ct: CancellationToken,
    ) -> Self {
        Self {
            remote_addr,
            server_name,
            channel,
            certs,
            ct,
        }
    }
    pub fn spawn(self) {
        tokio::spawn(self.serve());
    }
    #[instrument(name = "audio_network_service", skip(self))]
    pub async fn serve(self) -> crate::Result<()> {
        let AudioNetworkService {
            remote_addr,
            server_name,
            channel:
                AudioIoChannelNetworkSide {
                    audio_input,
                    audio_output,
                },
            certs,
            ct,
        } = self;
        let mut endpoint = Endpoint::client((std::net::Ipv6Addr::UNSPECIFIED, 0).into())
            .map_err(crate::ClientError::contextual("failed to create endpoint"))?;
        let mut roots = rustls::RootCertStore::empty();
        for cert in certs {
            roots
                .add(cert)
                .map_err(crate::ClientError::contextual("failed to add cert"))?;
        }
        let mut client_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        client_config.alpn_protocols = vec![CRABMIC_ALPN.to_vec()];

        let client_config = quinn::ClientConfig::new(Arc::new(
            QuicClientConfig::try_from(client_config).map_err(crate::ClientError::contextual(
                "failed to create quic client config",
            ))?,
        ));
        endpoint.set_default_client_config(client_config);
        tracing::debug!("endpoint created");
        let connection = endpoint
            .connect(remote_addr, &server_name)
            .map_err(crate::ClientError::contextual("fail to create connection"))?
            .await
            .map_err(crate::ClientError::contextual("fail to connect"))?;
        tracing::debug!("connection created");
        let mut hello_buf = [0; HELLO_SIGNAL_SIZE];
        let mut version_buf = [0; size_of::<ProtocolVersion>()];
        let (mut tx, mut rx) = connection
            .open_bi()
            .await
            .map_err(crate::ClientError::contextual("fail to open bi"))?;
        tx.write_all(crabmic_model::HELLO_SIGNAL)
            .await
            .map_err(ClientError::contextual("write hello"))?;
        tracing::debug!("write hello");
        tx.write_all(&crabmic_model::PROTOCOL_VERSION.to_bytes())
            .await
            .map_err(ClientError::contextual("write protocol version"))?;
        tracing::debug!(version=%crabmic_model::PROTOCOL_VERSION, "write protocol");
        rx.read_exact(&mut hello_buf)
            .await
            .map_err(crate::ClientError::contextual("fail to hello"))?;
        tracing::debug!("read hello");
        rx.read_exact(&mut version_buf)
            .await
            .map_err(crate::ClientError::contextual("fail to read version"))?;
        let version = ProtocolVersion::from_bytes(version_buf);
        tracing::debug!(version=%version, "read protocol version");
        let uid = rx
            .read_u64()
            .await
            .map_err(crate::ClientError::contextual("fail to read user id"))?;
        tracing::info!("Connected to server with user id: {}", uid);
        let input_task = InputTask {
            ct: ct.child_token(),
            audio_input,
            network_tx: tx,
        };
        let output_task = OutputTask {
            ct: ct.child_token(),
            audio_output,
            network_rx: rx,
        };
        input_task.spawn();
        output_task.spawn();
        Ok(())
    }
}

pub struct InputTask {
    ct: CancellationToken,
    audio_input: tokio::sync::mpsc::Receiver<Bytes>,
    network_tx: SendStream,
}

impl InputTask {
    fn spawn(self) {
        tokio::spawn(self.serve());
    }
    async fn serve(self) -> crate::Result<()> {
        let InputTask {
            ct,
            mut audio_input,
            mut network_tx,
        } = self;
        loop {
            let Some(audio) = cancellable!(ct, audio_input.recv()) else {
                return Ok(());
            };
            network_tx
                .write_u32(audio.len() as u32)
                .await
                .map_err(ClientError::contextual("write audio length"))?;
            network_tx
                .write_all(&audio)
                .await
                .map_err(ClientError::contextual("write audio"))?;
        }
        Ok(())
    }
}

pub struct OutputTask {
    ct: CancellationToken,
    audio_output: tokio::sync::mpsc::Sender<RoomAudioPacket>,
    network_rx: RecvStream,
}

impl OutputTask {
    pub async fn serve(self) -> crate::Result<()> {
        let OutputTask {
            ct,
            audio_output,
            mut network_rx,
        } = self;
        loop {
            let user_id = cancellable!(ct, network_rx.read_u64())
                .map_err(ClientError::contextual("read user id"))?;
            let audio_size = network_rx
                .read_u32()
                .await
                .map_err(ClientError::contextual("read audio.len"))?;
            let mut audio = vec![0; audio_size as usize];
            network_rx
                .read_exact(&mut audio)
                .await
                .map_err(ClientError::contextual("read audio"))?;
            audio_output
                .send(RoomAudioPacket::new(user_id, audio))
                .await
                .map_err(ClientError::contextual("send audio"))?;
        }
        Ok(())
    }
    fn spawn(self) {
        tokio::spawn(self.serve());
    }
}
