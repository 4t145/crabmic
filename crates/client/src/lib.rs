pub mod api;
mod error;
pub use error::{ClientError, Result};
use tokio_util::sync::CancellationToken;
pub mod audio_io;
pub mod config;

#[derive(Debug, Clone)]
pub struct Client {
    config: config::Config,
    io_client: audio_io::AudioIoClient,
    ct: CancellationToken,
}

impl Client {
    pub fn config(&self) -> &config::Config {
        &self.config
    }
    pub fn io_client(&self) -> &audio_io::AudioIoClient {
        &self.io_client
    }
    pub fn cancel(&self) {
        self.ct.cancel();
    }
}

pub async fn run(config: config::Config, ct: CancellationToken) -> Result<Client> {
    let (device, network) = audio_io::channel();
    let audio_network_service = api::audio::AudioNetworkService::new(
        config.socket_addr,
        config.server.clone(),
        network,
        config.certs.clone(),
        ct.child_token(),
    );
    let (audio_io_service, audio_io_client) =
        audio_io::AudioIoService::new(device, ct.child_token());
    audio_network_service.spawn();
    audio_io_service.spawn();
    let client = Client {
        config,
        io_client: audio_io_client,
        ct,
    };
    Ok(client)
}
