//! `vfp-server`: the workspace on a server you host.
//!
//! The server owns `master.key`, `vfp.db` and the originals. A device sends only the file it
//! wants to check. No original and no key ever leave the server.
//!
//! Run `vfp-server token new --role admin --label laptop` to make a token. The token is shown once.
//! Put an HTTPS reverse proxy (Caddy, nginx, Traefik) in front for use over the internet.

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use vfp_core::backend::{asset_info, AssetInfo, ReceiverInfo};
use vfp_core::pipeline::{self, PlanOptions};
use vfp_core::workspace::Workspace;

#[derive(Parser)]
#[command(name = "vfp-server", version, about = "Self-hosted VisualFingerprint server")]
struct Cli {
    /// Data folder: master.key, vfp.db, originals, tokens
    #[arg(long, global = true, env = "VFP_DATA", default_value = "data")]
    data: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the server
    Serve {
        #[arg(long, env = "VFP_BIND", default_value = "0.0.0.0:8443")]
        bind: String,
        /// Largest upload in megabytes
        #[arg(long, env = "VFP_MAX_UPLOAD_MB", default_value_t = 4096)]
        max_upload_mb: u64,
        /// Jobs that may run at the same time (marking and tracing use many CPU cores)
        #[arg(long, env = "VFP_JOBS", default_value_t = 2)]
        jobs: usize,
    },
    /// Manage API tokens
    Token {
        #[command(subcommand)]
        action: TokenCmd,
    },
}

#[derive(Subcommand)]
enum TokenCmd {
    /// Make a token. It is shown once and not stored in clear text.
    New {
        /// admin = everything; trace = may only check leaked files
        #[arg(long, default_value = "trace")]
        role: String,
        #[arg(long, default_value = "")]
        label: String,
    },
    List,
    Revoke { id: String },
}

// ---------------------------------------------------------------- tokens
#[derive(Serialize, Deserialize, Clone)]
struct TokenRec {
    id: String,
    label: String,
    role: String,
    hash: String,
}

