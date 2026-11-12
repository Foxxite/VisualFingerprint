# Android

Status: **prepared, not built.** This computer has no Android SDK or NDK, so nothing here was compiled for Android.
Treat every step below as untested.

## What is ready in the code

- `crates/vfp-app` is now a library (`vfp_app_lib`) plus a thin desktop `main.rs`. The library has the
  Tauri mobile entry point, so `cargo tauri android` can build it.
- On Android and iOS the key function uses 64 MiB (`KdfParams::MOBILE`). On a computer it uses 256 MiB.
- The Rust dependencies need no OpenSSL (`ureq` uses rustls, SQLite is bundled). The NDK C compiler builds SQLite.
- The React screens are the same. Zero-knowledge mode works against any `vfp-server`.

## Set up (once)

1. Install Android Studio. In SDK Manager add: SDK Platform 34 or newer, SDK Build-Tools, Platform-Tools, **NDK (Side by side)**, Command-line Tools.
2. Set `ANDROID_HOME` (for example `%LOCALAPPDATA%\Android\Sdk`) and `NDK_HOME` (the folder under `ndk\`).
3. Use JDK 17 or 21 and set `JAVA_HOME` to it. (This machine has `JAVA_HOME` on JDK 25 but `java` on 21: make them agree.)
4. Add the Rust targets:
   ```bash
   rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android i686-linux-android
   ```
5. Install the tool: `cargo install tauri-cli --version "^2"`.
6. Create the Android project (writes `crates/vfp-app/gen/android`):
   ```bash
   cd crates/vfp-app && cargo tauri android init
   ```

## Build and run

```bash
cargo tauri android dev                 # emulator or USB phone
cargo tauri android build --apk         # release APK (needs a signing key)
```

## Known gaps (do these before you trust it)

- **Audio and video do not work yet.** The core calls the `ffmpeg` and `ffprobe` programs. A phone has neither.
  Images work. Audio/video on Android needs a bundled FFmpeg library or the platform codecs (`MediaCodec`).
  Until then, the Android app should be used with a server (`Make copies` for video runs on the server in Protected mode).
- **File picker.** Android returns `content://` addresses, not file paths. The core reads normal paths. The
  dialog plugin may copy the file to app storage first; this must be checked on a real device. Saving marked
  copies to a user folder needs the same check (Storage Access Framework).
- **Memory.** The cache for zero-knowledge mode is on app storage. Set `VFP_CACHE_MB` low (or add a setting) on phones.
- **Screen size.** The layout was made for a window of 720 px or more. Check it on a phone.
- **Passphrase on phone.** Hand typing 8 words is slow. Paste from a password manager must work: test it.
- Multiple workspaces use folders inside app storage on Android; "Browse…" for a folder may not work.
