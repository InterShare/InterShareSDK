use crate::certificates;
use crate::stream::Close;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand_core::{OsRng, RngCore};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::StreamOwned;
use std::error::Error;
use std::fmt::Debug;
use std::io::{Read, Write};
use std::sync::Arc;

use log::{debug, info, warn};

pub fn generate_secure_base64_token(byte_length: usize) -> String {
    let mut bytes = vec![0u8; byte_length];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(&bytes)
}

const PROTOCOL_VERSIONS: &[&'static rustls::SupportedProtocolVersion] = &[&rustls::version::TLS13];
const DEFAULT_SERVER_NAME: &str = "intershare.local";

#[derive(Clone, Debug)]
struct TrustOnFirstUseVerifier {
    device_id: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl TrustOnFirstUseVerifier {
    fn new(device_id: String, provider: Arc<rustls::crypto::CryptoProvider>) -> Self {
        Self {
            device_id,
            provider,
        }
    }

    fn verify_or_remember(&self, certificate: &CertificateDer<'_>) -> Result<(), rustls::Error> {
        let presented = certificate.as_ref();
        if let Some(expected) = certificates::load_remote_certificate(&self.device_id) {
            if expected != presented {
                warn!(
                    "Rejecting TLS certificate for device {} due to fingerprint mismatch",
                    self.device_id
                );
                return Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::ApplicationVerificationFailure,
                ));
            }
            debug!(
                "Accepted TLS certificate for device {} using stored fingerprint",
                self.device_id
            );
            return Ok(());
        }

        certificates::store_remote_certificate(&self.device_id, presented);
        info!(
            "First trusted certificate for device {} (fingerprint {})",
            self.device_id,
            hex_fingerprint(presented)
        );
        Ok(())
    }
}

fn hex_fingerprint(data: &[u8]) -> String {
    use ring::digest::{digest, SHA256};
    let hash = digest(&SHA256, data);
    hash.as_ref()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<Vec<_>>()
        .join(":")
}

impl rustls::client::danger::ServerCertVerifier for TrustOnFirstUseVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        self.verify_or_remember(end_entity)?;
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn resolve_server_name(hostname: Option<&str>) -> Result<ServerName<'static>, Box<dyn Error>> {
    if let Some(name) = hostname {
        match ServerName::try_from(name) {
            Ok(server_name) => return Ok(server_name.to_owned()),
            Err(err) => warn!(
                "Provided server name {} is invalid for TLS SNI: {:?}. Falling back to default.",
                name, err
            ),
        }
    }

    Ok(ServerName::try_from(DEFAULT_SERVER_NAME)?.to_owned())
}

pub fn initiate_sender_communication<T>(
    device_id: &str,
    server_name_hint: Option<&str>,
    stream: T,
) -> Result<StreamOwned<rustls::ClientConnection, T>, Box<dyn Error>>
where
    T: Read + Write,
{
    use rustls::{ClientConfig, ClientConnection};

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = TrustOnFirstUseVerifier::new(device_id.to_string(), provider.clone());

    info!(
        "Starting TLS client session with device {} (SNI hint: {:?})",
        device_id, server_name_hint
    );

    let config = ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(PROTOCOL_VERSIONS)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();

    let server_name = resolve_server_name(server_name_hint)?;

    let connection = ClientConnection::new(config.into(), server_name)?;
    Ok(StreamOwned::new(connection, stream))
}

pub fn initiate_receiver_communication<T>(
    stream: T,
) -> Result<StreamOwned<rustls::ServerConnection, T>, Box<dyn Error>>
where
    T: Read + Write,
{
    use rustls::{ServerConfig, ServerConnection};

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let hosts = certificates::current_hosts();
    let fallback_hosts = vec!["localhost".to_string()];
    let identity = certificates::get_or_initialize_identity(if hosts.is_empty() {
        &fallback_hosts
    } else {
        &hosts
    })?;
    let (certificate, private_key) = certificates::identity_to_rustls(&identity)?;

    info!(
        "Preparing TLS server with certificate covering hosts: {:?}",
        hosts
    );

    let config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(PROTOCOL_VERSIONS)?
        .with_no_client_auth()
        .with_single_cert(vec![certificate], private_key)?;

    let connection = ServerConnection::new(config.into())?;
    Ok(StreamOwned::new(connection, stream))
}

pub trait EncryptedReadWrite: Read + Write + Send + Close {}
impl<T> EncryptedReadWrite for StreamOwned<rustls::ClientConnection, T> where
    T: Read + Write + Send + Close
{
}
impl<T> EncryptedReadWrite for StreamOwned<rustls::ServerConnection, T> where
    T: Read + Write + Send + Close
{
}
