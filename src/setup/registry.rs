use serde::{Deserialize, Serialize};

use crate::error::NodeError;

/// ukoo-node's own copy of `GET /node/v1/models`.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryResponse {
    pub models: Vec<RegistryModel>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegistryModel {
    pub model_id: String,
    pub display_name: String,
    pub version: String,
    pub niches: Vec<String>,
    pub license: String,
    pub gguf_files: Vec<ModelFile>,
    pub serve: ServeParams,
    pub default_ask_per_1m_input_usd: String,
    pub default_ask_per_1m_output_usd: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelFile {
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
}

impl ModelFile {
    /// The file name as published (the last path segment of the pinned URL).
    pub fn file_name(&self) -> &str {
        self.url.rsplit('/').next().unwrap_or(&self.url)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ServeParams {
    pub context_length: u64,
    pub kv_bytes_per_token: u64,
}

impl RegistryModel {
    pub fn is_unverified(&self) -> bool {
        self.status == "unverified"
    }

    pub fn download_bytes(&self) -> u64 {
        self.gguf_files.iter().map(|f| f.size_bytes).sum()
    }
}

pub async fn fetch(relayer_url: &str) -> Result<Vec<RegistryModel>, NodeError> {
    let url = format!("{}/node/v1/models", relayer_url.trim_end_matches('/'));
    let unreachable = |reason: String| NodeError::RelayerUnreachable {
        url: relayer_url.to_string(),
        reason,
    };
    let response = reqwest::get(&url)
        .await
        .map_err(|e| unreachable(e.to_string()))?;
    if !response.status().is_success() {
        return Err(unreachable(format!("it answered {}", response.status())));
    }
    let body: RegistryResponse = response
        .json()
        .await
        .map_err(|e| unreachable(e.to_string()))?;
    Ok(body.models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_relayer_contract() {
        let body = r#"{"models":[{"model_id":"qwen3-14b","display_name":"Qwen3 14B","version":"Q4_K_M@530227a7",
            "niches":["coding"],"license":"Apache-2.0",
            "gguf_files":[{"url":"https://huggingface.co/Qwen/Qwen3-14B-GGUF/resolve/abc/Qwen3-14B-Q4_K_M.gguf","sha256":"00","size_bytes":9001752960}],
            "serve":{"context_length":32768,"kv_bytes_per_token":163840},
            "default_ask_per_1m_input_usd":"0.12","default_ask_per_1m_output_usd":"0.24","status":"unverified"}]}"#;
        let parsed: RegistryResponse = serde_json::from_str(body).unwrap();
        let m = &parsed.models[0];
        assert_eq!(m.gguf_files[0].file_name(), "Qwen3-14B-Q4_K_M.gguf");
        assert_eq!(m.download_bytes(), 9001752960);
        assert!(m.is_unverified());
    }
}
