use crate::discovery::InternalDiscovery;
use crate::{BLE_DISCOVERY_CHARACTERISTIC_UUID, BLE_SERVICE_UUID};
use log::{error, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::runtime::Handle;
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::{
    core::{Result, GUID},
    Devices::Bluetooth::{
        Advertisement::{
            BluetoothLEAdvertisementFilter, BluetoothLEAdvertisementReceivedEventArgs,
            BluetoothLEAdvertisementWatcher, BluetoothLEAdvertisementWatcherStatus,
            BluetoothLEScanningMode,
        },
        BluetoothLEDevice,
        GenericAttributeProfile::GattCommunicationStatus,
    },
    Foundation::TypedEventHandler,
    Storage::Streams::DataReader,
};

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

        let discovered_devices = Arc::new(Mutex::new(Vec::new()));
        let discovered_devices_clone = discovered_devices.clone();
        let internal_discovery_clone = internal_discovery.clone();

        let handler = TypedEventHandler::new(
            move |_: &Option<BluetoothLEAdvertisementWatcher>,
                  args: &Option<BluetoothLEAdvertisementReceivedEventArgs>| {
                let args = args.as_ref().unwrap();
                let ble_address = args.BluetoothAddress()?;
                let discovered_devices = discovered_devices_clone.clone();
                let internal_discovery = internal_discovery_clone.clone();

                let advertisement = args.Advertisement()?;
                let local_name = advertisement.LocalName()?.to_string();

                info!("Discovered device: {}", local_name);

                let mut devices = discovered_devices.lock().unwrap();
                devices.push(ble_address);

                runtime_handle.spawn(async move {
                    if let Err(e) = Self::connect_and_read_characteristic(
                        ble_address,
                        internal_discovery,
                        local_name,
                    )
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

        // Wait until scanning is stopped
        while scanning.load(Ordering::Relaxed) {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
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
        device_name: String,
    ) -> Result<()> {
        // Connect to the device
        let device = BluetoothLEDevice::FromBluetoothAddressAsync(ble_address)?.get()?;
        let device_id = device.DeviceId()?.to_string();
        info!(
            "Found device with name: \"{:?}\" ID: {:?}",
            device_name, device_id
        );

        // Get the GATT services
        let services_result = device
            .GetGattServicesForUuidAsync(GUID::from(BLE_SERVICE_UUID))?
            .get()?;
        if services_result.Status()? != GattCommunicationStatus::Success {
            error!(
                "[{}, {:?}] Failed to get GATT services",
                device_name, device_id
            );
            return Ok(());
        }
        let services = services_result.Services()?;

        if services.Size()? == 0 {
            error!("[{}, {:?}] No services found", device_name, device_id);
            return Ok(());
        }

        let service = services.GetAt(0)?;

        // Get the characteristics
        let characteristics_result = service
            .GetCharacteristicsForUuidAsync(GUID::from(BLE_DISCOVERY_CHARACTERISTIC_UUID))?
            .get()?;
        if characteristics_result.Status()? != GattCommunicationStatus::Success {
            error!(
                "[{}, {:?}] Failed to get characteristics",
                device_name, device_id
            );
            return Ok(());
        }
        let characteristics = characteristics_result.Characteristics()?;

        if characteristics.Size()? == 0 {
            error!(
                "[{}, {:?}] No characteristics found",
                device_name, device_id
            );
            return Ok(());
        }

        let characteristic = characteristics.GetAt(0)?;

        // Read the characteristic value
        let read_result = characteristic.ReadValueAsync()?.get()?;
        if read_result.Status()? != GattCommunicationStatus::Success {
            error!(
                "[{}, {:?}] Failed to read characteristic",
                device_name, device_id
            );
            return Ok(());
        }
        let value = read_result.Value()?;
        let reader = DataReader::FromBuffer(&value)?;
        let length = reader.UnconsumedBufferLength()? as usize;
        let mut buffer = vec![0u8; length];
        reader.ReadBytes(&mut buffer)?;

        internal_discovery.parse_discovery_message(buffer, Some(device_id));

        Ok(())
    }
}
