use crabmic_client::{audio_io::VolumeOption, config::Config};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("crabmic_client=debug".parse().expect("parse directive")),
        )
        .init();
    let cert = std::fs::read("./resource/cert.pem")?;
    let certs = rustls_pemfile::certs(&mut cert.as_slice()).collect::<Result<Vec<_>, _>>()?;
    let config = Config {
        server: "localhost".to_string(),
        socket_addr: "[::1]:8080".parse().unwrap(),
        certs,
    };
    // client_crypto.alpn_protocols = HELLO_SIGNAL;
    let ct = tokio_util::sync::CancellationToken::new();
    let client = crabmic_client::run(config, ct).await?;
    client.io_client().set_output_device_as_default().await?;
    client.io_client().set_input_device_as_default().await?;
    client.io_client().set_input_volume(VolumeOption::default()).await?;
    tokio::signal::ctrl_c().await.unwrap();
    client.cancel();
    Ok(())
}

