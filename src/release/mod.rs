//! Signed releases: ukoo-node only installs an update whose manifest is signed with
//! Ukoo's release key and whose archive matches the manifest's SHA-256.

use std::io::Read;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signature, VerifyingKey};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::NodeError;

/// Set at build time by the release workflow; builds without it can't update themselves.
const RELEASE_PUBLIC_KEY: Option<&str> = option_env!("UKOO_RELEASE_PUBLIC_KEY");
const RELEASES_URL: Option<&str> = option_env!("UKOO_RELEASES_URL");

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Manifest {
    pub version: String,
    pub engine: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Asset {
    pub platform: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
}

/// Checks the manifest's signature against `public_key` before reading it.
pub fn verify(
    public_key: &[u8; 32],
    manifest: &[u8],
    signature_hex: &str,
) -> Result<Manifest, String> {
    let signature = hex::decode(signature_hex.trim())
        .ok()
        .and_then(|b| Signature::from_slice(&b).ok())
        .ok_or("the release signature is unreadable")?;
    VerifyingKey::from_bytes(public_key)
        .and_then(|key| key.verify_strict(manifest, &signature))
        .map_err(|_| "the release isn't signed by Ukoo")?;
    serde_json::from_slice(manifest).map_err(|e| format!("the release manifest is unreadable: {e}"))
}

/// `major.minor.patch`, compared numerically.
pub fn newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<(u64, u64, u64)> {
        let mut parts = v.trim().split('.').map(|p| p.parse::<u64>().ok());
        Some((parts.next()??, parts.next()??, parts.next()??))
    };
    matches!((parse(candidate), parse(current)), (Some(c), Some(v)) if c > v)
}

fn failed(reason: impl Into<String>) -> NodeError {
    NodeError::UpdateFailed(reason.into())
}

fn release_key() -> Result<[u8; 32], NodeError> {
    RELEASE_PUBLIC_KEY
        .and_then(|k| hex::decode(k).ok())
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or(NodeError::UpdatesUnavailable)
}

fn releases_url() -> Result<String, NodeError> {
    std::env::var("UKOO_RELEASES_URL")
        .ok()
        .or_else(|| RELEASES_URL.map(str::to_string))
        .map(|u| u.trim_end_matches('/').to_string())
        .ok_or(NodeError::UpdatesUnavailable)
}

/// The latest signed release, if it's newer than this one.
pub async fn available() -> Result<Option<Manifest>, NodeError> {
    let key = release_key()?;
    let base = releases_url()?;
    let http = reqwest::Client::new();
    let fetch = |name: &'static str| {
        let http = http.clone();
        let url = format!("{base}/{name}");
        async move {
            http.get(&url)
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(|e| failed(format!("couldn't download {name}: {e}")))?
                .bytes()
                .await
                .map_err(|e| failed(format!("couldn't download {name}: {e}")))
        }
    };
    let manifest = fetch("manifest.json").await?;
    let signature = fetch("manifest.json.sig").await?;
    let manifest = verify(&key, &manifest, &String::from_utf8_lossy(&signature)).map_err(failed)?;
    Ok(newer(&manifest.version, env!("CARGO_PKG_VERSION")).then_some(manifest))
}

/// Downloads this platform's archive, checks it, and swaps it in next to the running binary.
pub async fn install(manifest: &Manifest, platform: &str) -> Result<(), NodeError> {
    let asset = manifest
        .assets
        .iter()
        .find(|a| a.platform == platform)
        .ok_or_else(|| {
            failed(format!(
                "version {} has no build for {platform}",
                manifest.version
            ))
        })?;
    let bytes = reqwest::get(&asset.url)
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| failed(format!("couldn't download the update: {e}")))?
        .bytes()
        .await
        .map_err(|e| failed(format!("couldn't download the update: {e}")))?;
    if hex::encode(Sha256::digest(&bytes)) != asset.sha256 {
        return Err(failed("the download doesn't match the signed release"));
    }
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| failed("can't find the install folder"))?;
    let staging = dir.join(".update");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    unpack(&bytes, &asset.url, &staging)?;
    swap(dir, &staging, &exe)?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(())
}

