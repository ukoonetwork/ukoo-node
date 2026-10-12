use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::json;
use tokio::process::{Child, Command};

use crate::error::NodeError;
use crate::paths;
use crate::setup::NodeState;

/// The llama.cpp build every ukoo-node release ships with (scripts/fetch-engine.sh pins its checksum).
pub const ENGINE_BUILD: &str = "b11090";
const ENGINE_VERSION_MARKER: &str = "build 11090";
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);

#[cfg(windows)]
const BINARY: &str = "llama-server.exe";
#[cfg(not(windows))]
const BINARY: &str = "llama-server";

pub struct Engine {
    child: Child,
    port: u16,
    api_key: String,
}

/// The engine ships next to ukoo-node in `engine/`; UKOO_LLAMA_SERVER overrides it.
pub fn locate() -> Result<PathBuf, NodeError> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("UKOO_LLAMA_SERVER") {
        candidates.push(PathBuf::from(path));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        candidates.push(dir.join("engine").join(BINARY));
    }
    // `cargo run` builds land in target/, away from the fetched engine.
    if cfg!(debug_assertions) {
        candidates.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("engine")
                .join(BINARY),
        );
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or(NodeError::EngineMissing)
}

/// Refuses any engine but the pinned build.
pub async fn check_version(path: &Path) -> Result<(), NodeError> {
    let out = Command::new(path).arg("--version").output().await?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if text.contains(ENGINE_VERSION_MARKER) {
        Ok(())
    } else {
        Err(NodeError::EngineWrongVersion {
            path: path.to_path_buf(),
            expected: ENGINE_BUILD,
        })
    }
}

impl Engine {
    /// Starts llama-server on the verified file, reachable only from this machine and only with a
    /// per-run key, so other local programs can't use the model for free.
    pub async fn start(binary: &Path, state: &NodeState) -> Result<Self, NodeError> {
        let port = free_port()?;
        let api_key = random_hex(32);
        let log = paths::engine_log();
        std::fs::create_dir_all(log.parent().expect("log has a parent"))?;
        let log_file = std::fs::File::create(&log)?;

        let context = state.serve.context_length * state.parallel_slots;
        let child = Command::new(binary)
            .arg("--model")
            .arg(&state.files[0].path)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["--ctx-size", &context.to_string()])
            .args(["--parallel", &state.parallel_slots.to_string()])
            .args(["--n-gpu-layers", "999", "--jinja"])
            .args(["--alias", &state.model_id])
            .args(["--api-key", &api_key])
            .stdout(Stdio::from(log_file.try_clone()?))
            .stderr(Stdio::from(log_file))
            .kill_on_drop(true)
            .spawn()?;

        let mut engine = Self {
            child,
            port,
            api_key,
        };
        engine.wait_until_loaded().await?;
        Ok(engine)
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// What jobs from the relayer use to reach this engine.
    pub fn client(&self) -> EngineClient {
        EngineClient {
            url: format!("{}/v1/chat/completions", self.base_url()),
            api_key: self.api_key.clone(),
            http: reqwest::Client::new(),
        }
    }

    async fn wait_until_loaded(&mut self) -> Result<(), NodeError> {
        let failed = || NodeError::EngineFailed {
            log: paths::engine_log(),
        };
        let deadline = tokio::time::Instant::now() + LOAD_TIMEOUT;
        loop {
            if self.child.try_wait()?.is_some() || tokio::time::Instant::now() > deadline {
                return Err(failed());
            }
            // /health answers 503 while the model is loading, 200 once it can serve.
            if let Ok(r) = reqwest::get(format!("{}/health", self.base_url())).await {
                if r.status().is_success() {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// One short completion through the OpenAI-compatible endpoint the relayer will use.
    pub async fn self_test(&self, model_id: &str) -> Result<(), NodeError> {
        let failed = || NodeError::SelfTestFailed {
            log: paths::engine_log(),
        };
        let response = reqwest::Client::new()
            .post(format!("{}/v1/chat/completions", self.base_url()))
            .bearer_auth(&self.api_key)
            .json(&json!({
                "model": model_id,
                "messages": [{ "role": "user", "content": "Reply with the single word: ready" }],
                "max_tokens": 256,
                "temperature": 0
            }))
            .send()
            .await
            .map_err(|_| failed())?;
        if !response.status().is_success() {
            return Err(failed());
        }
        let body: serde_json::Value = response.json().await.map_err(|_| failed())?;
        if body["choices"].as_array().is_some_and(|c| !c.is_empty()) {
            Ok(())
        } else {
            Err(failed())
        }
    }

    /// Resolves if the engine exits on its own.
    pub async fn exited(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    pub async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

#[derive(Clone)]
pub struct EngineClient {
    url: String,
    api_key: String,
    http: reqwest::Client,
}

impl EngineClient {
    pub async fn chat(&self, request_json: Vec<u8>) -> reqwest::Result<reqwest::Response> {
        self.http
            .post(&self.url)
            .bearer_auth(&self.api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_json)
            .send()
            .await
    }
}

fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()?
        .port())
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).expect("OS random number generator unavailable");
    hex::encode(buf)
}
