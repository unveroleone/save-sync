<p align="center">
  <img src="screenshot.png" width="480" alt="Save Sync on PS Vita">
</p>

<h1 align="center">Save Sync</h1>

<p align="center">
  <em>Manual cloud save sync for two modded PS Vitas. Self-hosted, no BS.</em>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platform-PS%20Vita-111111?style=flat-square" alt="PS Vita">
  <img src="https://img.shields.io/badge/server-Node.js%20%7C%20Fastify-111111?style=flat-square" alt="Node.js">
  <img src="https://img.shields.io/badge/client-Rust%20%2B%20vita2d-111111?style=flat-square" alt="Rust">
  <img src="https://img.shields.io/badge/license-MIT-111111?style=flat-square" alt="MIT">
</p>

<p align="center">
  <strong>Play on Vita A &middot; Upload &middot; Pull on Vita B &middot; Restore</strong><br>
  <sub>Forked from <a href="https://github.com/save-cloud">Save Cloud Vita</a>. Baidu Cloud replaced with a self-hosted HTTP API. All strings translated to English.</sub>
</p>

---

## Features

- Local save backup to `ux0:data/save-sync/backups/<TITLEID>/`
- Upload saves to a self-hosted server with SHA-256 verification
- Download newer saves from the server
- Restore saves with automatic pre-restore safety backup
- Sync overview with per-game status badges (In Sync, Upload Needed, Download Available, Conflict)
- Sync All: upload changed, download newer, stop on conflicts
- Device pairing with a static API token
- Settings screen: server URL, token, device name, test connection
- TLS 1.2 support via iTLS-Enso + rustls on Vita

## Quick start

### 1. Run the server

```bash
cd server
cp .env.example .env
# Edit .env: set USER_TOKEN, USER_NAME, DATA_DIR
docker compose up -d
```

Or bare metal:

```bash
cd server && cp .env.example .env
pnpm install && pnpm run build
USER_TOKEN=your-secret USER_NAME=you DATA_DIR=/data/vita-save-sync node dist/index.js
```

Put it behind Nginx Proxy Manager or Cloudflare Tunnel for HTTPS access from outside.

### 2. Build the Vita app

Requires macOS or Linux:

```bash
# One-time setup
brew install cmake                              # macOS
git clone https://github.com/vitasdk/vdpm && cd vdpm
./bootstrap-vitasdk.sh && ./install-all.sh
# Add to ~/.zshrc:
#   export VITASDK=/usr/local/vitasdk
#   export PATH=$VITASDK/bin:$PATH
rustup install nightly-2025-06-01
rustup component add rust-src --toolchain nightly-2025-06-01
cargo +nightly install cargo-vita

# Then in the project directory:
rustup override set nightly-2025-06-01
git submodule update --init
cargo vita build vpk --release
```

The VPK lands at `target/armv7-sony-vita-newlibeabihf/release/vita-save-cloud.vpk`.

### 3. Install and use on Vita

1. Transfer the VPK to your Vita and install
2. Open the app, press **R** to switch to the Sync view
3. Press **Triangle** to open Settings
4. Enter your server URL, API token, and device name
5. Press "Test Connection" to verify

**Titles view (L):**
- Game icons with save paths
- **Circle**: per-game backup/restore drawer
- **Triangle**: game menu (backup all, update account ID, delete)

**Sync view (R):**
- Game list with sync status badges
- **X**: Sync All (upload changed, download newer, skip conflicts)
- **Triangle**: Settings

**Per-game save drawer:**
- Local tab: backup, restore, upload (SELECT)
- Server tab: upload to server, download, download & restore

## Server API

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/api/status` | No | Health check, server version |
| `POST` | `/api/pair` | Token in body | Register device |
| `GET` | `/api/manifest` | Bearer | Cloud manifest |
| `PUT` | `/api/manifest` | Bearer | Update manifest |
| `PUT` | `/api/save/:titleId` | Bearer | Upload save zip |
| `GET` | `/api/save/:titleId` | Bearer | Download save zip |

## Vita folder layout

```
ux0:data/save-sync/
  config.json          # server URL, token, device name
  manifest.json        # local sync baseline
  backups/             # local save zips
    PCSE00001/
      2026-06-21 15.42.00.zip
  downloads/           # staging before restore
  logs/
    latest.log
```

## Credits

Forked from [Save Cloud Vita](https://github.com/save-cloud) by iamcco.
Uses [VitaShell](https://github.com/TheOfficialFloW/VitaShell) kernel modules, [vita-rust](https://github.com/vita-rust), [VitaSDK](https://github.com/vitasdk).
