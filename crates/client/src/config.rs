use std::{net::SocketAddr, path::PathBuf};

use quinn::rustls::{self, pki_types::CertificateDer};
#[derive(Debug, Clone)]
pub struct Config {
    pub server: String,
    pub socket_addr: SocketAddr,
    pub certs: Vec<CertificateDer<'static>>,
}