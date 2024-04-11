use std::{
    io::Cursor,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use once_cell::sync::OnceCell;

fn get_certs(
    certificate_authorities: Option<Vec<Vec<u8>>>,
) -> Result<rustls::RootCertStore, std::io::Error> {
    static CERTS: OnceCell<rustls::RootCertStore> = OnceCell::new();

    CERTS
        .get_or_try_init(|| {
            let mut roots = rustls::RootCertStore::empty();

            for cert in rustls_native_certs::load_native_certs()? {
                roots.add(&rustls::Certificate(cert.0)).unwrap();
            }

            if let Some(certificate_authorities) = certificate_authorities {
                for ca in certificate_authorities {
                    for cert in rustls_pemfile::certs(&mut Cursor::new(ca))? {
                        roots.add(&rustls::Certificate(cert)).unwrap();
                    }
                }
            }

            Ok(roots)
        })
        .cloned()
}

#[derive(Debug)]
pub enum ClientError {
    CertRootStore(std::io::Error),
    Io(std::io::Error),
    QuinnConnect(quinn::ConnectError),
    QuinnConnection(quinn::ConnectionError),
    InvalidClientAuthCertificate(rustls::Error),
    InvalidClientAuthKey(std::io::Error),
}

impl ClientError {
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        match self {
            ClientError::CertRootStore(v) => v.to_string(),
            ClientError::Io(v) => v.to_string(),
            ClientError::QuinnConnect(v) => v.to_string(),
            ClientError::QuinnConnection(v) => v.to_string(),
            ClientError::InvalidClientAuthCertificate(v) => v.to_string(),
            ClientError::InvalidClientAuthKey(v) => v.to_string(),
        }
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

impl From<rustls::Error> for ClientError {
    fn from(value: rustls::Error) -> Self {
        Self::InvalidClientAuthCertificate(value)
    }
}

pub async fn get_client(
    addr: SocketAddr,
    hostname: &str,
    alpn_protocols: Option<Vec<Vec<u8>>>,
    certificate_authorities: Option<Vec<Vec<u8>>>,
    client_auth: Option<(Vec<u8>, Vec<u8>)>,
) -> Result<(quinn::Connection, quinn::Endpoint), ClientError> {
    let roots = get_certs(certificate_authorities).map_err(ClientError::CertRootStore)?;

    let client_crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots);

    let mut client_crypto = match client_auth {
        None => client_crypto.with_no_client_auth(),
        Some(client_auth) => {
            let certs = rustls_pemfile::certs(&mut Cursor::new(client_auth.0))
                .map_err(ClientError::Io)?
                .into_iter()
                .map(rustls::Certificate)
                .collect();

            let key = rustls_pemfile::read_one(&mut Cursor::new(client_auth.1))
                .map_err(ClientError::InvalidClientAuthKey)?
                .ok_or_else(|| {
                    ClientError::InvalidClientAuthKey(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "key file did not contain any keys",
                    ))
                })?;

            let key = match key {
                rustls_pemfile::Item::X509Certificate(v) => v,
                rustls_pemfile::Item::RSAKey(v) => v,
                rustls_pemfile::Item::PKCS8Key(v) => v,
                rustls_pemfile::Item::ECKey(v) => v,
                rustls_pemfile::Item::Crl(v) => v,
                _ => Vec::new(),
            };

            client_crypto.with_client_auth_cert(certs, rustls::PrivateKey(key))?
        }
    };

    if let Some(protocols) = alpn_protocols {
        client_crypto.alpn_protocols = protocols;
    }

    let mut transport_config = quinn::TransportConfig::default();
    transport_config.keep_alive_interval(Some(Duration::from_secs(1)));

    let mut client_config = quinn::ClientConfig::new(Arc::new(client_crypto));
    client_config.transport_config(Arc::new(transport_config));

    let mut endpoint = quinn::Endpoint::client(SocketAddr::new(
        if addr.is_ipv6() {
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        },
        0,
    ))
    .map_err(ClientError::Io)?;
    endpoint.set_default_client_config(client_config);

    let connection = endpoint.connect(addr, hostname)?.await?;

    Ok((connection, endpoint))
}