fn unpack(bytes: &[u8], url: &str, into: &Path) -> Result<(), NodeError> {
    if url.ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|e| failed(format!("the update archive is unreadable: {e}")))?;
        archive
            .extract(into)
            .map_err(|e| failed(format!("couldn't unpack the update: {e}")))
    } else {
        let mut data = Vec::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut data)
            .map_err(|e| failed(format!("the update archive is unreadable: {e}")))?;
        tar::Archive::new(data.as_slice())
            .unpack(into)
            .map_err(|e| failed(format!("couldn't unpack the update: {e}")))
    }
}

/// Moves the new binary and engine into place, keeping the old ones until it worked. A running
/// executable can be renamed on every platform, which is what makes this safe while serving.
fn swap(dir: &Path, staging: &Path, exe: &Path) -> Result<(), NodeError> {
    let name = exe
        .file_name()
        .ok_or_else(|| failed("can't find the binary"))?;
    let new_exe = staging.join(name);
    let new_engine = staging.join("engine");
    if !new_exe.is_file() || !new_engine.is_dir() {
        return Err(failed("the update archive is missing files"));
    }
    let old = |p: &Path| -> PathBuf {
        let mut s = p.as_os_str().to_owned();
        s.push(".old");
        PathBuf::from(s)
    };
    let engine = dir.join("engine");
    let _ = std::fs::remove_file(old(exe));
    let _ = std::fs::remove_dir_all(old(&engine));
    std::fs::rename(exe, old(exe))?;
    if let Err(e) = std::fs::rename(&new_exe, exe) {
        let _ = std::fs::rename(old(exe), exe);
        return Err(e.into());
    }
    if engine.exists() {
        std::fs::rename(&engine, old(&engine))?;
    }
    std::fs::rename(&new_engine, &engine)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use super::*;

    fn manifest() -> Vec<u8> {
        br#"{"version":"0.2.0","engine":"b11090","published_at":"2026-10-20T09:00:00Z","min_relayer":"0.2.0",
            "assets":[{"platform":"macos-aarch64-metal","url":"https://x/a.tar.gz","sha256":"ab","size_bytes":1}]}"#
            .to_vec()
    }

    #[test]
    fn only_a_manifest_signed_by_the_release_key_is_read() {
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let public = key.verifying_key().to_bytes();
        let sig = hex::encode(key.sign(&manifest()).to_bytes());
        let m = verify(&public, &manifest(), &sig).unwrap();
        assert_eq!(m.version, "0.2.0");
        assert_eq!(m.assets[0].platform, "macos-aarch64-metal");

        let mut tampered = manifest();
        tampered[14] = b'9';
        assert!(verify(&public, &tampered, &sig).is_err());
        let other = SigningKey::from_bytes(&[1u8; 32])
            .verifying_key()
            .to_bytes();
        assert!(verify(&other, &manifest(), &sig).is_err());
        assert!(verify(&public, &manifest(), "zz").is_err());
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.9", "0.2.0"));
        assert!(!newer("garbage", "0.1.0"));
    }

    #[test]
    fn an_update_swaps_binary_and_engine_and_keeps_the_old_ones() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("ukoo-node");
        std::fs::write(&exe, "old binary").unwrap();
        std::fs::create_dir(dir.path().join("engine")).unwrap();
        std::fs::write(dir.path().join("engine/llama-server"), "old engine").unwrap();
        let staging = dir.path().join(".update");
        std::fs::create_dir_all(staging.join("engine")).unwrap();
        std::fs::write(staging.join("ukoo-node"), "new binary").unwrap();
        std::fs::write(staging.join("engine/llama-server"), "new engine").unwrap();

        swap(dir.path(), &staging, &exe).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new binary");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("engine/llama-server")).unwrap(),
            "new engine"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("ukoo-node.old")).unwrap(),
            "old binary"
        );
    }

    #[test]
    fn a_tar_gz_release_unpacks() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "engine/llama-server", &b"bin"[..])
                .unwrap();
            builder.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
        let bytes = gz.finish().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unpack(&bytes, "x.tar.gz", dir.path()).unwrap();
        assert_eq!(
            std::fs::read(dir.path().join("engine/llama-server")).unwrap(),
            b"bin"
        );
    }
}
