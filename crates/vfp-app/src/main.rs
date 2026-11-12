//! VisualFingerprint desktop app. The window is plain HTML in `ui/`. This file holds the commands it calls.
//! All work goes through `Backend`, so the screens are the same for "this device" and "my server".

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};
use vfp_core::backend::{check_url, AssetInfo, Backend, LocalBackend, ReceiverInfo, RemoteBackend, ServerLink};
use vfp_core::pipeline::{self, Plan, PlanOptions, TraceResult};
use vfp_core::strength::{self, Strength};
use vfp_core::vault::{Dek, KdfParams, Protection, VaultFile, VAULT_FILE};
use vfp_core::workspace::{Protection as WsProtection, Workspace};
use std::sync::Mutex;

/// The data key lives in memory only. `lock` drops it (the key is wiped on drop).
#[derive(Default)]
struct Keys(Mutex<Option<Dek>>);

fn key_of(app: &AppHandle) -> Option<Dek> {
    app.state::<Keys>().0.lock().ok().and_then(|g| g.clone())
}
fn set_key(app: &AppHandle, k: Option<Dek>) {
    if let Ok(mut g) = app.state::<Keys>().0.lock() {
        *g = k;
    }
}

fn has_workspace(h: &Path) -> bool {
    h.join("master.key").exists() || Workspace::protection(h) == WsProtection::Vault
}

type Res<T> = Result<T, String>;

fn e<E: std::fmt::Display>(x: E) -> String {
    x.to_string()
}

/// The workspace folder of this app. It also holds `server.json` when the user links a server.
/// Small pointer file in the app data folder. It holds the workspace folder the user chose.
fn pointer(app: &AppHandle) -> Res<PathBuf> {
    Ok(app.path().app_data_dir().map_err(e)?.join("workspace-path.txt"))
}

fn default_home(app: &AppHandle) -> Res<PathBuf> {
    Ok(app.path().app_data_dir().map_err(e)?.join("workspace"))
}

fn home(app: &AppHandle) -> Res<PathBuf> {
    if let Ok(s) = std::fs::read_to_string(pointer(app)?) {
        let s = s.trim();
        if !s.is_empty() {
            return Ok(PathBuf::from(s));
        }
    }
    default_home(app)
}

fn backend(app: &AppHandle) -> Res<Box<dyn Backend + Send>> {
    let h = home(app)?;
    let key = key_of(app);
    if let Some(l) = ServerLink::load(&h).map_err(e)? {
        return Ok(Box::new(RemoteBackend::new(&l.url, &l.token).with_key(key).map_err(e)?));
    }
    if !has_workspace(&h) {
        return Err("Set up the app first.".into());
    }
    if Workspace::protection(&h) == WsProtection::Vault && key.is_none() {
        return Err("The workspace is locked. Enter your passphrase.".into());
    }
    Ok(Box::new(LocalBackend::with_key(h, key)))
}

/// Run slow work on a worker thread so the window stays alive.
async fn work<T: Send + 'static>(f: impl FnOnce() -> Res<T> + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(e)?
}

#[derive(Serialize)]
struct State {
    /// "none" (first run), "local" or "server"
    mode: &'static str,
    place: String,
    server_url: Option<String>,
    home: String,
    /// The data is encrypted (local vault, or the server has a vault)
    encrypted: bool,
    /// Encrypted, but the key is not in memory
    locked: bool,
    /// A server with no vault yet: the user must create one
    needs_vault: bool,
}

#[tauri::command]
async fn app_state(app: AppHandle) -> Res<State> {
    let h = home(&app)?;
    let link = ServerLink::load(&h).map_err(e)?;
    let unlocked = key_of(&app).is_some();
    let (mode, place) = match (&link, has_workspace(&h)) {
        (Some(l), _) => ("server", format!("Server {}", l.url)),
        (None, true) => ("local", "This device".to_string()),
        (None, false) => ("none", String::new()),
    };
    let (mut encrypted, mut needs_vault) = (false, false);
    if let Some(l) = &link {
        let l = l.clone();
        // If the server cannot be reached, assume nothing special. Later calls show the real error.
        if let Ok((vault, protection)) = work(move || {
            let r = RemoteBackend::new(&l.url, &l.token);
            let v = r.vault().map_err(e)?.is_some();
            let p = r.info().map_err(e)?["protection"].as_str().unwrap_or("").to_string();
            Ok((v, p))
        })
        .await
        {
            encrypted = vault;
            needs_vault = protection == "uninitialized";
        }
    } else if mode == "local" {
        encrypted = Workspace::protection(&h) == WsProtection::Vault;
    }
    Ok(State {
        mode,
        place,
        server_url: link.map(|l| l.url),
        home: h.display().to_string(),
        encrypted,
        locked: encrypted && !unlocked,
        needs_vault,
    })
}

