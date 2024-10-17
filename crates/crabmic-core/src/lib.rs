mod error;
mod utils;
use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    sync::{atomic::AtomicU64, Arc},
    vec,
};

use bytes::Bytes;
use cpal::{SampleFormat, ALL_HOSTS};
use error::InternalError;
use opus::{Decoder, Encoder};
use quinn::{crypto::rustls::QuicServerConfig, ClientConfig, Endpoint, Incoming, ServerConfig};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(u64);
impl UserId {
    pub fn next(&self) -> UserId {
        UserId(self.0 + 1)
    }
}

#[derive(Debug, Clone)]
pub struct AudioPacket {
    pub user_id: UserId,
    pub data: Bytes,
}
#[derive(Debug, Clone)]
pub struct UserAudioPacketBroadcast {
    txs: Arc<RwLock<HashMap<UserId, tokio::sync::mpsc::Sender<AudioPacket>>>>,
}

impl UserAudioPacketBroadcast {
    pub async fn send(&self, packet: AudioPacket) {
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
    pub async fn remove_channel(&self, user_id: UserId) {
        let mut txs = self.txs.write().await;
        txs.remove(&user_id);
    }
    const CHANNEL_SIZE: usize = 64;
    pub async fn create_channel(
        &self,
        user_id: UserId,
    ) -> tokio::sync::mpsc::Receiver<AudioPacket> {
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
    user_id: UserId,
    network_tx: quinn::SendStream,
    packet_inbound: tokio::sync::mpsc::Receiver<AudioPacket>,
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
                .write_u64(packet.user_id.0)
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
    user_id: UserId,
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
            let packet = AudioPacket {
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
    pub next_user_id: UserId,
    pub server_config: Arc<QuicServerConfig>,
    pub prk: Arc<ring::hkdf::Prk>,
    pub addr: SocketAddr,
    pub ct: CancellationToken,
    pub user_audio_broadcast: UserAudioPacketBroadcast,
}

pub struct UserIncomingConnection {
    pub user_id: UserId,
    pub incoming: Incoming,
    pub ct: CancellationToken,
    pub user_audio_broadcast: UserAudioPacketBroadcast,
}

impl AudioChannelService {
    pub fn next_user_id(&self) -> UserId {
        self.next_user_id.next()
    }
    pub fn new_incoming_connection(&self, incoming: Incoming) -> UserIncomingConnection {
        UserIncomingConnection {
            user_id: self.next_user_id(),
            incoming,
            ct: self.ct.child_token(),
            user_audio_broadcast: self.user_audio_broadcast.clone(),
        }
    }

    pub async fn serve(self) -> Result<(), InternalError> {
        let server_config = ServerConfig::new(self.server_config.clone(), self.prk.clone());

        let endpoint =
            Endpoint::server(server_config, self.addr).expect("failed to create endpoint");
        loop {
            let Some(incoming) = cancellable!(self.ct, endpoint.accept()) else {
                return Ok(());
            };
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
        let (network_tx, network_rx) = connection
            .open_bi()
            .await
            .map_err(InternalError::contextual("accept connection"))?;
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
