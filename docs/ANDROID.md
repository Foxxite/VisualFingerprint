# Android

Status: **builds, installs and starts on a real phone (Pixel 10, Android 16).** The debug APK builds with
`cargo tauri android build --apk --debug --target aarch64`. Hand-tested on the phone: the welcome screens,
choosing the folder, the passphrase screen with strength meter, "Make one for me", encrypted workspace
creation (64 MiB key function), dark mode, the phone layout, adding receivers, registering an image from the
file picker, making copies (one save dialog per copy), tracing a saved copy (it named the right receiver),
and the key backup.
Not yet tried on the phone: the server path, saving the trace report, pasting a passphrase.

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

## Things we learned on the real phone

- **16 KB memory pages.** The Pixel 10 uses 16 KB pages. The NDK r27 links for 4 KB and Android shows a
  warning. `.cargo/config.toml` sets `-Wl,-z,max-page-size=16384` for the Android targets. Keep it.
- **Tauri hooks.** `beforeBuildCommand` and `beforeDevCommand` run inside `frontend/`, so they must not use `--prefix`.
- **Screen edges.** The app draws under the status bar and navigation bar. `viewport-fit=cover` and
  `env(safe-area-inset-*)` in `style.css` keep content clear of them. The header wraps below 600 px.
- **Size.** The debug APK is about 370 MB. The release build will be much smaller.

## Build and run

```bash
cargo tauri android dev                 # emulator or USB phone
cargo tauri android build --apk         # release APK (needs a signing key)
```

## Known gaps (do these before you trust it)

- **Audio and video do not work yet.** The core calls the `ffmpeg` and `ffprobe` programs. A phone has neither.
  Images work. Audio/video on Android needs a bundled FFmpeg library or the platform codecs (`MediaCodec`).
  Until then, the Android app should be used with a server (`Make copies` for video runs on the server in Protected mode).
- **File picker and save dialog (fixed, tested on the phone).** Android returns `content://` addresses, not
  paths. The Rust side (`tauri-plugin-fs`) opens them: a picked file is copied into app cache with its real
  extension (found from the first bytes) and used from there. Results are written to the chosen address.
  There is no folder picker on Android, so "Make copies" saves the copies in app storage and then opens one
  save dialog per copy. A registered original is named `picked`, because the address has no real file name.
- **Memory.** The cache for zero-knowledge mode is on app storage. Set `VFP_CACHE_MB` low (or add a setting) on phones.
- **Passphrase on phone.** Hand typing 8 words is slow. Paste from a password manager must work: test it.
- Multiple workspaces use folders inside app storage on Android; "Browse…" for a folder may not work.
