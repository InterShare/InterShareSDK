use crate::stream::NativeStreamDelegate;
use std::fmt::Debug;
use std::io::{Read, Write};
use std::sync::Arc;
use local_ip_address::local_ip;
use log::{error, info};
use prost_stream::Stream;
use protocol::communication::request::RequestTypes;
use protocol::communication::Request;
use protocol::discovery::{BluetoothLeConnectionInfo, Device, DeviceConnectionInfo, DeviceDiscoveryMessage, TcpConnectionInfo};
use tokio::runtime::Handle;
use tokio::sync::RwLock;
use url::Url;
use protocol::discovery::device_discovery_message::Content;
use crate::encryption::initiate_receiver_communication;
use crate::connection::Connection;
use crate::connection_request::ConnectionRequest;
use crate::errors::RequestConvenienceShareErrors;
use crate::share_store::ShareStore;
use crate::{init_logger, PROTOCOL_VERSION};
use crate::stream::Close;
use crate::transmission::tcp::TcpServer;
use protocol::prost::Message;

#[cfg(target_os="windows")]
use windows::Devices::Bluetooth::GenericAttributeProfile::*;

#[uniffi::export(callback_interface)]
pub trait BleServerImplementationDelegate: Send + Sync + Debug {
    fn start_server(&self);
    fn stop_server(&self);
}

#[uniffi::export(callback_interface)]
pub trait L2CapDelegate: Send + Sync + Debug {
    fn open_l2cap_connection(&self, connection_id: String, peripheral_uuid: String, psm: u32);
}

#[derive(PartialEq)]
pub enum ConnectionIntentType {
    FileTransfer,
    Clipboard
}

#[uniffi::export(callback_interface)]
pub trait NearbyConnectionDelegate: Send + Sync + Debug {
    fn received_connection_request(&self, request: Arc<ConnectionRequest>);
}

#[uniffi::export(callback_interface)]
pub trait NearbyInstantReceiveDelegate: Send + Sync + Debug {
    fn requested_instant_file_receive(&self, device: Device, request_id: String) -> bool;
}

pub struct CurrentShareStore {
    pub request_id: String,
    pub file_paths: Option<Vec<String>>,
    pub clipboard: Option<String>
}

#[derive(uniffi::Object)]
pub struct InternalNearbyServer {
    pub(crate) tcp_server: RwLock<Option<TcpServer>>,
    ble_server_implementation: RwLock<Option<Box<dyn BleServerImplementationDelegate>>>,
    ble_l2_cap_client: Arc<RwLock<Option<Box<dyn L2CapDelegate>>>>,
    pub advertise: RwLock<bool>,
    file_storage: String,
    pub device_connection_info: RwLock<DeviceConnectionInfo>,
    nearby_connection_delegate: Option<Arc<RwLock<Box<dyn NearbyConnectionDelegate>>>>,
    pub(crate) current_share_store: Arc<RwLock<Option<Arc<ShareStore>>>>,

    #[cfg(target_os="windows")]
    pub(crate) gatt_service_provider: std::sync::RwLock<Option<GattServiceProvider>>,

    requested_download_id: Arc<RwLock<Option<String>>>
}

#[uniffi::export(async_runtime = "tokio")]
impl InternalNearbyServer {
    #[uniffi::constructor]
    pub fn new(my_device: Device, file_storage: String, delegate: Option<Box<dyn NearbyConnectionDelegate>>) -> Self {
        init_logger();

        let mut my_device = my_device.clone();
        my_device.protocol_version = Some(PROTOCOL_VERSION);

        let device_connection_info = DeviceConnectionInfo {
            device: Some(my_device),
            ble: None,
            tcp: None
        };

        let nearby_connection_delegate = match delegate {
            Some(d) => Some(Arc::new(RwLock::new(d))),
            None => None
        };

        return Self {
            tcp_server: RwLock::new(None),
            ble_server_implementation: RwLock::new(None),
            ble_l2_cap_client: Arc::new(RwLock::new(None)),
            advertise: RwLock::new(false),
            file_storage,
            device_connection_info: RwLock::new(device_connection_info),
            nearby_connection_delegate,
            current_share_store: Arc::new(RwLock::new(None)),

            #[cfg(target_os="windows")]
            gatt_service_provider: std::sync::RwLock::new(None),

            requested_download_id: Arc::new(RwLock::new(None))
        };
    }

    pub fn add_l2_cap_client(&self, delegate: Box<dyn L2CapDelegate>) {
        *self.ble_l2_cap_client.blocking_write() = Some(delegate);
    }

    pub fn add_bluetooth_implementation(&self, implementation: Box<dyn BleServerImplementationDelegate>) {
        *self.ble_server_implementation.blocking_write() = Some(implementation)
    }

