//! `vfp-server`: the workspace on a server you host.
//!
//! The server owns the workspace. A device sends only the file it wants to check. No original
//! ever leaves the server.
//!
//! **Protected mode (default).** Everything on disk is encrypted. The key comes from the client,
//! as the `X-VFP-Key` header, on each request. The server accepts that header only over HTTPS
//! (behind a proxy with `--trust-proxy`) or from a loopback client. A fresh server waits until
//! a client creates the vault (`POST /v1/vault`). `--plain` skips encryption (tests and dev only).
//!
//! Run `vfp-server token new --role admin --label laptop` to make a token. The token is shown once.
//! Put an HTTPS reverse proxy (Caddy, nginx, Traefik) in front for use over the internet.

use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Query, Request, State};
use axum::middleware::{self, Next};
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
use vfp_core::securetemp::{self, SecureTemp};
use vfp_core::vault::{self, Dek, VaultFile, VAULT_FILE};
use vfp_core::workspace::{wipe_file, Protection, Workspace};

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
        /// No encryption for a NEW data folder (tests and development only)
        #[arg(long)]
        plain: bool,
        /// Trust `X-Forwarded-Proto` from a reverse proxy on this machine
        #[arg(long, env = "VFP_TRUST_PROXY")]
        trust_proxy: bool,
    },
    /// Encrypt an existing plain data folder. Reads the passphrase from the file in
    /// VFP_PASSPHRASE_FILE, or from the first line of standard input.
    Migrate,
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
    trust_proxy: bool,
    /// One blob write at a time, so the `If-Match` check and the rename cannot interleave
    blob_lock: tokio::sync::Mutex<()>,
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

fn state_of(st: &State_) -> &'static str {
    match Workspace::protection(&st.data) {
        Protection::Vault => match VaultFile::load(&st.data.join(VAULT_FILE)) {
            Ok(Some(v)) if v.protection == vault::Protection::ZeroKnowledge => "zero-knowledge",
            _ => "protected",
        },
        Protection::None if st.data.join("vfp.db").exists() => "plain",
        Protection::None => "uninitialized",
    }
}

/// The key the client sent (if any).
fn key_of(h: &HeaderMap) -> Result<Option<Dek>, ApiErr> {
    match h.get("x-vfp-key") {
        None => Ok(None),
        Some(v) => {
            let t = v.to_str().map_err(|_| ApiErr(StatusCode::BAD_REQUEST, "bad key header".into()))?;
            Ok(Some(Dek::from_hex(t).map_err(|_| ApiErr(StatusCode::BAD_REQUEST, "bad key header".into()))?))
        }
    }
}

/// Open the workspace with the client's key. The key is checked here, never stored.
fn ws(st: &State_, h: &HeaderMap) -> Result<(Workspace, Option<Dek>), ApiErr> {
    match state_of(st) {
        "uninitialized" => Err(ApiErr(StatusCode::SERVICE_UNAVAILABLE, "this server has no vault yet. Create one first (app or: vfp vault init)".into())),
        "zero-knowledge" => Err(ApiErr(StatusCode::CONFLICT, "this server is zero-knowledge: it only stores encrypted blobs. The client does the work.".into())),
        "plain" => Ok((Workspace::open(&st.data)?, None)),
        _ => {
            let dek = key_of(h)?.ok_or_else(|| ApiErr(StatusCode::LOCKED, "locked: the request needs the key (unlock with your passphrase)".into()))?;
            match Workspace::open_with(&st.data, Some(&dek)) {
                Ok(w) => Ok((w, Some(dek))),
                Err(e) => Err(ApiErr(StatusCode::FORBIDDEN, e.to_string())),
            }
        }
    }
}

