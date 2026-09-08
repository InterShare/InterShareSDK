use crate::encryption::generate_secure_base64_token;
use crate::errors::DiscoverySetupError;
use crate::init_logger;
use log::{info, warn};
use protocol::discovery;
use protocol::discovery::device_discovery_message::Content;
use protocol::discovery::{Device, DeviceConnectionInfo, DeviceDiscoveryMessage};
use protocol::prost::Message;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

/// Minimum time between GATT read attempts for a peer we have not yet resolved.
/// Prevents reconnecting to the same advertising peer on every packet while a
/// previous attempt is still in flight or recently failed.
const READ_ATTEMPT_COOLDOWN: Duration = Duration::from_secs(4);

/// Tracks a peer seen in the advertisement stream. The key is the stable device
/// token when advertised (stable across BLE MAC rotation), otherwise the
/// platform peripheral identifier.
struct PeerState {
    /// Last time this peer was observed in the advertisement stream.
    last_seen: Instant,
    /// Last time we asked the native layer to connect+read this peer.
    last_attempt: Option<Instant>,
    /// The resolved application-level device id, known after a successful read.
    device_id: Option<String>,
    /// Whether we have fully resolved this peer's connection info.
    resolved: bool,
    /// The advertised data-version that was current when we last resolved this
    /// peer. If the live advertised version differs, the peer's connection info
    /// changed (e.g. a network switch) and we re-read it.
    resolved_version: Option<String>,
}

/// Splits an advertised correlation string into its stable device-token prefix
/// and its data-version suffix. The string is produced by
/// [`crate::nearby_server::InternalNearbyServer::get_ble_advertisement_name`].
fn split_advertised_token(token: &str) -> (String, Option<String>) {
    if token.len() > crate::BLE_DEVICE_TOKEN_LEN && token.is_char_boundary(crate::BLE_DEVICE_TOKEN_LEN)
    {
        let (device_token, version) = token.split_at(crate::BLE_DEVICE_TOKEN_LEN);
        (device_token.to_string(), Some(version.to_string()))
    } else {
        (token.to_string(), None)
    }
}

#[uniffi::export(callback_interface)]
pub trait BleDiscoveryImplementationDelegate: Send + Sync + Debug {
    fn start_scanning(&self);
    fn stop_scanning(&self);
}

#[uniffi::export(callback_interface)]
pub trait DeviceListUpdateDelegate: Send + Sync + Debug {
    fn device_added(&self, value: discovery::Device);
    fn device_removed(&self, device_id: String);
}

static DISCOVERED_DEVICES: OnceLock<RwLock<HashMap<String, DeviceConnectionInfo>>> =
    OnceLock::new();

static DELEGATES: OnceLock<RwLock<HashMap<String, Arc<Box<dyn DeviceListUpdateDelegate>>>>> =
    OnceLock::new();

pub fn get_connection_details(device: Device) -> Option<DeviceConnectionInfo> {
    DISCOVERED_DEVICES
        .get()
        .unwrap()
        .read()
        .unwrap()
        .get(&device.id)
        .cloned()
}

#[derive(uniffi::Object)]
pub struct InternalDiscovery {
    pub ble_discovery_implementation:
        tokio::sync::RwLock<Option<Box<dyn BleDiscoveryImplementationDelegate>>>,
    current_delegate_id: String,
    discovered_devices: RwLock<HashMap<String, DeviceConnectionInfo>>,
    /// Liveness/dedup table keyed by advertisement correlation token (or the
    /// platform identifier as a fallback). Drives both the "should I connect?"
    /// decision and TTL-based removal of departed devices.
    peers: RwLock<HashMap<String, PeerState>>,

    #[cfg(target_os = "windows")]
    pub(crate) scanning: Arc<AtomicBool>,
}

impl Debug for InternalDiscovery {
    fn fmt(&self, _f: &mut Formatter<'_>) -> std::fmt::Result {
        unimplemented!()
    }
}

#[uniffi::export]
impl InternalDiscovery {
    #[uniffi::constructor]
    pub fn new(
        delegate: Option<Box<dyn DeviceListUpdateDelegate>>,
    ) -> Result<Arc<Self>, DiscoverySetupError> {
        init_logger();

        DISCOVERED_DEVICES.get_or_init(|| RwLock::new(HashMap::new()));
        DELEGATES.get_or_init(|| RwLock::new(HashMap::new()));

        let delegate_id = generate_secure_base64_token(4);

        if let Some(delegate) = delegate {
            info!("Adding delegate: {:?}", delegate_id);
            DELEGATES
                .get()
                .unwrap()
                .write()
                .unwrap()
                .insert(delegate_id.clone(), Arc::new(delegate));
        };

        return Ok(Arc::new(Self {
            ble_discovery_implementation: tokio::sync::RwLock::new(None),
            current_delegate_id: delegate_id,
            discovered_devices: RwLock::new(HashMap::new()),
            peers: RwLock::new(HashMap::new()),

            #[cfg(target_os = "windows")]
            scanning: Arc::new(AtomicBool::new(false)),
        }));
    }

