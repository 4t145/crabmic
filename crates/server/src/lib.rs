pub mod error;
mod utils;
use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    sync::{Arc, atomic::AtomicU64},
    vec,
};

use bytes::Bytes;
use crabmic_model::{HELLO_SIGNAL_SIZE, ProtocolVersion, RoomAudioPacket, RoomUserId, cancellable};
use error::InternalError;
use quinn::{
    ClientConfig, Endpoint, Incoming, ServerConfig,
    crypto::rustls::QuicServerConfig,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer},
        quic::Suite,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Default)]
pub struct UserAudioPacketBroadcast {
    txs: Arc<RwLock<HashMap<RoomUserId, tokio::sync::mpsc::Sender<RoomAudioPacket>>>>,
}

impl UserAudioPacketBroadcast {
    pub async fn send(&self, packet: RoomAudioPacket) {
        let mut need_clear = false;
        let txs = self.txs.read().await;
        for (user_id, tx) in txs.iter() {
            if *user_id != packet.user_id {
                let result = tx.send(packet.clone()).await;
                if result.is_err() {
                    need_clear = true;
                }
            }
        }
        if need_clear {
            tokio::spawn(self.clear_dropped_channels());
        }
    }
    pub async fn remove_channel(&self, user_id: RoomUserId) {
        let mut txs = self.txs.write().await;
        txs.remove(&user_id);
    }
    const CHANNEL_SIZE: usize = 64;
    pub async fn create_channel(
        &self,
        user_id: RoomUserId,
    ) -> tokio::sync::mpsc::Receiver<RoomAudioPacket> {
        let (tx, rx) = tokio::sync::mpsc::channel(Self::CHANNEL_SIZE);
        let mut txs = self.txs.write().await;
        txs.insert(user_id, tx.clone());
        rx
    }
    pub fn clear_dropped_channels(&self) -> impl Future<Output = ()> + Send + 'static {
        let txs = self.txs.clone();
        async move {
            let mut txs = txs.write().await;
            txs.retain(|_, tx| !tx.is_closed());
        }
    }
}
pub struct AudioMixedOutChannel {
    user_id: RoomUserId,
    network_tx: quinn::SendStream,
    packet_inbound: tokio::sync::mpsc::Receiver<RoomAudioPacket>,
    ct: CancellationToken,
}

impl AudioMixedOutChannel {
    pub async fn serve(mut self) -> Result<(), InternalError> {
        tracing::debug!(user_id = ?self.user_id, "audio mixed out channel started");
        loop {
            let Some(packet) = cancellable!(self.ct, self.packet_inbound.recv()) else {
                break;
            };
            self.network_tx
                .write_u64(packet.user_id.into_inner())
                .await
                .map_err(InternalError::contextual("write user id"))?;
            self.network_tx
                .write_u32(packet.data.len() as u32)
                .await
                .map_err(InternalError::contextual("write data size"))?;
            self.network_tx
                .write_all(&packet.data)
                .await
                .map_err(InternalError::contextual("write data packet"))?;
        }
        Ok(())
    }
    pub fn spawn(self) {
        tokio::spawn(self.serve());
    }
}

pub struct AudioMixedInChannel {
    user_id: RoomUserId,
    network_rx: quinn::RecvStream,
    broadcast: UserAudioPacketBroadcast,
    ct: CancellationToken,
}

impl AudioMixedInChannel {
    pub async fn serve(mut self) -> Result<(), InternalError> {
        tracing::info!(user_id = ?self.user_id, "audio mixed in channel started");
        loop {
            let data_size = cancellable!(self.ct, self.network_rx.read_u32())
                .map_err(InternalError::contextual("read data size"))?;
            let mut data = vec![0; data_size as usize];
            self.network_rx
                .read_buf(&mut data)
                .await
                .map_err(InternalError::contextual("read data packet"))?;
            let packet = RoomAudioPacket {
                user_id: self.user_id,
                data: Bytes::from(data),
            };
            self.broadcast.send(packet).await;
        }
        Ok(())
    }
    pub fn spawn(self) {
        tokio::spawn(self.serve());
    }
}

pub struct AudioChannelService {
    pub next_user_id: RoomUserId,
    pub cert_chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub addr: SocketAddr,
    pub ct: CancellationToken,
    pub user_audio_broadcast: UserAudioPacketBroadcast,
}

pub struct UserIncomingConnection {
    pub user_id: RoomUserId,
    pub incoming: Incoming,
    pub ct: CancellationToken,
    pub user_audio_broadcast: UserAudioPacketBroadcast,
}

