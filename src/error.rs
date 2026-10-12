use std::path::PathBuf;

// Every message here is shown to node operators.
#[derive(thiserror::Error, Debug)]
pub enum NodeError {
    #[error("Couldn't reach Ukoo at {url}: {reason}")]
    RelayerUnreachable { url: String, reason: String },
    #[error("No model in Ukoo's list fits this machine yet.")]
    NothingFits,
    #[error(
        "'{0}' isn't a model this machine can serve. Run `ukoo-node setup` to see the options."
    )]
    UnknownModel(String),
    #[error("No model was chosen. Run `ukoo-node setup` again, or pass --model <id>.")]
    NoChoice,
    #[error("Not enough free disk space: {needed} needed, {free} free.")]
    NotEnoughDisk { needed: String, free: String },
    #[error("Download of {file} failed: {reason}")]
    DownloadFailed { file: String, reason: String },
    #[error(
        "{file} doesn't match Ukoo's approved fingerprint, so it was deleted. Run `ukoo-node setup` to download it again."
    )]
    ChecksumMismatch { file: String },
    #[error("This node isn't set up yet. Run `ukoo-node setup` first.")]
    NotSetUp,
    #[error(
        "The model engine (llama-server) wasn't found. Reinstall ukoo-node, or set UKOO_LLAMA_SERVER to its path."
    )]
    EngineMissing,
    #[error("The model engine at {path} isn't the version this ukoo-node ships with ({expected}).")]
    EngineWrongVersion {
        path: PathBuf,
        expected: &'static str,
    },
    #[error("The model engine stopped while loading. Details are in {log}.")]
    EngineFailed { log: PathBuf },
    #[error("The model engine stopped unexpectedly. Details are in {log}.")]
    EngineStopped { log: PathBuf },
    #[error("The model loaded but didn't answer a test prompt. Details are in {log}.")]
    SelfTestFailed { log: PathBuf },
    #[error("ukoo-node is already running on this machine.")]
    AlreadyRunning,
    #[error("Serving hours look like 19:00-08:00 (24-hour clock, this machine's local time).")]
    InvalidServeHours,
    #[error("Prices are in USD per 1M tokens, like `ukoo-node price 0.10 0.30`.")]
    InvalidPrice,
    #[error("That's above the limit for {model}: ${max_in} in / ${max_out} out per 1M tokens.")]
    PriceAboveLimit {
        model: String,
        max_in: String,
        max_out: String,
    },
    #[error("This ukoo-node build can't update itself. Reinstall it from https://ukoo.network.")]
    UpdatesUnavailable,
    #[error("Update failed: {0}. ukoo-node is unchanged.")]
    UpdateFailed(String),
    #[error("Couldn't set up the background service: {0}.")]
    ServiceFailed(String),
    /// The relayer's own message, written for operators.
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}
