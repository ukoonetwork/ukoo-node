use std::path::PathBuf;
use std::time::SystemTime;

use super::sha256_file;

/// Re-checks the model files for every heartbeat. A file whose size and modified time are
/// unchanged keeps its hash; any change triggers a full re-hash.
pub struct Fingerprints {
    files: Vec<Entry>,
}

struct Entry {
    path: PathBuf,
    seen: Option<(u64, SystemTime, String)>,
}

impl Fingerprints {
    /// `verified` pairs each file with the hash it was just verified against at startup.
    pub fn new(verified: Vec<(PathBuf, String)>) -> Self {
        let files = verified
            .into_iter()
            .map(|(path, sha256)| {
                let seen = stamp(&path).map(|(len, modified)| (len, modified, sha256));
                Entry { path, seen }
            })
            .collect();
        Self { files }
    }

    pub async fn current(&mut self) -> Vec<String> {
        let mut hashes = Vec::with_capacity(self.files.len());
        for entry in &mut self.files {
            let Some((len, modified)) = stamp(&entry.path) else {
                entry.seen = None;
                hashes.push("missing".to_string());
                continue;
            };
            let cached = entry
                .seen
                .as_ref()
                .filter(|(l, m, _)| *l == len && *m == modified)
                .map(|(_, _, sha)| sha.clone());
            let sha = match cached {
                Some(sha) => sha,
                None => {
                    let sha = sha256_file(&entry.path)
                        .await
                        .unwrap_or_else(|_| "unreadable".to_string());
                    entry.seen = Some((len, modified, sha.clone()));
                    sha
                }
            };
            hashes.push(sha);
        }
        hashes
    }
}

fn stamp(path: &PathBuf) -> Option<(u64, SystemTime)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_untouched_file_keeps_its_verified_hash_and_a_changed_one_is_rehashed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.gguf");
        std::fs::write(&path, b"ukoo").unwrap();
        // Seeded with a made-up hash: proves an unchanged file isn't re-read.
        let mut prints = Fingerprints::new(vec![(path.clone(), "seeded".to_string())]);
        assert_eq!(prints.current().await, vec!["seeded"]);

        std::fs::write(&path, b"ukoo, tampered").unwrap();
        let rehashed = prints.current().await;
        assert_ne!(rehashed, vec!["seeded"]);
        assert_eq!(rehashed[0].len(), 64);

        std::fs::remove_file(&path).unwrap();
        assert_eq!(prints.current().await, vec!["missing"]);
    }
}
