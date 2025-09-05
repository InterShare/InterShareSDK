use crate::encryption::generate_iv;
use crate::encryption::EncryptedStream;
use log::info;
use prost_stream::Stream;
use protocol::communication::{EncryptionRequest, EncryptionResponse};
use rand_core::OsRng;
use std::error::Error;
use std::io::{Read, Write};
use x25519_dalek::{EphemeralSecret, PublicKey};

pub async fn initiate_sender_communication<T>(
    mut stream: T,
) -> Result<EncryptedStream<T>, Box<dyn Error>>
where
    T: Read + Write,
{
    info!("[Encryption] Initiating sender encryption communication");
    let secret = EphemeralSecret::random_from_rng(OsRng);
    let public_key = PublicKey::from(&secret);
    let encryption_request = EncryptionRequest {
        public_key: public_key.as_bytes().to_vec(),
    };

    info!("[Encryption] Sending public key");
    let mut prost_stream = Stream::new(&mut stream);
    let _ = prost_stream.send(&encryption_request);

    let encryption_response: EncryptionResponse = match prost_stream.recv::<EncryptionResponse>() {
        Ok(message) => message,
        Err(error) => return Err(Box::new(error)),
    };

    info!("[Encryption] Received foreign public key");

    let public_key: [u8; 32] = encryption_response
        .public_key
        .try_into()
        .expect("Vec length is not 32");
    let foreign_public_key = PublicKey::from(public_key);

    info!("[Encryption] Doin the diffie hellman. Yeah.");
    let shared_secret = secret.diffie_hellman(&foreign_public_key);

    let iv: [u8; 24] = encryption_response
        .iv
        .try_into()
        .expect("Vec length is not 24");

    let encrypted_stream = EncryptedStream::new(shared_secret.to_bytes(), iv, stream);

    return Ok(encrypted_stream);
}

pub fn initiate_receiver_communication<T>(
    mut stream: T,
) -> Result<EncryptedStream<T>, Box<dyn Error>>
where
    T: Read + Write,
{
    let secret = EphemeralSecret::random_from_rng(OsRng);
    let public_key = PublicKey::from(&secret);

    let iv = generate_iv();

    let mut prost_stream = Stream::new(&mut stream);

    let encryption_request = match prost_stream.recv::<EncryptionRequest>() {
        Ok(message) => message,
        Err(error) => return Err(Box::new(error)),
    };

    let _ = prost_stream.send(&EncryptionResponse {
        public_key: public_key.as_bytes().to_vec(),
        iv: iv.to_vec(),
    });

    let public_key: [u8; 32] = encryption_request
        .public_key
        .try_into()
        .expect("Vec length is not 32");
    let foreign_public_key = PublicKey::from(public_key);

    let shared_secret = secret.diffie_hellman(&foreign_public_key);

    let encrypted_stream = EncryptedStream::new(shared_secret.to_bytes(), iv, stream);

    return Ok(encrypted_stream);
}
