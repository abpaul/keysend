use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use zeroize::Zeroizing;

use crate::{Context, Error, MasterKey, Result};

const HEADER: &[u8] = b"credseal-v1\n";
const VERSION_PREFIX: &[u8] = b"credseal-v";
const NONCE_BYTES: usize = 12;
const TAG_BYTES: usize = 16;
pub(crate) const MAX_PLAINTEXT_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

#[cfg(any(feature = "cli", test))]
pub(crate) fn seal(key: &MasterKey, context: &Context, plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; NONCE_BYTES];
    getrandom::fill(&mut nonce).map_err(|_| Error::Random)?;
    let mut buffer = Zeroizing::new(Vec::with_capacity(plaintext.len() + TAG_BYTES));
    buffer.extend_from_slice(plaintext);
    let nonce = Nonce::from(nonce);
    cipher(key)
        .encrypt_in_place(&nonce, &aad(context), &mut *buffer)
        .expect("credentials are far below the AES-GCM message limit");

    let mut binary = Vec::with_capacity(NONCE_BYTES + buffer.len());
    binary.extend_from_slice(&nonce);
    binary.extend_from_slice(&buffer);
    let mut file = HEADER.to_vec();
    file.extend_from_slice(STANDARD.encode(binary).as_bytes());
    file.push(b'\n');
    Ok(file)
}

pub(crate) fn open(key: &MasterKey, context: &Context, file: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let Some(body) = file.strip_prefix(HEADER) else {
        let first_line = file.split(|byte| *byte == b'\n').next().unwrap_or_default();
        return Err(match first_line.strip_prefix(VERSION_PREFIX) {
            Some(version) if version.trim_ascii() != b"1" => Error::UnsupportedVersion,
            _ => Error::Malformed,
        });
    };
    let body = body.strip_suffix(b"\n").unwrap_or(body);
    let binary = STANDARD.decode(body).map_err(|_| Error::Malformed)?;
    if binary.len() < NONCE_BYTES + TAG_BYTES
        || binary.len() > NONCE_BYTES + MAX_PLAINTEXT_BYTES + TAG_BYTES
    {
        return Err(Error::Malformed);
    }
    let (nonce, sealed) = binary.split_at(NONCE_BYTES);
    let mut buffer = Zeroizing::new(sealed.to_vec());
    let nonce = Nonce::from(<[u8; NONCE_BYTES]>::try_from(nonce).map_err(|_| Error::Malformed)?);
    cipher(key)
        .decrypt_in_place(&nonce, &aad(context), &mut *buffer)
        .map_err(|_| Error::Decrypt)?;
    Ok(buffer)
}

fn cipher(key: &MasterKey) -> Aes256Gcm {
    Aes256Gcm::new_from_slice(key.bytes()).expect("MasterKey contains exactly 32 bytes")
}

fn aad(context: &Context) -> Vec<u8> {
    let mut aad = HEADER.to_vec();
    for name in [context.application(), context.environment()] {
        let length = u8::try_from(name.len()).expect("Context names are at most 64 bytes");
        aad.push(length);
        aad.extend_from_slice(name.as_bytes());
    }
    aad
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealing_the_same_text_twice_uses_a_fresh_nonce_each_time() {
        let key = MasterKey::from_base64("AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=").unwrap();
        let context = Context::new("myapp", "development").unwrap();
        let first = seal(&key, &context, b"token = \"x\"\n").unwrap();
        let second = seal(&key, &context, b"token = \"x\"\n").unwrap();

        assert_ne!(first, second);
        assert_eq!(
            &open(&key, &context, &first).unwrap()[..],
            b"token = \"x\"\n"
        );
        assert_eq!(
            &open(&key, &context, &second).unwrap()[..],
            b"token = \"x\"\n"
        );
    }
}