/// The key header is only accepted on a safe connection.
async fn require_secure(State(st): State<Shared>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, req: Request, next: Next) -> Response {
    if req.headers().contains_key("x-vfp-key") {
        let https = req.headers().get("x-forwarded-proto").and_then(|v| v.to_str().ok()).map(|v| v.eq_ignore_ascii_case("https")).unwrap_or(false);
        let ok = if st.trust_proxy { https } else { peer.ip().is_loopback() };
        if !ok {
            return ApiErr(StatusCode::BAD_REQUEST, "the key is only accepted over HTTPS. Use an HTTPS proxy and --trust-proxy.".into()).into_response();
        }
    }
    next.run(req).await
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
        if let Ok(rd) = std::fs::read_dir(&self.0) {
            for e in rd.flatten() {
                wipe_file(&e.path());
            }
        }
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
    let state = state_of(&st);
    // the asset count needs the key on a protected server
    let n = ws(&st, &h).ok().and_then(|(w, _)| w.assets().ok()).map(|a| a.len());
    Ok(Json(serde_json::json!({"server": "vfp-server", "version": env!("CARGO_PKG_VERSION"), "role": who.role, "assets": n, "protection": state})))
}

async fn list_receivers(State(st): State<Shared>, h: HeaderMap) -> ApiResult<Vec<ReceiverInfo>> {
    auth(&st, &h, false)?;
    let r = ws(&st, &h)?.0.receivers()?;
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
    ws(&st, &h)?.0.add_receiver(&b.name, &b.note)?;
    audit(&st, &who, "add-receiver", "");
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn list_assets(State(st): State<Shared>, h: HeaderMap) -> ApiResult<Vec<AssetInfo>> {
    auth(&st, &h, false)?;
    let (w, _) = ws(&st, &h)?;
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
    let name = q.name.clone();
    let opened = ws(&st, &h)?;
    let (info, plan) = job(&st, move || {
        let w = opened.0;
        let (a, plan) = pipeline::add_asset(&w, &path, name.as_deref(), &opts)?;
        Ok((asset_info(&w, &a)?, plan))
    })
    .await?;
    audit(&st, &who, "add-asset", &format!("asset {}", info.id));
    Ok(Json(serde_json::json!({"asset": info, "plan": plan})))
}

#[derive(Deserialize)]
struct MarkBody {
    asset: String,
    receivers: Vec<String>,
}

async fn mark(State(st): State<Shared>, h: HeaderMap, Json(b): Json<MarkBody>) -> ApiResult<serde_json::Value> {
    let who = auth(&st, &h, true)?;
    let opened = ws(&st, &h)?;
    let st2 = st.clone();
    let asset = b.asset.clone();
    let n = b.receivers.len();
    let copies = job(&st, move || {
        let (w, dek) = opened;
        let a = w.asset(&asset)?;
        let marked = st2.data.join("marked");
        std::fs::create_dir_all(&marked).map_err(internal)?;
        let mut out = vec![];
        match dek {
            None => {
                for r in &b.receivers {
                    let p = pipeline::mark_for_receiver(&w, &a, r, &marked, None)?;
                    out.push((r.clone(), p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
                }
            }
            Some(dek) => {
                // make the copies in a private folder, store them encrypted, wipe the clear text
                let tmp = SecureTemp::new(&st2.data.join("tmp"))?;
                for r in &b.receivers {
                    let p = pipeline::mark_for_receiver(&w, &a, r, tmp.path(), None)?;
                    let file = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    store_marked(&dek, &marked, &p, &file)?;
                    wipe_file(&p);
                    // the stored path is only the display name, never a folder on this machine
                    w.issue(a.id, w.receiver(r)?.id, Some(Path::new(&file)))?;
                    out.push((r.clone(), file));
                }
            }
        }
        Ok(out)
    })
    .await?;
    audit(&st, &who, "mark", &format!("{} copies", n.min(copies.len())));
    Ok(Json(serde_json::json!({"copies": copies})))
}

/// Disk name of a stored marked copy: a keyed hash, so file names never show names.
fn stored_name(dek: &Dek, file: &str) -> String {
    format!("{}.vfpenc", vault::blind_index(&dek.subkey("db-index"), &format!("marked:{file}")))
}

/// Encrypt one marked copy into the `marked` folder.
fn store_marked(dek: &Dek, marked: &Path, src: &Path, file: &str) -> Result<(), ApiErr> {
    let mut input = std::fs::File::open(src).map_err(internal)?;
    let mut o = std::io::BufWriter::new(std::fs::File::create(marked.join(stored_name(dek, file))).map_err(internal)?);
    vault::encrypt_stream(&dek.subkey("files"), format!("marked:{file}").as_bytes(), &mut input, &mut o)?;
    o.flush().map_err(internal)?;
    Ok(())
}

#[derive(Deserialize)]
struct FileQuery {
    file: String,
}

/// Writes decrypted chunks into a channel, so the response streams without a temp file.
struct ChanWriter(tokio::sync::mpsc::Sender<Result<Vec<u8>, String>>);

impl std::io::Write for ChanWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.blocking_send(Ok(buf.to_vec())).map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client left"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

async fn copy_file(State(st): State<Shared>, h: HeaderMap, Query(q): Query<FileQuery>) -> Result<Response, ApiErr> {
    let who = auth(&st, &h, true)?;
    // a bare file name only: no folders, no ".."
    if Path::new(&q.file).file_name().map(|n| n.to_string_lossy() != q.file).unwrap_or(true) || q.file.contains("..") {
        return Err(ApiErr(StatusCode::BAD_REQUEST, "bad file name".into()));
    }
    let (_w, dek) = ws(&st, &h)?;
    audit(&st, &who, "download", "");
    let Some(dek) = dek else {
        let f = tokio::fs::File::open(st.data.join("marked").join(&q.file)).await.map_err(|_| ApiErr(StatusCode::NOT_FOUND, "no such copy".into()))?;
        return Ok(Body::from_stream(tokio_util::io::ReaderStream::new(f)).into_response());
    };
    let path = st.data.join("marked").join(stored_name(&dek, &q.file));
    let f = std::fs::File::open(&path).map_err(|_| ApiErr(StatusCode::NOT_FOUND, "no such copy".into()))?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, String>>(4);
    let (tx_err, file) = (tx.clone(), q.file.clone());
    tokio::task::spawn_blocking(move || {
        let mut input = std::io::BufReader::new(f);
        let mut w = ChanWriter(tx);
        if let Err(e) = vault::decrypt_stream(&dek.subkey("files"), format!("marked:{file}").as_bytes(), &mut input, &mut w) {
            let _ = tx_err.blocking_send(Err(e.to_string()));
        }
    });
    // look at the first piece before answering, so a wrong key gives a clear error
    match rx.recv().await {
        Some(Err(e)) => Err(ApiErr(StatusCode::FORBIDDEN, e)),
        None => Ok(Body::empty().into_response()),
        Some(Ok(first)) => {
            let rest = futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|m| (m.map_err(std::io::Error::other), rx))
            });
            let all = futures_util::stream::once(async move { Ok::<_, std::io::Error>(first) }).chain(rest);
            Ok(Body::from_stream(all.map(|r| r.map(axum::body::Bytes::from))).into_response())
        }
    }
}