    pub async fn get_advertisement_data(&self) -> Vec<u8> {
        if *self.advertise.read().await {
            return DeviceDiscoveryMessage {
                content: Some(
                    Content::DeviceConnectionInfo(
                        self.device_connection_info.read().await.clone()
                    )
                ),
            }.encode_length_delimited_to_vec();

            // self.mut_variables.write().await.discovery_message = message;
        } else {
            // return DeviceDiscoveryMessage {
            //     content: Some(
            //         Content::OfflineDeviceId(
            //             self.handler.variables
            //                 .read()
            //                 .await
            //                 .device_connection_info.device?.id.clone()
            //         )
            //     ),
            // }.encode_length_delimited_to_vec();
        }

        return vec![];
    }

    pub fn change_device(&self, new_device: Device) {
        let mut device = new_device.clone();
        device.protocol_version = Some(PROTOCOL_VERSION);
        self.device_connection_info.blocking_write().device = Some(device);
    }

    pub fn set_bluetooth_le_details(&self, ble_info: BluetoothLeConnectionInfo) {
        self.device_connection_info.blocking_write().ble = Some(ble_info)
    }

    pub fn set_tcp_details(&self, tcp_info: TcpConnectionInfo) {
        self.device_connection_info.blocking_write().tcp = Some(tcp_info)
    }

    pub fn get_current_ip(&self) -> Option<String> {
        let ip = local_ip();
        if let Ok(my_local_ip) = ip {
            return Some(my_local_ip.to_string());
        }
        else if let Err(error) = ip {
            info!("Unable to obtain IP address: {:?}", error);
        }

        return None;
    }

    /// https://share.intershare.app?id=hgf8o47fdsb394mv385&ip=192.168.12.13&port=5200&device_id=9A403351-A926-4D1C-855F-432A6ED51E0E&protocol_version=1
    pub async fn request_download(&self, link: String) -> Result<(), RequestConvenienceShareErrors> {
        let parsed_url = Url::parse(&link)
            .map_err(|_| RequestConvenienceShareErrors::NotAValidLink)?;

        if parsed_url.host_str() != Some("s.intershare.app") {
            error!("Invalid host: {:?}", parsed_url.host_str());
            return Err(RequestConvenienceShareErrors::NotAValidLink);
        }

        let id = parsed_url
            .query_pairs()
            .find(|(key, _)| key == "i")
            .map(|(_, value)| value)
            .filter(|val| !val.is_empty())
            .ok_or(RequestConvenienceShareErrors::NotAValidLink)
            ?.to_string();

        let ip = parsed_url
            .query_pairs()
            .find(|(key, _)| key == "ip")
            .map(|(_, value)| value)
            .filter(|val| !val.is_empty())
            .ok_or(RequestConvenienceShareErrors::NotAValidLink)
            ?.to_string();

        let port = parsed_url
            .query_pairs()
            .find(|(key, _)| key == "p")
            .map(|(_, value)| value)
            .filter(|val| !val.is_empty())
            .ok_or(RequestConvenienceShareErrors::NotAValidLink)?
            .parse::<u32>()
            .map_err(|_| RequestConvenienceShareErrors::NotAValidLink)?;

        // let device_id = parsed_url
        //     .query_pairs()
        //     .find(|(key, _)| key == "d")
        //     .map(|(_, value)| value)
        //     .filter(|val| !val.is_empty())
        //     .ok_or(RequestConvenienceShareErrors::NotAValidLink)
        //     ?.to_string();

        // let protocol_version = parsed_url
        //     .query_pairs()
        //     .find(|(key, _)| key == "v")
        //     .map(|(_, value)| value)
        //     .filter(|val| !val.is_empty())
        //     .ok_or(RequestConvenienceShareErrors::NotAValidLink)
        //     ?.to_string();


        let connection = Connection::new(self.ble_l2_cap_client.clone());

        let connection_details = DeviceConnectionInfo {
            device: None,
            tcp: Some(TcpConnectionInfo {
                hostname: ip,
                port
            }),
            ble: None,
        };

        let mut encrypted_stream = match connection.connect_tcp(&connection_details).await {
            Ok(connection) => connection,
            Err(err) => {
                error!("Error while trying to connect: {:?}", err);
                return Err(RequestConvenienceShareErrors::FailedToConnect { error: err.to_string() });
            }
        };

        let request = Request {
            r#type: RequestTypes::ConvenienceDownloadRequest as i32,
            device: self.device_connection_info.read().await.device.clone(),
            share_id: Some(id.clone()),
            intent: None
        };

        *self.requested_download_id.write().await = Some(id);

        let mut proto_stream = Stream::new(&mut encrypted_stream);
        let _ = proto_stream.send(&request);

        return Ok(());
    }

