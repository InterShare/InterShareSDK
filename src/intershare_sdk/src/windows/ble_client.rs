use crate::discovery::InternalDiscovery;
use crate::{
    BLE_DEVICE_TTL_SECONDS, BLE_DISCOVERY_CHARACTERISTIC_UUID, BLE_MANUFACTURER_ID, BLE_SERVICE_UUID,
};
use log::{error, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::runtime::Handle;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::{
    core::{Result, GUID},
    Devices::Bluetooth::{
        Advertisement::{
            BluetoothLEAdvertisement, BluetoothLEAdvertisementFilter,
            BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
            BluetoothLEAdvertisementWatcherStatus, BluetoothLEScanningMode,
        },
        BluetoothCacheMode, BluetoothLEDevice,
        GenericAttributeProfile::GattCommunicationStatus,
    },
    Foundation::TypedEventHandler,
    Storage::Streams::DataReader,
};

/// Extracts the advertised correlation token: the local name if present
/// (iOS/macOS peers), otherwise our manufacturer-specific data (Android peers).
fn extract_token(advertisement: &BluetoothLEAdvertisement) -> Option<String> {
    if let Ok(name) = advertisement.LocalName() {
        let name = name.to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }

    if let Ok(manufacturer_sections) = advertisement.ManufacturerData() {
        if let Ok(size) = manufacturer_sections.Size() {
            for index in 0..size {
                let Ok(section) = manufacturer_sections.GetAt(index) else {
                    continue;
                };

                if section.CompanyId().ok() != Some(BLE_MANUFACTURER_ID) {
                    continue;
                }

                let Ok(buffer) = section.Data() else { continue };
                let Ok(reader) = DataReader::FromBuffer(&buffer) else {
                    continue;
                };
                let length = reader.UnconsumedBufferLength().unwrap_or(0) as usize;
                let mut bytes = vec![0u8; length];
                if reader.ReadBytes(&mut bytes).is_ok() {
                    if let Ok(token) = String::from_utf8(bytes) {
                        return Some(token);
                    }
                }
            }
        }
    }

    None
}

impl InternalDiscovery {
    pub(crate) fn windows_start_scanning(self: Arc<Self>) {
        let scanning = self.scanning.clone();
        let self_copy = self.clone();

        scanning.store(true, Ordering::Relaxed);

        std::thread::spawn(move || {
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED).unwrap();
            }

            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();

            let handle = rt.handle().clone();

            rt.block_on(async {
                if let Err(e) = Self::scan_and_connect(self_copy, scanning, handle).await {
                    error!("Error during scanning: {:?}", e);
                }
            });
        });
    }

    pub(crate) fn windows_stop_scanning(&self) {
        self.scanning.store(false, Ordering::Relaxed);
    }
}

impl InternalDiscovery {
    async fn scan_and_connect(
        internal_discovery: Arc<Self>,
        scanning: Arc<AtomicBool>,
        runtime_handle: Handle,
    ) -> Result<()> {
        let watcher = BluetoothLEAdvertisementWatcher::new()?;

        // Set up the filter for the service UUID
        let filter = BluetoothLEAdvertisementFilter::new()?;
        filter
            .Advertisement()?
            .ServiceUuids()?
            .Append(GUID::from(BLE_SERVICE_UUID))?;
        watcher.SetAdvertisementFilter(&filter)?;

        watcher.SetScanningMode(BluetoothLEScanningMode::Active)?;

        let internal_discovery_clone = internal_discovery.clone();

        let handler = TypedEventHandler::new(
            move |_: &Option<BluetoothLEAdvertisementWatcher>,
                  args: &Option<BluetoothLEAdvertisementReceivedEventArgs>| {
                let args = args.as_ref().unwrap();
                let ble_address = args.BluetoothAddress()?;
                let internal_discovery = internal_discovery_clone.clone();

                let advertisement = args.Advertisement()?;
                let token = extract_token(&advertisement);

                // Record the advertisement heartbeat and only connect+read when
                // the SDK says this peer is not yet resolved. This replaces the
                // previous behavior of spawning a fresh GATT connection on every
                // single advertisement packet.
                if !internal_discovery
                    .clone()
                    .should_connect(token.clone(), ble_address.to_string())
                {
                    return Ok(());
                }

                info!("Connecting to advertised device: {:?}", token);

                runtime_handle.spawn(async move {
                    if let Err(e) =
                        Self::connect_and_read_characteristic(ble_address, internal_discovery, token)
                            .await
                    {
                        error!("Error connecting to device: {:?}", e);
                    }
                });

                Ok(())
            },
        );

        watcher.Received(&handler)?;
        watcher.Start()?;

        // Wait until scanning is stopped, sweeping for departed devices each tick.
        while scanning.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            internal_discovery.clone().expire_devices(BLE_DEVICE_TTL_SECONDS);
        }

        if watcher.Status()? == BluetoothLEAdvertisementWatcherStatus::Started {
            watcher.Stop()?;
            info!("Stopped BLE advertisement watcher");
        }

        Ok(())
    }

    async fn connect_and_read_characteristic(
        ble_address: u64,
        internal_discovery: Arc<Self>,
        token: Option<String>,
    ) -> Result<()> {
        // Connect to the device
        let device = BluetoothLEDevice::FromBluetoothAddressAsync(ble_address)?.get()?;
        let device_id = device.DeviceId()?.to_string();
        info!("Found device ID: {:?} (token {:?})", device_id, token);

        // Use uncached reads so a peer that restarted (new port/PSM, or it came
        // back after going away) is not served stale GATT data from the OS cache.
        let services_result = device
            .GetGattServicesForUuidWithCacheModeAsync(
                GUID::from(BLE_SERVICE_UUID),
                BluetoothCacheMode::Uncached,
            )?
            .get()?;
        if services_result.Status()? != GattCommunicationStatus::Success {
            error!("[{:?}] Failed to get GATT services", device_id);
            return Ok(());
        }
        let services = services_result.Services()?;

        if services.Size()? == 0 {
            error!("[{:?}] No services found", device_id);
            return Ok(());
        }

        let service = services.GetAt(0)?;

        // Get the characteristics
        let characteristics_result = service
            .GetCharacteristicsForUuidWithCacheModeAsync(
                GUID::from(BLE_DISCOVERY_CHARACTERISTIC_UUID),
                BluetoothCacheMode::Uncached,
            )?
            .get()?;
        if characteristics_result.Status()? != GattCommunicationStatus::Success {
            error!("[{:?}] Failed to get characteristics", device_id);
            return Ok(());
        }
        let characteristics = characteristics_result.Characteristics()?;

        if characteristics.Size()? == 0 {
            error!("[{:?}] No characteristics found", device_id);
            return Ok(());
        }

        let characteristic = characteristics.GetAt(0)?;

        // Read the characteristic value
        let read_result = characteristic
            .ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached)?
            .get()?;
        if read_result.Status()? != GattCommunicationStatus::Success {
            error!("[{:?}] Failed to read characteristic", device_id);
            return Ok(());
        }
        let value = read_result.Value()?;
        let reader = DataReader::FromBuffer(&value)?;
        let length = reader.UnconsumedBufferLength()? as usize;
        let mut buffer = vec![0u8; length];
        reader.ReadBytes(&mut buffer)?;

        // Use the BLE address (the same value passed to `should_connect`) as the
        // identifier so the liveness key stays consistent for token-less peers.
        internal_discovery.parse_discovery_message(buffer, Some(ble_address.to_string()), token);

        Ok(())
    }
}
