<p align="center">
  <img src="assets/icons/save-sync-transparent.png" width="200" alt="Save Sync on PS Vita">
</p>

<h1 align="center">Save Sync</h1>

<p align="center">
  <em>Self-hosted cloud save sync for PS Vita, PPSSPP, and RetroArch, across all your devices</em>
</p>

<p align="center">
  <img src="https://img.shields.io/github/v/release/unveroleone/vita-save-sync" alt="version">
  <img src="https://img.shields.io/badge/server-Node.js%20%7C%20Fastify-111111?style=flat-square" alt="Node.js">
  <img src="https://img.shields.io/badge/vita-Rust%20%2B%20vita2d-111111?style=flat-square" alt="Vita client">
  <img src="https://img.shields.io/badge/desktop%20%2F%20android-Tauri%20v2%20%2B%20Svelte-111111?style=flat-square" alt="Desktop client">
  <img src="https://img.shields.io/badge/license-GPL--3.0-111111?style=flat-square" alt="GPL-3.0">
</p>

<p align="center">
  <strong>Play anywhere &middot; Upload your save &middot; Restore on any device</strong><br>
  <sub>Forked from <a href="https://github.com/save-cloud">Save Cloud Vita</a>. Baidu Cloud replaced with a self-hosted HTTP API. All strings translated to English.</sub>
</p>

---

## What it does

One server, multiple clients. All saves go to the same place.

**PS Vita app**:
- Backs up native PS Vita saves, PSP/Adrenaline saves, and RetroArch saves
- Uploads to your server as a zip, downloads and restores on another Vita
- Shows per-game sync status on the Cloud tab with one-tap Sync All

**Save Sync Hub** (Desktop / Android):
- Syncs PPSSPP and RetroArch saves from macOS, Windows, Linux, and Android
- Detects default save locations automatically per platform
- Upload to server, download to local, or restore directly from cloud
- Connects to the same server and same API as the Vita app

---

## Clients at a glance

| Client | Runs on | Emu  lators / saves |
|--------|---------|-------------------|
| Save Sync (Vita app) | PS Vita, PSTV | Native Vita · PSP (Adrenaline) · RetroArch |
| Save Sync Hub | macOS · Windows · Linux · Android | PPSSPP · RetroArch · Custom path |

The Vita app requires HENkaku + iTLS-Enso. Save Sync Hub is a standard desktop/Android app (no jailbreak needed on the other end).

---

## Server setup

The server is a small Node.js app. It stores save files on disk and exposes a REST API. You need to keep it running while you sync, it does not need to be on 24/7.

Pick the option that fits your situation.

---

<details>
<summary><strong>Option A | Docker</strong></summary>

<br>