    pub async fn start(&self) {
        if self.tcp_server.read().await.is_none() {
            let delegate = self.nearby_connection_delegate.clone();

            let Some(delegate) = delegate else {
                return;
            };

            let file_storage = self.file_storage.clone();
            let tcp_server = self.new_tcp_server(delegate, file_storage).await;

            if let Ok(tcp_server) = tcp_server {
                let ip = self.get_current_ip();

                if let Some(my_local_ip) = ip {
                    info!("IP: {}", my_local_ip);
                    info!("Port: {}", tcp_server.port);

                    let port = tcp_server.port.clone();
                    *self.tcp_server.write().await = Some(tcp_server);

                    self.start_loop().await;

                    self.device_connection_info.write().await.tcp = Some(TcpConnectionInfo {
                        hostname: my_local_ip,
                        port: port as u32,
                    });
                }
            } else if let Err(error) = tcp_server {
                error!("Error trying to start TCP server: {:?}", error);
            }
        }

        *self.advertise.write().await = true;

        #[cfg(target_os="windows")]
        {
            self.start_windows_server().await;
        }

        #[cfg(not(target_os="windows"))]
        {
            if let Some(ble_advertisement_implementation) = &*self.ble_server_implementation.read().await {
                ble_advertisement_implementation.start_server();
            };
        }
    }

    pub async fn restart_server(&self) {
        self.stop().await;
        self.start().await;
    }

    pub async fn share_text(&self, text: String, allow_convenience_share: bool) -> Arc<ShareStore> {

        let share_store = Arc::new(ShareStore::new(
            None,
            Some(text),
            allow_convenience_share,
            self.ble_l2_cap_client.clone(),
            self.device_connection_info.read().await.clone()
        ));

        *self.current_share_store.write().await = Some(share_store.clone());

        return share_store;
    }

    pub fn handle_incoming_connection(&self, native_stream_handle: Box<dyn NativeStreamDelegate>) {
        self.handle_incoming_connection_generic(native_stream_handle);
    }

    pub async fn share_files(&self, file_paths: Vec<String>, allow_convenience_share: bool) -> Arc<ShareStore> {
        let share_store = Arc::new(ShareStore::new(
            Some(file_paths),
            None,
            allow_convenience_share,
            self.ble_l2_cap_client.clone(),
            self.device_connection_info.read().await.clone()
        ));

        *self.current_share_store.write().await = Some(share_store.clone());

        return share_store
    }

    // pub(crate) async fn received_convenience_download_request(request: Request, current_share_store: Arc<RwLock<Option<Arc<ShareStore>>>>) {
    //     let Some(current_share_store) = &*current_share_store.read().await else {
    //         return;
    //     };
    //
    //     if request.share_id != Some(current_share_store.request_id.clone()) {
    //         warn!("Received convenience download request, but wrong with a wrong ID. Expected: {}, but received {:?}", current_share_store.request_id.clone(), request.share_id());
    //         return;
    //     }
    //
    //     let Some(device) = request.device else {
    //         return;
    //     };
    //
    //     let _ = current_share_store.send_to(device, None).await;
    // }

    pub async fn stop(&self) {
        *self.advertise.write().await = false;
        self.stop_tcp_server().await;

        *self.tcp_server.write().await = None;

        #[cfg(target_os="windows")]
        self.stop_windows_server();

        #[cfg(not(target_os="windows"))]
        if let Some(ble_advertisement_implementation) = &*self.ble_server_implementation.blocking_read() {
            ble_advertisement_implementation.stop_server();
        }
    }

    pub fn get_device_name(&self) -> Option<String> {
        let device = self.device_connection_info.blocking_read().device.clone();
        return Some(device?.name)
    }
}

impl InternalNearbyServer {
    fn handle_incoming_connection_generic<T>(&self, native_stream_handle: T) where T: Read + Write + Send + Close + 'static {
        let delegate = self.nearby_connection_delegate.clone();

        let Some(delegate) = delegate else {
            return;
        };

        let file_storage = self.file_storage.clone();
        // let current_share_store = self.current_share_store.clone();

        if Handle::try_current().is_err() {
            // Create a new runtime if one doesn't exist
            let rt = tokio::runtime::Runtime::new().expect("Failed to create Tokio runtime");
            rt.spawn(async move {
                Self::process_incoming_connection(native_stream_handle, delegate, file_storage).await;
            });
        } else {
            // Already in a Tokio runtime
            tokio::spawn(async move {
                Self::process_incoming_connection(native_stream_handle, delegate, file_storage).await;
            });
        }
    }


    async fn process_incoming_connection<T>(
        native_stream_handle: T,
        delegate: Arc<RwLock<Box<dyn NearbyConnectionDelegate>>>,
        file_storage: String,
    ) where
        T: Read + Write + Send + Close + 'static,
    {
        let mut encrypted_stream = match initiate_receiver_communication(native_stream_handle) {
            Ok(request) => request,
            Err(error) => {
                error!("Encryption error {:}", error);
                return;
            }
        };

        info!("Received encrypted connection request.");

        let mut prost_stream = Stream::new(&mut encrypted_stream);
        let request = match prost_stream.recv::<Request>() {
            Ok(message) => message,
            Err(error) => {
                error!("Error {:}", error);
                return;
            }
        };

        if request.r#type == RequestTypes::ShareRequest as i32 {
            let connection_request = ConnectionRequest::new(
                request,
                Box::new(encrypted_stream),
                file_storage.clone()
            );

            info!("Sending received_connection_request delegate.");
            delegate.read().await.received_connection_request(Arc::new(connection_request));
        } else {
            // NearbyServer::received_convenience_download_request(request, current_share_store).await;
        }
    }
}
