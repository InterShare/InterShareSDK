use crate::tar::untar_stream;
use crate::{encryption::EncryptedReadWrite, nearby_server::ConnectionIntentType};
use log::error;
use prost_stream::Stream;
use protocol::communication::request::Intent;
use protocol::communication::{
    ClipboardTransferIntent, FileTransferIntent, Request, TransferRequestResponse,
};
use protocol::discovery::Device;
use regex::Regex;
use std::fmt::Debug;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::RwLock;

#[derive(uniffi::Enum)]
pub enum ReceiveProgressState {
    Unknown,
    Handshake,
    Receiving { progress: f64 },
    Extracting,
    Cancelled,
    Finished,
}

#[uniffi::export(callback_interface)]
pub trait ReceiveProgressDelegate: Send + Sync + Debug {
    fn progress_changed(&self, progress: ReceiveProgressState);
}

struct SharedVariables {
    receive_progress_delegate: Option<Box<dyn ReceiveProgressDelegate>>,
}

#[derive(uniffi::Object)]
pub struct ConnectionRequest {
    transfer_request: Request,
    connection: Arc<Mutex<Box<dyn EncryptedReadWrite>>>,
    file_storage: String,
    should_cancel: AtomicBool,
    variables: Arc<RwLock<SharedVariables>>,
}

impl ConnectionRequest {
    pub fn new(
        transfer_request: Request,
        connection: Box<dyn EncryptedReadWrite>,
        file_storage: String,
    ) -> Self {
        Self {
            transfer_request,
            connection: Arc::new(Mutex::new(connection)),
            file_storage,
            should_cancel: AtomicBool::new(false),
            variables: Arc::new(RwLock::new(SharedVariables {
                receive_progress_delegate: None,
            })),
        }
    }

    fn handle_file(
        &self,
        mut stream: MutexGuard<Box<dyn EncryptedReadWrite>>,
        file_transfer: FileTransferIntent,
    ) -> Option<Vec<String>> {
        match untar_stream(
            &mut *stream,
            self.file_storage.as_ref(),
            file_transfer.file_size,
            |progress| {
                self.update_progress(ReceiveProgressState::Receiving { progress: progress });
            },
            &self.should_cancel,
        ) {
            Ok(files) => {
                self.update_progress(ReceiveProgressState::Finished);
                stream.close();
                Some(files)
            }
            Err(error) => {
                error!("Error while unpacking: {}", error);
                self.update_progress(ReceiveProgressState::Cancelled);
                stream.close();
                None
            }
        }
    }

    pub fn get_intent(&self) -> Intent {
        self.transfer_request
            .intent
            .clone()
            .expect("Intent information missing")
    }
}

#[uniffi::export]
impl ConnectionRequest {
    pub fn set_progress_delegate(&self, delegate: Box<dyn ReceiveProgressDelegate>) {
        let mut variables = self.variables.blocking_write();
        variables.receive_progress_delegate = Some(delegate);
    }

    pub fn get_sender(&self) -> Device {
        self.transfer_request
            .device
            .clone()
            .expect("Device information missing")
    }

    pub fn get_intent_type(&self) -> ConnectionIntentType {
        match self
            .transfer_request
            .intent
            .clone()
            .expect("Intent information missing")
        {
            Intent::FileTransfer(_) => ConnectionIntentType::FileTransfer,
            Intent::Clipboard(_) => ConnectionIntentType::Clipboard,
        }
    }

    pub fn is_link(&self) -> bool {
        if let Some(clipboard_intent) = self.get_clipboard_intent() {
            let url_regex = Regex::new(r"^https?:\/\/(?:www\.)?[-a-zA-Z0-9@:%._\+~#=]{1,256}\.[a-zA-Z0-9()]{1,6}\b(?:[-a-zA-Z0-9()@:%_\+.~#?&\/=]*)$").unwrap();
            return url_regex.is_match(&clipboard_intent.clipboard_content);
        }

        return false;
    }

    pub fn get_file_transfer_intent(&self) -> Option<FileTransferIntent> {
        match self
            .transfer_request
            .intent
            .clone()
            .expect("Intent information missing")
        {
            Intent::FileTransfer(file_transfer_intent) => Some(file_transfer_intent),
            Intent::Clipboard(_) => None,
        }
    }

    pub fn get_clipboard_intent(&self) -> Option<ClipboardTransferIntent> {
        match self
            .transfer_request
            .intent
            .clone()
            .expect("Intent information missing")
        {
            Intent::FileTransfer(_) => None,
            Intent::Clipboard(clipboard_intent) => Some(clipboard_intent),
        }
    }

    pub fn decline(&self) {
        if self.get_intent_type() == ConnectionIntentType::Clipboard {
            if let Ok(connection_guard) = self.connection.lock() {
                connection_guard.close();
            }

            return;
        }

        if let Ok(mut connection_guard) = self.connection.lock() {
            let mut stream = Stream::new(&mut *connection_guard);

            let _ = stream.send(&TransferRequestResponse { accepted: false });
            connection_guard.close();
        }
    }

    fn update_progress(&self, new_state: ReceiveProgressState) {
        if let Some(receive_progress_delegate) =
            &self.variables.blocking_read().receive_progress_delegate
        {
            receive_progress_delegate.progress_changed(new_state);
        }
    }

    pub fn cancel(&self) {
        self.should_cancel.store(true, Ordering::Relaxed);
    }

    pub fn accept(&self) -> Option<Vec<String>> {
        if self.get_intent_type() == ConnectionIntentType::Clipboard {
            if let Ok(connection_guard) = self.connection.lock() {
                connection_guard.close();
            }

            return Some(vec![]);
        }

        self.update_progress(ReceiveProgressState::Handshake);

        if let Ok(mut connection_guard) = self.connection.lock() {
            let mut stream = Stream::new(&mut *connection_guard);

            let _ = stream.send(&TransferRequestResponse { accepted: true });

            match self.get_intent() {
                Intent::FileTransfer(file_transfer) => {
                    self.handle_file(connection_guard, file_transfer)
                }
                Intent::Clipboard(_) => None,
            }
        } else {
            None
        }
    }
}
