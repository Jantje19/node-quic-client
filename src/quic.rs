use std::{
    io::Cursor,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use once_cell::sync::OnceCell;
use quinn::crypto::rustls::QuicClientConfig;
use rustls_native_certs::CertificateResult;

#[derive(Debug)]
pub enum GetCertsError {
    NativeLoad(Vec<rustls_native_certs::Error>),
    CertificateAuthority(std::io::Error),
    Load(rustls::Error),
}

impl GetCertsError {
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        match self {
            GetCertsError::NativeLoad(v) => {
                format!("Unable to load native certificate(s): {:?}", v)
            }
            GetCertsError::CertificateAuthority(e) => {
                format!("Unable to load certificate authority file: {e}")
            }
            GetCertsError::Load(e) => format!("Unable to load certificate: {e}"),
        }
    }
}

fn get_certs(
    certificate_authorities: Option<Vec<Vec<u8>>>,
) -> Result<rustls::RootCertStore, GetCertsError> {
    static CERTS: OnceCell<rustls::RootCertStore> = OnceCell::new();

    CERTS
        .get_or_try_init(|| {
            let mut roots = rustls::RootCertStore::empty();

            let CertificateResult { certs, errors, .. } = rustls_native_certs::load_native_certs();

            if !errors.is_empty() {
                return Err(GetCertsError::NativeLoad(errors));
            }

            for cert in certs {
                roots.add(cert).map_err(GetCertsError::Load)?;
            }

            if let Some(certificate_authorities) = certificate_authorities {
                for ca in certificate_authorities {
                    for cert in rustls_pemfile::certs(&mut Cursor::new(ca)) {
                        let cert = cert.map_err(GetCertsError::CertificateAuthority)?;

                        roots.add(cert).map_err(GetCertsError::Load)?;
                    }
                }
            }

            Ok(roots)
        })
        .cloned()
}

#[derive(Debug)]
pub enum ClientError {
    CertRootStore(GetCertsError),
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

    let client_crypto = rustls::ClientConfig::builder().with_root_certificates(roots);

    let mut client_crypto = match client_auth {
        None => client_crypto.with_no_client_auth(),
        Some(client_auth) => {
            let certs = rustls_pemfile::certs(&mut Cursor::new(client_auth.0))
                .collect::<Result<Vec<_>, _>>()
                .map_err(ClientError::Io)?;

            let key = rustls_pemfile::read_one(&mut Cursor::new(client_auth.1))
                .map_err(ClientError::InvalidClientAuthKey)?
                .ok_or_else(|| {
                    ClientError::InvalidClientAuthKey(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "key file did not contain any keys",
                    ))
                })?;

            let key: rustls::pki_types::PrivateKeyDer = match key {
                rustls_pemfile::Item::Pkcs1Key(v) => v.into(),
                rustls_pemfile::Item::Pkcs8Key(v) => v.into(),
                rustls_pemfile::Item::Sec1Key(v) => v.into(),
                _ => {
                    return Err(ClientError::InvalidClientAuthKey(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "Invalid file type",
                    )))
                }
            };

            client_crypto.with_client_auth_cert(certs, key)?
        }
    };

    if let Some(protocols) = alpn_protocols {
        client_crypto.alpn_protocols = protocols;
    }

    client_crypto.key_log = Arc::new(rustls::KeyLogFile::new());

    let mut transport_config = quinn::TransportConfig::default();
    transport_config.keep_alive_interval(Some(Duration::from_secs(1)));

    let client_config = QuicClientConfig::try_from(client_crypto).unwrap();
    let mut client_config = quinn::ClientConfig::new(Arc::new(client_config));
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
