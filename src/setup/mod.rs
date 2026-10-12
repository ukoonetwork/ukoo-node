mod fit;
mod hardware;
mod registry;
mod state;

pub use fit::*;
pub use hardware::*;
pub use registry::*;
pub use state::*;

use std::io::Write;

use crate::error::NodeError;
use crate::paths::{self, gib};
use crate::verify;

pub async fn run(
    relayer_url: &str,
    chosen: Option<String>,
    parallel_slots: u64,
) -> Result<(), NodeError> {
    let hw = probe(&paths::data_dir());
    println!("{}", describe(&hw));

    let models = fetch(relayer_url).await?;
    let (fitting, too_big): (Vec<_>, Vec<_>) = models
        .into_iter()
        .partition(|m| fits(m, &hw, parallel_slots));
    if fitting.is_empty() {
        return Err(NodeError::NothingFits);
    }

    println!("\nModels this machine can serve:");
    for (i, m) in fitting.iter().enumerate() {
        println!(
            "  {}) {:<22} needs {:>8} memory   {:>8} download   {}{}",
            i + 1,
            m.display_name,
            gib(memory_needed(m, parallel_slots)),
            gib(m.download_bytes()),
            m.niches.join(", "),
            if m.is_unverified() {
                "   (unverified)"
            } else {
                ""
            },
        );
    }
    if fitting.iter().any(RegistryModel::is_unverified) {
        println!(
            "  Unverified models haven't been checked by Ukoo yet, so they can't receive requests.\n  Pick one only if you want it ready the moment Ukoo verifies it."
        );
    }
    if !too_big.is_empty() {
        println!(
            "  ({} more need more memory than this machine has)",
            too_big.len()
        );
    }

    let model = match chosen {
        Some(id) => fitting
            .iter()
            .find(|m| m.model_id == id)
            .ok_or(NodeError::UnknownModel(id))?,
        None => &fitting[ask_choice(fitting.len())? - 1],
    };

    let model_dir = paths::models_dir().join(&model.model_id);
    let still_to_download: u64 = model
        .gguf_files
        .iter()
        .filter(|f| !model_dir.join(f.file_name()).exists())
        .map(|f| f.size_bytes)
        .sum();
    if still_to_download > hw.free_disk_bytes {
        return Err(NodeError::NotEnoughDisk {
            needed: gib(still_to_download),
            free: gib(hw.free_disk_bytes),
        });
    }

    println!(
        "\nDownloading {} ({}) and checking it against Ukoo's fingerprint…",
        model.display_name, model.license
    );
    let mut files = Vec::new();
    for file in &model.gguf_files {
        let dest = model_dir.join(file.file_name());
        verify::download_verified(file, &dest).await?;
        files.push(LocalFile {
            path: dest,
            file: file.clone(),
        });
    }

    let mut state = NodeState::new(relayer_url, model, files, parallel_slots);
    if let Ok(previous) = NodeState::load() {
        state.paused = previous.paused;
        state.serve_hours = previous.serve_hours;
        state.autoupdate = previous.autoupdate;
    }
    state.save()?;
    println!(
        "\n{} is downloaded and verified. Run `ukoo-node start` to serve it.",
        model.display_name
    );
    Ok(())
}

pub fn describe(hw: &Hardware) -> String {
    let memory = match hw.kind {
        MemoryKind::Unified => format!("{} unified memory", gib(hw.total_ram_bytes)),
        MemoryKind::NvidiaGpu { count } => {
            format!("{count} NVIDIA GPU(s), {} VRAM", gib(hw.vram_bytes))
        }
        MemoryKind::CpuOnly => format!("{} RAM, no supported GPU", gib(hw.total_ram_bytes)),
    };
    format!(
        "This machine: {memory} ({} usable for models), {} free disk",
        gib(hw.usable_bytes()),
        gib(hw.free_disk_bytes)
    )
}

fn ask_choice(count: usize) -> Result<usize, NodeError> {
    loop {
        print!("Choose a model [1-{count}]: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            return Err(NodeError::NoChoice);
        }
        match line.trim().parse::<usize>() {
            Ok(n) if (1..=count).contains(&n) => return Ok(n),
            _ => println!("Please type a number from 1 to {count}."),
        }
    }
}
