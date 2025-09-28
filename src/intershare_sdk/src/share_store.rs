use crate::nearby_server::L2CapDelegate;
use crate::tar::stream_tar;
use crate::{
    connection::Connection, convert_os_str, encryption::generate_secure_base64_token,
    errors::ConnectErrors,
};
use fast_qr::convert::{image::ImageBuilder, Builder, Shape};
use fast_qr::qr::QRBuilder;
use log::{error, info};
use prost_stream::Stream;
use protocol::{
    communication::{
        request::{Intent, RequestTypes},
        ClipboardTransferIntent, FileTransferIntent, Request, TransferRequestResponse,
    },
    discovery::{Device, DeviceConnectionInfo},
};
use std::{fmt::Debug, fs::File, path::Path, sync::Arc};
use tokio::sync::RwLock;

pub enum ConnectionMedium {
    BLE,
    WiFi,
}

pub enum SendProgressState {
    Unknown,
    Connecting,
    Requesting,
    ConnectionMediumUpdate { medium: ConnectionMedium },
    Transferring { progress: f64 },
    Cancelled,
    Finished,
    Declined,
}

pub trait SendProgressDelegate: Send + Sync + Debug {
    fn progress_changed(&self, progress: SendProgressState);
}

pub struct ShareStore {
    pub request_id: String,
    pub file_paths: Option<Vec<String>>,
    pub clipboard: Option<String>,
    allow_convenience_share: bool,
    ble_l2_cap_client: Arc<RwLock<Option<Box<dyn L2CapDelegate>>>>,
    device_connection_info: DeviceConnectionInfo,
}

pub(crate) fn update_progress(
    progress_delegate: &Option<Box<dyn SendProgressDelegate>>,
    state: SendProgressState,
) {
    if let Some(progress_delegate) = progress_delegate {
        progress_delegate.progress_changed(state);
    }
}

impl ShareStore {
    #[uniffi::constructor]
    pub fn new(
        file_paths: Option<Vec<String>>,
        clipboard: Option<String>,
        allow_convenience_share: bool,
        ble_l2_cap_client: Arc<RwLock<Option<Box<dyn L2CapDelegate>>>>,
        device_connection_info: DeviceConnectionInfo,
    ) -> Self {
        Self {
            request_id: generate_secure_base64_token(23),
            file_paths,
            clipboard,
            allow_convenience_share,
            ble_l2_cap_client,
            device_connection_info,
        }
    }

    pub async fn send_to(
        &self,
        receiver: Device,
        progress_delegate: Option<Box<dyn SendProgressDelegate>>,
    ) -> Result<(), ConnectErrors> {
        return if self.file_paths.is_none() {
            self.send_text(receiver, progress_delegate).await
        } else {
            self.send_files(receiver, progress_delegate).await
        };
    }

    async fn send_text(
        &self,
        receiver: Device,
        progress_delegate: Option<Box<dyn SendProgressDelegate>>,
    ) -> Result<(), ConnectErrors> {
        let Some(text) = &self.clipboard else {
            return Err(ConnectErrors::NoTextProvided);
        };

        update_progress(&progress_delegate, SendProgressState::Connecting);

        let connection = Connection::new(self.ble_l2_cap_client.clone());

        let mut encrypted_stream = connection
            .connect(receiver, &progress_delegate)
            .await
            .inspect_err(|_| update_progress(&progress_delegate, SendProgressState::Unknown))?;

        let mut proto_stream = Stream::new(&mut encrypted_stream);

        update_progress(
            &progress_delegate,
            SendProgressState::Transferring { progress: 0.0 },
        );

        let transfer_request = Request {
            r#type: RequestTypes::ShareRequest as i32,
            device: self.device_connection_info.device.clone(),
            share_id: None,
            intent: Some(Intent::Clipboard(ClipboardTransferIntent {
                clipboard_content: text.to_string(),
            })),
        };

        update_progress(
            &progress_delegate,
            SendProgressState::Transferring { progress: 0.8 },
        );
        let _ = proto_stream.send(&transfer_request);
        update_progress(&progress_delegate, SendProgressState::Finished);

        return Ok(());
    }