    pub fn get_devices(self: Arc<Self>) -> Vec<Device> {
        let discovered_devices = self.discovered_devices.read().unwrap();

        discovered_devices
            .iter()
            .map(|(_, device_info)| {
                device_info
                    .device
                    .clone()
                    .expect("No device in DeviceConnectionInfo")
            })
            .collect()
    }

    pub fn add_ble_implementation(
        self: Arc<Self>,
        implementation: Box<dyn BleDiscoveryImplementationDelegate>,
    ) {
        *self.ble_discovery_implementation.blocking_write() = Some(implementation)
    }

    pub fn start(self: Arc<Self>) {
        DISCOVERED_DEVICES.get().unwrap().write().unwrap().clear();
        self.discovered_devices.write().unwrap().clear();
        self.peers.write().unwrap().clear();

        #[cfg(target_os = "windows")]
        self.windows_start_scanning();

        #[cfg(not(target_os = "windows"))]
        if let Some(ble_discovery_implementation) =
            &*self.ble_discovery_implementation.blocking_read()
        {
            ble_discovery_implementation.start_scanning();
        }
    }

    pub fn stop(self: Arc<Self>) {
        #[cfg(target_os = "windows")]
        self.windows_stop_scanning();

        info!("Removing delegate: {:?}", self.current_delegate_id);
        DELEGATES
            .get()
            .unwrap()
            .write()
            .expect("Failed to read delegates")
            .remove(&self.current_delegate_id);

        #[cfg(not(target_os = "windows"))]
        if let Some(ble_discovery_implementation) =
            self.ble_discovery_implementation.blocking_read().as_ref()
        {
            ble_discovery_implementation.stop_scanning();
        }
    }

    /// Called by the native layer after it has connected to a peer and read the
    /// discovery characteristic.
    ///
    /// - `ble_uuid` is the platform peripheral identifier, stored as the BLE
    ///   connection target (used later to open the L2CAP channel).
    /// - `token` is the advertised correlation token, used as the liveness key
    ///   so the peer can be tracked across BLE MAC-address rotation.
    pub fn parse_discovery_message(
        self: Arc<Self>,
        data: Vec<u8>,
        ble_uuid: Option<String>,
        token: Option<String>,
    ) {
        let Ok(discovery_message) =
            DeviceDiscoveryMessage::decode_length_delimited(data.as_slice())
        else {
            return;
        };

        match discovery_message.content {
            None => {
                warn!("[{:?}] Discovery message has no content", ble_uuid);
            }
            Some(Content::DeviceConnectionInfo(device_connection_info)) => {
                let Some(device) = &device_connection_info.device else {
                    warn!(
                        "[{:?}] Discovery message does not contain any device info",
                        ble_uuid
                    );
                    return;
                };

                let mut device_connection_info = device_connection_info.clone();

                if let Some(ble_uuid) = ble_uuid.clone() {
                    if let Some(mut ble_info) = device_connection_info.ble {
                        ble_info.uuid = ble_uuid;
                        device_connection_info.ble = Some(ble_info);
                    }
                }

                // Mark the peer resolved in the liveness table so we stop
                // reconnecting to it (until its advertised data version changes)
                // and start tracking it for TTL expiry.
                let (key, version) = match &token {
                    Some(token) => {
                        let (device_token, version) = split_advertised_token(token);
                        (Some(device_token), version)
                    }
                    None => (ble_uuid.clone(), None),
                };

                if let Some(key) = key {
                    let mut peers = self.peers.write().unwrap();
                    let entry = peers.entry(key).or_insert_with(|| PeerState {
                        last_seen: Instant::now(),
                        last_attempt: None,
                        device_id: None,
                        resolved: false,
                        resolved_version: None,
                    });
                    entry.last_seen = Instant::now();
                    entry.device_id = Some(device.id.clone());
                    entry.resolved = true;
                    entry.resolved_version = version;
                }

                let mut discovered_devices = self.discovered_devices.write().unwrap();

                if discovered_devices.contains_key(&device.id) {
                    if discovered_devices[&device.id] != device_connection_info {
                        info!("Device {:} already exist, updating...", &device.name);
                        self.clone().add_discovered_device(device.clone());
                    }
                } else {
                    info!("Device {:} discovered", &device.name);
                    Arc::clone(&self).add_discovered_device(device.clone());
                }

                discovered_devices.insert(device.id.clone(), device_connection_info.clone());

                DISCOVERED_DEVICES
                    .get()
                    .unwrap()
                    .write()
                    .unwrap()
                    .insert(device.id.clone(), device_connection_info.clone());
            }
            Some(Content::OfflineDeviceId(device_id)) => {
                self.discovered_devices.write().unwrap().remove(&device_id);
                self.peers
                    .write()
                    .unwrap()
                    .retain(|_, state| state.device_id.as_deref() != Some(&device_id));
                self.remove_discovered_device(device_id);
            }
        };
    }

