//! Linux's stand-in for DPAPI: secrets in the settings files are sealed with
//! ChaCha20-Poly1305 under one 32-byte key for this user. The key lives in
//! the desktop's Secret Service (GNOME Keyring, KWallet) under
//! `application=portal, purpose=settings-key`. Where there is none, or it is
//! locked, the key is `secret.key` in Portal's data folder, readable by this
//! user only (0600). A key file, once made, is kept, so values sealed with it
//! stay readable. The log says which one is used.

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305, NONCE_LEN};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

type Key = [u8; 32];

pub fn seal(data: &[u8]) -> Option<Vec<u8>> {
    let key = cipher()?;
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).ok()?;
    let mut sealed = data.to_vec();
    key.seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut sealed)
        .ok()?;
    let mut out = nonce.to_vec();
    out.extend(sealed);
    Some(out)
}

pub fn open(data: &[u8]) -> Option<Vec<u8>> {
    let key = cipher()?;
    let nonce: [u8; NONCE_LEN] = data.get(..NONCE_LEN)?.try_into().ok()?;
    let mut sealed = data.get(NONCE_LEN..)?.to_vec();
    let opened = key
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::empty(), &mut sealed)
        .ok()?;
    Some(opened.to_vec())
}

fn cipher() -> Option<LessSafeKey> {
    static KEY: OnceLock<Option<Key>> = OnceLock::new();
    let key = KEY
        .get_or_init(|| load_key(&crate::paths::portal_dir().join("secret.key")))
        .as_ref()?;
    Some(LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, key).ok()?))
}

/// The key file when there is one, else the Secret Service's key, else a new key file.
fn load_key(file: &Path) -> Option<Key> {
    if let Some(key) = read_key_file(file) {
        tracing::info!("Secrets in settings are sealed with the key in {} (0600)", file.display());
        return Some(key);
    }
    match secret_service_key() {
        Ok(key) => {
            tracing::info!("Secrets in settings are sealed with a key kept in the Secret Service");
            Some(key)
        }
        Err(err) => {
            tracing::warn!(
                "Secret Service unavailable ({err}); secrets in settings are sealed with the key in {} (0600)",
                file.display()
            );
            make_key_file(file)
        }
    }
}

fn read_key_file(file: &Path) -> Option<Key> {
    let bytes = std::fs::read(file).ok()?;
    let key: Key = bytes.as_slice().try_into().ok()?;
    // Tightened if something loosened it.
    let _ = std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600));
    Some(key)
}

/// A new random key, written so only this user can read it. Another Portal
/// that wrote one first wins, and its key is used.
fn make_key_file(file: &Path) -> Option<Key> {
    let mut key: Key = [0; 32];
    getrandom::getrandom(&mut key).ok()?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file);
    match made {
        Ok(mut out) => {
            if out.write_all(&key).and_then(|()| out.sync_all()).is_err() {
                drop(out);
                let _ = std::fs::remove_file(file);
                return None;
            }
            Some(key)
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => read_key_file(file),
        Err(_) => None,
    }
}

/// The key from the Secret Service, made and stored there on first use. On a
/// thread of its own with a time limit: a bus that never answers must not
/// hold up saving settings.
fn secret_service_key() -> Result<Key, String> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(secret_service::key().map_err(|err| err.to_string()));
    });
    receive
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| Err("no answer".into()))
}

mod secret_service {
    use super::Key;
    use std::collections::HashMap;
    use zbus::blocking::Connection;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

    const DEST: &str = "org.freedesktop.secrets";
    const SERVICE_PATH: &str = "/org/freedesktop/secrets";
    const SERVICE: &str = "org.freedesktop.Secret.Service";

    type Fail = Box<dyn std::error::Error + Send + Sync>;

    fn attributes() -> HashMap<&'static str, &'static str> {
        HashMap::from([("application", "portal"), ("purpose", "settings-key")])
    }

    pub fn key() -> Result<Key, Fail> {
        let bus = Connection::session()?;
        // A plain session: the secret crosses only the user's own session bus.
        let (_, session): (OwnedValue, OwnedObjectPath) = bus
            .call_method(Some(DEST), SERVICE_PATH, Some(SERVICE), "OpenSession", &("plain", Value::from("")))?
            .body()
            .deserialize()?;
        let (unlocked, _locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = bus
            .call_method(Some(DEST), SERVICE_PATH, Some(SERVICE), "SearchItems", &(attributes(),))?
            .body()
            .deserialize()?;
        if let Some(item) = unlocked.first() {
            let (_, _, value, _): (OwnedObjectPath, Vec<u8>, Vec<u8>, String) = bus
                .call_method(
                    Some(DEST),
                    item.as_str(),
                    Some("org.freedesktop.Secret.Item"),
                    "GetSecret",
                    &(session.as_ref(),),
                )?
                .body()
                .deserialize()?;
            return value
                .as_slice()
                .try_into()
                .map_err(|_| "the stored key is not 32 bytes".into());
        }
        let collection: OwnedObjectPath = bus
            .call_method(Some(DEST), SERVICE_PATH, Some(SERVICE), "ReadAlias", &("default",))?
            .body()
            .deserialize()?;
        if collection.as_str() == "/" {
            return Err("no default keyring".into());
        }
        let mut key: Key = [0; 32];
        getrandom::getrandom(&mut key).map_err(|err| err.to_string())?;
        let properties: HashMap<&str, Value<'_>> = HashMap::from([
            ("org.freedesktop.Secret.Item.Label", Value::from("Portal settings key")),
            ("org.freedesktop.Secret.Item.Attributes", Value::from(attributes())),
        ]);
        let secret = (session.as_ref(), Vec::<u8>::new(), key.to_vec(), "application/octet-stream");
        let (item, _prompt): (OwnedObjectPath, OwnedObjectPath) = bus
            .call_method(
                Some(DEST),
                collection.as_str(),
                Some("org.freedesktop.Secret.Collection"),
                "CreateItem",
                &(properties, secret, true),
            )?
            .body()
            .deserialize()?;
        // "/" means the keyring wants a prompt first: it is locked.
        if item.as_str() == "/" {
            return Err("the keyring is locked".into());
        }
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_file_is_private_and_kept() {
        let dir = std::env::temp_dir().join(format!("portal-sealed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("secret.key");
        let made = make_key_file(&file).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(make_key_file(&file), Some(made));
        assert_eq!(read_key_file(&file), Some(made));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
