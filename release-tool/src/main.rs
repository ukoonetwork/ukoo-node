//! Ukoo release signing. Run on the release-key holder's machine; the private key never leaves it.
//!
//!   release-tool keygen <private-key-file>
//!   release-tool manifest <version> <engine> <base-url> <archives-dir>   (writes manifest.json)
//!   release-tool sign <private-key-file> <manifest.json>                  (writes manifest.json.sig[.bin])
//!   release-tool verify <public-key-hex> <manifest.json> <manifest.json.sig>

use std::path::Path;
use std::process::exit;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

const PLATFORMS: [&str; 5] = [
    "macos-aarch64-metal",
    "linux-x86_64-cpu",
    "linux-x86_64-cuda",
    "windows-x86_64-cpu",
    "windows-x86_64-cuda",
];

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    exit(2)
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| fail(&format!("can't read {path}: {e}")))
}

fn signing_key(path: &str) -> SigningKey {
    let seed = hex::decode(String::from_utf8_lossy(&read(path)).trim())
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .unwrap_or_else(|| fail("the private key file should hold 64 hex characters"));
    SigningKey::from_bytes(&seed)
}

fn keygen(path: &str) {
    if Path::new(path).exists() {
        fail(&format!(
            "{path} already exists; refusing to overwrite a release key"
        ));
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).expect("OS random number generator unavailable");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(path)
        .unwrap_or_else(|e| fail(&format!("can't create {path}: {e}")));
    std::io::Write::write_all(&mut file, hex::encode(seed).as_bytes()).expect("write key");
    let public = SigningKey::from_bytes(&seed).verifying_key();
    println!("Release key saved to {path}. Back it up somewhere safe and offline.");
    println!("Public key (set as UKOO_RELEASE_PUBLIC_KEY in the repo's Actions variables):");
    println!("{}", hex::encode(public.to_bytes()));
    println!("Same key as PEM (paste into RELEASE_KEY_PEM in the website's install.sh):");
    println!("{}", public_pem(&public.to_bytes()));
}

/// SubjectPublicKeyInfo for an Ed25519 key, base64: what OpenSSL reads.
fn public_pem(key: &[u8; 32]) -> String {
    use base64::Engine;
    let der = [
        &hex::decode("302a300506032b6570032100").expect("valid hex")[..],
        key,
    ]
    .concat();
    format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
        base64::engine::general_purpose::STANDARD.encode(der)
    )
}

fn manifest(version: &str, engine: &str, base_url: &str, dir: &str) {
    let mut assets = Vec::new();
    for platform in PLATFORMS {
        let found = ["tar.gz", "zip"]
            .iter()
            .map(|ext| format!("ukoo-node-{version}-{platform}.{ext}"))
            .find(|name| Path::new(dir).join(name).is_file());
        let Some(name) = found else {
            fail(&format!("no archive for {platform} in {dir}"));
        };
        let bytes = read(&Path::new(dir).join(&name).to_string_lossy());
        assets.push(serde_json::json!({
            "platform": platform,
            "url": format!("{}/{name}", base_url.trim_end_matches('/')),
            "sha256": hex::encode(Sha256::digest(&bytes)),
            "size_bytes": bytes.len(),
        }));
    }
    let manifest = serde_json::json!({
        "version": version,
        "engine": engine,
        "published_at": now_rfc3339(),
        "min_relayer": "0.1.0",
        "assets": assets,
    });
    let out = Path::new(dir).join("manifest.json");
    std::fs::write(
        &out,
        serde_json::to_vec_pretty(&manifest).expect("serializes"),
    )
    .unwrap_or_else(|e| fail(&format!("can't write manifest: {e}")));
    println!("Wrote {}", out.display());
}

fn now_rfc3339() -> String {
    let out = std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .unwrap_or_else(|e| fail(&format!("can't read the clock: {e}")));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn sign(key: &str, manifest: &str) {
    let signature = signing_key(key).sign(&read(manifest));
    // Hex for ukoo-node; raw bytes for the install script's OpenSSL check.
    let out = format!("{manifest}.sig");
    std::fs::write(&out, hex::encode(signature.to_bytes()))
        .unwrap_or_else(|e| fail(&format!("can't write {out}: {e}")));
    std::fs::write(format!("{out}.bin"), signature.to_bytes())
        .unwrap_or_else(|e| fail(&format!("can't write {out}.bin: {e}")));
    println!("Wrote {out} and {out}.bin");
}

fn verify(public_hex: &str, manifest: &str, sig: &str) {
    let public = hex::decode(public_hex)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .and_then(|b| VerifyingKey::from_bytes(&b).ok())
        .unwrap_or_else(|| fail("the public key should be 64 hex characters"));
    let signature = hex::decode(String::from_utf8_lossy(&read(sig)).trim())
        .ok()
        .and_then(|b| Signature::from_slice(&b).ok())
        .unwrap_or_else(|| fail("the signature file is unreadable"));
    match public.verify(&read(manifest), &signature) {
        Ok(()) => println!("Signature OK."),
        Err(_) => fail("Signature does NOT match."),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["keygen", path] => keygen(path),
        ["manifest", version, engine, base_url, dir] => manifest(version, engine, base_url, dir),
        ["sign", key, manifest] => sign(key, manifest),
        ["verify", public, manifest, sig] => verify(public, manifest, sig),
        _ => fail(
            "usage:\n  release-tool keygen <private-key-file>\n  release-tool manifest <version> <engine> <base-url> <archives-dir>\n  release-tool sign <private-key-file> <manifest.json>\n  release-tool verify <public-key-hex> <manifest.json> <manifest.json.sig>",
        ),
    }
}