If you have Docker installed ([Docker Desktop](https://www.docker.com/products/docker-desktop/) for macOS/Windows, or `apt install docker.io` on Linux), create two files and run one command:

```yaml
# docker-compose.yml
services:
  vita-save-sync:
    image: ghcr.io/unveroleone/vita-save-sync-server:latest
    container_name: vita-save-sync
    restart: unless-stopped
    ports:
      - "3099:3000"
    environment:
      - USER_TOKEN=${USER_TOKEN:?set a strong token}
      - DATA_DIR=/data
    volumes:
      - vita-save-data:/data

volumes:
  vita-save-data:
    driver: local
```

```bash
# .env
USER_TOKEN=change-me-to-a-long-random-string
```

```bash
docker compose up -d
```

To update later: `docker compose pull && docker compose up -d`

> **Note:** `docker compose restart` reuses the cached environment. If you change `.env`, run `docker compose up -d` instead.

Put the server behind Nginx Proxy Manager or Cloudflare Tunnel for HTTPS. The Vita needs HTTPS with a valid certificate; iTLS-Enso provides the modern TLS roots.

</details>

---

<details>
<summary><strong>Option B | Node.js directly</strong> &nbsp;(quickest to get started)</summary>

<br>

You need [Node.js](https://nodejs.org/) 20 or newer. Works on macOS, Windows, and Linux.

```bash
git clone https://github.com/unveroleone/vita-save-sync.git
cd vita-save-sync/server
npm install
npm run build
USER_TOKEN=your-secret-token DATA_DIR=./data node dist/index.js
```

The server listens on port 3000. Add `PORT=3099` to the command if you need a different port.

</details>

---

<details>
<summary><strong>Option C | Raspberry Pi</strong> &nbsp;(always-on home server)</summary>

<br>

Start from a fresh Raspberry Pi OS (Bookworm or newer). Ethernet is recommended for reliability, but Wi-Fi works too.

**Install Node.js (LTS):**

```bash
curl -fsSL https://deb.nodesource.com/setup_lts.x | sudo -E bash -
sudo apt install -y nodejs git
```

**Clone and start:**

```bash
git clone https://github.com/unveroleone/vita-save-sync.git
cd vita-save-sync/server
npm install
npm run build
USER_TOKEN=your-secret-token DATA_DIR=./data node dist/index.js
```

**Auto-start on boot with systemd:**

Replace `pi` with your actual username if it differs (run `whoami` to check).

```bash
sudo tee /etc/systemd/system/vita-save-sync.service << 'EOF'
[Unit]
Description=Save Sync server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=pi
WorkingDirectory=/home/pi/vita-save-sync/server
Environment=USER_TOKEN=your-secret-token
Environment=DATA_DIR=/home/pi/vita-save-sync/server/data
Environment=PORT=3000
ExecStart=/usr/bin/node dist/index.js
Restart=on-failure
RestartSec=10

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl daemon-reload
sudo systemctl enable --now vita-save-sync
```

Check status: `sudo systemctl status vita-save-sync`

</details>

---

<details>
<summary><strong>Option D | Background service on your laptop</strong> &nbsp;(macOS / Linux / Windows)</summary>

<br>

**macOS**: create a launchd plist so the server starts automatically at login.

First find your Node.js path: `which node` (commonly `/usr/local/bin/node` on Intel, `/opt/homebrew/bin/node` on Apple Silicon).

Save this as `~/Library/LaunchAgents/com.vita-save-sync.plist`, replacing the paths and token:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.vita-save-sync</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/local/bin/node</string>
        <string>/Users/you/vita-save-sync/server/dist/index.js</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>USER_TOKEN</key>
        <string>your-secret-token</string>
        <key>DATA_DIR</key>
        <string>/Users/you/vita-save-sync/server/data</string>
        <key>PORT</key>
        <string>3000</string>
    </dict>
    <key>WorkingDirectory</key>
    <string>/Users/you/vita-save-sync/server</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
```

Load it: `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.vita-save-sync.plist`

---

**Linux**: use the systemd unit from Option C. Same file, same commands.

---

**Windows**: run a terminal on login and keep it open, or use Task Scheduler to launch `node dist/index.js` at boot with `USER_TOKEN` and `DATA_DIR` set as environment variables.

</details>

---

### HTTPS

The Vita requires HTTPS with a valid certificate. The easiest option is **Cloudflare Quick Tunnel** (no account or domain needed). Install and run:

| Platform | Install command |
|----------|----------------|
| macOS | `brew install cloudflared` |
| Linux | `curl -L https://pkg.cloudflare.com/install.sh \| sudo bash && sudo apt install cloudflared` |
| Windows | `winget install -e --id Cloudflare.cloudflared` |

While your server is running:

```bash
cloudflared tunnel --url http://localhost:3000
```

It prints a `*.trycloudflare.com` address. Use that as the server URL on the Vita. The tunnel closes when you close the terminal, which is fine for occasional sync sessions.

For a permanent setup with your own domain, [Cloudflare Tunnel with a named tunnel](https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/) is also free.

---

## First-time setup on the Vita

Install the VPK via [VitaDB Downloader](https://www.rinnegatamante.eu/vitadb/#/info/1418) (search "Save Sync") or download it from [GitHub Releases](https://github.com/unveroleone/vita-save-sync/releases) and install manually via VitaShell.

1. Install the VPK
2. Open Save Sync
3. Press **R** to switch to the Cloud tab
4. Press **Triangle** to open Settings
5. Fill in **Server URL** (e.g. `https://vita-sync.example.com`), **API Token**, and **Device Name**
6. Select **Test Connection** (checks both reachability and your token)
7. Press **o** to go back, or select **Save && Back**

<p align="center">
  <img src="screenshot-game.png" width="480" alt="Save Sync on PS Vita">
</p>

---

## Save Sync Hub (Desktop / Android)

Save Sync Hub is the companion app for PC and Android. It syncs PPSSPP and RetroArch saves using the same server and API token as the Vita app.

Download the latest release from [GitHub Releases](https://github.com/unveroleone/vita-save-sync/releases) for your platform (`.dmg` for macOS, `.exe` for Windows, `.AppImage` for Linux, `.apk` for Android).

### First-time setup

1. Open Save Sync Hub
2. In the sidebar, enter your **Server URL**, **API Token**, and **Device Name** (same values as on the Vita)
3. Click **Save Config**
4. Select your platform (PSP / RetroArch / Custom path)
5. The app auto-fills the default save folder for your OS, or browse to a custom path
6. Click **Scan Saves** to list your save directories

### Default save paths

| Platform | PPSSPP | RetroArch |
|----------|--------|-----------|
| macOS | `~/Documents/PPSSPP/PSP/SAVEDATA` | `~/Library/Application Support/RetroArch` |
| Windows | `%USERPROFILE%\Documents\PPSSPP\PSP\SAVEDATA` | `%APPDATA%\RetroArch` |
| Linux | `~/.config/ppsspp/PSP/SAVEDATA` | `~/.config/retroarch` |
| Android | `/storage/emulated/0/PSP/SAVEDATA` | `/storage/emulated/0/RetroArch` |

> These are the most common defaults. PPSSPP varies by install method — if your saves are not found, use Browse to point to the right folder. For RetroArch, the app scans for `saves/` and `states/` subdirectories inside the path above.

### Workflow

- **Upload**: zips the save folder and sends it to the server
- **Download**: fetches the latest backup from the server to a local zip
- **Restore**: downloads from the server and extracts directly into the save folder

Save IDs on the server use the folder name as the title ID, prefixed with `PSP_` or `RA_` to avoid collisions with native Vita saves.

---

## Workflow (PS Vita)

### Uploading a save to the server

1. Press **L** to go to the Games tab
2. Select a game
3. Press **X** to open the save drawer
4. Press **R** to switch to the **Server Backup** tab
5. Select **Upload to Server** and press **X**

The app zips the save, uploads it, and removes the temp file. One step.

### Restoring on another device

1. Set up the other device with the same server URL and token (use a different device name)
2. Games tab → select the game → **X** to open the drawer
3. Press **R** for the Server Backup tab
4. Select **Download & Restore** and press **X**

The app downloads from the server and restores directly. It creates a safety backup of the current save first.

To download without restoring yet, select **Download from server** instead.

### Local backups (optional)

The **Local Backup** tab (press **L** in the drawer) manages save slots on the Vita itself, independent of the server. Use it to keep manual snapshots or restore a previous local slot.

### Cloud tab

Press **R** (main screen) to switch to the Cloud tab. It shows every game with a sync status badge:

| Badge | Meaning |
|-------|---------|
| Synced | In sync with server |
| Not Uploaded | Never uploaded to server |
| Upload | Local is newer |
| Download | Server has a newer version |
| Cloud Only | On server, not on this Vita |
| Conflict | Both sides changed |

Press **X** to run **Sync All**. It uploads everything marked Upload and downloads everything marked Download. Conflicts are reported but not touched.

Press **Triangle** to open Settings.

<p align="center">
  <img src="screenshot-cloud.png" width="480" alt="Save Sync on PS Vita">
</p>

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

## Credits

Forked from [Save Cloud Vita](https://github.com/save-cloud) by iamcco.
Vita app uses [VitaShell](https://github.com/TheOfficialFloW/VitaShell) SQLite VFS and kernel modules, [vita-rust](https://github.com/vita-rust), [VitaSDK](https://github.com/vitasdk).
Save Sync Hub built with [Tauri v2](https://tauri.app) and [Svelte 5](https://svelte.dev).

Built with the help of AI