#[derive(Deserialize)]
struct TraceQuery {
    filename: String,
    asset: Option<String>,
}

async fn trace(State(st): State<Shared>, h: HeaderMap, Query(q): Query<TraceQuery>, body: Body) -> ApiResult<pipeline::TraceResult> {
    let who = auth(&st, &h, false)?;
    let opened = ws(&st, &h)?;
    let (_guard, path) = save_upload(&st, &q.filename, body).await?;
    let asset = q.asset.clone();
    let r = job(&st, move || Ok(pipeline::trace_leak(&opened.0, &path, asset.as_deref())?)).await?;
    audit(&st, &who, "trace", &format!("{} flagged", r.flagged.len()));
    Ok(Json(r))
}

// ---------------------------------------------------------------- vault
async fn get_vault(State(st): State<Shared>, h: HeaderMap) -> Result<Response, ApiErr> {
    auth(&st, &h, false)?;
    match VaultFile::load(&st.data.join(VAULT_FILE))? {
        Some(v) => Ok(Json(v).into_response()),
        None => Err(ApiErr(StatusCode::NOT_FOUND, "this server has no vault".into())),
    }
}

/// Create the vault on a fresh server. The client made the key; the server gets it for this request only.
async fn create_vault(State(st): State<Shared>, h: HeaderMap, Json(v): Json<VaultFile>) -> ApiResult<serde_json::Value> {
    let who = auth(&st, &h, true)?;
    if state_of(&st) != "uninitialized" {
        return Err(ApiErr(StatusCode::CONFLICT, "this server already holds a workspace".into()));
    }
    if v.protection == vault::Protection::ZeroKnowledge {
        // the server must never get the key in this mode
        if h.contains_key("x-vfp-key") {
            return Err(ApiErr(StatusCode::BAD_REQUEST, "a zero-knowledge vault must not be created with a key".into()));
        }
        std::fs::create_dir_all(st.data.join("blobs")).map_err(internal)?;
        v.save(&st.data.join(VAULT_FILE))?;
        audit(&st, &who, "create-vault", "zero-knowledge");
        return Ok(Json(serde_json::json!({"ok": true})));
    }
    let dek = key_of(&h)?.ok_or_else(|| ApiErr(StatusCode::BAD_REQUEST, "the key header is missing".into()))?;
    Workspace::init_encrypted(&st.data, &v, &dek)?;
    std::fs::create_dir_all(st.data.join("marked")).map_err(internal)?;
    audit(&st, &who, "create-vault", "");
    Ok(Json(serde_json::json!({"ok": true})))
}

