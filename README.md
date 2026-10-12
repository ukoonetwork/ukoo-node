# ukoo-node

Run an open-weight AI model on your own machine and get paid in USDC for every token it writes.

`ukoo-node` connects a Mac, Linux or Windows machine to [Ukoo Network](https://ukoo.network). Developers send
OpenAI-compatible requests, and each one is answered by a node like yours.

## Install

```sh
curl -fsSL https://ukoo.network/install.sh | sh        # macOS, Linux
```

```powershell
irm https://ukoo.network/install.ps1 | iex             # Windows
```

Then:

```sh
ukoo-node setup     # check this machine, pick a model that fits, download and verify it
ukoo-node link      # link the node to your Ukoo account, so earnings are paid to you
ukoo-node start
```

## Commands

| Command | What it does |
|---|---|
| `setup` | Check this machine, pick a model that fits, download and verify it. |
| `start` | Verify the model and start serving it. |
| `status` | Show what this node is set up to serve. |
| `pause` / `resume` | Stop serving and free memory after requests in progress finish, or serve again. |
| `schedule 19:00-08:00` | Serve only during these hours each day (`off` to serve any time). |
| `price` | Show or set your price per 1M tokens, from free up to twice the model's default. |
| `link` | Link this node to your Ukoo account. |
| `update` / `autoupdate on\|off` | Install the latest signed release, or turn automatic updates on or off. |
| `service install\|uninstall\|status` | Run in the background, starting with this computer. |

## What it does on your machine

- **Runs a pinned, checksum-verified llama.cpp** (`llama-server`) on a model file whose SHA-256 is checked
  against Ukoo's registry, at startup and while serving.
- **Dials out** to the relayer. It opens no ports, and its pause/resume control listens only on localhost.
- **Keeps the computer awake only while a model is loaded.** Pause or stop it and the computer sleeps normally.
- **Identifies itself with a key it generates.** Ukoo never issues or sees the private key.
- **Only installs updates signed with Ukoo's release key.** The signing tool is in [`release-tool/`](release-tool).

## Build from source

Needs Rust 1.88+.

```sh
scripts/fetch-engine.sh    # downloads and verifies the pinned llama.cpp build into ./engine
cargo build --release
```

## Releasing

1. Bump `version` in `Cargo.toml`, commit, and push a tag `node-v<version>`.
2. **Build ukoo-node release** builds every platform and creates a draft release.
3. On the release-key holder's machine: `scripts/sign-release.sh node-v<version> <key-file>`. It downloads
   the archives, writes and signs `manifest.json`, checks the signature, and publishes after you type
   `publish`. Running nodes pick it up within a day.

`scripts/release-key.sh <key-file>` creates the release key, once.

Upgrading llama.cpp changes the engine pin in `scripts/package.sh`, `scripts/fetch-engine.sh` and
`src/engine/mod.rs` together, plus the relayer's token counter, so billing counts tokens with the same code.

## License

Apache-2.0. The bundled llama.cpp engine is MIT-licensed; its license ships with every release.
