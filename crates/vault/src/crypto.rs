use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use rand::RngCore;

pub struct Crypto;

impl Crypto {
    pub fn derive_key(passphrase: &[u8], salt: &[u8]) -> Result<[u8; 32], anyhow::Error> {
        let mut key = [0u8; 32];
        let params = Params::new(19456, 3, 1, Some(32))
            .map_err(|e| anyhow::anyhow!("Argon2 params error: {}", e))?;
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        argon2
            .hash_password_into(passphrase, salt, &mut key)
            .map_err(|e| anyhow::anyhow!("Argon2 derivation error: {}", e))?;
        Ok(key)
    }

    pub fn encrypt(key_bytes: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, anyhow::Error> {
        let key = Key::from_slice(key_bytes);
        let cipher = ChaCha20Poly1305::new(key);

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

        // Format: [12 bytes nonce] + [ciphertext + tag]
        let mut result = Vec::with_capacity(12 + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);
        Ok(result)
    }

    pub fn decrypt(key_bytes: &[u8; 32], payload: &[u8]) -> Result<Vec<u8>, anyhow::Error> {
        if payload.len() < 12 {
            return Err(anyhow::anyhow!("Invalid ciphertext payload length"));
        }

        let (nonce_bytes, ciphertext) = payload.split_at(12);
        let key = Key::from_slice(key_bytes);
        let cipher = ChaCha20Poly1305::new(key);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| anyhow::anyhow!("Decryption failed (bad key or corrupted data): {}", e))?;

        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroize::Zeroize;

    #[test]
    fn test_crypto_round_trip() {
        let salt = [42u8; 16];
        let mut key = Crypto::derive_key(b"my-secure-passphrase", &salt).unwrap();
        let message = b"super secret api token 12345";

        let encrypted = Crypto::encrypt(&key, message).unwrap();
        let decrypted = Crypto::decrypt(&key, &encrypted).unwrap();
        assert_eq!(decrypted, message);

        key.zeroize();
    }
}