/// Replace `vault.json` (new passphrase, same key). Only for a zero-knowledge server: there the
/// server cannot check the key, so it trusts an admin token with this one file.
async fn put_vault(State(st): State<Shared>, h: HeaderMap, Json(v): Json<VaultFile>) -> ApiResult<serde_json::Value> {
    let who = auth(&st, &h, true)?;
    if state_of(&st) != "zero-knowledge" || v.protection != vault::Protection::ZeroKnowledge {
        return Err(ApiErr(StatusCode::CONFLICT, "only a zero-knowledge vault can be replaced this way".into()));
    }
    let _g = st.blob_lock.lock().await;
    v.save(&st.data.join(VAULT_FILE))?;
    audit(&st, &who, "replace-vault", "");
    Ok(Json(serde_json::json!({"ok": true})))
}

// ---------------------------------------------------------------- blobs (zero-knowledge)
/// Blob ids are plain file names. The client picks them. They are never paths.
fn blob_path(st: &State_, id: &str) -> Result<PathBuf, ApiErr> {
    let ok = !id.is_empty() && id.len() <= 100 && !id.starts_with('.') && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if ok { Ok(st.data.join("blobs").join(id)) } else { Err(ApiErr(StatusCode::BAD_REQUEST, "bad blob id".into())) }
}

/// A tag that changes whenever the blob changes (size and write time).
fn etag_of(p: &Path) -> Option<String> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{:x}-{:x}", m.len(), t))
}

/// Blob routes work only on a zero-knowledge server, and never with a key header.
fn blob_gate(st: &State_, h: &HeaderMap) -> Result<(), ApiErr> {
    if state_of(st) != "zero-knowledge" {
        return Err(ApiErr(StatusCode::CONFLICT, "this server is not in zero-knowledge mode".into()));
    }
    if h.contains_key("x-vfp-key") {
        return Err(ApiErr(StatusCode::BAD_REQUEST, "a zero-knowledge server does not take the key. Remove the X-VFP-Key header.".into()));
    }
    Ok(())
}

#[derive(Serialize)]
struct BlobInfo {
    id: String,
    size: u64,
    etag: String,
}