impl AudioChannelService {
    pub fn new(
        cert_chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
        addr: SocketAddr,
        ct: CancellationToken,
    ) -> Self {
        Self {
            next_user_id: RoomUserId::default(),
            cert_chain,
            key,
            addr,
            ct,
            user_audio_broadcast: UserAudioPacketBroadcast::default(),
        }
    }
    pub fn next_user_id(&mut self) -> RoomUserId {
        self.next_user_id = self.next_user_id.next();
        self.next_user_id
    }
    pub fn new_incoming_connection(&mut self, incoming: Incoming) -> UserIncomingConnection {
        UserIncomingConnection {
            user_id: self.next_user_id(),
            incoming,
            ct: self.ct.child_token(),
            user_audio_broadcast: self.user_audio_broadcast.clone(),
        }
    }
    pub fn spawn(self) {
        tokio::spawn(self.serve());
    }
    pub async fn serve(mut self) -> Result<(), InternalError> {
        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(self.cert_chain.clone(), self.key.clone_key())
            .map_err(InternalError::contextual("create server config"))?;
        server_crypto.alpn_protocols = vec![crabmic_model::CRABMIC_ALPN.to_vec()];
        let server_config = QuicServerConfig::try_from(server_crypto)
            .map_err(InternalError::contextual("create quic server config"))?;
        let server_config = ServerConfig::with_crypto(Arc::new(server_config));
        let endpoint =
            Endpoint::server(server_config, self.addr).expect("failed to create endpoint");
        // endpoint
        let local_addr = endpoint
            .local_addr()
            .map_err(InternalError::contextual("fail to get local addr"))?;
        tracing::info!(%local_addr, "endpoint created");
        loop {
            let Some(incoming) = cancellable!(self.ct, endpoint.accept()) else {
                return Ok(());
            };
            tracing::info!(remote=%incoming.remote_address(), "incoming connection");

            self.new_incoming_connection(incoming).spawn();
        }
        Ok(())
    }
}

impl UserIncomingConnection {
    pub async fn serve(self) -> Result<(), InternalError> {
        let UserIncomingConnection {
            incoming,
            ct,
            user_audio_broadcast: broadcast,
            user_id,
        } = self;
        let connection = incoming
            .await
            .map_err(InternalError::contextual("accept connection"))?;
        let (mut network_tx, mut network_rx) = connection
            .accept_bi()
            .await
            .map_err(InternalError::contextual("accept connection"))?;
        let mut hello_buf = [0; HELLO_SIGNAL_SIZE];
        let mut version_buf = [0; size_of::<ProtocolVersion>()];
        network_rx
            .read_exact(&mut hello_buf)
            .await
            .map_err(InternalError::contextual("read hello"))?;

        network_rx
            .read_exact(&mut version_buf)
            .await
            .map_err(InternalError::contextual("read protocol version"))?;
        network_tx
            .write_all(crabmic_model::HELLO_SIGNAL)
            .await
            .map_err(InternalError::contextual("write hello"))?;
        tracing::debug!("hello signal sent");
        network_tx
            .write_all(&crabmic_model::PROTOCOL_VERSION.to_bytes())
            .await
            .map_err(InternalError::contextual("write protocol version"))?;
        tracing::debug!(version=%crabmic_model::PROTOCOL_VERSION, "protocol version sent");

        if hello_buf != crabmic_model::HELLO_SIGNAL {
            return Err(InternalError::protocol("expect hello signal"));
        }

        let version = ProtocolVersion::from_bytes(version_buf);
        if !version.compatible(&crabmic_model::PROTOCOL_VERSION) {
            return Err(InternalError::protocol("incompatible protocol version"));
        }
        tracing::debug!(version=%version, "read client protocol version");

        network_tx
            .write_u64(user_id.into_inner())
            .await
            .map_err(InternalError::contextual("write user id"))?;
        tracing::info!(user_id = ?user_id, "user id dispatched");
        let packet_inbound = broadcast.create_channel(self.user_id).await;
        let mixed_in_channel = AudioMixedInChannel {
            user_id,
            network_rx,
            broadcast: broadcast.clone(),
            ct: ct.clone(),
        };
        let mixed_out_channel = AudioMixedOutChannel {
            user_id,
            network_tx,
            packet_inbound,
            ct: ct.clone(),
        };
        mixed_in_channel.spawn();
        mixed_out_channel.spawn();
        ct.cancelled().await;
        broadcast.remove_channel(self.user_id).await;
        connection.close(0u8.into(), b"");
        Ok(())
    }
    pub fn spawn(self) {
        tokio::spawn(self.serve());
    }
}
