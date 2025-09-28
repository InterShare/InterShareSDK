use std::sync::atomic::{AtomicBool, Ordering};
use std::io;
use std::net::SocketAddr;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;
use log::info;
use prost_stream::Stream;
use protocol::communication::request::RequestTypes;
use protocol::communication::Request;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use crate::encryption::initiate_receiver_communication;
use crate::connection_request::ConnectionRequest;
use crate::nearby_server::{NearbyConnectionDelegate, InternalNearbyServer};
use crate::stream::Close;

pub struct TcpServer {
    pub port: u16,
    listener: Option<TcpListener>,
    delegate: Arc<RwLock<Box<dyn NearbyConnectionDelegate>>>,
    file_storage: String,
    running: Arc<AtomicBool>,
    tcp_server_task: RwLock<Option<JoinHandle<()>>>
}

impl InternalNearbyServer {
    pub(crate) async fn new_tcp_server(&self, delegate: Arc<RwLock<Box<dyn NearbyConnectionDelegate>>>, file_storage: String) -> Result<TcpServer, io::Error> {
        let addresses = [
            SocketAddr::from(([0, 0, 0, 0], 4251)),
            SocketAddr::from(([0, 0, 0, 0], 80)),
            SocketAddr::from(([0, 0, 0, 0], 8080)),
            SocketAddr::from(([0, 0, 0, 0], 0))
        ];

        let listener = TcpListener::bind(&addresses[..])?;
        listener.set_nonblocking(false).expect("Failed to set non blocking");
        let port = listener.local_addr()?.port();

        info!("Started tcp listener on port {}", port);

        return Ok(TcpServer {
            port,
            listener: Some(listener),
            delegate,
            file_storage,
            running: Arc::new(AtomicBool::new(true)),
            tcp_server_task: RwLock::new(None)
        });
    }

    pub async fn start_loop(&self) {
        let mut guard = self.tcp_server.write().await;
        let Some(tcp_server) = guard.as_mut() else {
            return;
        };

        if let Some(existing_task) = tcp_server.tcp_server_task.write().await.take() {
            existing_task.abort();
        }

        tcp_server.running.store(true, Ordering::SeqCst);

        // let listener = tcp_server.listener.as_ref().expect("Listener is not initialized").try_clone().expect("Failed to clone listener");
        let listener = tcp_server.listener.take().expect("Listener is not initialized");
        listener.set_nonblocking(true).expect("Failed to set non blocking");
        let delegate = tcp_server.delegate.clone();
        let file_storage = tcp_server.file_storage.clone();
        let running = tcp_server.running.clone();

        let handle = tokio::spawn(async move {
            info!("Started loop");
            while running.load(Ordering::SeqCst) {
                let Ok((tcp_stream, _socket_address)) = listener.accept() else {
                    continue
                };

                tcp_stream.set_nonblocking(false).expect("Failed to set non blocking");

                let mut encrypted_stream = match initiate_receiver_communication(tcp_stream) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("Encryption error {:}", error);
                        continue;
                    }
                };

                let mut prost_stream = Stream::new(&mut encrypted_stream);
                let transfer_request = match prost_stream.recv::<Request>() {
                    Ok(message) => message,
                    Err(error) => {
                        println!("Error {:}", error);
                        continue;
                    }
                };

                if transfer_request.r#type== RequestTypes::ShareRequest as i32 {
                    let connection_request = ConnectionRequest::new(
                        transfer_request,
                        Box::new(encrypted_stream),
                        file_storage.clone()
                    );

                    delegate.read().await.received_connection_request(Arc::new(connection_request));
                } else {
                    // NearbyServer::received_convenience_download_request(transfer_request, current_share_store.clone()).await;
                }
            }

            info!("Stopped loop");
        });

        *tcp_server.tcp_server_task.write().await = Some(handle);
    }

    pub async fn stop_tcp_server(&self) {
        // let Some(tcp_server) = &*self.tcp_server.read().await else {
        //     return;
        // };
        let mut tcp_server_guard = self.tcp_server.write().await;
        let Some(tcp_server) = tcp_server_guard.as_mut() else {
            return;
        };

        info!("Stopping TCP server port {}", tcp_server.port);

        tcp_server.running.store(false, Ordering::SeqCst);


        if let Some(task) = tcp_server.tcp_server_task.write().await.take() {
            task.abort();
            info!("Stopped TCP connection handle task")
        }

        tcp_server.listener = None;
        *tcp_server_guard = None;

        info!("TCP server stopped.");
    }
}

pub struct TcpClient {
}

impl TcpClient {
    pub fn connect(address: SocketAddr) -> Result<TcpStream, io::Error> {
        let std_stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
        std_stream.set_nonblocking(false).expect("Failed to set non blocking");

        return Ok(std_stream);
    }
}

impl Close for TcpStream {
    fn close(&self) {
        // Do nothing. TCPStream closes automatically.
    }
}