async fn list_blobs(State(st): State<Shared>, h: HeaderMap) -> ApiResult<Vec<BlobInfo>> {
    auth(&st, &h, false)?;
    blob_gate(&st, &h)?;
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(st.data.join("blobs")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            if let (Ok(m), Some(etag)) = (e.metadata(), etag_of(&e.path())) {
                out.push(BlobInfo { id: name, size: m.len(), etag });
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Json(out))
}

async fn get_blob(State(st): State<Shared>, h: HeaderMap, axum::extract::Path(id): axum::extract::Path<String>) -> Result<Response, ApiErr> {
    let who = auth(&st, &h, false)?;
    blob_gate(&st, &h)?;
    let path = blob_path(&st, &id)?;
    let f = tokio::fs::File::open(&path).await.map_err(|_| ApiErr(StatusCode::NOT_FOUND, "no such blob".into()))?;
    let etag = etag_of(&path).unwrap_or_default();
    audit(&st, &who, "blob-get", "");
    let mut r = Body::from_stream(tokio_util::io::ReaderStream::new(f)).into_response();
    if let Ok(v) = etag.parse() {
        r.headers_mut().insert(header::ETAG, v);
    }
    Ok(r)
}

/// Store a blob. `If-Match: <etag>` = replace only that version. `If-None-Match: *` = create only.
/// A failed check gives 412, so two devices cannot overwrite each other without noticing.
async fn put_blob(State(st): State<Shared>, h: HeaderMap, axum::extract::Path(id): axum::extract::Path<String>, body: Body) -> Result<Response, ApiErr> {
    let who = auth(&st, &h, true)?;
    blob_gate(&st, &h)?;
    let path = blob_path(&st, &id)?;
    let if_match = h.get(header::IF_MATCH).and_then(|v| v.to_str().ok()).map(|v| v.trim().trim_matches('"').to_string());
    let create_only = h.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).map(|v| v.trim() == "*").unwrap_or(false);
    if if_match.is_none() && !create_only {
        return Err(ApiErr(StatusCode::PRECONDITION_REQUIRED, "send If-Match (replace) or If-None-Match: * (create)".into()));
    }
    // receive into a hidden file first; the check and the rename happen under one lock
    let tmp = st.data.join("blobs").join(format!(".tmp-{}", hex_rand(8)));
    let result = async {
        let mut file = tokio::fs::File::create(&tmp).await.map_err(internal)?;
        let (mut total, mut stream) = (0u64, body.into_data_stream());
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| ApiErr(StatusCode::BAD_REQUEST, format!("upload failed: {e}")))?;
            total += chunk.len() as u64;
            if total > st.max_upload {
                return Err(ApiErr(StatusCode::PAYLOAD_TOO_LARGE, "blob is larger than the limit".into()));
            }
            file.write_all(&chunk).await.map_err(internal)?;
        }
        file.flush().await.map_err(internal)?;
        drop(file);
        let _g = st.blob_lock.lock().await;
        let current = etag_of(&path);
        let fine = match &if_match {
            Some(m) => current.as_deref() == Some(m.as_str()),
            None => current.is_none(),
        };
        if !fine {
            return Err(ApiErr(StatusCode::PRECONDITION_FAILED, "the blob changed since you read it".into()));
        }
        tokio::fs::rename(&tmp, &path).await.map_err(internal)?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result?;
    audit(&st, &who, "blob-put", "");
    Ok(Json(serde_json::json!({"etag": etag_of(&path).unwrap_or_default()})).into_response())
}

fn router(st: Shared) -> Router {
    let limit = (st.max_upload as usize).saturating_add(1 << 20);
    Router::new()
        .route("/v1/info", get(info))
        .route("/v1/vault", get(get_vault).post(create_vault).put(put_vault))
        .route("/v1/blobs", get(list_blobs))
        .route("/v1/blobs/:id", get(get_blob).put(put_blob))
        .route("/v1/receivers", get(list_receivers).post(add_receiver))
        .route("/v1/assets", get(list_assets).post(add_asset))
        .route("/v1/mark", post(mark))
        .route("/v1/copies", get(copy_file))
        .route("/v1/trace", post(trace))
        .layer(DefaultBodyLimit::max(limit))
        .layer(middleware::from_fn_with_state(st.clone(), require_secure))
        .with_state(st)
}

