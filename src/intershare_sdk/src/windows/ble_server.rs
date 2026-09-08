use crate::nearby_server::InternalNearbyServer;
use crate::{BLE_DISCOVERY_CHARACTERISTIC_UUID, BLE_SERVICE_UUID};
use log::{error, info, warn};
use protocol::discovery::device_discovery_message::Content;
use protocol::discovery::DeviceDiscoveryMessage;
use protocol::prost::Message;
use windows::{
    core::{Result as WinResult, GUID},
    Devices::Bluetooth::GenericAttributeProfile::*,
    Foundation::TypedEventHandler,
    Storage::Streams::*,
};

// Constants for optimized advertising
const MAX_ADVERTISING_RETRIES: u32 = 3;
const ADVERTISING_RETRY_DELAY_MS: u64 = 1000;

impl InternalNearbyServer {
    pub(crate) async fn setup_gatt_server(&self) -> WinResult<GattServiceProvider> {
        let service_uuid = GUID::from(BLE_SERVICE_UUID);

        let service_provider_result: GattServiceProviderResult =
            GattServiceProvider::CreateAsync(service_uuid)?.get()?;
        let gatt_service_provider = service_provider_result.ServiceProvider()?;

        let characteristic_uuid = GUID::from(BLE_DISCOVERY_CHARACTERISTIC_UUID);

        let characteristic_parameters = GattLocalCharacteristicParameters::new()?;
        characteristic_parameters
            .SetCharacteristicProperties(GattCharacteristicProperties::Read)?;

        characteristic_parameters.SetReadProtectionLevel(GattProtectionLevel::Plain)?;

        // Seed the shared advertisement payload from the current device info.
        // We intentionally do NOT call SetStaticValue: when a static value is
        // set, Windows answers reads itself and never raises ReadRequested, which
        // would freeze the advertised data at setup time. The dynamic handler
        // below serves the always-current payload instead.
        let initial_value = self.windows_current_advertisement_payload().await;
        *self
            .advertised_payload
            .write()
            .expect("Failed to lock advertised_payload") = initial_value;

        let characteristic_result: GattLocalCharacteristicResult = gatt_service_provider
            .Service()?
            .CreateCharacteristicAsync(characteristic_uuid, &characteristic_parameters)?
            .get()?;

        let gatt_characteristic = characteristic_result.Characteristic()?;

        // The handler reads the always-current payload rather than a one-time
        // snapshot, so values stay correct after `change_device`.
        let advertised_payload = self.advertised_payload.clone();
        let read_requested_handler = TypedEventHandler::new(
            move |_sender: &Option<GattLocalCharacteristic>,
                  args: &Option<GattReadRequestedEventArgs>| {
                if let Some(args) = args {
                    let deferral = args.GetDeferral()?;
                    let request: GattReadRequest = args.GetRequestAsync()?.get()?;

                    let value = advertised_payload
                        .read()
                        .map(|payload| payload.clone())
                        .unwrap_or_default();

                    let writer = DataWriter::new()?;
                    writer.WriteBytes(&value)?;
                    let buffer = writer.DetachBuffer()?;
                    request.RespondWithValue(&buffer)?;
                    deferral.Complete()?;
                }
                Ok(())
            },
        );

        gatt_characteristic.ReadRequested(&read_requested_handler)?;

        return Ok(gatt_service_provider);
    }

    /// Encodes the current device connection info into a discovery payload.
    async fn windows_current_advertisement_payload(&self) -> Vec<u8> {
        let device_connection_info = self.device_connection_info.read().await.clone();
        DeviceDiscoveryMessage {
            content: Some(Content::DeviceConnectionInfo(device_connection_info)),
        }
        .encode_length_delimited_to_vec()
    }

    /// Recomputes and stores the shared advertisement payload from the current
    /// device info (sync entry point used by `change_device`).
    pub(crate) fn windows_refresh_advertised_payload(&self) {
        let device_connection_info = self.device_connection_info.blocking_read().clone();
        let payload = DeviceDiscoveryMessage {
            content: Some(Content::DeviceConnectionInfo(device_connection_info)),
        }
        .encode_length_delimited_to_vec();

        if let Ok(mut stored) = self.advertised_payload.write() {
            *stored = payload;
        }
    }

    pub(crate) async fn start_windows_server(&self) {
        let gatt = match self.setup_gatt_server().await {
            Ok(p) => {
                info!("Successfully created GATT service provider");
                p
            }
            Err(e) => {
                error!("Failed to start GATT server: {:?}", e);
                return;
            }
        };

        let adv_parameters = match GattServiceProviderAdvertisingParameters::new() {
            Ok(params) => params,
            Err(e) => {
                error!("Failed to create advertising parameters: {:?}", e);
                return;
            }
        };
        if let Err(e) = adv_parameters.SetIsConnectable(true) {
            error!("...{:?}", e);
            return;
        }
        if let Err(e) = adv_parameters.SetIsDiscoverable(true) {
            error!("...{:?}", e);
            return;
        }

        let mut retry_count = 0;
        while retry_count < MAX_ADVERTISING_RETRIES {
            match gatt.StartAdvertisingWithParameters(&adv_parameters) {
                Ok(_) => {
                    info!("Successfully started optimized BLE advertising");
                    {
                        let mut w = self
                            .gatt_service_provider
                            .write()
                            .expect("Failed to lock GattServiceProvider");
                        *w = Some(gatt.clone());
                    }
                    return;
                }
                Err(e) => {
                    retry_count += 1;
                    warn!("Advertising attempt {} failed: {:?}", retry_count, e);
                    if retry_count < MAX_ADVERTISING_RETRIES {
                        let delay = ADVERTISING_RETRY_DELAY_MS * retry_count as u64;
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                    }
                }
            }
        }
        error!(
            "Failed to start BLE advertising after {} attempts",
            MAX_ADVERTISING_RETRIES
        );
    }

    pub(crate) fn stop_windows_server(&self) {
        let gatt_service_provider = self
            .gatt_service_provider
            .read()
            .expect("Failed to lock GattServiceProvider");

        if let Some(gatt_service_provider) = gatt_service_provider.as_ref() {
            match gatt_service_provider.StopAdvertising() {
                Ok(_) => info!("Successfully stopped BLE advertising"),
                Err(e) => error!("Failed to stop advertising: {:?}", e),
            }
        }
    }
}
