mod awake;
mod control;
mod engine;
mod error;
mod link;
mod paths;
mod release;
mod service;
mod setup;
mod verify;

use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use clap::{Parser, Subcommand};
use rust_decimal::Decimal;
use tokio::sync::watch;

use control::ServeHours;
use engine::Engine;
use error::NodeError;
use paths::gib;
use setup::NodeState;

#[derive(Parser)]
#[command(
    name = "ukoo-node",
    version,
    about = "Serve AI models on the Ukoo network and get paid for it."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check this machine, pick a model that fits, download and verify it.
    Setup {
        /// Ukoo relayer to fetch the approved model list from.
        #[arg(
            long,
            env = "UKOO_RELAYER_URL",
            default_value = "https://api.ukoo.network"
        )]
        relayer: String,
        /// Skip the menu and choose this model ID.
        #[arg(long)]
        model: Option<String>,
        /// Requests served at the same time. Each one needs its own context memory.
        #[arg(long, default_value_t = 1)]
        slots: u64,
    },
    /// Verify the model and start serving it.
    Start,
    /// Show what this node is set up to serve.
    Status,
    /// Stop serving and free this machine's memory, after requests in progress finish.
    Pause,
    /// Serve again after `pause`.
    Resume,
    /// Link this node to your Ukoo account, so its earnings are paid to you.
    Link,
    /// Serve only during these hours each day, e.g. `19:00-08:00`, or `off` to serve any time.
    Schedule {
        /// Hours in this machine's local time, 24-hour clock.
        hours: String,
    },
    /// Show or set your price in USD per 1M input and output tokens, e.g. `0.10 0.30`, or `default`.
    Price {
        input: Option<String>,
        output: Option<String>,
    },
    /// Install the latest release signed by Ukoo.
    Update,
    /// Turn automatic updates `on` or `off`.
    Autoupdate { setting: OnOff },
    /// Run ukoo-node in the background, starting with this computer.
    Service { action: ServiceAction },
}

#[derive(Clone, clap::ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Clone, clap::ValueEnum)]
enum ServiceAction {
    Install,
    Uninstall,
    Status,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Setup {
            relayer,
            model,
            slots,
        } => setup::run(&relayer, model, slots.max(1)).await,
        Commands::Start => start().await,
        Commands::Status => status().await,
        Commands::Pause => set_paused(true).await,
        Commands::Resume => set_paused(false).await,
        Commands::Schedule { hours } => schedule(&hours).await,
        Commands::Link => link_account().await,
        Commands::Price { input, output } => price(input, output).await,
        Commands::Update => update().await,
        Commands::Autoupdate { setting } => autoupdate(setting),
        Commands::Service { action } => service_command(action),
    };
    if let Err(err) = result {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}

/// Pausing waits this long for requests in progress before unloading anyway.
const DRAIN_LIMIT: Duration = Duration::from_secs(600);
/// How often serving hours are re-checked.
const SCHEDULE_TICK: Duration = Duration::from_secs(30);
/// A running node looks for a signed update an hour after starting, then daily.
const UPDATE_FIRST_CHECK: Duration = Duration::from_secs(3600);
const UPDATE_EVERY: Duration = Duration::from_secs(24 * 3600);

