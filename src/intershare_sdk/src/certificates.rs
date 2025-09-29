use log::{debug, error, info, warn};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

#[derive(Clone, Debug)]
pub struct TlsIdentity {
    pub certificate_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
}

pub trait CertificateStoreDelegate: Send + Sync + std::fmt::Debug {
    fn load_identity(&self) -> Option<TlsIdentity>;
    fn store_identity(&self, identity: TlsIdentity);
    fn clear_identity(&self);
    fn load_remote_certificate(&self, device_id: String) -> Option<Vec<u8>>;
    fn store_remote_certificate(&self, device_id: String, certificate_der: Vec<u8>);
    fn clear_remote_certificate(&self, device_id: String);
}

#[derive(Clone, Debug)]
pub struct Identity {
    pub certificate_der: Vec<u8>,
    pub private_key_der: Vec<u8>,
}

impl Identity {
    pub fn as_tls_identity(&self) -> TlsIdentity {
        TlsIdentity {
            certificate_der: self.certificate_der.clone(),
            private_key_der: self.private_key_der.clone(),
        }
    }
}

impl TryFrom<TlsIdentity> for Identity {
    type Error = String;

    fn try_from(value: TlsIdentity) -> Result<Self, Self::Error> {
        if value.certificate_der.is_empty() || value.private_key_der.is_empty() {
            return Err("Certificate or private key was empty".into());
        }

        Ok(Self {
            certificate_der: value.certificate_der,
            private_key_der: value.private_key_der,
        })
    }
}

static LOCAL_IDENTITY: OnceLock<RwLock<Option<Identity>>> = OnceLock::new();
static STORE_DELEGATE: OnceLock<RwLock<Option<Box<dyn CertificateStoreDelegate>>>> =
    OnceLock::new();
static HOST_METADATA: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
static REMOTE_CERT_CACHE: OnceLock<RwLock<HashMap<String, Vec<u8>>>> = OnceLock::new();

fn identity_lock() -> &'static RwLock<Option<Identity>> {
    LOCAL_IDENTITY.get_or_init(|| RwLock::new(None))
}

fn delegate_lock() -> &'static RwLock<Option<Box<dyn CertificateStoreDelegate>>> {
    STORE_DELEGATE.get_or_init(|| RwLock::new(None))
}

fn host_metadata_lock() -> &'static RwLock<Vec<String>> {
    HOST_METADATA.get_or_init(|| RwLock::new(Vec::new()))
}

fn remote_certificate_cache() -> &'static RwLock<HashMap<String, Vec<u8>>> {
    REMOTE_CERT_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

pub fn set_delegate(delegate: Box<dyn CertificateStoreDelegate>) {
    let mut guard = delegate_lock().write().unwrap();
    *guard = Some(delegate);
    drop(guard);

    info!("Certificate store delegate registered");
    reset_cached_identity();
}

pub fn clear_delegate() {
    let mut guard = delegate_lock().write().unwrap();
    *guard = None;
    drop(guard);

    reset_cached_identity();
    info!("Certificate store delegate cleared");
    clear_all_remote_certificates();
}

pub fn reset_cached_identity() {
    let mut identity_guard = identity_lock().write().unwrap();
    *identity_guard = None;
}

fn load_from_delegate() -> Option<Identity> {
    let delegate_guard = delegate_lock().read().unwrap();
    let Some(delegate) = delegate_guard.as_ref() else {
        return None;
    };

    match delegate.load_identity() {
        Some(identity) => match Identity::try_from(identity) {
            Ok(identity) => {
                info!("Restored TLS identity from secure store");
                Some(identity)
            }
            Err(err) => {
                warn!("Certificate delegate returned invalid identity: {}", err);
                None
            }
        },
        None => None,
    }
}

fn store_with_delegate(identity: &Identity) {
    let delegate_guard = delegate_lock().read().unwrap();
    if let Some(delegate) = delegate_guard.as_ref() {
        delegate.store_identity(identity.as_tls_identity());
        debug!("Persisted TLS identity to secure store");
    }
}

fn clear_delegate_identity() {
    let delegate_guard = delegate_lock().read().unwrap();
    if let Some(delegate) = delegate_guard.as_ref() {
        delegate.clear_identity();
        info!("Cleared persisted TLS identity");
    }
}

fn load_remote_from_delegate(device_id: &str) -> Option<Vec<u8>> {
    let delegate_guard = delegate_lock().read().unwrap();
    let Some(delegate) = delegate_guard.as_ref() else {
        return None;
    };

    let result = delegate.load_remote_certificate(device_id.to_string());
    if result.is_some() {
        info!(
            "Loaded stored certificate fingerprint for device {}",
            device_id
        );
    }

    result
}

fn store_remote_with_delegate(device_id: &str, certificate_der: &[u8]) {
    let delegate_guard = delegate_lock().read().unwrap();
    if let Some(delegate) = delegate_guard.as_ref() {
        delegate.store_remote_certificate(device_id.to_string(), certificate_der.to_vec());
        info!("Saved certificate fingerprint for device {}", device_id);
    }
}