/// Passphrase for `migrate`: a file (VFP_PASSPHRASE_FILE) or the first line of standard input.
fn read_passphrase() -> Result<String, String> {
    if let Ok(p) = std::env::var("VFP_PASSPHRASE_FILE") {
        return std::fs::read_to_string(&p).map(|s| s.trim_end_matches(['\r', '\n']).to_string()).map_err(|e| format!("cannot read {p}: {e}"));
    }
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
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
        Cmd::Migrate => {
            if Workspace::protection(&data) == Protection::Vault {
                return Err("this data folder is already encrypted".into());
            }
            let pass = read_passphrase()?;
            let (vf, dek) = VaultFile::create(&pass, vault::Protection::Protected, vault::KdfParams::DESKTOP).map_err(|e| e.to_string())?;
            Workspace::migrate_to_vault(&data, &vf, &dek).map_err(|e| e.to_string())?;
            // marked copies from the plain days: encrypt them too
            let marked = data.join("marked");
            if let Ok(rd) = std::fs::read_dir(&marked) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension().is_some_and(|x| x == "vfpenc") || !p.is_file() {
                        continue;
                    }
                    let file = e.file_name().to_string_lossy().to_string();
                    store_marked(&dek, &marked, &p, &file).map_err(|e| e.1)?;
                    wipe_file(&p);
                }
            }
            println!("Done. The data folder is encrypted. Back up {}.", data.join(VAULT_FILE).display());
            println!("Clients now need the passphrase. Lose it and the data is gone.");
            Ok(())
        }
        Cmd::Serve { bind, max_upload_mb, jobs, plain, trust_proxy } => {
            std::fs::create_dir_all(&data).map_err(|e| format!("cannot create data folder {}: {e}", data.display()))?;
            let probe = State_ { data: data.clone(), max_upload: 0, jobs: Semaphore::new(1), trust_proxy, blob_lock: tokio::sync::Mutex::new(()) };
            let state = state_of(&probe);
            if state == "uninitialized" && plain {
                Workspace::init(&data).map_err(|e| e.to_string())?;
                println!("A new secret key was made in {}. BACK UP master.key. Everything is stored in CLEAR TEXT (--plain).", data.display());
            }
            match state {
                "plain" => println!("WARNING: this data folder is not encrypted. Run: vfp-server migrate"),
                "uninitialized" if !plain => println!("No vault yet. Create one from the app or the CLI (vfp vault init) to start."),
                _ => {}
            }
            // anything left from an earlier run (for example after a crash) is wiped
            securetemp::sweep(&data.join("uploads"));
            securetemp::sweep(&data.join("tmp"));
            // half-received blobs from a crash
            if let Ok(rd) = std::fs::read_dir(data.join("blobs")) {
                for e in rd.flatten() {
                    if e.file_name().to_string_lossy().starts_with(".tmp-") {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            for d in ["uploads", "marked", "tmp"] {
                std::fs::create_dir_all(data.join(d)).map_err(|e| e.to_string())?;
            }
            if load_tokens(&data).is_empty() {
                let t = new_token(&data, "admin", "first admin")?;
                println!("No token yet. First admin token (shown once, copy it now):\n\n  {t}\n");
            }
            let st: Shared = Arc::new(State_ { data, max_upload: max_upload_mb << 20, jobs: Semaphore::new(jobs.max(1)), trust_proxy, blob_lock: tokio::sync::Mutex::new(()) });
            let listener = tokio::net::TcpListener::bind(&bind).await.map_err(|e| format!("cannot listen on {bind}: {e}"))?;
            println!("vfp-server listening on {bind} (plain HTTP: the key is accepted from this machine only, or behind an HTTPS proxy with --trust-proxy)");
            axum::serve(listener, router(st).into_make_service_with_connect_info::<std::net::SocketAddr>())
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                })
                .await
                .map_err(|e| e.to_string())
        }
    }
}
