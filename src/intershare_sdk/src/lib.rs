use std::ffi::OsStr;
use std::path::PathBuf;

// Only Android
#[cfg(target_os = "android")]
use android_logger::Config;
#[cfg(target_os = "android")]
use log::LevelFilter;
#[cfg(target_os = "android")]
use std::sync::RwLock;

// If not Android

#[cfg(not(target_os = "android"))]
use directories::BaseDirs;
#[cfg(not(target_os = "android"))]
use log::{info, LevelFilter};
#[cfg(not(target_os = "android"))]
use simplelog::{Config, WriteLogger};
#[cfg(not(target_os = "android"))]
use std::fs;
#[cfg(not(target_os = "android"))]
use std::fs::File;
#[cfg(not(target_os = "android"))]
use std::panic;
#[cfg(not(target_os = "android"))]
use std::sync::Once;

pub use crate::certificates::{CertificateStoreDelegate, TlsIdentity};
pub use crate::connection_request::{
    ConnectionRequest, ReceiveProgressDelegate, ReceiveProgressState,
};
pub use crate::errors::ConnectErrors;
pub use crate::nearby_server::ConnectionIntentType;
pub use crate::nearby_server::{InternalNearbyServer, NearbyConnectionDelegate};
pub use crate::protocol::communication::FileTransferIntent;
pub use crate::protocol::discovery::{BluetoothLeConnectionInfo, TcpConnectionInfo};
pub use crate::share_store::{
    ConnectionMedium, SendProgressDelegate, SendProgressState, ShareStore,
};
pub use protocol;
pub use protocol::communication::ClipboardTransferIntent;
pub use protocol::discovery::Device;
pub use thiserror::Error;

pub mod certificates;
pub mod connection;
pub mod connection_request;
pub mod discovery;
pub mod encryption;
pub mod errors;
pub mod nearby_server;
mod progress;
pub mod share_store;
pub mod stream;
mod tar;
pub mod transmission;
#[cfg(target_os = "windows")]
mod windows;

pub const PROTOCOL_VERSION: u32 = 0;
pub const BLE_SERVICE_UUID: &str = "68D60EB2-8AAA-4D72-8851-BD6D64E169B7";
pub const BLE_DISCOVERY_CHARACTERISTIC_UUID: &str = "0BEBF3FE-9A5E-4ED1-8157-76281B3F0DA5";
pub const BLE_BUFFER_SIZE: usize = 10240;

/// Company identifier used for the manufacturer-specific data that carries the
/// discovery correlation token in BLE advertisements. 0xFFFF is the reserved
/// "no company / testing" id, which is appropriate for a closed ecosystem.
pub const BLE_MANUFACTURER_ID: u16 = 0xFFFF;

/// How long (in seconds) a peer may go unseen in the advertisement stream before
/// it is considered gone and removed from the discovered-devices list.
pub const BLE_DEVICE_TTL_SECONDS: u64 = 10;

/// Length (in chars) of the stable device-token prefix inside the advertised
/// correlation string. The remainder of the string is the data-version suffix.
pub const BLE_DEVICE_TOKEN_LEN: usize = 8;

#[cfg(not(target_os = "android"))]
static INIT_LOGGER: Once = Once::new();

#[uniffi::export]
pub fn get_ble_service_uuid() -> String {
    return BLE_SERVICE_UUID.to_string();
}

#[uniffi::export]
pub fn get_ble_discovery_characteristic_uuid() -> String {
    return BLE_DISCOVERY_CHARACTERISTIC_UUID.to_string();
}

#[uniffi::export]
pub fn get_ble_manufacturer_id() -> u16 {
    return BLE_MANUFACTURER_ID;
}

/// Derives the stable correlation token from a device id (always
/// [`BLE_DEVICE_TOKEN_LEN`] chars). This part of the advertised string never
/// changes for a given device, so scanners use it to correlate the same device
/// across BLE MAC-address rotation without reconnecting.
pub fn compact_device_token(device_id: &str) -> String {
    use base64::Engine;

    let digest = ring::digest::digest(&ring::digest::SHA256, device_id.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest.as_ref()[0..6])
}

