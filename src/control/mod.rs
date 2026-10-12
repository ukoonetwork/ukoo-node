mod schedule;

pub use schedule::ServeHours;

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use crate::engine::random_hex;
use crate::error::NodeError;
use crate::paths;
use crate::setup::NodeState;

/// Whether this node should be serving right now: not paused, and inside its serving hours.
pub fn serving_now(state: &NodeState) -> bool {
    !state.paused && state.serve_hours.is_none_or(|h| h.contains_now())
}

/// How `pause`, `resume` and `schedule` reach a running `ukoo-node start`: a localhost-only port
/// plus a secret in a file only this user can read, so other programs can't pause the node.
#[derive(Serialize, Deserialize)]
struct ControlFile {
    port: u16,
    token: String,
}

fn control_file() -> PathBuf {
    paths::data_dir().join("control.json")
}

pub struct Control {
    listener: TcpListener,
    token: String,
}

impl Control {
    pub async fn open() -> Result<Self, NodeError> {
        if poke().await {
            return Err(NodeError::AlreadyRunning);
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let file = ControlFile {
            port: listener.local_addr()?.port(),
            token: random_hex(32),
        };
        write_private(
            &control_file(),
            &serde_json::to_vec(&file).expect("control file serializes"),
        )?;
        Ok(Self {
            listener,
            token: file.token,
        })
    }

    /// Resolves when another `ukoo-node` command asks the running node to re-read its settings.
    pub async fn changed(&self) {
        loop {
            let Ok((stream, _)) = self.listener.accept().await else {
                continue;
            };
            if self.authorized(stream).await {
                return;
            }
        }
    }

    async fn authorized(&self, stream: TcpStream) -> bool {
        let (read, mut write) = stream.into_split();
        let mut line = String::new();
        let read = timeout(
            Duration::from_secs(2),
            BufReader::new(read).read_line(&mut line),
        )
        .await;
        if !matches!(read, Ok(Ok(_))) || line.trim() != self.token {
            return false;
        }
        write.write_all(b"ok\n").await.is_ok()
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(control_file());
    }
}

/// Tells a running node its settings changed. `false` if no node is running.
pub async fn poke() -> bool {
    let Ok(raw) = std::fs::read(control_file()) else {
        return false;
    };
    let Ok(file) = serde_json::from_slice::<ControlFile>(&raw) else {
        return false;
    };
    let exchange = async {
        let mut stream = TcpStream::connect(("127.0.0.1", file.port)).await.ok()?;
        stream
            .write_all(format!("{}\n", file.token).as_bytes())
            .await
            .ok()?;
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).await.ok()?;
        (reply.trim() == "ok").then_some(())
    };
    matches!(
        timeout(Duration::from_secs(3), exchange).await,
        Ok(Some(()))
    )
}

fn write_private(path: &PathBuf, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().expect("control file has a parent"))?;
    let _ = std::fs::remove_file(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(path)?, bytes)
}