fn hash_token(t: &str) -> String {
    Sha256::digest(t.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_rand(n: usize) -> String {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b).expect("random bytes");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn tokens_path(data: &Path) -> PathBuf {
    data.join("tokens.json")
}

fn load_tokens(data: &Path) -> Vec<TokenRec> {
    std::fs::read_to_string(tokens_path(data)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn save_tokens(data: &Path, t: &[TokenRec]) -> std::io::Result<()> {
    std::fs::write(tokens_path(data), serde_json::to_string_pretty(t).unwrap_or_default())
}

fn new_token(data: &Path, role: &str, label: &str) -> Result<String, String> {
    if role != "admin" && role != "trace" {
        return Err("role must be 'admin' or 'trace'".into());
    }
    let token = format!("vfp_{}", hex_rand(24));
    let mut all = load_tokens(data);
    all.push(TokenRec { id: hex_rand(4), label: label.into(), role: role.into(), hash: hash_token(&token) });
    save_tokens(data, &all).map_err(|e| e.to_string())?;
    Ok(token)
}

// ---------------------------------------------------------------- state and errors
struct State_ {
    data: PathBuf,
    max_upload: u64,
    jobs: Semaphore,
}
type Shared = Arc<State_>;

struct ApiErr(StatusCode, String);

impl IntoResponse for ApiErr {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error": self.1}))).into_response()
    }
}

impl From<vfp_core::Error> for ApiErr {
    fn from(e: vfp_core::Error) -> Self {
        ApiErr(StatusCode::BAD_REQUEST, e.to_string())
    }
}

fn internal(e: impl std::fmt::Display) -> ApiErr {
    ApiErr(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

type ApiResult<T> = Result<Json<T>, ApiErr>;

fn audit(st: &State_, who: &TokenRec, action: &str, detail: &str) {
    let line = format!("{} {} ({}) {} {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), who.label, who.id, action, detail);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(st.data.join("audit.log")) {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Check the bearer token. `admin` = the call needs the admin role.
fn auth(st: &State_, h: &HeaderMap, admin: bool) -> Result<TokenRec, ApiErr> {
    let given = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| ApiErr(StatusCode::UNAUTHORIZED, "token missing".into()))?;
    let hash = hash_token(given.trim());
    let rec = load_tokens(&st.data).into_iter().find(|t| t.hash == hash).ok_or_else(|| ApiErr(StatusCode::UNAUTHORIZED, "token not valid".into()))?;
    if admin && rec.role != "admin" {
        return Err(ApiErr(StatusCode::FORBIDDEN, "this token may only trace".into()));
    }
    Ok(rec)
}

fn ws(st: &State_) -> Result<Workspace, ApiErr> {
    Ok(Workspace::open(&st.data)?)
}

/// Run heavy work off the async threads, at most `jobs` at a time.
async fn job<T: Send + 'static>(st: &Shared, f: impl FnOnce() -> Result<T, ApiErr> + Send + 'static) -> Result<T, ApiErr> {
    let _permit = st.jobs.acquire().await.map_err(internal)?;
    tokio::task::spawn_blocking(f).await.map_err(internal)?
}

// ---------------------------------------------------------------- uploads
/// A folder for one upload. It is deleted when the request ends.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn clean_filename(f: &str) -> String {
    let base = Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let s: String = base.chars().map(|c| if c.is_alphanumeric() || "._- ".contains(c) { c } else { '_' }).collect();
    let s = s.trim_matches(|c| c == '.' || c == ' ').to_string();
    if s.is_empty() { "upload".into() } else { s }
}

/// Stream the request body to a file. Stop at the size limit.
async fn save_upload(st: &State_, filename: &str, body: Body) -> Result<(TempDir, PathBuf), ApiErr> {
    let dir = st.data.join("uploads").join(hex_rand(8));
    tokio::fs::create_dir_all(&dir).await.map_err(internal)?;
    let guard = TempDir(dir.clone());
    let path = dir.join(clean_filename(filename));
    let mut file = tokio::fs::File::create(&path).await.map_err(internal)?;
    let (mut total, mut stream) = (0u64, body.into_data_stream());
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiErr(StatusCode::BAD_REQUEST, format!("upload failed: {e}")))?;
        total += chunk.len() as u64;
        if total > st.max_upload {
            return Err(ApiErr(StatusCode::PAYLOAD_TOO_LARGE, format!("file is larger than the limit of {} MB", st.max_upload >> 20)));
        }
        file.write_all(&chunk).await.map_err(internal)?;
    }
    file.flush().await.map_err(internal)?;
    if total == 0 {
        return Err(ApiErr(StatusCode::BAD_REQUEST, "the upload is empty".into()));
    }
    Ok((guard, path))
}

// ---------------------------------------------------------------- handlers
async fn info(State(st): State<Shared>, h: HeaderMap) -> ApiResult<serde_json::Value> {
    let who = auth(&st, &h, false)?;
    let n = ws(&st)?.assets()?.len();
    Ok(Json(serde_json::json!({"server": "vfp-server", "version": env!("CARGO_PKG_VERSION"), "role": who.role, "assets": n})))
}

async fn list_receivers(State(st): State<Shared>, h: HeaderMap) -> ApiResult<Vec<ReceiverInfo>> {
    auth(&st, &h, false)?;
    let r = ws(&st)?.receivers()?;
    Ok(Json(r.into_iter().map(|r| ReceiverInfo { id: r.id, name: r.name, note: r.note }).collect()))
}

#[derive(Deserialize)]
struct NewReceiver {
    name: String,
    #[serde(default)]
    note: String,
}

async fn add_receiver(State(st): State<Shared>, h: HeaderMap, Json(b): Json<NewReceiver>) -> ApiResult<serde_json::Value> {
    let who = auth(&st, &h, true)?;
    ws(&st)?.add_receiver(&b.name, &b.note)?;
    audit(&st, &who, "add-receiver", &b.name);
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn list_assets(State(st): State<Shared>, h: HeaderMap) -> ApiResult<Vec<AssetInfo>> {
    auth(&st, &h, false)?;
    let w = ws(&st)?;
    let out: Result<Vec<_>, _> = w.assets()?.iter().map(|a| asset_info(&w, a)).collect();
    Ok(Json(out?))
}

#[derive(Deserialize)]
struct AddQuery {
    filename: String,
    name: Option<String>,
    max_receivers: Option<u64>,
    colluders: Option<u64>,
    eps: Option<f64>,
    strength: Option<f32>,
    slots: Option<usize>,
    crf: Option<i32>,
}

async fn add_asset(State(st): State<Shared>, h: HeaderMap, Query(q): Query<AddQuery>, body: Body) -> ApiResult<serde_json::Value> {
    let who = auth(&st, &h, true)?;
    let (_guard, path) = save_upload(&st, &q.filename, body).await?;
    let d = PlanOptions::default();
    let opts = PlanOptions {
        max_receivers: q.max_receivers.unwrap_or(d.max_receivers),
        eps: q.eps.unwrap_or(d.eps),
        strength: q.strength,
        slots: q.slots,
        n_colluders: q.colluders.unwrap_or(d.n_colluders),
        crf: q.crf,
    };
    let st2 = st.clone();
    let name = q.name.clone();
    let (info, plan) = job(&st, move || {
        let w = ws(&st2)?;
        let (a, plan) = pipeline::add_asset(&w, &path, name.as_deref(), &opts)?;
        Ok((asset_info(&w, &a)?, plan))
    })
    .await?;
    audit(&st, &who, "add-asset", &info.name);
    Ok(Json(serde_json::json!({"asset": info, "plan": plan})))
}

#[derive(Deserialize)]
struct MarkBody {
    asset: String,
    receivers: Vec<String>,
}

async fn mark(State(st): State<Shared>, h: HeaderMap, Json(b): Json<MarkBody>) -> ApiResult<serde_json::Value> {
    let who = auth(&st, &h, true)?;
    let st2 = st.clone();
    let asset = b.asset.clone();
    let copies = job(&st, move || {
        let w = ws(&st2)?;
        let a = w.asset(&asset)?;
        let outdir = st2.data.join("marked");
        let mut out = vec![];
        for r in &b.receivers {
            let p = pipeline::mark_for_receiver(&w, &a, r, &outdir, None)?;
            out.push((r.clone(), p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
        }
        Ok(out)
    })
    .await?;
    audit(&st, &who, "mark", &format!("{} for {} receivers", b.asset, copies.len()));
    Ok(Json(serde_json::json!({"copies": copies})))
}

#[derive(Deserialize)]
struct FileQuery {
    file: String,
}

async fn copy_file(State(st): State<Shared>, h: HeaderMap, Query(q): Query<FileQuery>) -> Result<Response, ApiErr> {
    let who = auth(&st, &h, true)?;
    // a bare file name only: no folders, no ".."
    if Path::new(&q.file).file_name().map(|n| n.to_string_lossy() != q.file).unwrap_or(true) || q.file.contains("..") {
        return Err(ApiErr(StatusCode::BAD_REQUEST, "bad file name".into()));
    }
    let f = tokio::fs::File::open(st.data.join("marked").join(&q.file)).await.map_err(|_| ApiErr(StatusCode::NOT_FOUND, "no such copy".into()))?;
    audit(&st, &who, "download", &q.file);
    Ok(Body::from_stream(tokio_util::io::ReaderStream::new(f)).into_response())
}

#[derive(Deserialize)]
struct TraceQuery {
    filename: String,
    asset: Option<String>,
}

async fn trace(State(st): State<Shared>, h: HeaderMap, Query(q): Query<TraceQuery>, body: Body) -> ApiResult<pipeline::TraceResult> {
    let who = auth(&st, &h, false)?;
    let (_guard, path) = save_upload(&st, &q.filename, body).await?;
    let st2 = st.clone();
    let asset = q.asset.clone();
    let r = job(&st, move || Ok(pipeline::trace_leak(&ws(&st2)?, &path, asset.as_deref())?)).await?;
    audit(&st, &who, "trace", &format!("{} -> {} | {}", q.filename, r.asset, r.verdict));
    Ok(Json(r))
}

fn router(st: Shared) -> Router {
    let limit = (st.max_upload as usize).saturating_add(1 << 20);
    Router::new()
        .route("/v1/info", get(info))
        .route("/v1/receivers", get(list_receivers).post(add_receiver))
        .route("/v1/assets", get(list_assets).post(add_asset))
        .route("/v1/mark", post(mark))
        .route("/v1/copies", get(copy_file))
        .route("/v1/trace", post(trace))
        .layer(DefaultBodyLimit::max(limit))
        .with_state(st)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    let data = cli.data;
    match cli.cmd {
        Cmd::Token { action } => {
            match action {
                TokenCmd::New { role, label } => {
                    std::fs::create_dir_all(&data)
                        .map_err(|e| format!("cannot create data folder {}: {e}", data.display()))?;
                    let t = new_token(&data, &role, &label)?;
                    println!("New {role} token (shown once, copy it now):\n\n  {t}\n");
                }
                TokenCmd::List => {
                    for t in load_tokens(&data) {
                        println!("{}  {:<6} {}", t.id, t.role, t.label);
                    }
                }
                TokenCmd::Revoke { id } => {
                    let mut all = load_tokens(&data);
                    let n = all.len();
                    all.retain(|t| t.id != id);
                    save_tokens(&data, &all).map_err(|e| e.to_string())?;
                    println!("{}", if all.len() < n { "revoked" } else { "no token with that id" });
                }
            }
            Ok(())
        }
        Cmd::Serve { bind, max_upload_mb, jobs } => {
            let existed = data.join("master.key").exists();
            Workspace::init(&data).map_err(|e| e.to_string())?;
            std::fs::create_dir_all(data.join("marked")).map_err(|e| e.to_string())?;
            // uploads from an earlier run (for example after a crash) are not needed
            let _ = std::fs::remove_dir_all(data.join("uploads"));
            std::fs::create_dir_all(data.join("uploads")).map_err(|e| e.to_string())?;
            if !existed {
                println!("A new secret key was made in {}. BACK UP master.key. Without it nobody can trace a leak.", data.display());
            }
            if load_tokens(&data).is_empty() {
                let t = new_token(&data, "admin", "first admin")?;
                println!("No token yet. First admin token (shown once, copy it now):\n\n  {t}\n");
            }
            let st: Shared = Arc::new(State_ { data, max_upload: max_upload_mb << 20, jobs: Semaphore::new(jobs.max(1)) });
            let listener = tokio::net::TcpListener::bind(&bind).await.map_err(|e| format!("cannot listen on {bind}: {e}"))?;
            println!("vfp-server listening on {bind} (plain HTTP: use an HTTPS proxy for the internet)");
            axum::serve(listener, router(st))
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
                .map_err(|e| e.to_string())
        }
    }
}
