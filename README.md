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
  <img src="https://img.shields.io/badge/license-GPL--3.0-111111?style=flat-square" alt="GPL-3.0">
</p>

<p align="center">
  <strong>Play on Vita A &middot; Upload &middot; Pull on Vita B &middot; Restore</strong><br>
  <sub>Forked from <a href="https://github.com/save-cloud">Save Cloud Vita</a>. Baidu Cloud replaced with a self-hosted HTTP API. All strings translated to English.</sub>
</p>

---

## What it does

- Backs up PS Vita save data to a zip locally, then uploads to your own server
- Downloads saves from the server to a second Vita
- Restores downloaded saves (creates a safety backup first)
- Shows per-game sync status on the Cloud tab
- TLS via iTLS-Enso + rustls on Vita

Requires HENkaku + iTLS-Enso on the Vita and a server you control (VPS, home server, etc.).

---

## Server setup

```bash
cd server
cp .env.example .env
# Edit .env: set USER_TOKEN to a long random string, USER_NAME to your username
docker compose up -d
```

Put it behind Nginx Proxy Manager or Cloudflare Tunnel for HTTPS. The Vita needs HTTPS with a valid certificate — iTLS-Enso provides the modern TLS roots.

Bare metal alternative:

```bash
cd server && pnpm install && pnpm run build
USER_TOKEN=your-secret USER_NAME=you DATA_DIR=/data node dist/index.js
```

> **Note:** `docker compose restart` keeps the old environment. If you change `.env`, run `docker compose up -d` to recreate the container.

---

## Build the Vita app

Requires macOS or Linux with VitaSDK installed.

```bash
# One-time setup
brew install cmake
git clone https://github.com/vitasdk/vdpm && cd vdpm
./bootstrap-vitasdk.sh && ./install-all.sh
# Add to ~/.zshrc:
#   export VITASDK=/usr/local/vitasdk
#   export PATH=$VITASDK/bin:$PATH

rustup install nightly-2025-06-01
rustup component add rust-src --toolchain nightly-2025-06-01
cargo +nightly install cargo-vita

# Build
rustup override set nightly-2025-06-01
cargo vita build vpk --release
```

VPK output: `target/armv7-sony-vita-newlibeabihf/release/vita-save-cloud.vpk`

---

## First-time setup on the Vita

1. Install the VPK via VitaShell
2. Open Save Sync
3. Press **R** to switch to the Cloud tab
4. Press **Triangle** to open Settings
5. Fill in **Server URL** (e.g. `https://vita-sync.example.com`), **API Token**, and **Device Name**
6. Select **Test Connection** — it checks both reachability and your token
7. Press **O** to go back, or select **Save && Back**

---

## Workflow

### Backing up and uploading a save

1. Press **L** to go to the Games tab
2. Select a game with **Up/Down**
3. Press **O** to open the save drawer
4. In the **Local Backup** column, press **X** to back up locally
   — This zips the raw save files to `ux0:data/save-sync/backups/<TITLEID>/`
5. In the **Server Backup** column, press **Select** to upload to the server
   — The server stores the zip and records the hash and timestamp in the manifest

That's one complete backup. The server now has this save.

### Restoring on a second Vita

1. Set up the second Vita with the same server URL and token (different device name)
2. Go to the Games tab, select the game, press **O** to open the drawer
3. In the **Server Backup** column, press **Square** to download
   — The zip lands in `ux0:data/save-sync/backups/<TITLEID>/`
4. In the **Local Backup** column, press **Square** to restore
   — Save Sync backs up the current save first, then replaces it with the downloaded one

### Other actions in the save drawer

| Button | Local Backup column | Server Backup column |
|--------|---------------------|----------------------|
| X | Back up now | — |
| Select | Upload to server | Download from server |
| Square | Restore | Download & restore |
| Triangle | Delete local backup | — |
| O | Close drawer | — |

### Cloud tab

Press **R** to switch to the Cloud tab. It shows every game with a sync status badge:

| Badge | Meaning |
|-------|---------|
| Synced | Local and cloud match |
| Not Uploaded | Local backup exists, nothing on server yet |
| Upload | Local is newer than server |
| Download | Server has a newer version |
| Cloud Only | On server, no local backup on this Vita |
| Conflict | Both sides changed since last sync |

Press **X** to run **Sync All** — it uploads everything marked Upload and downloads everything marked Download. It stops and reports any Conflicts without touching them.

Press **Triangle** to open Settings from the Cloud tab.

---

## Server API

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| `GET` | `/api/status` | No | Health check, server version |
| `GET` | `/api/manifest` | Bearer | Cloud manifest for all games |
| `PUT` | `/api/save/:titleId` | Bearer | Upload save zip |
| `GET` | `/api/save/:titleId` | Bearer | Download save zip |

Upload sends `X-Save-Hash` (SHA-256), `X-Save-Timestamp`, and `X-Device-Id` headers. The server verifies the hash before writing.

---

## Vita folder layout

```
ux0:data/save-sync/
  config.json          # server URL, token, device name
  backups/
    PCSE00001/
      2026-06-21 15.42.00.zip   # local backup zips
  logs/
    latest.log
```

---

## Credits

Forked from [Save Cloud Vita](https://github.com/save-cloud) by iamcco.
Uses [VitaShell](https://github.com/TheOfficialFloW/VitaShell) SQLite VFS and kernel modules, [vita-rust](https://github.com/vita-rust), [VitaSDK](https://github.com/vitasdk).
