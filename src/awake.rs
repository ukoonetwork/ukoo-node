//! Keeps the computer from going to sleep while a model is loaded, so the node stays reachable.
//! The display can still sleep. Pausing or stopping ukoo-node lets the computer sleep again.

/// Held while serving; dropping it lets the computer sleep again.
pub struct KeepAwake {
    #[cfg(unix)]
    child: Option<std::process::Child>,
    #[cfg(windows)]
    release: Option<std::sync::mpsc::Sender<()>>,
}

impl KeepAwake {
    /// Best effort: a machine with no sleep control (e.g. a Linux server without systemd) just
    /// keeps working as before.
    pub fn hold() -> Self {
        imp::hold()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::process::{Command, Stdio};

    use super::KeepAwake;

    pub fn hold() -> KeepAwake {
        // -w: caffeinate exits by itself if ukoo-node dies.
        let child = Command::new("caffeinate")
            .args(["-i", "-w", &std::process::id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
        KeepAwake { child }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::process::{Command, Stdio};

    use super::KeepAwake;

    pub fn hold() -> KeepAwake {
        // The inhibitor lasts as long as `tail`, which exits by itself if ukoo-node dies.
        let child = Command::new("systemd-inhibit")
            .args([
                "--what=idle:sleep",
                "--who=ukoo-node",
                "--why=Serving AI requests on the Ukoo network",
                "--mode=block",
                "tail",
                &format!("--pid={}", std::process::id()),
                "-f",
                "/dev/null",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
        KeepAwake { child }
    }
}

#[cfg(unix)]
impl Drop for KeepAwake {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::KeepAwake;

    const ES_CONTINUOUS: u32 = 0x8000_0000;
    const ES_SYSTEM_REQUIRED: u32 = 0x0000_0001;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetThreadExecutionState(flags: u32) -> u32;
    }

    pub fn hold() -> KeepAwake {
        // The request belongs to the thread that made it, so one thread holds it until released.
        let (release, released) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
            let _ = released.recv();
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        });
        KeepAwake {
            release: Some(release),
        }
    }
}

#[cfg(windows)]
impl Drop for KeepAwake {
    fn drop(&mut self) {
        self.release.take();
    }
}
