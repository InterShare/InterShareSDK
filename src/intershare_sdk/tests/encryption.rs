use crate::helper::MemoryStream;
use chacha20::cipher::StreamCipherSeek;
use intershare_sdk::encryption::{generate_iv, generate_key, EncryptedStream};
use rand_core::{OsRng, RngCore};
use std::io::{Read, Write};
use x25519_dalek::{EphemeralSecret, PublicKey};

mod helper;

#[test]
pub fn diffie_hellman() {
    let alice_secret = EphemeralSecret::random_from_rng(OsRng);
    let alice_public_key = PublicKey::from(&alice_secret);

    let bob_secret = EphemeralSecret::random_from_rng(OsRng);
    let bob_public_key = PublicKey::from(&bob_secret);

    assert_ne!(alice_public_key.as_bytes(), bob_public_key.as_bytes());

    let alice_shared_secret = alice_secret.diffie_hellman(&bob_public_key);
    let bob_shared_secret = bob_secret.diffie_hellman(&alice_public_key);

    assert_eq!(alice_shared_secret.as_bytes(), bob_shared_secret.as_bytes());
}

#[test]
pub fn stream_encryption() {
    let key = generate_key();
    let nonce = generate_iv();

    let memory_stream = MemoryStream::new();
    let mut encrypted_stream = EncryptedStream::new(key, nonce, memory_stream);

    let write_data = &vec![1, 2, 3];

    let written_bytes = encrypted_stream
        .write(write_data)
        .expect("Something went wrong, while trying to write to EncryptedStream");

    assert_eq!(written_bytes, 3);

    encrypted_stream.raw_stream.set_position(0);
    encrypted_stream.cipher.seek(0);

    let mut encrypted_gibberish = Vec::new();
    encrypted_stream
        .raw_stream
        .read_to_end(&mut encrypted_gibberish)
        .expect("Error reading memory_stream");

    assert_ne!(write_data, &encrypted_gibberish);

    let write_data = &vec![1, 2, 3];

    encrypted_stream
        .write(write_data)
        .expect("Something went wrong, while trying to write to EncryptedStream");

    encrypted_stream.raw_stream.set_position(0);
    encrypted_stream.cipher.seek(0);

    let mut decrypted = [0u8; 3];
    encrypted_stream
        .read(&mut decrypted)
        .expect("Error decrypting memory_stream");

    assert_eq!(write_data, &decrypted);
}

#[test]
pub fn large_stream_encryption() {
    let key = generate_key();
    let nonce = generate_iv();

    let memory_stream = MemoryStream::new();
    let mut encrypted_stream = EncryptedStream::new(key, nonce, Box::new(memory_stream));

    let mut write_data: [u8; 100000] = [0; 100000];
    let rng = &mut OsRng;
    rng.fill_bytes(&mut write_data);

    let write_data = write_data.as_slice();

    let written_bytes = encrypted_stream
        .write(write_data)
        .expect("Something went wrong, while trying to write to EncryptedStream");

    assert!(written_bytes > 0);
    encrypted_stream.raw_stream.set_position(0);
    encrypted_stream.cipher.seek(0);

    let mut encrypted_gibberish = Vec::new();
    encrypted_stream
        .raw_stream
        .read_to_end(&mut encrypted_gibberish)
        .expect("Error reading memory_stream");

    assert_ne!(write_data, &encrypted_gibberish);
    encrypted_stream.raw_stream.set_position(0);
    encrypted_stream.cipher.seek(0);

    let mut decrypted_buffer: [u8; 100000] = [0; 100000];
    let read_bytes = encrypted_stream
        .read(&mut decrypted_buffer)
        .expect("Something went wrong, while trying to decrypt the stream");

    assert_eq!(write_data, &decrypted_buffer[..read_bytes]);
}
