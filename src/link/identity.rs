use std::path::Path;

use ed25519_dalek::SigningKey;

use crate::error::NodeError;

/// The node's identity: an Ed25519 keypair generated on this machine and never sent anywhere.
/// The relayer only ever sees the public key (from L3), which becomes the node's ID.
pub fn load_or_create(path: &Path) -> Result<SigningKey, NodeError> {
    if let Ok(hex_secret) = std::fs::read_to_string(path) {
        if let Ok(bytes) = hex::decode(hex_secret.trim()) {
            if let Ok(secret) = <[u8; 32]>::try_from(bytes.as_slice()) {
                return Ok(SigningKey::from_bytes(&secret));
            }
        }
    }

    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).expect("OS random number generator unavailable");
    std::fs::create_dir_all(path.parent().expect("identity path has a parent"))?;
    write_private(path, hex::encode(secret).as_bytes())?;
    Ok(SigningKey::from_bytes(&secret))
}

pub fn public_key_hex(key: &SigningKey) -> String {
    hex::encode(key.verifying_key().to_bytes())
}

#[cfg(unix)]
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)
}

#[cfg(not(unix))]
fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_key_comes_back_on_every_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.key");
        let first = load_or_create(&path).unwrap();
        let second = load_or_create(&path).unwrap();
        assert_eq!(public_key_hex(&first), public_key_hex(&second));
        assert_eq!(public_key_hex(&first).len(), 64);
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_is_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.key");
        load_or_create(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