#[tauri::command]
fn check_strength(passphrase: String) -> Strength {
    strength::assess(&passphrase)
}

#[tauri::command]
fn generate_passphrase() -> String {
    strength::generate()
}

/// Make a key from the passphrase, on a worker thread (Argon2 is slow on purpose).
async fn new_vault(passphrase: String) -> Res<(VaultFile, Dek)> {
    work(move || {
        strength::require_strong(&passphrase).map_err(e)?;
        VaultFile::create(&passphrase, Protection::Protected, KdfParams::DESKTOP).map_err(e)
    })
    .await
}

/// Unlock with the passphrase. Works for this device and for a server vault.
#[tauri::command]
async fn unlock(app: AppHandle, passphrase: String) -> Res<()> {
    let h = home(&app)?;
    let link = ServerLink::load(&h).map_err(e)?;
    let dek = work(move || {
        let v = match &link {
            Some(l) => RemoteBackend::new(&l.url, &l.token).vault().map_err(e)?,
            None => VaultFile::load(&h.join(VAULT_FILE)).map_err(e)?,
        };
        v.ok_or_else(|| "There is no vault to unlock.".to_string())?.unlock(&passphrase).map_err(e)
    })
    .await?;
    set_key(&app, Some(dek));
    Ok(())
}

#[tauri::command]
fn lock(app: AppHandle) {
    set_key(&app, None);
}

/// Create the vault on a fresh server. The passphrase stays here.
#[tauri::command]
async fn create_server_vault(app: AppHandle, passphrase: String) -> Res<()> {
    let h = home(&app)?;
    let l = ServerLink::load(&h).map_err(e)?.ok_or("Not connected to a server.")?;
    let (vf, dek) = new_vault(passphrase).await?;
    let d2 = dek.clone();
    work(move || RemoteBackend::new(&l.url, &l.token).with_key(Some(d2)).map_err(e)?.create_vault(&vf).map_err(e)).await?;
    set_key(&app, Some(dek));
    Ok(())
}

/// Change the passphrase of the encrypted workspace on this device. Files are not touched.
#[tauri::command]
async fn change_passphrase(app: AppHandle, old: String, new: String) -> Res<()> {
    let path = home(&app)?.join(VAULT_FILE);
    work(move || {
        strength::require_strong(&new).map_err(e)?;
        let v = VaultFile::load(&path).map_err(e)?.ok_or("This workspace is not encrypted.")?;
        v.change_passphrase(&old, &new).map_err(e)?.save(&path).map_err(e)
    })
    .await
}

/// Encrypt a workspace that is not encrypted yet.
#[tauri::command]
async fn encrypt_workspace(app: AppHandle, passphrase: String) -> Res<String> {
    let h = home(&app)?;
    let (vf, dek) = new_vault(passphrase).await?;
    let d2 = dek.clone();
    let h2 = h.clone();
    work(move || Workspace::migrate_to_vault(&h2, &vf, &d2).map_err(e)).await?;
    set_key(&app, Some(dek));
    Ok(h.join(VAULT_FILE).display().to_string())
}

#[tauri::command]
fn default_workspace(app: AppHandle) -> Res<String> {
    Ok(default_home(&app)?.display().to_string())
}

/// `folder` is where the originals and the key are kept. `None` means the default place.
#[tauri::command]
async fn setup_local(app: AppHandle, folder: Option<String>, passphrase: Option<String>) -> Res<String> {
    if let Some(f) = folder.filter(|f| !f.trim().is_empty()) {
        let p = PathBuf::from(f.trim());
        std::fs::create_dir_all(&p).map_err(e)?;
        let ptr = pointer(&app)?;
        if let Some(d) = ptr.parent() {
            std::fs::create_dir_all(d).map_err(e)?;
        }
        std::fs::write(ptr, p.display().to_string()).map_err(e)?;
    }
    let h = home(&app)?;
    match passphrase.filter(|p| !p.is_empty()) {
        Some(p) => {
            let (vf, dek) = new_vault(p).await?;
            Workspace::init_encrypted(&h, &vf, &dek).map_err(e)?;
            set_key(&app, Some(dek));
            Ok(h.join(VAULT_FILE).display().to_string())
        }
        None => {
            Workspace::init(&h).map_err(e)?;
            Ok(h.join("master.key").display().to_string())
        }
    }
}

