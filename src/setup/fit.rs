use super::{Hardware, RegistryModel};

/// Headroom for the engine itself, buffers and the OS.
const OVERHEAD_BYTES: u64 = 1 << 30;

/// Memory needed to serve `model` with `parallel_slots` requests at once: weights plus one full
/// context window of KV cache per slot.
pub fn memory_needed(model: &RegistryModel, parallel_slots: u64) -> u64 {
    model.download_bytes()
        + model.serve.kv_bytes_per_token * model.serve.context_length * parallel_slots
        + OVERHEAD_BYTES
}

pub fn fits(model: &RegistryModel, hw: &Hardware, parallel_slots: u64) -> bool {
    memory_needed(model, parallel_slots) <= hw.usable_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::{MemoryKind, ModelFile, ServeParams};

    fn qwen3_14b() -> RegistryModel {
        RegistryModel {
            model_id: "qwen3-14b".into(),
            display_name: "Qwen3 14B".into(),
            version: "v".into(),
            niches: vec![],
            license: "Apache-2.0".into(),
            gguf_files: vec![ModelFile {
                url: "u".into(),
                sha256: "s".into(),
                size_bytes: 9_001_752_960,
            }],
            serve: ServeParams {
                context_length: 32768,
                kv_bytes_per_token: 163840,
            },
            default_ask_per_1m_input_usd: "0.12".into(),
            default_ask_per_1m_output_usd: "0.24".into(),
            status: "active".into(),
        }
    }

    fn mac(ram_gib: u64) -> Hardware {
        Hardware {
            kind: MemoryKind::Unified,
            total_ram_bytes: ram_gib << 30,
            vram_bytes: 0,
            free_disk_bytes: 0,
        }
    }

    #[test]
    fn counts_the_context_window_not_just_the_weights() {
        // 9.0 GB weights + 5.0 GiB KV cache + 1 GiB overhead
        assert_eq!(
            memory_needed(&qwen3_14b(), 1),
            9_001_752_960 + 5_368_709_120 + (1 << 30)
        );
    }

    #[test]
    fn a_second_slot_needs_a_second_context_window() {
        assert!(fits(&qwen3_14b(), &mac(24), 1));
        assert!(!fits(&qwen3_14b(), &mac(24), 2));
        assert!(!fits(&qwen3_14b(), &mac(16), 1));
    }
}
