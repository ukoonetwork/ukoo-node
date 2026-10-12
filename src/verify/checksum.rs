use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::NodeError;
use crate::setup::ModelFile;

const CHUNK: usize = 8 << 20;

/// SHA-256 of a file, hex-encoded. Runs on a blocking thread: model files are gigabytes.
pub async fn sha256_file(path: &Path) -> Result<String, NodeError> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hex::encode(hasher.finalize()))
    })
    .await
    .expect("hashing thread panicked")
}

/// Fails closed: a file that doesn't match the registry's fingerprint is never served.
pub async fn verify_file(path: &Path, expected: &ModelFile) -> Result<(), NodeError> {
    let mismatch = || NodeError::ChecksumMismatch {
        file: expected.file_name().to_string(),
    };
    let len = tokio::fs::metadata(path)
        .await
        .map_err(|_| mismatch())?
        .len();
    if len != expected.size_bytes || sha256_file(path).await? != expected.sha256 {
        let _ = tokio::fs::remove_file(path).await;
        return Err(mismatch());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hashes_like_sha256sum() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"ukoo").unwrap();
        assert_eq!(
            sha256_file(&path).await.unwrap(),
            "ada2c02b4a47629d96bac8972daae2595d75ed85e01590ca35d8d8a1889179ae"
        );
    }

    #[tokio::test]
    async fn a_tampered_file_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        std::fs::write(&path, b"not the model").unwrap();
        let expected = ModelFile {
            url: "https://x/model.gguf".into(),
            sha256: hex::encode(Sha256::digest(b"the model")),
            size_bytes: 13,
        };
        assert!(matches!(
            verify_file(&path, &expected).await,
            Err(NodeError::ChecksumMismatch { .. })
        ));
        assert!(!path.exists());
    }
}