/// Derives a short "data version" from the bytes a scanner would read over GATT.
/// It changes whenever the advertised connection info changes (e.g. the device
/// switched networks and now has a different IP/port, or restarted with a new
/// L2CAP PSM). Appending it to the advertised token lets scanners that have
/// already resolved a device notice the change and re-read it.
pub fn compact_data_version(payload: &[u8]) -> String {
    use base64::Engine;

    let digest = ring::digest::digest(&ring::digest::SHA256, payload);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest.as_ref()[0..3])
}

#[uniffi::export]
pub fn get_compact_device_token(device_id: String) -> String {
    return compact_device_token(&device_id);
}

#[derive(uniffi::Enum)]
pub enum VersionCompatibility {
    Compatible,
    OutdatedVersion,
    IncompatibleNewVersion,
}

#[uniffi::export]
pub fn is_compatible(device: Device) -> VersionCompatibility {
    let Some(remote_device_version) = device.protocol_version else {
        return VersionCompatibility::OutdatedVersion;
    };

    if remote_device_version < PROTOCOL_VERSION {
        return VersionCompatibility::OutdatedVersion;
    }

    if remote_device_version > PROTOCOL_VERSION {
        return VersionCompatibility::IncompatibleNewVersion;
    }

    return VersionCompatibility::Compatible;
}

fn convert_os_str(os_str: &OsStr) -> String {
    return os_str.to_string_lossy().to_string();
}

#[cfg(not(target_os = "android"))]
fn get_log_file_path() -> Option<PathBuf> {
    let project_dirs = BaseDirs::new()?;
    let config_dir = project_dirs.config_dir();

    return Some(config_dir.join("InterShare").join("intershare.log"));
}

#[cfg(target_os = "android")]
fn get_log_file_path() -> Option<PathBuf> {
    return None;
}

#[cfg(target_os = "android")]
pub fn init_logger() {
    android_logger::init_once(Config::default().with_max_level(LevelFilter::Trace));
}

#[cfg(not(target_os = "android"))]
fn set_panic_logger() {
    panic::set_hook(Box::new(|panic_info| {
        let location = panic_info.location().unwrap();
        let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic message".to_string()
        };

        log::error!(
            "Panic occurred at file '{}' line {}: {}",
            location.file(),
            location.line(),
            message
        );
    }));
}

#[cfg(not(target_os = "android"))]
pub fn init_logger() {
    INIT_LOGGER.call_once(|| {
        // Get the platform-specific configuration folder
        let log_file_path = get_log_file_path().expect("Failed to get log file path");

        // Ensure the directory exists
        if let Some(parent) = log_file_path.parent() {
            fs::create_dir_all(parent).expect("Failed to create log directory");
        }

        println!("Log file path: {:?}", log_file_path);

        // Initialize the logger
        let log_file = File::create(log_file_path).expect("Failed to create log file");
        WriteLogger::init(LevelFilter::Info, Config::default(), log_file)
            .expect("Failed to initialize logger");

        set_panic_logger();

        info!("Logger initialized successfully.");
    });
}

#[uniffi::export]
pub fn get_log_file_path_str() -> Option<String> {
    init_logger();

    get_log_file_path()?.into_os_string().into_string().ok()
}

#[cfg(target_os = "android")]
static TMP_DIR: RwLock<Option<String>> = RwLock::new(None);

#[cfg(target_os = "android")]
#[uniffi::export]
pub fn set_tmp_dir(tmp: String) {
    let mut tmp_dir = TMP_DIR.write().unwrap();
    *tmp_dir = Some(tmp);
}

#[uniffi::export]
pub fn set_certificate_store_delegate(delegate: Box<dyn CertificateStoreDelegate>) {
    certificates::set_delegate(delegate);
}

#[uniffi::export]
pub fn clear_certificate_store_delegate() {
    certificates::clear_delegate();
}

uniffi::include_scaffolding!("intershare_sdk");
