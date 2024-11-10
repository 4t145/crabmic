use std::fs;

use crabmic_server::AudioChannelService;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::Layer;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("crabmic_server=debug".parse()?),
        )
        .init();
    let ct = CancellationToken::new();
    let cwd = std::env::current_dir()?;
    println!("Current working directory: {:?}", cwd);
    let cert_chain = fs::read("./resource/cert.pem")?;
    let key = fs::read("./resource/key.pem")?;
    let certs = rustls_pemfile::certs(&mut cert_chain.as_slice()).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut key.as_slice())?.expect("no prk");
    let socket_addr = "[::]:8080".parse()?;
    let service = AudioChannelService::new(certs, key, socket_addr, ct.child_token());
    service.spawn();
    tokio::signal::ctrl_c().await.unwrap();
    ct.cancel();
    Ok(())
}
