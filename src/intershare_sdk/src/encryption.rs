use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand_core::{OsRng, RngCore};
use ring::pkcs8;
use std::io::{Read, Write};
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::StreamOwned;
use std::error::Error;
use std::sync::Arc;
use crate::stream::Close;

pub fn generate_secure_base64_token(byte_length: usize) -> String {
    let mut bytes = vec![0u8; byte_length];
    OsRng.fill_bytes(&mut bytes);
    return URL_SAFE_NO_PAD.encode(&bytes);
}

const PROTOCOL_VERSIONS: &[&'static rustls::SupportedProtocolVersion] = &[&rustls::version::TLS13];

/// DANGER: This certificate verifier accepts ALL certificates without validation.
/// This should ONLY be used for testing/development purposes, never in production!
#[derive(Debug)]
struct DangerousAcceptAllCertificates;

impl rustls::client::danger::ServerCertVerifier for DangerousAcceptAllCertificates {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![rustls::SignatureScheme::ED25519]
    }
}

pub fn generate_keypair() -> Result<pkcs8::Document, ring::error::Unspecified> {
    use ring::signature::Ed25519KeyPair;
    Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
}

pub async fn initiate_sender_communication<'s, T>(
    stream: T,
) -> Result<rustls::StreamOwned<rustls::ClientConnection, T>, Box<dyn Error>>
where
    T: Read + Write + 's,
{
    use rustls::{ClientConfig, ClientConnection};

    // TODO verify certificate GUI flow
    let provider = rustls::crypto::ring::default_provider();

    let config = ClientConfig::builder_with_provider(provider.into())
        .with_protocol_versions(PROTOCOL_VERSIONS)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(DangerousAcceptAllCertificates))
        .with_no_client_auth();

    // TODO change to client name
    let server_name = ServerName::try_from("intershare")?;

    let conn = ClientConnection::new(config.into(), server_name)?;

    let tls = StreamOwned::new(conn, stream);

    return Ok(tls);
}

pub fn initiate_receiver_communication<T>(
    stream: T,
) -> Result<rustls::StreamOwned<rustls::ServerConnection, T>, Box<dyn Error>>
where
    T: Read + Write,
{
    use rustls::{pki_types::PrivateKeyDer, ServerConfig, ServerConnection};

    let key = generate_keypair()?;
    let key_der = PrivateKeyDer::from_pem_slice(document.as_ref())?;

    let provider = rustls::crypto::ring::default_provider();

    // TODO add client auth
    let config = ServerConfig::builder_with_provider(provider.into())
        .with_protocol_versions(PROTOCOL_VERSIONS)?
        .with_no_client_auth()
        .with_single_cert(Vec::new(), key_der)?;

    let conn = ServerConnection::new(config.into())?;

    let stream = StreamOwned::new(conn, stream);

    return Ok(stream);
}

pub trait EncryptedReadWrite: Read + Write + Send + Close {}
impl<T> EncryptedReadWrite for rustls::StreamOwned<rustls::ClientConnection, T> where
    T: Read + Write + Send + Close
{
}
impl<T> EncryptedReadWrite for rustls::StreamOwned<rustls::ServerConnection, T> where
    T: Read + Write + Send + Close
{
}
