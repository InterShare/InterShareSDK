use windows::{
    core::{Result as WinResult, GUID},
    Devices::Bluetooth::GenericAttributeProfile::*,
    Foundation::TypedEventHandler,
    Storage::Streams::*,
};
use protocol::discovery::device_discovery_message::Content;
use protocol::discovery::DeviceDiscoveryMessage;
use protocol::prost::Message;
use crate::{BLE_DISCOVERY_CHARACTERISTIC_UUID, BLE_SERVICE_UUID};
use crate::nearby_server::InternalNearbyServer;
use log::{error, info, warn};

// Constants for optimized advertising
const MAX_ADVERTISING_RETRIES: u32 = 3;
const ADVERTISING_RETRY_DELAY_MS: u64 = 1000;

impl InternalNearbyServer {
    pub(crate) async fn setup_gatt_server(&self) -> WinResult<GattServiceProvider> {
        let service_uuid = GUID::from(BLE_SERVICE_UUID);

        let service_provider_result: GattServiceProviderResult = GattServiceProvider::CreateAsync(service_uuid)?.get()?;
        let gatt_service_provider = service_provider_result.ServiceProvider()?;

        let characteristic_uuid = GUID::from(BLE_DISCOVERY_CHARACTERISTIC_UUID);

        let characteristic_parameters = GattLocalCharacteristicParameters::new()?;
        characteristic_parameters.SetCharacteristicProperties(
            GattCharacteristicProperties::Read
        )?;

        characteristic_parameters.SetReadProtectionLevel(GattProtectionLevel::Plain)?;

        let characteristic_result: GattLocalCharacteristicResult = gatt_service_provider
            .Service()?
            .CreateCharacteristicAsync(characteristic_uuid, &characteristic_parameters)?
            .get()?;

        let gatt_characteristic = characteristic_result.Characteristic()?;
        let device_connection_info = self.device_connection_info.read().await.clone();

        let read_requested_handler = TypedEventHandler::new(
            move |_sender: &Option<GattLocalCharacteristic>, args: &Option<GattReadRequestedEventArgs>| {
                if let Some(args) = args {
                    let deferral = args.GetDeferral()?;
                    let request: GattReadRequest = args.GetRequestAsync()?.get()?;

                    let value = DeviceDiscoveryMessage {
                        content: Some(
                            Content::DeviceConnectionInfo(
                                device_connection_info.clone()
                            )
                        ),
                    }.encode_length_delimited_to_vec();

                    let writer = DataWriter::new()?;
                    writer.WriteBytes(&value)?;
                    let buffer = writer.DetachBuffer()?;
                    request.RespondWithValue(&buffer)?;
                    deferral.Complete()?;
                    
                    info!("Responded to GATT read request");
                }
                Ok(())
            },
        );

        gatt_characteristic.ReadRequested(&read_requested_handler)?;

        return Ok(gatt_service_provider);
    }
    
    
    pub(crate) async fn start_windows_server(&self) {
        let gatt_service_provider = match self.setup_gatt_server().await {
            Ok(provider) => {
                info!("Successfully created GATT service provider");
                provider
            }
            Err(e) => {
                error!("Failed to start GATT server: {:?}", e);
                return;
            }
        };

        let mut writable_gatt_service = self.gatt_service_provider
            .write()
            .expect("Failed to unwrap gatt_service_provider");

        let service_provider = writable_gatt_service.insert(gatt_service_provider);

        let adv_parameters = match GattServiceProviderAdvertisingParameters::new() {
            Ok(params) => params,
            Err(e) => {
                error!("Failed to create advertising parameters: {:?}", e);
                return;
            }
        };

        // Set optimized advertising parameters for maximum discoverability
        if let Err(e) = adv_parameters.SetIsConnectable(true) {
            error!("Failed to set IsConnectable: {:?}", e);
            return;
        }
        
        if let Err(e) = adv_parameters.SetIsDiscoverable(true) {
            error!("Failed to set IsDiscoverable: {:?}", e);
            return;
        }

        // Try to start advertising with optimized retry logic
        let mut retry_count = 0;

        while retry_count < MAX_ADVERTISING_RETRIES {
            match service_provider.StartAdvertisingWithParameters(&adv_parameters) {
                Ok(_) => {
                    info!("Successfully started optimized BLE advertising");
                    return;
                }
                Err(e) => {
                    retry_count += 1;
                    warn!("Advertising attempt {} failed: {:?}", retry_count, e);
                    
                    if retry_count < MAX_ADVERTISING_RETRIES {
                        // Exponential backoff for retry
                        let delay = ADVERTISING_RETRY_DELAY_MS * retry_count as u64;
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                    }
                }
            }
        }

        error!("Failed to start BLE advertising after {} attempts", MAX_ADVERTISING_RETRIES);
    }

    pub(crate) fn stop_windows_server(&self) {
        let gatt_service_provider = self.gatt_service_provider
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
