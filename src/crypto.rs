use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
    Engine,
};
use loco_rs::{Error, Result};

pub struct MasterKey([u8; 32]);

impl MasterKey {
    pub fn from_base64(encoded: &str) -> Result<Self> {
        let bytes = STANDARD
            .decode(encoded)
            .or_else(|_| STANDARD_NO_PAD.decode(encoded))
            .map_err(|_| Error::Message("KEYSEND_MASTER_KEY must be base64 of 32 bytes".into()))?;
        let key = bytes
            .try_into()
            .map_err(|_| Error::Message("KEYSEND_MASTER_KEY must be base64 of 32 bytes".into()))?;
        Ok(Self(key))
    }

    pub fn encrypt(&self, workspace_id: &str, connection_id: &str, value: &str) -> Result<String> {
        let cipher = Aes256Gcm::new_from_slice(&self.0).map_err(|_| invalid())?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let aad = associated_data(workspace_id, connection_id);
        let encrypted = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: value.as_bytes(),
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| invalid())?;
        let mut bytes = vec![1];
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&encrypted);
        Ok(STANDARD_NO_PAD.encode(bytes))
    }

    pub fn decrypt(
        &self,
        workspace_id: &str,
        connection_id: &str,
        encoded: &str,
    ) -> Result<String> {
        let bytes = STANDARD_NO_PAD.decode(encoded).map_err(|_| invalid())?;
        if bytes.len() < 29 || bytes[0] != 1 {
            return Err(invalid());
        }
        let cipher = Aes256Gcm::new_from_slice(&self.0).map_err(|_| invalid())?;
        let nonce = Nonce::from(<[u8; 12]>::try_from(&bytes[1..13]).map_err(|_| invalid())?);
        let aad = associated_data(workspace_id, connection_id);
        let plain = cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &bytes[13..],
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| invalid())?;
        String::from_utf8(plain).map_err(|_| invalid())
    }
}

fn associated_data(workspace_id: &str, connection_id: &str) -> String {
    format!("google_connections:{workspace_id}:{connection_id}:encrypted_refresh_token")
}

fn invalid() -> Error {
    Error::Message("Cannot open the encrypted Google Connection".into())
}
