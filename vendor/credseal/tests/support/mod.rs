#![allow(dead_code)]

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;

pub const KEY: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31,
];
pub const KEY_BASE64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
pub const OTHER_KEY_BASE64: &str = "Hx4dHBsaGRgXFhUUExIREA8ODQwLCgkIBwYFBAMCAQA=";

pub fn sealed_binary(application: &str, environment: &str, plaintext: &[u8]) -> Vec<u8> {
    let nonce = [7u8; 12];
    let mut aad = b"credseal-v1\n".to_vec();
    for name in [application, environment] {
        aad.push(name.len() as u8);
        aad.extend_from_slice(name.as_bytes());
    }
    let ciphertext = Aes256Gcm::new_from_slice(&KEY)
        .unwrap()
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .unwrap();
    [nonce.as_slice(), &ciphertext].concat()
}

pub fn file_from_binary(binary: &[u8]) -> Vec<u8> {
    format!("credseal-v1\n{}\n", STANDARD.encode(binary)).into_bytes()
}

pub fn sealed_file(application: &str, environment: &str, plaintext: &[u8]) -> Vec<u8> {
    file_from_binary(&sealed_binary(application, environment, plaintext))
}
