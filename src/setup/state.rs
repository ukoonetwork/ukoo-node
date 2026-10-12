use std::path::PathBuf;
use std::str::FromStr;

use rust_decimal::Decimal;

use serde::{Deserialize, Serialize};

use super::{ModelFile, RegistryModel, ServeParams};
use crate::control::ServeHours;
use crate::error::NodeError;
use crate::paths;

/// What this node serves, saved by `setup` and read by `start`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeState {
    pub relayer_url: String,
    pub model_id: String,
    pub display_name: String,
    pub version: String,
    pub files: Vec<LocalFile>,
    pub serve: ServeParams,
    pub parallel_slots: u64,
    pub ask_per_1m_input_usd: String,
    pub ask_per_1m_output_usd: String,
    /// The model's default ask when this node was set up; `ukoo-node price default` restores it.
    #[serde(default)]
    pub default_ask_per_1m_input_usd: Option<String>,
    #[serde(default)]
    pub default_ask_per_1m_output_usd: Option<String>,
    /// Set by `ukoo-node pause`, cleared by `resume`; kept across restarts.
    #[serde(default)]
    pub paused: bool,
    /// Set by `ukoo-node schedule`.
    #[serde(default)]
    pub serve_hours: Option<ServeHours>,
    /// Install signed updates automatically; on unless turned off.
    #[serde(default = "on")]
    pub autoupdate: bool,
}

fn on() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalFile {
    pub path: PathBuf,
    pub file: ModelFile,
}

impl NodeState {
    pub fn new(
        relayer_url: &str,
        model: &RegistryModel,
        files: Vec<LocalFile>,
        parallel_slots: u64,
    ) -> Self {
        Self {
            relayer_url: relayer_url.to_string(),
            model_id: model.model_id.clone(),
            display_name: model.display_name.clone(),
            version: model.version.clone(),
            files,
            serve: model.serve,
            parallel_slots,
            ask_per_1m_input_usd: model.default_ask_per_1m_input_usd.clone(),
            ask_per_1m_output_usd: model.default_ask_per_1m_output_usd.clone(),
            default_ask_per_1m_input_usd: Some(model.default_ask_per_1m_input_usd.clone()),
            default_ask_per_1m_output_usd: Some(model.default_ask_per_1m_output_usd.clone()),
            paused: false,
            serve_hours: None,
            autoupdate: true,
        }
    }

    /// The most this node may ask, twice the model's default; the relayer enforces it too.
    pub fn price_limit(&self) -> Option<(Decimal, Decimal)> {
        let twice = |d: &Option<String>| {
            d.as_deref()
                .and_then(|d| Decimal::from_str(d).ok())
                .map(|d| d * Decimal::TWO)
        };
        Some((
            twice(&self.default_ask_per_1m_input_usd)?,
            twice(&self.default_ask_per_1m_output_usd)?,
        ))
    }

    pub fn load() -> Result<Self, NodeError> {
        let raw = std::fs::read(paths::state_file()).map_err(|_| NodeError::NotSetUp)?;
        serde_json::from_slice(&raw).map_err(|_| NodeError::NotSetUp)
    }

    pub fn save(&self) -> Result<(), NodeError> {
        let path = paths::state_file();
        std::fs::create_dir_all(path.parent().expect("state file has a parent"))?;
        std::fs::write(
            path,
            serde_json::to_vec_pretty(self).expect("state serializes"),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_price_limit_is_twice_the_default_and_unknown_for_old_setups() {
        let mut state: NodeState = serde_json::from_value(serde_json::json!({
            "relayer_url": "https://api.ukoo.network", "model_id": "qwen3-14b",
            "display_name": "Qwen3 14B", "version": "Q4_K_M@abc", "files": [],
            "serve": { "context_length": 8192, "kv_bytes_per_token": 1 }, "parallel_slots": 1,
            "ask_per_1m_input_usd": "0.12", "ask_per_1m_output_usd": "0.24"
        }))
        .unwrap();
        assert_eq!(state.price_limit(), None);
        state.default_ask_per_1m_input_usd = Some("0.12".into());
        state.default_ask_per_1m_output_usd = Some("0.24".into());
        assert_eq!(
            state.price_limit(),
            Some((Decimal::new(24, 2), Decimal::new(48, 2)))
        );
    }
}
