use crate::discovery::get_connection_details;
use crate::{
    encryption::initiate_sender_communication,
    encryption::{EncryptedReadWrite, TlsStream},
    errors::ConnectErrors,
    nearby_server::L2CapDelegate,
    share_store::{ConnectionMedium, SendProgressDelegate, SendProgressState},
    stream::NativeStreamDelegate,
    transmission::tcp::TcpClient,
};
use log::{error, info};
use protocol::discovery::{Device, DeviceConnectionInfo};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::ToSocketAddrs,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::{
    oneshot::{self, Sender},
    RwLock,
};
use uuid::Uuid;

/// How long to wait for the native layer to open the BLE L2CAP channel before
/// giving up, so a failed open never hangs the send indefinitely.
const L2CAP_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

static L2CAP_CONNECTIONS: OnceLock<RwLock<HashMap<String, Sender<Box<dyn NativeStreamDelegate>>>>> =
    OnceLock::new();

#[uniffi::export]
pub async fn handle_incoming_l2cap_connection(
    connection_id: String,
    native_stream: Box<dyn NativeStreamDelegate>,
) {
    info!("Received incomming L2CAP connection");

    let sender = L2CAP_CONNECTIONS
        .get_or_init(|| RwLock::new(HashMap::new()))
        .write()
        .await
        .remove(&connection_id);

    if let Some(sender) = sender {
        info!("Passing incoming L2CAP connection...");
        let _ = sender.send(native_stream);
    }
}

pub struct Connection {
    ble_l2_cap_client: Arc<RwLock<Option<Box<dyn L2CapDelegate>>>>,
}

fn update_progress(
    progress_delegate: &Option<Box<dyn SendProgressDelegate>>,
    progress: SendProgressState,
) {
    if let Some(progress_delegate) = progress_delegate {
        progress_delegate.progress_changed(progress);
    }
}

impl Connection {
    pub fn new(ble_l2_cap_client: Arc<RwLock<Option<Box<dyn L2CapDelegate>>>>) -> Self {
        return Self { ble_l2_cap_client };
    }

    async fn initiate_sender<T>(
        &self,
        remote_device_id: &str,
        server_name_hint: Option<&str>,
        raw_stream: T,
    ) -> Result<TlsStream<T, rustls::ClientConnection>, ConnectErrors>
    where
        T: Read + Write,
    {
        return Ok(
            match initiate_sender_communication(remote_device_id, server_name_hint, raw_stream) {
                Ok(stream) => stream,
                Err(error) => {
                    return Err(ConnectErrors::FailedToEncryptStream {
                        error: error.to_string(),
                    })
                }
            },
        );
    }

    pub async fn connect_tcp(
        &self,
        device: &Device,
        connection_details: &DeviceConnectionInfo,
    ) -> Result<Box<dyn EncryptedReadWrite>, ConnectErrors> {
        let Some(tcp_connection_details) = &connection_details.tcp else {
            return Err(ConnectErrors::FailedToGetTcpDetails);
        };

        let socket_string = format!(
            "{0}:{1}",
            tcp_connection_details.hostname, tcp_connection_details.port
        );
        info!("Connecting to: {}", socket_string);

        let socket_address = socket_string.to_socket_addrs();

        let Ok(socket_address) = socket_address else {
            error!("{}", socket_address.unwrap_err());
            return Err(ConnectErrors::FailedToGetSocketAddress);
        };

        let mut socket_address = socket_address.as_slice()[0].clone();
        socket_address.set_port(tcp_connection_details.port as u16);

        let raw_stream = TcpClient::connect(socket_address).map_err(|err| {
            ConnectErrors::FailedToOpenTcpStream {
                error: err.to_string(),
            }
        })?;

        let hostname = Some(tcp_connection_details.hostname.as_str());
        let encrypted_stream = self
            .initiate_sender(device.id.as_str(), hostname, raw_stream)
            .await?;
        return Ok(Box::new(encrypted_stream));
    }

    pub async fn connect(
        &self,
        device: Device,
        progress_delegate: &Option<Box<dyn SendProgressDelegate>>,
    ) -> Result<Box<dyn EncryptedReadWrite>, ConnectErrors> {
        L2CAP_CONNECTIONS.get_or_init(|| RwLock::new(HashMap::new()));

        let connection_details = get_connection_details(device.clone())
            .ok_or(ConnectErrors::FailedToGetConnectionDetails)?;

        let device_info = connection_details.device.as_ref().unwrap_or(&device);

        let encrypted_stream = self.connect_tcp(device_info, &connection_details).await;

        if let Ok(encrypted_stream) = encrypted_stream {
            update_progress(
                progress_delegate,
                SendProgressState::ConnectionMediumUpdate {
                    medium: ConnectionMedium::WiFi,
                },
            );

            return Ok(encrypted_stream);
        }

        info!("Could not connect via WiFi");

        if let Err(error) = encrypted_stream {
            error!("{}", error)
        }

        // Use BLE if TCP fails
        let ble_connection_details = &connection_details
            .ble
            .ok_or(ConnectErrors::FailedToGetBleDetails)?;

        info!("Trying BLE...");

        let bluetooth_l2cap_id = Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel::<Box<dyn NativeStreamDelegate>>();

        L2CAP_CONNECTIONS
            .get()
            .unwrap()
            .write()
            .await
            .insert(bluetooth_l2cap_id.clone(), sender);

        if let Some(ble_l2cap_client) = &*self.ble_l2_cap_client.read().await {
            info!("Requesting L2CAP connection...");
            ble_l2cap_client.open_l2cap_connection(
                bluetooth_l2cap_id.clone(),
                ble_connection_details.uuid.clone(),
                ble_connection_details.psm,
            );
        } else {
            L2CAP_CONNECTIONS
                .get()
                .unwrap()
                .write()
                .await
                .remove(&bluetooth_l2cap_id);
            return Err(ConnectErrors::InternalBleHandlerNotAvailable);
        }

        // Bound the wait without depending on a tokio reactor being present:
        // `send_to` is not necessarily driven by a tokio runtime, so a tokio
        // timer here would panic ("there is no reactor running"). Instead a
        // background thread drops the pending sender after the deadline, which
        // makes `receiver.await` resolve with an error rather than hang forever.
        let timeout_id = bluetooth_l2cap_id.clone();
        std::thread::spawn(move || {
            std::thread::sleep(L2CAP_CONNECT_TIMEOUT);
            if let Some(connections) = L2CAP_CONNECTIONS.get() {
                connections.blocking_write().remove(&timeout_id);
            }
        });

        let connection = match receiver.await {
            Ok(connection) => connection,
            Err(_) => {
                // Sender dropped: either the timeout fired or the native layer
                // failed to open the channel.
                error!("L2CAP channel did not open (timed out or failed)");
                L2CAP_CONNECTIONS
                    .get()
                    .unwrap()
                    .write()
                    .await
                    .remove(&bluetooth_l2cap_id);
                return Err(ConnectErrors::FailedToEstablishBleConnection);
            }
        };

        info!("Opened a L2CAP connection");

        let encrypted_stream = self
            .initiate_sender(device_info.id.as_str(), None, connection)
            .await?;

        update_progress(
            progress_delegate,
            SendProgressState::ConnectionMediumUpdate {
                medium: ConnectionMedium::BLE,
            },
        );

        return Ok(Box::new(encrypted_stream));
    }
}