#[tauri::command]
async fn connect_server(app: AppHandle, url: String, token: String) -> Res<String> {
    let h = home(&app)?;
    work(move || {
        let url = url.trim().to_string();
        check_url(&url).map_err(e)?;
        let info = RemoteBackend::new(&url, token.trim()).info().map_err(e)?;
        ServerLink { url, token: token.trim().to_string() }.save(&h).map_err(e)?;
        Ok(format!("Connected. Your role: {}.", info["role"].as_str().unwrap_or("?")))
    })
    .await
}

#[tauri::command]
fn disconnect_server(app: AppHandle) -> Res<()> {
    set_key(&app, None);
    let p = ServerLink::path(&home(&app)?);
    if p.exists() {
        std::fs::remove_file(p).map_err(e)?;
    }
    Ok(())
}

/// Copy the key file to a place the user picks (a backup). An encrypted workspace has `vault.json`.
#[tauri::command]
fn export_key(app: AppHandle, dest: String) -> Res<()> {
    let h = home(&app)?;
    let key = if h.join(VAULT_FILE).exists() { h.join(VAULT_FILE) } else { h.join("master.key") };
    if !key.exists() {
        return Err("This device holds no key. The key is on your server.".into());
    }
    std::fs::copy(key, dest).map_err(e)?;
    Ok(())
}

#[tauri::command]
async fn receivers(app: AppHandle) -> Res<Vec<ReceiverInfo>> {
    let b = backend(&app)?;
    work(move || b.receivers().map_err(e)).await
}

/// One name per line. A line may be `name,note`.
#[tauri::command]
async fn add_receivers(app: AppHandle, text: String) -> Res<usize> {
    let b = backend(&app)?;
    work(move || {
        let mut n = 0;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let (name, note) = line.split_once(',').unwrap_or((line, ""));
            b.add_receiver(name.trim(), note.trim()).map_err(e)?;
            n += 1;
        }
        Ok(n)
    })
    .await
}

#[tauri::command]
async fn assets(app: AppHandle) -> Res<Vec<AssetInfo>> {
    let b = backend(&app)?;
    work(move || b.assets().map_err(e)).await
}

/// What can this file carry? Runs on this device only. Nothing is stored.
#[tauri::command]
async fn preview_file(path: String, max_receivers: u64) -> Res<Plan> {
    work(move || pipeline::plan_file(Path::new(&path), &PlanOptions { max_receivers, ..Default::default() }).map_err(e)).await
}

#[tauri::command]
async fn add_asset(app: AppHandle, path: String, name: Option<String>, max_receivers: u64) -> Res<(AssetInfo, Plan)> {
    let b = backend(&app)?;
    work(move || b.add_asset(Path::new(&path), name.as_deref(), &PlanOptions { max_receivers, ..Default::default() }).map_err(e)).await
}

#[tauri::command]
async fn make_copies(app: AppHandle, asset: String, receivers: Vec<String>, outdir: String) -> Res<Vec<(String, String)>> {
    let b = backend(&app)?;
    let ap = app.clone();
    work(move || {
        let show = |done: usize, total: usize| {
            let _ = ap.emit("progress", serde_json::json!({"done": done, "total": total}));
        };
        let out = b.mark(&asset, &receivers, Path::new(&outdir), Some(&show)).map_err(e)?;
        Ok(out.into_iter().map(|(r, p)| (r, p.display().to_string())).collect())
    })
    .await
}

#[tauri::command]
async fn trace_file(app: AppHandle, path: String, asset: Option<String>) -> Res<TraceResult> {
    let b = backend(&app)?;
    work(move || b.trace(Path::new(&path), asset.as_deref().filter(|a| !a.is_empty())).map_err(e)).await
}

#[tauri::command]
fn save_report(result: TraceResult, leaked: String, dest: String) -> Res<()> {
    let text = pipeline::report_text(&result, Path::new(&leaked)).map_err(e)?;
    std::fs::write(dest, text).map_err(e)
}

fn main() {
    tauri::Builder::default()
        .manage(Keys::default())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            app_state,
            check_strength,
            generate_passphrase,
            unlock,
            lock,
            create_server_vault,
            change_passphrase,
            encrypt_workspace,
            setup_local,
            default_workspace,
            connect_server,
            disconnect_server,
            export_key,
            receivers,
            add_receivers,
            assets,
            preview_file,
            add_asset,
            make_copies,
            trace_file,
            save_report
        ])
        .run(tauri::generate_context!())
        .expect("the app could not start");
}
