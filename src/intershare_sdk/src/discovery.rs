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

    pub fn parse_discovery_message(self: Arc<Self>, data: Vec<u8>, ble_uuid: Option<String>) {
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

                if let Some(ble_uuid) = ble_uuid {
                    if let Some(mut ble_info) = device_connection_info.ble {
                        ble_info.uuid = ble_uuid;
                        device_connection_info.ble = Some(ble_info);
                    }
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
                self.remove_discovered_device(device_id);
            }
        };
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