    async fn send_files(
        &self,
        receiver: Device,
        progress_delegate: Option<Box<dyn SendProgressDelegate>>,
    ) -> Result<(), ConnectErrors> {
        let Some(file_paths) = &self.file_paths else {
            return Err(ConnectErrors::NoFilesProvided);
        };

        update_progress(&progress_delegate, SendProgressState::Connecting);

        let connection = Connection::new(self.ble_l2_cap_client.clone());

        let mut encrypted_stream = connection
            .connect(receiver, &progress_delegate)
            .await
            .inspect_err(|_| update_progress(&progress_delegate, SendProgressState::Unknown))?;

        let mut proto_stream = Stream::new(&mut encrypted_stream);

        update_progress(&progress_delegate, SendProgressState::Requesting);

        let file_name = file_paths.first().map(|file_path| {
            convert_os_str(
                Path::new(file_path)
                    .file_name()
                    .expect("Failed to get file name"),
            )
        });

        let mut file_size: u64 = 0;

        for file_path in file_paths {
            let metadata = File::open(file_path)
                .expect("Failed to read file")
                .metadata()
                .expect("Failed to get file metadata");

            file_size = file_size + metadata.len();
        }

        info!("Total size of files: {}", file_size);

        let transfer_request = Request {
            r#type: RequestTypes::ShareRequest as i32,
            device: self.device_connection_info.device.clone(),
            share_id: None,
            intent: Some(Intent::FileTransfer(FileTransferIntent {
                file_name,
                file_size,
                file_count: file_paths.len() as u64,
            })),
        };

        let _ = proto_stream.send(&transfer_request);

        let response = proto_stream
            .recv::<TransferRequestResponse>()
            .map_err(|error| ConnectErrors::FailedToGetTransferRequestResponse {
                error: error.to_string(),
            })?;

        if !response.accepted {
            update_progress(&progress_delegate, SendProgressState::Declined);
            return Err(ConnectErrors::Declined);
        }

        update_progress(
            &progress_delegate,
            SendProgressState::Transferring { progress: 0.0 },
        );

        let tar_result = stream_tar(
            &mut encrypted_stream,
            file_paths,
            file_size,
            &progress_delegate,
        );

        if let Err(error) = tar_result {
            error!("Error while tarring: {}", error);
            update_progress(&progress_delegate, SendProgressState::Cancelled);
        }

        update_progress(&progress_delegate, SendProgressState::Finished);

        return Ok(());
    }

    /// https://share.intershare.app?id=hgf8o47fdsb394mv385&ip=192.168.12.13&port=5200&device_id=9A403351-A926-4D1C-855F-432A6ED51E0E&protocol_version=1
    pub fn generate_link(&self) -> Option<String> {
        if !self.allow_convenience_share {
            return None;
        }

        let Some(device) = self.device_connection_info.device.clone() else {
            return None;
        };

        let Some(tcp_connection_info) = self.device_connection_info.tcp.clone() else {
            return None;
        };

        let ip = tcp_connection_info.hostname;
        let port = tcp_connection_info.port;

        let link = format!(
            "https://s.intershare.app?i={0}&ip={1}&p={2}&d={3}",
            self.request_id, ip, port, device.id
        );

        return Some(link);
    }

    pub fn generate_qr_code(&self, dark_mode: bool) -> Option<Vec<u8>> {
        let link = self.generate_link()?;

        let qrcode = QRBuilder::new(link).build().unwrap();

        let img = ImageBuilder::default()
            .shape(Shape::Circle)
            .module_color(if dark_mode {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            })
            .background_color([0, 0, 0, 0])
            .fit_width(300)
            .to_bytes(&qrcode);

        img.inspect_err(|error_message| {
            error!(
                "Error while trying to generate QR code: {:?}",
                error_message
            )
        })
        .ok()
    }
}
