use chrono::{DateTime, Utc};
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use zeroize::{Zeroize, Zeroizing};

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
    derived_key: Option<Zeroizing<[u8; 32]>>,
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
            derived_key: None,
        }
    }

    pub fn is_unlocked(&self) -> bool {
        if !self.is_unlocked || self.derived_key.is_none() {
            return false;
        }
        !self.idle_expired()
    }

    /// An idle timeout of 0 disables auto-lock.
    fn idle_expired(&self) -> bool {
        if self.idle_timeout_mins == 0 {
            return false;
        }
        let elapsed = Utc::now().signed_duration_since(self.last_accessed);
        elapsed.num_minutes() >= self.idle_timeout_mins as i64
    }

    /// Lock (and wipe decrypted secrets and key from memory) once the idle timeout
    /// has passed. Returns true if the vault was locked by this call.
    pub fn enforce_idle_lock(&mut self) -> bool {
        if self.is_unlocked && self.idle_expired() {
            self.lock();
            return true;
        }
        false
    }

    /// Whether a vault file exists yet (unlocking a missing vault creates a new one).
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    pub fn lock(&mut self) {
        self.secrets.clear();
        self.derived_key = None;
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
            let key = Crypto::derive_key(passphrase.as_bytes(), &salt)?;
            self.derived_key = Some(Zeroizing::new(key));
            self.save_current()?;
            return Ok(());
        }

        let raw_file = fs::read(&self.path)?;
        if raw_file.len() < 16 {
            return Err(anyhow::anyhow!("Corrupt vault file"));
        }

        let (salt_slice, ciphertext) = raw_file.split_at(16);
        let mut salt = [0u8; 16];
        salt.copy_from_slice(salt_slice);

        let key = Crypto::derive_key(passphrase.as_bytes(), &salt)?;
        let decrypt_res = Crypto::decrypt(&key, ciphertext);
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
        self.derived_key = Some(Zeroizing::new(key));
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

    pub fn delete_secret(&mut self, key: &str) -> bool {
        self.last_accessed = Utc::now();
        self.secrets.remove(key).is_some()
    }

    pub fn get_secret(&mut self, key: &str) -> Option<&SecretString> {
        self.last_accessed = Utc::now();
        self.secrets.get(key)
    }

    pub fn get_secret_value(&mut self, key: &str) -> Option<String> {
        self.last_accessed = Utc::now();
        self.secrets.get(key).map(|s| s.expose_secret().to_string())
    }

    pub fn list_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.secrets.keys().cloned().collect();
        keys.sort();
        keys
    }

    pub fn get_env_set(&mut self) -> HashMap<String, String> {
        self.last_accessed = Utc::now();
        let mut map = HashMap::new();
        for (k, v) in &self.secrets {
            map.insert(k.clone(), v.expose_secret().to_string());
        }
        map
    }

    pub fn save_current(&self) -> Result<(), anyhow::Error> {
        let key = self
            .derived_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Cannot save: vault is locked"))?;

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
        let encrypt_res = Crypto::encrypt(key, &json_bytes);
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

    pub fn save(&mut self, passphrase: &str) -> Result<(), anyhow::Error> {
        let key = Crypto::derive_key(passphrase.as_bytes(), &self.salt)?;
        self.derived_key = Some(Zeroizing::new(key));
        self.save_current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idle_lock_wipes_and_zero_timeout_never_expires() {
        let dir = std::env::temp_dir().join(format!("tag_test_vault_{}", rand::random::<u64>()));
        let mut vault = VaultStore::new(dir.join("vault.enc"), 1);
        vault.unlock("pw").unwrap();
        vault.set_secret("K".into(), "v".into());
        vault.last_accessed = Utc::now() - chrono::Duration::minutes(5);
        assert!(!vault.is_unlocked());
        assert!(vault.enforce_idle_lock());
        assert!(vault.secrets.is_empty());
        assert!(vault.derived_key.is_none());

        let mut never = VaultStore::new(dir.join("vault.enc"), 0);
        never.unlock("pw").unwrap();
        never.last_accessed = Utc::now() - chrono::Duration::days(30);
        assert!(never.is_unlocked());
        assert!(!never.enforce_idle_lock());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_vault_lifecycle() {
        let dir = std::env::temp_dir().join(format!("tag_test_vault_{}", rand::random::<u64>()));
        let vault_file = dir.join("vault.enc");

        let mut vault = VaultStore::new(vault_file.clone(), 30);
        assert!(!vault.is_unlocked());

        // Initializing new vault with master passphrase
        vault.unlock("master-password-123").unwrap();
        assert!(vault.is_unlocked());
        assert_eq!(vault.list_keys().len(), 0);

        // Add secrets
        vault.set_secret("OPENAI_KEY".into(), "sk-12345".into());
        vault.set_secret("ANTHROPIC_KEY".into(), "ant-secret".into());
        assert_eq!(vault.list_keys(), vec!["ANTHROPIC_KEY", "OPENAI_KEY"]);

        // Save encrypted state
        vault.save_current().unwrap();
        assert!(vault_file.exists());

        // Lock vault
        vault.lock();
        assert!(!vault.is_unlocked());
        assert_eq!(vault.list_keys().len(), 0);

        // Wrong passphrase fails
        assert!(vault.unlock("wrong-password").is_err());
        assert!(!vault.is_unlocked());

        // Correct passphrase restores secrets
        vault.unlock("master-password-123").unwrap();
        assert!(vault.is_unlocked());
        assert_eq!(vault.list_keys(), vec!["ANTHROPIC_KEY", "OPENAI_KEY"]);
        assert_eq!(
            vault.get_secret("OPENAI_KEY").unwrap().expose_secret(),
            "sk-12345"
        );

        // Delete secret and verify
        assert!(vault.delete_secret("OPENAI_KEY"));
        vault.save_current().unwrap();

        let mut vault2 = VaultStore::new(vault_file.clone(), 30);
        vault2.unlock("master-password-123").unwrap();
        assert_eq!(vault2.list_keys(), vec!["ANTHROPIC_KEY"]);

        let _ = fs::remove_dir_all(dir);
    }
}
