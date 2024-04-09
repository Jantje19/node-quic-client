use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};

use once_cell::sync::OnceCell;

fn get_certs() -> Result<rustls::RootCertStore, std::io::Error> {
    static CERTS: OnceCell<rustls::RootCertStore> = OnceCell::new();

    CERTS
        .get_or_try_init(|| {
            let mut roots = rustls::RootCertStore::empty();

            for cert in rustls_native_certs::load_native_certs()? {
                roots.add(&rustls::Certificate(cert.0)).unwrap();
            }

            // TODO: Implement
            // for cert in rustls_pemfile::certs(&mut std::io::BufReader::new(std::fs::File::open(path)?))? {
            //     roots.add(&rustls::Certificate(cert)).unwrap();
            // }

            Ok(roots)
        })
        .cloned()
}

#[derive(Debug)]
pub enum ClientError {
    CertRootStore(std::io::Error),
    QuinnConnect(quinn::ConnectError),
    QuinnConnection(quinn::ConnectionError),
}

impl ClientError {
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        match self {
            ClientError::CertRootStore(v) => v.to_string(),
            ClientError::QuinnConnect(v) => v.to_string(),
            ClientError::QuinnConnection(v) => v.to_string(),
        }
    }
}

impl From<std::io::Error> for ClientError {
    fn from(value: std::io::Error) -> Self {
        Self::CertRootStore(value)
    }
}

impl From<quinn::ConnectError> for ClientError {
    fn from(value: quinn::ConnectError) -> Self {
        Self::QuinnConnect(value)
    }
}

impl From<quinn::ConnectionError> for ClientError {
    fn from(value: quinn::ConnectionError) -> Self {
        Self::QuinnConnection(value)
    }
}

pub async fn get_client(
    addr: SocketAddr,
    hostname: &str,
) -> Result<(quinn::Connection, quinn::Endpoint), ClientError> {
    let roots = get_certs()?;

    let client_crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_no_client_auth();

    // TODO: Allow setting these
    // pub const ALPN_QUIC_HTTP: &[&[u8]] = &[b"h3"];
    // client_crypto.alpn_protocols = ALPN_QUIC_HTTP.iter().map(|&x| x.into()).collect();

    let client_config = quinn::ClientConfig::new(Arc::new(client_crypto));
    let mut endpoint = quinn::Endpoint::client(SocketAddr::new(
        if addr.is_ipv6() {
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        },
        0,
    ))?;
    endpoint.set_default_client_config(client_config);

    let connection = endpoint.connect(addr, hostname)?.await?;

    Ok((connection, endpoint))
}
