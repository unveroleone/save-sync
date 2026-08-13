# Android Signing

The Save Sync Hub Android APK is signed in CI (`.github/workflows/hub.yml`) so
every release is installable and upgrades cleanly from the previous one.

## How it works

- `build-android` needs `build-desktop` (which creates the draft release the
  APK is attached to).
- If the four `ANDROID_*` secrets are set, `tauri android build --apk` runs,
  then `zipalign` + `apksigner` sign the unsigned APK with the release
  keystore and the APK is attached to the release via `gh release upload
  --clobber`.
- Without the secrets, the job falls back to a **debug-signed** APK. It is
  installable, but debug keys differ between CI machines, so a later release
  signed with the real keystore cannot upgrade it — users must uninstall
  first. Never ship two releases in a row this way.

## The keystore

- Location (Leo's machine): `~/Documents/save-sync/release-keystore.jks`
- Alias: `save-sync`
- Password (keystore and key): see `~/Documents/save-sync/keystore-info.txt`
  (chmod 600, not in git, not on GitHub)
- Keep the `.jks` file backed up. Losing it means the next release cannot
  upgrade existing installs; there is no recovery.

## GitHub secrets

| Secret | Content |
| --- | --- |
| `ANDROID_KEYSTORE` | base64 of `release-keystore.jks` (`base64 -i ...jks`) |
| `ANDROID_KEYSTORE_PASSWORD` | keystore password |
| `ANDROID_KEY_ALIAS` | `save-sync` |
| `ANDROID_KEY_PASSWORD` | key password (same as keystore password here) |

Regenerate one with:

```bash
keytool -genkeypair -v -keystore release-keystore.jks \
  -keyalg RSA -keysize 2048 -validity 10000 -alias save-sync
```

## Fixing a release that shipped a debug APK

Set the secrets, then re-run the workflow for the tag:

```bash
gh run rerun <run-id>   # re-evaluates secrets; Android re-signs and
                        # --clobber replaces the APK on the release
```
