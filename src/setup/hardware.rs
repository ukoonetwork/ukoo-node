use std::path::Path;
use std::process::Command;

use sysinfo::{Disks, System};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MemoryKind {
    /// Apple Silicon: the GPU shares system RAM.
    Unified,
    /// One or more NVIDIA GPUs; models live in VRAM.
    NvidiaGpu {
        count: usize,
    },
    CpuOnly,
}

#[derive(Debug, Clone)]
pub struct Hardware {
    pub kind: MemoryKind,
    pub total_ram_bytes: u64,
    pub vram_bytes: u64,
    pub free_disk_bytes: u64,
}

impl Hardware {
    /// Memory a model plus its context may use without starving the machine.
    pub fn usable_bytes(&self) -> u64 {
        match self.kind {
            MemoryKind::Unified => self.total_ram_bytes / 10 * 7,
            MemoryKind::NvidiaGpu { .. } => self.vram_bytes,
            MemoryKind::CpuOnly => self.total_ram_bytes / 10 * 6,
        }
    }

    /// e.g. "macos-aarch64-metal", "linux-x86_64-cuda", sent to the relayer on connect.
    pub fn platform(&self) -> String {
        let accel = match self.kind {
            MemoryKind::Unified => "metal",
            MemoryKind::NvidiaGpu { .. } => "cuda",
            MemoryKind::CpuOnly => "cpu",
        };
        format!(
            "{}-{}-{accel}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    }
}

pub fn probe(data_dir: &Path) -> Hardware {
    let mut sys = System::new();
    sys.refresh_memory();
    let total_ram_bytes = sys.total_memory();

    let (kind, vram_bytes) = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        (MemoryKind::Unified, 0)
    } else {
        match nvidia_vram() {
            Some((count, bytes)) => (MemoryKind::NvidiaGpu { count }, bytes),
            None => (MemoryKind::CpuOnly, 0),
        }
    };

    Hardware {
        kind,
        total_ram_bytes,
        vram_bytes,
        free_disk_bytes: free_disk(data_dir),
    }
}

/// Total VRAM across NVIDIA GPUs, via nvidia-smi (present wherever the NVIDIA driver is).
fn nvidia_vram() -> Option<(usize, u64)> {
    let out = Command::new("nvidia-smi")
        .args(["--query-gpu=memory.total", "--format=csv,noheader,nounits"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let mib: Vec<u64> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    (!mib.is_empty()).then(|| (mib.len(), mib.iter().sum::<u64>() * 1024 * 1024))
}

/// Free space on the disk that holds `path` (the longest matching mount point).
fn free_disk(path: &Path) -> u64 {
    let disks = Disks::new_with_refreshed_list();
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        if !probe.pop() {
            break;
        }
    }
    let probe = probe.canonicalize().unwrap_or(probe);
    disks
        .iter()
        .filter(|d| probe.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map_or(0, |d| d.available_space())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(kind: MemoryKind, ram_gib: u64, vram_gib: u64) -> Hardware {
        Hardware {
            kind,
            total_ram_bytes: ram_gib << 30,
            vram_bytes: vram_gib << 30,
            free_disk_bytes: 0,
        }
    }

    #[test]
    fn usable_memory_depends_on_the_machine() {
        assert_eq!(
            hw(MemoryKind::Unified, 24, 0).usable_bytes(),
            (24u64 << 30) / 10 * 7
        );
        assert_eq!(
            hw(MemoryKind::NvidiaGpu { count: 2 }, 64, 48).usable_bytes(),
            48 << 30
        );
        assert_eq!(
            hw(MemoryKind::CpuOnly, 32, 0).usable_bytes(),
            (32u64 << 30) / 10 * 6
        );
    }

    #[test]
    fn probe_sees_this_machine() {
        let h = probe(Path::new("."));
        assert!(h.total_ram_bytes > 0);
        assert!(h.free_disk_bytes > 0);
    }
}
