use std::path::PathBuf;

use directories::ProjectDirs;

/// Where ukoo-node keeps its state, identity key, models and logs.
pub fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("UKOO_NODE_HOME") {
        return PathBuf::from(dir);
    }
    ProjectDirs::from("network", "ukoo", "ukoo-node")
        .expect("no home directory for this user")
        .data_dir()
        .to_path_buf()
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

pub fn state_file() -> PathBuf {
    data_dir().join("node.json")
}

pub fn identity_file() -> PathBuf {
    data_dir().join("identity.key")
}

pub fn engine_log() -> PathBuf {
    data_dir().join("logs").join("engine.log")
}

pub fn gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
}