async fn start() -> Result<(), NodeError> {
    let state = NodeState::load()?;
    let control = control::Control::open().await?;
    let identity = link::load_or_create(&paths::identity_file())?;

    println!(
        "Checking {} against Ukoo's fingerprint…",
        state.display_name
    );
    for local in &state.files {
        verify::verify_file(&local.path, &local.file).await?;
    }

    let binary = engine::locate()?;
    engine::check_version(&binary).await?;
    println!("Node key: {}", link::public_key_hex(&identity));

    let hardware = setup::probe(&paths::data_dir());
    let fingerprints = verify::Fingerprints::new(
        state
            .files
            .iter()
            .map(|f| (f.path.clone(), f.file.sha256.clone()))
            .collect(),
    );
    let (serving, serving_rx) = watch::channel(false);
    let (price_tx, price_rx) = watch::channel(asking(&state));
    let slot = link::EngineSlot::default();
    let active = Arc::new(AtomicU32::new(0));
    let name = state.display_name.clone();
    let relayer_url = state.relayer_url.clone();
    let node = link::Node {
        engine: slot.clone(),
        identity,
        platform: hardware.platform(),
        usable_memory_bytes: hardware.usable_bytes(),
        serving: serving_rx,
        active: active.clone(),
        state: state.clone(),
        price: price_rx,
    };
    let link = link::run(node, fingerprints);
    tokio::pin!(link);

    let mut settings = state.clone();
    let mut engine: Option<Engine> = None;
    let mut awake: Option<awake::KeepAwake> = None;
    let mut served_before = false;
    let mut tick = tokio::time::interval(SCHEDULE_TICK);
    let mut update_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + UPDATE_FIRST_CHECK,
        UPDATE_EVERY,
    );
    let platform = hardware.platform();
    if !control::serving_now(&settings) {
        println!("Not serving yet: {}.", not_serving_reason(&settings));
    }

    let result = loop {
        let want = control::serving_now(&settings);
        if want && engine.is_none() {
            println!("Loading {name} into memory…");
            let loaded = Engine::start(&binary, &state).await?;
            loaded.self_test(&state.model_id).await?;
            *slot.write().expect("engine lock") = Some(loaded.client());
            engine = Some(loaded);
            awake = Some(awake::KeepAwake::hold());
            serving.send_replace(true);
            if served_before {
                println!("{name} is loaded again and taking requests.");
            } else {
                println!(
                    "{name} is verified, loaded and answering on this machine.\nThis computer won't go to sleep while it's serving; run `ukoo-node pause` to let it.\nConnecting to Ukoo at {relayer_url}… Press Ctrl-C to stop."
                );
            }
            served_before = true;
        } else if !want && engine.is_some() {
            serving.send_replace(false);
            println!(
                "{}: finishing requests in progress, then unloading {name}…",
                stop_reason(&settings)
            );
            let deadline = tokio::time::Instant::now() + DRAIN_LIMIT;
            while active.load(Ordering::Acquire) > 0 && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            *slot.write().expect("engine lock") = None;
            engine.take().expect("engine is running").stop().await;
            awake = None;
            println!("{name} is unloaded and this machine's memory is free.");
        }

        tokio::select! {
            _ = tokio::signal::ctrl_c() => break Ok(None),
            _ = control.changed() => {
                settings = NodeState::load()?;
                price_tx.send_if_modified(|p| {
                    let changed = *p != asking(&settings);
                    *p = asking(&settings);
                    changed
                });
            }
            _ = tick.tick() => {}
            _ = update_tick.tick(), if settings.autoupdate => match release::available().await {
                Ok(Some(manifest)) => break Ok(Some(manifest)),
                Ok(None) => {}
                Err(err) => println!("Couldn't check for updates: {err}"),
            },
            _ = exited(&mut engine) => break Err(NodeError::EngineStopped { log: paths::engine_log() }),
            rejected = &mut link => break Err(rejected),
        }
    };
    if let Ok(Some(manifest)) = &result {
        println!(
            "Updating to ukoo-node {}: finishing requests in progress first…",
            manifest.version
        );
        serving.send_replace(false);
        let deadline = tokio::time::Instant::now() + DRAIN_LIMIT;
        while active.load(Ordering::Acquire) > 0 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if let Some(engine) = engine.take() {
            engine.stop().await;
        }
        match release::install(manifest, &platform).await {
            Ok(()) => println!("Updated to ukoo-node {}. Restarting…", manifest.version),
            Err(err) => println!(
                "Automatic update failed ({err}). Restarting ukoo-node {}; it will try again tomorrow.",
                env!("CARGO_PKG_VERSION")
            ),
        }
        drop(control);
        drop(awake);
        return Err(restart());
    }
    let result = result.map(|_| ());
    if let Some(engine) = engine {
        engine.stop().await;
    }
    if result.is_ok() {
        println!("\nStopped. {name} is no longer loaded.");
    }
    result
}

/// Replaces this process with the (possibly just updated) binary, same arguments.
fn restart() -> NodeError {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => return err.into(),
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(exe).args(args).exec().into()
    }
    #[cfg(not(unix))]
    {
        match std::process::Command::new(exe).args(args).spawn() {
            Ok(_) => std::process::exit(0),
            Err(err) => err.into(),
        }
    }
}

