use std::ffi::OsStr;
use std::panic;
use std::path::PathBuf;

// Only Android
#[cfg(target_os="android")]
use android_logger::Config;
#[cfg(target_os="android")]
use log::LevelFilter;
#[cfg(target_os = "android")]
use std::sync::RwLock;

// If not Android
#[cfg(not(target_os="android"))]
use simplelog::{Config, WriteLogger};
#[cfg(not(target_os="android"))]
use log::{info, LevelFilter};
#[cfg(not(target_os="android"))]
use directories::BaseDirs;
#[cfg(not(target_os="android"))]
use std::fs;
#[cfg(not(target_os="android"))]
use std::fs::File;
#[cfg(not(target_os="android"))]
use std::sync::Once;

pub use protocol;
pub use protocol::communication::ClipboardTransferIntent;
pub use protocol::discovery::Device;
pub use thiserror::Error;
pub use crate::nearby_server::ConnectionIntentType;
pub use crate::connection_request::{ConnectionRequest, ReceiveProgressState, ReceiveProgressDelegate};
pub use crate::protocol::discovery::{BluetoothLeConnectionInfo, TcpConnectionInfo};
pub use crate::protocol::communication::FileTransferIntent;
pub use crate::nearby_server::{InternalNearbyServer, NearbyConnectionDelegate};
pub use crate::errors::ConnectErrors;
pub use crate::share_store::{ShareStore, ConnectionMedium, SendProgressDelegate, SendProgressState};


pub mod discovery;
pub mod encryption;
pub mod stream;
pub mod nearby_server;
pub mod transmission;
pub mod connection_request;
pub mod errors;
pub mod share_store;
pub mod connection;
mod tar;
mod windows;
mod progress;

pub const PROTOCOL_VERSION: u32 = 0;
pub const BLE_SERVICE_UUID: &str = "68D60EB2-8AAA-4D72-8851-BD6D64E169B7";
pub const BLE_DISCOVERY_CHARACTERISTIC_UUID: &str = "0BEBF3FE-9A5E-4ED1-8157-76281B3F0DA5";
pub const BLE_BUFFER_SIZE: usize = 10240;

#[cfg(not(target_os="android"))]
static INIT_LOGGER: Once = Once::new();

#[uniffi::export]
pub fn get_ble_service_uuid() -> String {
    return BLE_SERVICE_UUID.to_string();
}

#[uniffi::export]
pub fn get_ble_discovery_characteristic_uuid() -> String {
    return BLE_DISCOVERY_CHARACTERISTIC_UUID.to_string();
}

#[derive(uniffi::Enum)]
pub enum VersionCompatibility {
    Compatible,
    OutdatedVersion,
    IncompatibleNewVersion
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

    return Some(config_dir
        .join("InterShare")
        .join("intershare.log")
    )
}

#[cfg(target_os="android")]
fn get_log_file_path() -> Option<PathBuf> {
    return None;
}

#[cfg(target_os="android")]
pub fn init_logger() {
    android_logger::init_once(
        Config::default().with_max_level(LevelFilter::Trace),
    );
}

#[cfg(not(target_os="android"))]
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

#[cfg(not(target_os="android"))]
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

    if let Ok(path) = get_log_file_path()?.into_os_string().into_string() {
        return Some(path);
    }

    return None;
}

#[cfg(target_os = "android")]
static TMP_DIR: RwLock<Option<String>> = RwLock::new(None);

#[cfg(target_os = "android")]
#[uniffi::export]
pub fn set_tmp_dir(tmp: String) {
    let mut tmp_dir = TMP_DIR.write().unwrap();
    *tmp_dir = Some(tmp);
}


uniffi::include_scaffolding!("intershare_sdk");