fn clear_remote_with_delegate(device_id: &str) {
    let delegate_guard = delegate_lock().read().unwrap();
    if let Some(delegate) = delegate_guard.as_ref() {
        delegate.clear_remote_certificate(device_id.to_string());
        info!(
            "Cleared stored certificate fingerprint for device {}",
            device_id
        );
    }
}

fn generate_params(hosts: &[String]) -> Result<CertificateParams, rcgen::Error> {
    let mut params = CertificateParams::new(hosts.to_vec())?;

    if let Some(primary) = hosts.first() {
        let mut distinguished_name = DistinguishedName::new();
        distinguished_name.push(DnType::CommonName, primary.clone());
        params.distinguished_name = distinguished_name;
    }

    Ok(params)
}

fn generate_certificate(hosts: &[String]) -> Result<Identity, rcgen::Error> {
    let params = generate_params(hosts)?;
    let key_pair = KeyPair::generate_for(&PKCS_ED25519)?;
    let certificate = params.self_signed(&key_pair)?;

    Ok(Identity {
        certificate_der: certificate.der().as_ref().to_vec(),
        private_key_der: key_pair.serialized_der().to_vec(),
    })
}

pub fn update_hosts(hosts: &[String]) {
    let mut host_guard = host_metadata_lock().write().unwrap();
    let mut combined = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    for host in hosts {
        if !host.is_empty() && !combined.contains(host) {
            combined.push(host.clone());
        }
    }
    *host_guard = combined;
    debug!("Updated certificate host metadata: {:?}", *host_guard);
}

fn load_cached_identity() -> Option<Identity> {
    identity_lock().read().unwrap().clone()
}

fn cache_identity(identity: Identity) -> Identity {
    let mut guard = identity_lock().write().unwrap();
    *guard = Some(identity.clone());
    identity
}

pub fn clear_identity() {
    clear_delegate_identity();
    let mut guard = identity_lock().write().unwrap();
    *guard = None;
}

pub fn get_or_initialize_identity(default_hosts: &[String]) -> Result<Identity, String> {
    if let Some(identity) = load_cached_identity() {
        debug!("Using cached TLS identity in memory");
        return Ok(identity);
    }

    if let Some(identity) = load_from_delegate() {
        debug!("Using TLS identity provided by delegate");
        return Ok(cache_identity(identity));
    }

    let hosts_guard = host_metadata_lock().read().unwrap();
    let hosts = if hosts_guard.is_empty() {
        default_hosts.to_vec()
    } else {
        hosts_guard.clone()
    };
    drop(hosts_guard);

    match generate_certificate(&hosts) {
        Ok(identity) => {
            info!(
                "Generated new self-signed TLS identity for hosts: {:?}",
                hosts
            );
            store_with_delegate(&identity);
            Ok(cache_identity(identity))
        }
        Err(err) => {
            error!("Failed to generate TLS identity: {}", err);
            Err(format!("Failed to generate TLS identity: {}", err))
        }
    }
}

pub fn identity_to_rustls(
    identity: &Identity,
) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), String> {
    let certificate_der = CertificateDer::from(identity.certificate_der.clone());
    let private_key_der = PrivateKeyDer::try_from(identity.private_key_der.clone())
        .map_err(|err| format!("Invalid private key: {}", err))?;

    Ok((certificate_der, private_key_der))
}

pub fn load_remote_certificate(device_id: &str) -> Option<Vec<u8>> {
    if let Some(cached) = remote_certificate_cache()
        .read()
        .unwrap()
        .get(device_id)
        .cloned()
    {
        debug!(
            "Using cached certificate fingerprint for device {}",
            device_id
        );
        return Some(cached);
    }

    let Some(from_delegate) = load_remote_from_delegate(device_id) else {
        return None;
    };

    remote_certificate_cache()
        .write()
        .unwrap()
        .insert(device_id.to_string(), from_delegate.clone());

    Some(from_delegate)
}

pub fn store_remote_certificate(device_id: &str, certificate_der: &[u8]) {
    remote_certificate_cache()
        .write()
        .unwrap()
        .insert(device_id.to_string(), certificate_der.to_vec());

    store_remote_with_delegate(device_id, certificate_der);
}

pub fn clear_remote_certificate(device_id: &str) {
    remote_certificate_cache()
        .write()
        .unwrap()
        .remove(device_id);

    clear_remote_with_delegate(device_id);
}

pub fn clear_all_remote_certificates() {
    let keys: Vec<String> = remote_certificate_cache()
        .read()
        .unwrap()
        .keys()
        .cloned()
        .collect();

    for key in keys {
        clear_remote_certificate(&key);
    }

    info!("Cleared all cached remote certificate fingerprints");
}

pub fn current_hosts() -> Vec<String> {
    host_metadata_lock().read().unwrap().clone()
}