    /// Called by the native layer for every advertisement packet observed.
    /// Updates the peer's last-seen time (the liveness heartbeat) and returns
    /// whether the native layer should perform a GATT connect+read.
    ///
    /// Returns `false` for peers we have already resolved or attempted very
    /// recently, which is what eliminates the connect-on-every-packet flood.
    pub fn should_connect(
        self: Arc<Self>,
        token: Option<String>,
        device_identifier: String,
    ) -> bool {
        // The stable device token is the liveness key. The version (if any) tells
        // us whether already-resolved data has changed. When there's no advertised
        // token (e.g. backgrounded iOS) we fall back to the platform identifier
        // and have no version, so such a peer is resolved exactly once.
        let (key, version) = match token {
            Some(token) => split_advertised_token(&token),
            None => (device_identifier, None),
        };
        let now = Instant::now();

        let mut peers = self.peers.write().unwrap();
        let entry = peers.entry(key).or_insert_with(|| PeerState {
            last_seen: now,
            last_attempt: None,
            device_id: None,
            resolved: false,
            resolved_version: None,
        });

        entry.last_seen = now;

        if entry.resolved {
            // Already resolved; only re-read if the advertised data version
            // changed (the peer's connection info was updated).
            let version_changed = version.is_some() && entry.resolved_version != version;
            if !version_changed {
                return false;
            }
        }

        if let Some(last_attempt) = entry.last_attempt {
            if now.duration_since(last_attempt) < READ_ATTEMPT_COOLDOWN {
                return false;
            }
        }

        entry.last_attempt = Some(now);
        return true;
    }

    /// Removes peers that have not been seen in the advertisement stream within
    /// `ttl_seconds`, firing `device_removed` for any that were resolved. The
    /// native layer should call this periodically (e.g. every couple seconds).
    pub fn expire_devices(self: Arc<Self>, ttl_seconds: u64) {
        let ttl = Duration::from_secs(ttl_seconds);
        let now = Instant::now();

        let mut expired_device_ids: Vec<String> = Vec::new();
        {
            let mut peers = self.peers.write().unwrap();
            peers.retain(|_, state| {
                let alive = now.duration_since(state.last_seen) < ttl;
                if !alive {
                    if let Some(device_id) = &state.device_id {
                        expired_device_ids.push(device_id.clone());
                    }
                }
                alive
            });
        }

        if expired_device_ids.is_empty() {
            return;
        }

        {
            let mut discovered_devices = self.discovered_devices.write().unwrap();
            let mut global = DISCOVERED_DEVICES.get().unwrap().write().unwrap();
            for device_id in &expired_device_ids {
                discovered_devices.remove(device_id);
                global.remove(device_id);
            }
        }

        for device_id in expired_device_ids {
            info!("Device {:} expired (not seen within TTL)", device_id);
            self.clone().remove_discovered_device(device_id);
        }
    }

    fn add_discovered_device(self: Arc<Self>, device: Device) {
        let delegates = DELEGATES
            .get()
            .unwrap()
            .read()
            .expect("Failed to read delegates");

        for values in delegates.values() {
            values.device_added(device.clone());
        }

        // if let Some(discovery_delegate) = &self.discovery_delegate {
        //     discovery_delegate.read().expect("Failed to lock discovery_delegate").device_added(device);
        // }
    }

    fn remove_discovered_device(self: Arc<Self>, device_id: String) {
        // if let Some(discovery_delegate) = &self.discovery_delegate {
        //     discovery_delegate.read().expect("Failed to lock discovery_delegate").device_removed(device_id);
        // }
        let delegates = DELEGATES
            .get()
            .unwrap()
            .read()
            .expect("Failed to read delegates");

        for values in delegates.values() {
            values.device_removed(device_id.clone());
        }
    }
}
