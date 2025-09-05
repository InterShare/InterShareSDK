use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::XChaCha20;
use rand_core::{OsRng, RngCore};
use std::io;
use std::io::ErrorKind::Other;
use std::io::{Error, Read, Write};
use std::iter::repeat;

use crate::stream::Close;

pub fn generate_key() -> [u8; 32] {
    let key = XChaCha20::generate_key(&mut OsRng);

    return key.into();
}

pub fn generate_secure_base64_token(byte_length: usize) -> String {
    let mut bytes = vec![0u8; byte_length];
    OsRng.fill_bytes(&mut bytes);
    return URL_SAFE_NO_PAD.encode(&bytes);
}

pub fn generate_iv() -> [u8; 24] {
    let nonce = XChaCha20::generate_iv(&mut OsRng);

    return nonce.into();
}

pub struct EncryptedStream<TStream>
where
    TStream: Read + Write,
{
    pub cipher: XChaCha20,
    pub raw_stream: TStream,
}

impl<TStream> EncryptedStream<TStream>
where
    TStream: Read + Write,
{
    pub fn new(key: [u8; 32], iv: [u8; 24], stream: TStream) -> Self {
        let cipher = XChaCha20::new(&key.into(), &iv.into());

        Self {
            cipher,
            raw_stream: stream,
        }
    }
}

impl<TStream> Read for EncryptedStream<TStream>
where
    TStream: Read + Write,
{
    fn read(&mut self, read_buffer: &mut [u8]) -> io::Result<usize> {
        let mut buffer: Vec<u8> = repeat(0).take(read_buffer.len()).collect();
        let read_bytes = self
            .raw_stream
            .read(&mut buffer)
            .expect("Failed to read from encrypted buffer");

        if read_bytes <= 0 {
            return Ok(0);
        }

        self.cipher
            .apply_keystream_b2b(&buffer[..read_bytes], &mut read_buffer[..read_bytes])
            .map_err(|error| Error::new(Other, error.to_string()))?;

        return Ok(read_bytes);
    }
}

impl<TStream> Write for EncryptedStream<TStream>
where
    TStream: Read + Write,
{
    fn write(&mut self, write_buffer: &[u8]) -> io::Result<usize> {
        let mut buffer: Vec<u8> = repeat(0).take(write_buffer.len()).collect();

        let Ok(()) = self.cipher.apply_keystream_b2b(write_buffer, &mut buffer) else {
            return Ok(0);
        };

        return self.raw_stream.write(&buffer);
    }

    fn flush(&mut self) -> io::Result<()> {
        return self.raw_stream.flush();
    }
}

impl<TStream> Close for EncryptedStream<TStream>
where
    TStream: Close + Read + Write,
{
    fn close(&self) {
        self.raw_stream.close();
    }
}

pub trait EncryptedReadWrite: Read + Write + Send + Close {}
impl<TStream> EncryptedReadWrite for EncryptedStream<TStream> where
    TStream: Read + Write + Send + Close
{
}
