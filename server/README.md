# Save Sync Server

Self-hosted HTTP backend for the Save Sync PS Vita app. Handles save upload/download, manifest tracking, and device pairing with token-based auth.

## Setup

```bash
cp .env.example .env
# Edit .env with your values
pnpm install
pnpm run build
pnpm run start
```

## Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `USER_TOKEN` | (required) | Bearer token shared between server and Vita clients |
| `USER_NAME` | `default` | Used in storage paths under `/data/vita-save-sync/users/` |
| `DATA_DIR` | `./data` | Where save archives and manifests are stored |
| `RAW_SAVES_DIR` | unset (disabled) | Opt-in plain-file mirror of every save (see below) |
| `PORT` | `3000` | HTTP listen port |

## API

### `GET /api/status`
Health check, no auth required.

### `POST /api/pair`
Register a device. Body: `{ "token": "<token>", "deviceName": "<name>" }`.

### `GET /api/manifest`
Returns the user's cloud manifest. Auth: `Authorization: Bearer <token>`.

### `PUT /api/manifest`
Update manifest metadata. Auth required.

### `PUT /api/save/:titleId`
Upload a zipped save. Auth required.
Headers: `X-Save-Hash`, `X-Save-Timestamp`, `X-Device-Id`.
Body: multipart form with `file` field.

### `GET /api/save/:titleId`
Download the latest save zip for a title. Auth required.

## Storage layout

```
$DATA_DIR/users/<username>/
  manifest.json
  devices/
    vita-oled.json
    vita-slim.json
  saves/
    <TITLEID>/
      current.zip
      versions/
        2026-06-21T15-42-00Z.zip
```

## Unzipped saves (Syncthing)

Saves are normally stored as zip archives. If you want the raw files (for
example, to sync them with Syncthing), set `RAW_SAVES_DIR` to a directory:

```bash
RAW_SAVES_DIR=/path/to/raw-saves
```

- Every uploaded save is extracted into `$RAW_SAVES_DIR/<TITLEID> - <Game Name>/` as plain files.
- Saves uploaded before you enabled the option are extracted on their next download.
- Point Syncthing at the `RAW_SAVES_DIR` root itself, so Syncthing's own `.stfolder`/`.stversions` folders stay outside the game folders.
- Keep `RAW_SAVES_DIR` **outside** `DATA_DIR` — the server refuses to start if the two overlap.

The mirror works in both directions:

- **Save edits flow back**: when a save is downloaded, the server compares the mirror folder against the stored archive. If files changed (e.g. a save edited on your PC and synced back by Syncthing), the archive is rebuilt from the mirror, the previous archive is kept in `versions/`, and the Vita gets the new version.
- Changes are picked up lazily on the next download; the server does not watch the folder.

Things to know:

- **Deleting all files in a mirror folder creates an empty archive, and restoring it wipes the save on the Vita.** Deleting individual files also propagates.
- A new upload from the Vita overwrites the mirror and wins over un-merged edits. If mirroring the upload fails, the mirror is removed and re-created from the upload on the next download — the upload still wins. (That removal also propagates to all Syncthing peers, like any other mirror change.)
- While Syncthing is actively transferring into a mirror (`.syncthing.*.tmp` files present), the server skips the round-trip that cycle and serves the stored archive instead.
- Syncthing conflict files (`.sync-conflict-*`) are regular files and get included in rebuilt archives.
- Symbolic links in the mirror are followed when rebuilding — don't point links at unrelated files.
- Every upload replaces the mirror folder, which Syncthing sees as delete+add churn even when nothing changed.

## Production deployment

Put behind Nginx or Caddy with HTTPS:

```nginx
server {
    listen 443 ssl;
    server_name vita-sync.example.com;
    ssl_certificate /etc/letsencrypt/live/vita-sync.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/vita-sync.example.com/privkey.pem;
    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $host;
    }
}
```