async fn update() -> Result<(), NodeError> {
    println!("Checking for updates…");
    let Some(manifest) = release::available().await? else {
        println!(
            "ukoo-node {} is the latest version.",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(());
    };
    println!("Updating to ukoo-node {}…", manifest.version);
    let platform = setup::probe(&paths::data_dir()).platform();
    release::install(&manifest, &platform).await?;
    println!(
        "Updated to ukoo-node {}. A running node switches over when it next restarts.",
        manifest.version
    );
    Ok(())
}

fn autoupdate(setting: OnOff) -> Result<(), NodeError> {
    let mut state = NodeState::load()?;
    state.autoupdate = matches!(setting, OnOff::On);
    state.save()?;
    if state.autoupdate {
        println!(
            "Automatic updates are on. ukoo-node checks once a day and only installs releases signed by Ukoo."
        );
    } else {
        println!("Automatic updates are off. Run `ukoo-node update` to update by hand.");
    }
    Ok(())
}

fn service_command(action: ServiceAction) -> Result<(), NodeError> {
    match action {
        ServiceAction::Install => {
            NodeState::load()?;
            service::install()?;
            let when = if cfg!(windows) { "sign in" } else { "log in" };
            println!(
                "ukoo-node now runs in the background and starts when you {when}.\nLogs: {}\nStop it with `ukoo-node service uninstall`.",
                service::log_file().display()
            );
            if cfg!(target_os = "linux") {
                println!(
                    "To keep serving after you log out (e.g. on a server), run: sudo loginctl enable-linger $USER"
                );
            }
        }
        ServiceAction::Uninstall => {
            service::uninstall()?;
            println!("ukoo-node no longer runs in the background.");
        }
        ServiceAction::Status => println!(
            "Background service: {}",
            match service::state()? {
                service::State::Running => "installed, running",
                service::State::Stopped => "installed, not running",
                service::State::NotInstalled => "not installed",
            }
        ),
    }
    Ok(())
}

async fn exited(engine: &mut Option<Engine>) {
    match engine {
        Some(engine) => {
            let _ = engine.exited().await;
        }
        None => std::future::pending().await,
    }
}

fn not_serving_reason(state: &NodeState) -> String {
    match state.serve_hours {
        _ if state.paused => "paused. Run `ukoo-node resume` to serve".to_string(),
        Some(hours) => format!(
            "outside serving hours ({hours}). Serving starts at {}",
            hours.starts_at()
        ),
        None => "paused".to_string(),
    }
}

fn stop_reason(state: &NodeState) -> String {
    match state.serve_hours {
        _ if state.paused => "Paused".to_string(),
        Some(hours) => format!("Outside serving hours ({hours})"),
        None => "Paused".to_string(),
    }
}

async fn set_paused(paused: bool) -> Result<(), NodeError> {
    let mut state = NodeState::load()?;
    state.paused = paused;
    state.save()?;
    let running = control::poke().await;
    let name = &state.display_name;
    match (paused, running) {
        (true, true) => println!(
            "Paused. Requests in progress will finish, then {name} is unloaded to free this machine's memory. Run `ukoo-node resume` to serve again."
        ),
        (true, false) => println!(
            "Paused. The next `ukoo-node start` won't serve until you run `ukoo-node resume`."
        ),
        (false, false) => println!("Resumed. Run `ukoo-node start` to serve."),
        (false, true) => match state.serve_hours.filter(|h| !h.contains_now()) {
            Some(hours) => println!(
                "Resumed, but it's outside your serving hours ({hours}), so serving starts at {}.",
                hours.starts_at()
            ),
            None => println!("Resumed. {name} is loading and will take requests again shortly."),
        },
    }
    Ok(())
}

/// Asks the relayer for a one-time code, proving this machine holds the node's key.
async fn link_account() -> Result<(), NodeError> {
    use ed25519_dalek::Signer;

    let state = NodeState::load()?;
    let identity = link::load_or_create(&paths::identity_file())?;
    let public_key = identity.verifying_key().to_bytes();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is before 1970")
        .as_secs() as i64;
    let signed = [
        b"ukoo-node-link-v1".as_slice(),
        &timestamp.to_be_bytes(),
        &public_key,
    ]
    .concat();
    let body = serde_json::json!({
        "public_key": hex::encode(public_key),
        "timestamp": timestamp,
        "signature": hex::encode(identity.sign(&signed).to_bytes()),
    });
    let url = format!(
        "{}/node/v1/link-codes",
        state.relayer_url.trim_end_matches('/')
    );
    let unreachable = |reason: String| NodeError::RelayerUnreachable {
        url: state.relayer_url.clone(),
        reason,
    };
    let response = reqwest::Client::new()
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| unreachable(e.to_string()))?;
    let ok = response.status().is_success();
    let reply: serde_json::Value = response
        .json()
        .await
        .map_err(|e| unreachable(e.to_string()))?;
    if !ok {
        let message = reply["error"]["message"]
            .as_str()
            .unwrap_or("unknown error");
        return Err(NodeError::Rejected(message.to_string()));
    }
    println!(
        "Enter this code in your Ukoo dashboard within 10 minutes: {}",
        reply["code"].as_str().unwrap_or_default()
    );
    Ok(())
}

fn asking(state: &NodeState) -> (String, String) {
    (
        state.ask_per_1m_input_usd.clone(),
        state.ask_per_1m_output_usd.clone(),
    )
}

