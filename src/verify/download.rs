use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::StatusCode;
use reqwest::header::RANGE;
use tokio::io::AsyncWriteExt;

use super::verify_file;
use crate::error::NodeError;
use crate::setup::ModelFile;

/// Downloads `file` to `dest` (resuming a partial download), then verifies it against the
/// registry fingerprint. An already-present, matching file is not downloaded again.
pub async fn download_verified(file: &ModelFile, dest: &Path) -> Result<(), NodeError> {
    if dest.exists() && verify_file(dest, file).await.is_ok() {
        println!("  {} already downloaded and verified", file.file_name());
        return Ok(());
    }
    tokio::fs::create_dir_all(dest.parent().expect("model path has a parent")).await?;

    let part = part_path(dest);
    fetch_to(file, &part).await?;
    tokio::fs::rename(&part, dest).await?;
    verify_file(dest, file).await?;
    println!("  {} verified", file.file_name());
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .expect("model path has a file name")
        .to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

async fn fetch_to(file: &ModelFile, part: &Path) -> Result<(), NodeError> {
    let failed = |reason: String| NodeError::DownloadFailed {
        file: file.file_name().to_string(),
        reason,
    };

    let already = tokio::fs::metadata(part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if already == file.size_bytes {
        return Ok(());
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("ukoo-node/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| failed(e.to_string()))?;
    let mut request = client.get(&file.url);
    if already > 0 {
        request = request.header(RANGE, format!("bytes={already}-"));
    }
    let response = request.send().await.map_err(|e| failed(e.to_string()))?;

    let (mut out, start) = match response.status() {
        StatusCode::PARTIAL_CONTENT => (
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(part)
                .await?,
            already,
        ),
        StatusCode::OK => (tokio::fs::File::create(part).await?, 0),
        status => return Err(failed(format!("the server answered {status}"))),
    };

    let bar = ProgressBar::new(file.size_bytes).with_position(start);
    bar.set_style(
        ProgressStyle::with_template(
            "  {msg} [{bar:30}] {bytes}/{total_bytes} {bytes_per_sec} {eta}",
        )
        .expect("valid template")
        .progress_chars("=> "),
    );
    bar.set_message(file.file_name().to_string());

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| failed(e.to_string()))?;
        out.write_all(&chunk).await?;
        bar.inc(chunk.len() as u64);
    }
    out.flush().await?;
    bar.finish_and_clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const BODY: &[u8] = b"pretend this is a 9 GB model file";

    fn model_file(server: &MockServer) -> ModelFile {
        ModelFile {
            url: format!("{}/m.gguf", server.uri()),
            sha256: hex::encode(Sha256::digest(BODY)),
            size_bytes: BODY.len() as u64,
        }
    }

    #[tokio::test]
    async fn downloads_and_verifies() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/m.gguf"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");

        download_verified(&model_file(&server), &dest)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
    }

    #[tokio::test]
    async fn resumes_a_partial_download() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/m.gguf"))
            .and(header("range", "bytes=10-"))
            .respond_with(ResponseTemplate::new(206).set_body_bytes(&BODY[10..]))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        std::fs::write(part_path(&dest), &BODY[..10]).unwrap();

        download_verified(&model_file(&server), &dest)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
    }

    #[tokio::test]
    async fn a_tampered_download_is_rejected_and_removed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/m.gguf"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(&b"pretend this is a 9 GB model fil!"[..]),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");

        let result = download_verified(&model_file(&server), &dest).await;
        assert!(matches!(result, Err(NodeError::ChecksumMismatch { .. })));
        assert!(!dest.exists());
    }
}
