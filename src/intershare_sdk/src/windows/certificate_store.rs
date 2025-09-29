use crate::certificates::{self, CertificateStoreDelegate, TlsIdentity};
use dirs::data_dir;
use log::{error, info, warn};
use ring::digest::{digest, SHA256};
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Once;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

static STORE_INIT: Once = Once::new();

pub(crate) fn ensure_certificate_store_registered() {
    STORE_INIT.call_once(|| match WindowsCertificateStore::new() {
        Ok(store) => certificates::set_delegate(Box::new(store)),
        Err(err) => warn!("Failed to initialize Windows certificate store: {}", err),
    });
}

#[derive(Clone)]
struct WindowsCertificateStore {
    root: PathBuf,
}

impl WindowsCertificateStore {
    fn new() -> Result<Self, String> {
        let mut root = data_dir().ok_or("Unable to determine data directory")?;
        root.push("InterShare");
        root.push("tls");
        fs::create_dir_all(&root).map_err(|err| format!("Failed to create store dir: {}", err))?;
        Ok(Self { root })
    }

    fn cert_path(&self) -> PathBuf {
        self.root.join("identity.cert")
    }

    fn key_path(&self) -> PathBuf {
        self.root.join("identity.key")
    }

    fn remote_path(&self, device_id: &str) -> PathBuf {
        let hash = digest(&SHA256, device_id.as_bytes());
        let fingerprint = hash
            .as_ref()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();
        self.root.join(format!("remote_{}.bin", fingerprint))
    }

    fn read_protected(&self, path: &Path) -> Option<Vec<u8>> {
        let mut file = File::open(path).ok()?;
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            warn!("Failed to read protected data at {:?}", path);
            return None;
        }
        decrypt(&buf).ok()
    }

    fn write_protected(&self, path: &Path, data: &[u8]) {
        match encrypt(data) {
            Ok(cipher) => {
                if let Err(err) = File::create(path).and_then(|mut f| f.write_all(&cipher)) {
                    error!("Failed to write protected data at {:?}: {}", path, err);
                }
            }
            Err(err) => error!("Failed to encrypt data for {:?}: {}", path, err),
        }
    }
}

impl fmt::Debug for WindowsCertificateStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowsCertificateStore")
            .field("root", &self.root)
            .finish()
    }
}

impl CertificateStoreDelegate for WindowsCertificateStore {
    fn load_identity(&self) -> Option<TlsIdentity> {
        let certificate = self.read_protected(&self.cert_path())?;
        let private_key = self.read_protected(&self.key_path())?;
        info!("Loaded TLS identity from Windows credential store");
        Some(TlsIdentity {
            certificate_der: certificate,
            private_key_der: private_key,
        })
    }

    fn store_identity(&self, identity: TlsIdentity) {
        info!("Persisting TLS identity to Windows credential store");
        self.write_protected(&self.cert_path(), &identity.certificate_der);
        self.write_protected(&self.key_path(), &identity.private_key_der);
    }

    fn clear_identity(&self) {
        let _ = fs::remove_file(self.cert_path());
        let _ = fs::remove_file(self.key_path());
        info!("Cleared TLS identity from Windows credential store");
    }

    fn load_remote_certificate(&self, device_id: String) -> Option<Vec<u8>> {
        let path = self.remote_path(&device_id);
        let data = self.read_protected(&path)?;
        info!("Loaded certificate fingerprint for device {}", device_id);
        Some(data)
    }

    fn store_remote_certificate(&self, device_id: String, certificate_der: Vec<u8>) {
        let path = self.remote_path(&device_id);
        self.write_protected(&path, &certificate_der);
        info!("Stored certificate fingerprint for device {}", device_id);
    }

    fn clear_remote_certificate(&self, device_id: String) {
        let _ = fs::remove_file(self.remote_path(&device_id));
        info!("Cleared certificate fingerprint for device {}", device_id);
    }
}

fn encrypt(data: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();

        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|err| format!("CryptProtectData failed: {}", err.code().0))?;

        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let result = slice.to_vec();
        if !output.pbData.is_null() {
            LocalFree(HLOCAL(output.pbData as isize));
        }
        Ok(result)
    }
}

fn decrypt(data: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let mut input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();

        CryptUnprotectData(
            &mut input,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .map_err(|err| format!("CryptUnprotectData failed: {}", err.code().0))?;

        let slice = std::slice::from_raw_parts(output.pbData, output.cbData as usize);
        let result = slice.to_vec();
        if !output.pbData.is_null() {
            LocalFree(HLOCAL(output.pbData as isize));
        }
        Ok(result)
    }
}