/// From $0 up to twice the model's default.
async fn price(input: Option<String>, output: Option<String>) -> Result<(), NodeError> {
    let mut state = NodeState::load()?;
    let limit = state.price_limit();
    let (ask_in, ask_out) = match (input.as_deref(), output) {
        (None, _) => {
            println!(
                "Price:  ${} in / ${} out per 1M tokens",
                state.ask_per_1m_input_usd, state.ask_per_1m_output_usd
            );
            if let Some((max_in, max_out)) = limit {
                println!(
                    "Limit:  up to ${} / ${} for {}. Change it with `ukoo-node price <input> <output>`, or go back to Ukoo's default with `ukoo-node price default`.",
                    max_in.normalize(),
                    max_out.normalize(),
                    state.display_name
                );
            }
            return Ok(());
        }
        (Some("default"), None) => match (
            state.default_ask_per_1m_input_usd.clone(),
            state.default_ask_per_1m_output_usd.clone(),
        ) {
            (Some(ask_in), Some(ask_out)) => (ask_in, ask_out),
            _ => return Err(NodeError::InvalidPrice),
        },
        (Some(ask_in), Some(ask_out)) => {
            let read = |s: &str| {
                Decimal::from_str(s.trim().trim_start_matches('$'))
                    .ok()
                    .filter(|d| !d.is_sign_negative())
                    .ok_or(NodeError::InvalidPrice)
            };
            let (ask_in, ask_out) = (read(ask_in)?, read(&ask_out)?);
            if let Some((max_in, max_out)) = limit
                && (ask_in > max_in || ask_out > max_out)
            {
                return Err(NodeError::PriceAboveLimit {
                    model: state.display_name,
                    max_in: max_in.normalize().to_string(),
                    max_out: max_out.normalize().to_string(),
                });
            }
            (
                ask_in.normalize().to_string(),
                ask_out.normalize().to_string(),
            )
        }
        _ => return Err(NodeError::InvalidPrice),
    };
    state.ask_per_1m_input_usd = ask_in;
    state.ask_per_1m_output_usd = ask_out;
    state.save()?;
    let running = control::poke().await;
    println!(
        "Price set to ${} in / ${} out per 1M tokens.",
        state.ask_per_1m_input_usd, state.ask_per_1m_output_usd
    );
    if running {
        println!(
            "The running node switches over within seconds; requests in progress finish at the old price."
        );
    } else {
        println!("It applies the next time ukoo-node starts.");
    }
    Ok(())
}

async fn schedule(hours: &str) -> Result<(), NodeError> {
    let mut state = NodeState::load()?;
    state.serve_hours = if hours.trim().eq_ignore_ascii_case("off") {
        None
    } else {
        Some(
            hours
                .parse::<ServeHours>()
                .map_err(|_| NodeError::InvalidServeHours)?,
        )
    };
    state.save()?;
    control::poke().await;
    match state.serve_hours {
        Some(hours) => println!(
            "Serving hours set to {hours}, this machine's local time. Outside them, {} is unloaded and the memory is free.",
            state.display_name
        ),
        None => println!(
            "Serving hours removed. This node serves whenever it's running and not paused."
        ),
    }
    Ok(())
}

async fn status() -> Result<(), NodeError> {
    let state = NodeState::load()?;
    let identity = link::load_or_create(&paths::identity_file())?;
    let downloaded = state.files.iter().all(|f| f.path.exists());

    println!("Model:     {} ({})", state.display_name, state.version);
    println!(
        "Files:     {} ({})",
        if downloaded {
            "downloaded"
        } else {
            "missing — run `ukoo-node setup`"
        },
        gib(state.files.iter().map(|f| f.file.size_bytes).sum()),
    );
    println!(
        "Serves:    {} request(s) at a time, {} tokens of context each",
        state.parallel_slots, state.serve.context_length
    );
    println!(
        "Price:     ${} in / ${} out per 1M tokens",
        state.ask_per_1m_input_usd, state.ask_per_1m_output_usd
    );
    println!(
        "Engine:    {}",
        match engine::locate() {
            Ok(path) => match engine::check_version(&path).await {
                Ok(()) => format!("llama.cpp {} at {}", engine::ENGINE_BUILD, path.display()),
                Err(e) => e.to_string(),
            },
            Err(e) => e.to_string(),
        }
    );
    println!(
        "Serving:   {}",
        match state.serve_hours {
            _ if state.paused => "no, paused (run `ukoo-node resume`)".to_string(),
            Some(h) if h.contains_now() => format!("{h}, this machine's time (now inside them)"),
            Some(h) => format!("{h}, this machine's time (now outside them)"),
            None => "whenever ukoo-node is running".to_string(),
        }
    );
    println!("Node key:  {}", link::public_key_hex(&identity));
    println!("Relayer:   {}", state.relayer_url);
    Ok(())
}
