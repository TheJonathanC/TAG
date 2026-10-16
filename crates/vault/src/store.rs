use chrono::{DateTime, Utc};
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use zeroize::Zeroize;

use crate::crypto::Crypto;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretEntry {
    pub key: String,
    pub description: String,
}

pub struct VaultStore {
    path: PathBuf,
    is_unlocked: bool,
    last_accessed: DateTime<Utc>,
    idle_timeout_mins: u64,
    secrets: HashMap<String, SecretString>,
    salt: [u8; 16],
}

impl VaultStore {
    pub fn new(path: PathBuf, idle_timeout_mins: u64) -> Self {
        Self {
            path,
            is_unlocked: false,
            last_accessed: Utc::now(),
            idle_timeout_mins,
            secrets: HashMap::new(),
            salt: [0u8; 16],
        }
    }

    pub fn is_unlocked(&self) -> bool {
        if !self.is_unlocked {
            return false;
        }
        let elapsed = Utc::now().signed_duration_since(self.last_accessed);
        elapsed.num_minutes() < self.idle_timeout_mins as i64
    }

    pub fn lock(&mut self) {
        self.secrets.clear();
        self.is_unlocked = false;
    }

    pub fn unlock(&mut self, passphrase: &str) -> Result<(), anyhow::Error> {
        if !self.path.exists() {
            // Initialize new vault
            let mut salt = [0u8; 16];
            rand::thread_rng().fill_bytes(&mut salt);
            self.salt = salt;
            self.secrets.clear();
            self.is_unlocked = true;
            self.last_accessed = Utc::now();
            self.save(passphrase)?;
            return Ok(());
        }

        let raw_file = fs::read(&self.path)?;
        if raw_file.len() < 16 {
            return Err(anyhow::anyhow!("Corrupt vault file"));
        }

        let (salt_slice, ciphertext) = raw_file.split_at(16);
        let mut salt = [0u8; 16];
        salt.copy_from_slice(salt_slice);

        let mut key = Crypto::derive_key(passphrase.as_bytes(), &salt)?;
        let decrypt_res = Crypto::decrypt(&key, ciphertext);
        key.zeroize();
        let mut decrypted_bytes = decrypt_res?;

        let parse_res: Result<HashMap<String, String>, _> =
            serde_json::from_slice(&decrypted_bytes);
        decrypted_bytes.zeroize();
        let raw_map = parse_res?;
        self.secrets.clear();
        for (k, mut v) in raw_map {
            let secret = SecretString::from(v.clone());
            v.zeroize();
            self.secrets.insert(k, secret);
        }

        self.salt = salt;
        self.is_unlocked = true;
        self.last_accessed = Utc::now();
        Ok(())
    }

    pub fn set_secret(&mut self, key: String, mut val: String) {
        self.last_accessed = Utc::now();
        let secret = SecretString::from(val.clone());
        val.zeroize();
        self.secrets.insert(key, secret);
    }

    pub fn get_secret(&mut self, key: &str) -> Option<&SecretString> {
        self.last_accessed = Utc::now();
        self.secrets.get(key)
    }

    pub fn list_keys(&self) -> Vec<String> {
        self.secrets.keys().cloned().collect()
    }

    pub fn get_env_set(&mut self) -> HashMap<String, String> {
        self.last_accessed = Utc::now();
        let mut map = HashMap::new();
        for (k, v) in &self.secrets {
            map.insert(k.clone(), v.expose_secret().to_string());
        }
        map
    }

    pub fn save(&self, passphrase: &str) -> Result<(), anyhow::Error> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
            }
        }

        let mut raw_map: HashMap<&str, &str> = HashMap::new();
        for (k, v) in &self.secrets {
            raw_map.insert(k.as_str(), v.expose_secret());
        }

        let mut json_bytes = serde_json::to_vec(&raw_map)?;
        let key_res = Crypto::derive_key(passphrase.as_bytes(), &self.salt);
        let mut key = match key_res {
            Ok(k) => k,
            Err(e) => {
                json_bytes.zeroize();
                return Err(e);
            }
        };
        let encrypt_res = Crypto::encrypt(&key, &json_bytes);
        key.zeroize();
        json_bytes.zeroize();
        let ciphertext = encrypt_res?;

        let mut payload = Vec::with_capacity(16 + ciphertext.len());
        payload.extend_from_slice(&self.salt);
        payload.extend_from_slice(&ciphertext);

        let tmp_path = self.path.with_extension("enc.tmp");
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)?;
            file.write_all(&payload)?;
            file.sync_all()?;
            let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
        }
        #[cfg(not(unix))]
        {
            fs::write(&tmp_path, &payload)?;
        }
        fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }
}
