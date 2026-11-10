# Contributing to Save Sync

## Prerequisites

- **VitaSDK** — the arm-vita-eabi toolchain. Install via [vdpm](https://github.com/vitasdk/vdpm) or download a [pre-built tarball](https://github.com/vitasdk/buildscripts/releases).
- **Rust nightly-2025-06-01** — pinned in `rust-toolchain.toml`. Install with `rustup toolchain install nightly-2025-06-01 --component rust-src`.
- **cargo-vita** — `cargo install cargo-vita --locked`.

## Building

```bash
VITASDK=$HOME/vitasdk PATH="$HOME/vitasdk/bin:$HOME/.cargo/bin:$PATH" cargo vita build vpk --release
```

The VPK lands at `target/armv7-sony-vita-newlibeabihf/release/vita-save-cloud.vpk`.

CI builds via `.github/workflows/build.yml` on every `v*` tag push.

## Project structure

```
.
├── src/                    # Rust source
│   ├── main.rs             # Entry point, tokio runtime
│   ├── config.rs           # JSON config read/write (server URL, token)
│   ├── api.rs              # HTTP client (ureq) — server communication
│   ├── sync.rs             # Content-hash comparison, status_for() sync status
│   ├── sync_engine.rs      # SyncEngine — fetch/upload/download/Sync All, shared by the Games tab
│   ├── emulator.rs         # PSP/RetroArch entry scanning
│   ├── utils.rs            # Backup/restore, SHA-256, ZIP handling
│   ├── tai.rs              # taiHEN integration, game detection, PFS mount
│   ├── ime.rs              # On-screen keyboard wrapper
│   ├── constant.rs         # Paths, labels, all user-facing strings
│   └── ui/                 # UI layer
│       ├── ui_base.rs      # UIBase trait (update, draw, invalidate)
│       ├── ui_desktop.rs   # Top-level two-tab view (Games | Settings), splash screen
│       ├── ui_settings.rs  # Settings tab — server config + sync exclusions
│       ├── ui_titles/      # Games tab: grid + save drawer (local + manage tabs)
│       ├── list_state.rs   # Shared scroll/selection state for list-style screens
│       ├── ui_drawer.rs    # Half-screen slide-in drawer used by the save menu
│       ├── ui_dialog.rs    # Confirmation dialogs
│       ├── ui_list.rs      # List rendering trait
│       ├── ui_loading.rs   # Full-screen loading overlay
│       ├── ui_scroll_progress.rs
│       └── ui_toast.rs     # Toast notifications
├── c/                      # C source
│   ├── tai.c               # taiHEN hooks, SQLite init, kernel calls
│   ├── vita_sqlite_vfs.c   # Custom SQLite VFS (from VitaShell) for ur0: access
│   ├── v2d.c               # vita2d rendering wrappers
│   ├── ime.c               # IME (keyboard) FFI
│   ├── sqlite3.h           # SQLite header (needed by tai.c and vita_sqlite_vfs.c)
│   └── libVitaShellUser_stub_weak.a  # Weak stub for VitaShell exports
├── server/                 # Node.js + Fastify backend
│   ├── src/
│   │   ├── index.ts        # Server entry point
│   │   ├── routes/         # API routes (auth, status, manifest, saves)
│   │   ├── middleware/      # Bearer token auth
│   │   └── storage/        # Disk read/write
│   └── docker-compose.yml
├── build.rs                # C compilation + Vita linking
├── rust-toolchain.toml      # Pins nightly-2025-06-01
├── .github/workflows/build.yml  # CI: builds VPK, creates GitHub Release
└── Cargo.toml              # version and deps
```

## Key things to know

### SQLite and ur0: access

The PS Vita system database at `ur0:/shell/db/app.db` contains game names and icon paths. Sony's standard SQLite VFS refuses to open it.

The fix: `c/vita_sqlite_vfs.c` (from VitaShell) registers a custom `"psp2_rw"` VFS that uses raw `sceIo*` calls. Call `sqlite_init()` instead of `sqlite3_initialize()`, and `sqlite_exit()` instead of `sqlite3_shutdown()`.

The firmware provides the SQLite API via `SceSqlite_stub` at link time. There is no need to compile the full SQLite amalgamation.

### Versioning

The version lives in exactly one place: `Cargo.toml` → `version = "X.Y.Z"`. It feeds `env!("CARGO_PKG_VERSION")` for the splash screen and `vita-mksfoex` for the SFO.

Git tags trigger CI releases. Match the tag to the Cargo.toml version: `v0.1.1` ↔ `version = "0.1.1"`.

### UI invalidation

`UIBase::invalidate()` re-syncs a screen with state that changed elsewhere. `UITitles` uses it to re-scan sync exclusions and clear `SyncEngine`'s cached games (forcing a re-fetch) after Settings closes or an exclusion is toggled from the save drawer's Manage tab, so badges never go stale.

### Server auth

`test_connection` does two checks: `/api/status` (public, verifies reachability) then `/api/manifest` (Bearer auth, verifies token). A single call to `/api/status` would always succeed even with a bad token.

### Docker env gotcha

`docker compose restart` keeps the original container environment. If you change `.env`, use `docker compose up -d` to recreate the container.

### ring crate

The `ring` crate's build.rs invokes `perl` for ARM assembly generation. CI needs `apk add perl` (Alpine) or equivalent.

## Before submitting

- Build and test on real hardware
- Version in `Cargo.toml` matches the git tag
- Splash screen shows the correct version and credit
- README screenshots are up to date
