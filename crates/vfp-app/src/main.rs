//! VisualFingerprint desktop app. The window is plain HTML in `ui/`. This file holds the commands it calls.
//! All work goes through `Backend`, so the screens are the same for "this device" and "my server".

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};
use vfp_core::backend::{check_url, AssetInfo, Backend, LocalBackend, ReceiverInfo, RemoteBackend, ServerLink};
use vfp_core::pipeline::{self, Plan, PlanOptions, TraceResult};
use vfp_core::workspace::Workspace;

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
    if let Some(l) = ServerLink::load(&h).map_err(e)? {
        return Ok(Box::new(RemoteBackend::new(&l.url, &l.token)));
    }
    if !h.join("master.key").exists() {
        return Err("Set up the app first.".into());
    }
    Ok(Box::new(LocalBackend::new(h)))
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
}

#[tauri::command]
fn app_state(app: AppHandle) -> Res<State> {
    let h = home(&app)?;
    let link = ServerLink::load(&h).map_err(e)?;
    let (mode, place) = match (&link, h.join("master.key").exists()) {
        (Some(l), _) => ("server", format!("Server {}", l.url)),
        (None, true) => ("local", "This device".to_string()),
        (None, false) => ("none", String::new()),
    };
    Ok(State { mode, place, server_url: link.map(|l| l.url), home: h.display().to_string() })
}

#[tauri::command]
fn default_workspace(app: AppHandle) -> Res<String> {
    Ok(default_home(&app)?.display().to_string())
}

/// `folder` is where the originals and the key are kept. `None` means the default place.
#[tauri::command]
fn setup_local(app: AppHandle, folder: Option<String>) -> Res<String> {
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
    Workspace::init(&h).map_err(e)?;
    Ok(h.join("master.key").display().to_string())
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
    let p = ServerLink::path(&home(&app)?);
    if p.exists() {
        std::fs::remove_file(p).map_err(e)?;
    }
    Ok(())
}

/// Copy `master.key` to a place the user picks (a backup).
#[tauri::command]
fn export_key(app: AppHandle, dest: String) -> Res<()> {
    let key = home(&app)?.join("master.key");
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
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            app_state,
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
