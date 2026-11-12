//! One interface for "where the workspace lives".
//!
//! `LocalBackend`: the workspace is a folder on this device.
//! `RemoteBackend` (feature `remote`): the workspace is on a `vfp-server`. The device sends only the
//! files it needs to send. Originals and the secret key stay on the server.
//!
//! The command line and the apps talk to `Backend` only. They look the same in both modes.

use crate::error::{err, Result};
use crate::pipeline::{self, Plan, PlanOptions, TraceResult};
use crate::vault::Dek;
use crate::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub type Progress<'a> = Option<&'a dyn Fn(usize, usize)>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiverInfo {
    pub id: i64,
    pub name: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetInfo {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub m: usize,
    pub max_receivers: u64,
    pub copies: usize,
}

pub trait Backend {
    /// Short text for the user: where the data lives.
    fn describe(&self) -> String;
    fn receivers(&self) -> Result<Vec<ReceiverInfo>>;
    fn add_receiver(&self, name: &str, note: &str) -> Result<()>;
    fn assets(&self) -> Result<Vec<AssetInfo>>;
    /// Register an original. A remote backend uploads the file once.
    fn add_asset(&self, path: &Path, name: Option<&str>, opts: &PlanOptions) -> Result<(AssetInfo, Plan)>;
    /// Make one marked copy per receiver in `outdir`. Returns `(receiver, file)`.
    fn mark(&self, asset: &str, receivers: &[String], outdir: &Path, progress: Progress) -> Result<Vec<(String, PathBuf)>>;
    /// Find who leaked a file. A remote backend uploads only the leaked file.
    fn trace(&self, leaked: &Path, asset: Option<&str>) -> Result<TraceResult>;
}

pub fn asset_info(ws: &Workspace, a: &crate::workspace::Asset) -> Result<AssetInfo> {
    Ok(AssetInfo { id: a.id, name: a.name.clone(), kind: a.kind.clone(), m: a.m, max_receivers: a.max_receivers, copies: ws.issues(a.id)?.len() })
}

// ---------------------------------------------------------------- local
pub struct LocalBackend {
    pub home: PathBuf,
    dek: Option<Dek>,
}

impl LocalBackend {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        LocalBackend { home: home.into(), dek: None }
    }
    /// Open an encrypted workspace with its key.
    pub fn with_key(home: impl Into<PathBuf>, dek: Option<Dek>) -> Self {
        LocalBackend { home: home.into(), dek }
    }
    fn ws(&self) -> Result<Workspace> {
        Workspace::open_with(&self.home, self.dek.as_ref())
    }
}

impl Backend for LocalBackend {
    fn describe(&self) -> String {
        format!("this device ({})", self.home.display())
    }

    fn receivers(&self) -> Result<Vec<ReceiverInfo>> {
        Ok(self.ws()?.receivers()?.into_iter().map(|r| ReceiverInfo { id: r.id, name: r.name, note: r.note }).collect())
    }

    fn add_receiver(&self, name: &str, note: &str) -> Result<()> {
        self.ws()?.add_receiver(name, note).map(|_| ())
    }

    fn assets(&self) -> Result<Vec<AssetInfo>> {
        let ws = self.ws()?;
        ws.assets()?.iter().map(|a| asset_info(&ws, a)).collect()
    }

    fn add_asset(&self, path: &Path, name: Option<&str>, opts: &PlanOptions) -> Result<(AssetInfo, Plan)> {
        let ws = self.ws()?;
        let (a, plan) = pipeline::add_asset(&ws, path, name, opts)?;
        Ok((asset_info(&ws, &a)?, plan))
    }

    fn mark(&self, asset: &str, receivers: &[String], outdir: &Path, progress: Progress) -> Result<Vec<(String, PathBuf)>> {
        let ws = self.ws()?;
        let a = ws.asset(asset)?;
        let mut out = vec![];
        for r in receivers {
            out.push((r.clone(), pipeline::mark_for_receiver(&ws, &a, r, outdir, progress)?));
        }
        Ok(out)
    }

    fn trace(&self, leaked: &Path, asset: Option<&str>) -> Result<TraceResult> {
        pipeline::trace_leak(&self.ws()?, leaked, asset)
    }
}

// ---------------------------------------------------------------- remote
#[cfg(feature = "remote")]
pub use remote::{RemoteBackend, SealedBackend};

#[cfg(feature = "remote")]
mod remote {
    use super::*;
    use crate::error::Error;
    use crate::vault::{Dek, VaultFile};
    use std::time::Duration;

    pub struct RemoteBackend {
        base: String,
        token: String,
        agent: ureq::Agent,
        key: Option<Dek>,
    }

    /// True for http://localhost, 127.0.0.1 and [::1].
    fn is_loopback_url(url: &str) -> bool {
        let rest = url.split("://").nth(1).unwrap_or("");
        let host = rest.split('/').next().unwrap_or("");
        let host = if let Some(h) = host.strip_prefix('[') { h.split(']').next().unwrap_or("") } else { host.rsplit_once(':').map(|x| x.0).unwrap_or(host) };
        host == "localhost" || host == "127.0.0.1" || host == "::1"
    }

    #[derive(Deserialize)]
    struct MarkReply {
        copies: Vec<(String, String)>,
    }

    #[derive(Deserialize)]
    struct AddReply {
        asset: AssetInfo,
        plan: Plan,
    }

    fn fail(e: ureq::Error) -> Error {
        match e {
            ureq::Error::Status(code, resp) => {
                let body = resp.into_string().unwrap_or_default();
                let msg = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| v["error"].as_str().map(String::from));
                match (code, msg) {
                    (401, _) => Error("the server refused the token (wrong, revoked, or missing)".into()),
                    (423, Some(m)) => Error(m),
                    (403, Some(m)) if m.contains("key") => Error(m),
                    (403, _) => Error("this token is not allowed to do that (it may be a trace-only token)".into()),
                    (_, Some(m)) => Error(m),
                    (c, None) => Error(format!("server error {c}")),
                }
            }
            ureq::Error::Transport(t) => Error(format!("cannot reach the server: {t}")),
        }
    }

    impl RemoteBackend {
        pub fn new(url: &str, token: &str) -> Self {
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(15))
                .timeout_read(Duration::from_secs(3 * 3600)) // a long video can take a while
                .build();
            RemoteBackend { base: url.trim_end_matches('/').to_string(), token: token.to_string(), agent, key: None }
        }

        /// Send this key with every data request. The key is never sent over plain HTTP to another machine.
        pub fn with_key(mut self, key: Option<Dek>) -> Result<Self> {
            if key.is_some() && !self.base.starts_with("https://") && !is_loopback_url(&self.base) {
                return Err(Error("refusing to send the key over plain HTTP. Use an https:// address.".into()));
            }
            self.key = key;
            Ok(self)
        }

        fn req(&self, method: &str, path: &str) -> ureq::Request {
            let r = self.agent.request(method, &format!("{}{}", self.base, path)).set("Authorization", &format!("Bearer {}", self.token));
            match &self.key {
                Some(k) => r.set("X-VFP-Key", &k.to_hex()),
                None => r,
            }
        }

        /// The server's `vault.json`, if it has one. A new device unlocks it with the passphrase.
        pub fn vault(&self) -> Result<Option<VaultFile>> {
            match self.req("GET", "/v1/vault").call() {
                Ok(r) => Ok(Some(serde_json::from_reader(r.into_reader())?)),
                Err(ureq::Error::Status(404, _)) => Ok(None),
                Err(e) => Err(fail(e)),
            }
        }

        /// Create the vault on a fresh server (admin token). Needs [`RemoteBackend::with_key`] with the new key.
        pub fn create_vault(&self, v: &VaultFile) -> Result<()> {
            self.req("POST", "/v1/vault").send_json(serde_json::to_value(v)?).map_err(fail)?;
            Ok(())
        }

        fn json<T: serde::de::DeserializeOwned>(&self, r: ureq::Request) -> Result<T> {
            let resp = r.call().map_err(fail)?;
            serde_json::from_reader(resp.into_reader()).map_err(Error::from)
        }

        fn upload<T: serde::de::DeserializeOwned>(&self, r: ureq::Request, file: &Path) -> Result<T> {
            let resp = r.send(std::fs::File::open(file)?).map_err(fail)?;
            serde_json::from_reader(resp.into_reader()).map_err(Error::from)
        }

        /// Ask the server who we are. Also checks the token.
        pub fn info(&self) -> Result<serde_json::Value> {
            self.json(self.req("GET", "/v1/info"))
        }
    }

    fn file_name(p: &Path) -> String {
        p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    }

    impl Backend for RemoteBackend {
        fn describe(&self) -> String {
            format!("server {}", self.base)
        }

        fn receivers(&self) -> Result<Vec<ReceiverInfo>> {
            self.json(self.req("GET", "/v1/receivers"))
        }

        fn add_receiver(&self, name: &str, note: &str) -> Result<()> {
            self.req("POST", "/v1/receivers").send_json(serde_json::json!({"name": name, "note": note})).map_err(fail)?;
            Ok(())
        }

        fn assets(&self) -> Result<Vec<AssetInfo>> {
            self.json(self.req("GET", "/v1/assets"))
        }

        fn add_asset(&self, path: &Path, name: Option<&str>, o: &PlanOptions) -> Result<(AssetInfo, Plan)> {
            let mut r = self
                .req("POST", "/v1/assets")
                .query("filename", &file_name(path))
                .query("max_receivers", &o.max_receivers.to_string())
                .query("colluders", &o.n_colluders.to_string())
                .query("eps", &o.eps.to_string());
            if let Some(n) = name {
                r = r.query("name", n);
            }
            if let Some(s) = o.strength {
                r = r.query("strength", &s.to_string());
            }
            if let Some(s) = o.slots {
                r = r.query("slots", &s.to_string());
            }
            if let Some(c) = o.crf {
                r = r.query("crf", &c.to_string());
            }
            let reply: AddReply = self.upload(r, path)?;
            Ok((reply.asset, reply.plan))
        }

        fn mark(&self, asset: &str, receivers: &[String], outdir: &Path, progress: Progress) -> Result<Vec<(String, PathBuf)>> {
            std::fs::create_dir_all(outdir)?;
            let resp = self
                .req("POST", "/v1/mark")
                .send_json(serde_json::json!({"asset": asset, "receivers": receivers}))
                .map_err(fail)?;
            let reply: MarkReply = serde_json::from_reader(resp.into_reader())?;
            let mut out = vec![];
            for (i, (rec, file)) in reply.copies.iter().enumerate() {
                let safe = Path::new(file).file_name().ok_or_else(|| Error("bad file name from server".into()))?;
                let dest = outdir.join(safe);
                let resp = self.req("GET", "/v1/copies").query("file", file).call().map_err(fail)?;
                std::io::copy(&mut resp.into_reader(), &mut std::fs::File::create(&dest)?)?;
                out.push((rec.clone(), dest));
                if let Some(cb) = progress {
                    cb(i + 1, reply.copies.len());
                }
            }
            Ok(out)
        }

        fn trace(&self, leaked: &Path, asset: Option<&str>) -> Result<TraceResult> {
            let mut r = self.req("POST", "/v1/trace").query("filename", &file_name(leaked));
            if let Some(a) = asset {
                r = r.query("asset", a);
            }
            self.upload(r, leaked)
        }
    }

    // ------------------------------------------------------------ blobs and the sealed backend
    #[derive(Debug, Clone, Deserialize)]
    struct BlobMeta {
        id: String,
        etag: String,
    }

    const B_MASTER: &str = "master.enc";
    const B_DB: &str = "vfp.db.enc";
    const B_PREFIX: &str = "o-";
    const DB_AAD: &[u8] = b"blob:vfp.db";
    /// One sync or job at a time in this process: the cache files are shared and the temp names are fixed.
    static CACHE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn cache_lock() -> std::sync::MutexGuard<'static, ()> {
        CACHE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Key in `sync.json` for the newest database generation this device has seen.
    const GEN_KEY: &str = "_gen";

    /// Writer that cuts the first 8 bytes (the generation number) and keeps them aside.
    struct GenSplit<W: std::io::Write> {
        inner: W,
        head: Vec<u8>,
    }
    impl<W: std::io::Write> std::io::Write for GenSplit<W> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let take = (8 - self.head.len()).min(buf.len());
            self.head.extend_from_slice(&buf[..take]);
            if take < buf.len() {
                self.inner.write_all(&buf[take..])?;
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    impl RemoteBackend {
        fn blob_list(&self) -> Result<Vec<BlobMeta>> {
            self.json(self.req("GET", "/v1/blobs"))
        }

        /// Download a blob to `dest` (through a temp file). Returns the ETag.
        fn blob_get(&self, id: &str, dest: &Path) -> Result<String> {
            let resp = self.req("GET", &format!("/v1/blobs/{id}")).call().map_err(fail)?;
            let etag = resp.header("ETag").unwrap_or("").trim_matches('"').to_string();
            let tmp = dest.with_extension("part");
            std::io::copy(&mut resp.into_reader(), &mut std::fs::File::create(&tmp)?)?;
            std::fs::rename(&tmp, dest)?;
            Ok(etag)
        }

        /// Upload a blob. `if_match = None` means "create only". Returns `Ok(None)` when the
        /// server says the blob changed (HTTP 412).
        fn blob_put(&self, id: &str, file: &Path, if_match: Option<&str>) -> Result<Option<String>> {
            let r = self.req("PUT", &format!("/v1/blobs/{id}"));
            let r = match if_match {
                Some(m) => r.set("If-Match", m),
                None => r.set("If-None-Match", "*"),
            };
            match r.send(std::fs::File::open(file)?) {
                Ok(resp) => {
                    let v: serde_json::Value = serde_json::from_reader(resp.into_reader())?;
                    Ok(Some(v["etag"].as_str().unwrap_or_default().to_string()))
                }
                Err(ureq::Error::Status(412, _)) => Ok(None),
                Err(e) => Err(fail(e)),
            }
        }

        /// Replace `vault.json` on a zero-knowledge server (new passphrase, same key).
        pub fn replace_vault(&self, v: &VaultFile) -> Result<()> {
            self.req("PUT", "/v1/vault").send_json(serde_json::to_value(v)?).map_err(fail)?;
            Ok(())
        }
    }

    /// Zero-knowledge mode. The server is only a store of encrypted blobs. This device keeps an
    /// encrypted copy of the workspace in `cache`, syncs it, and does all the work itself.
    /// The server never gets the passphrase or the key.
    pub struct SealedBackend {
        remote: RemoteBackend,
        dek: Dek,
        cache: PathBuf,
    }

    /// ETags we last saw, by blob id. A missing entry means "we have not seen this blob".
    type Seen = std::collections::BTreeMap<String, String>;

    impl SealedBackend {
        /// Where the encrypted cache for a server lives.
        pub fn cache_dir(home: &Path, url: &str) -> PathBuf {
            use sha2::{Digest, Sha256};
            let h = Sha256::digest(url.trim_end_matches('/').as_bytes());
            home.join("zk-cache").join(h[..6].iter().map(|b| format!("{b:02x}")).collect::<String>())
        }

        /// Use a zero-knowledge server whose vault already exists.
        pub fn open(url: &str, token: &str, dek: Dek, cache: PathBuf) -> Self {
            // no key is ever attached to the requests
            SealedBackend { remote: RemoteBackend::new(url, token), dek, cache }
        }

        /// Make the vault on a fresh server and upload the first (empty) workspace.
        pub fn create(url: &str, token: &str, vault: &VaultFile, dek: Dek, cache: PathBuf) -> Result<Self> {
            if vault.protection != crate::vault::Protection::ZeroKnowledge {
                return err("this vault is not a zero-knowledge vault");
            }
            if cache.exists() {
                std::fs::remove_dir_all(&cache)?;
            }
            Workspace::init_encrypted(&cache, vault, &dek)?;
            let me = SealedBackend { remote: RemoteBackend::new(url, token), dek, cache };
            me.remote.create_vault(vault)?;
            me.push()?;
            Ok(me)
        }

        fn seen_path(&self) -> PathBuf {
            self.cache.join("sync.json")
        }
        fn load_seen(&self) -> Seen {
            std::fs::read_to_string(self.seen_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
        }
        fn save_seen(&self, s: &Seen) -> Result<()> {
            std::fs::write(self.seen_path(), serde_json::to_string(s)?)?;
            Ok(())
        }

        fn local(&self) -> LocalBackend {
            LocalBackend::with_key(&self.cache, Some(self.dek.clone()))
        }

        fn orig_file(&self, blob_id: &str) -> PathBuf {
            self.cache.join("originals").join(blob_id.trim_start_matches(B_PREFIX))
        }

        /// Fetch the newest key file and database. Originals come later, only when needed.
        fn pull(&self) -> Result<Vec<BlobMeta>> {
            std::fs::create_dir_all(self.cache.join("originals"))?;
            let list = self.remote.blob_list()?;
            let vf = self.remote.vault()?.ok_or_else(|| Error("the server has no vault".into()))?;
            vf.save(&self.cache.join(crate::vault::VAULT_FILE))?;
            let mut seen = self.load_seen();
            let find = |id: &str| list.iter().find(|b| b.id == id);
            let (Some(master), Some(db)) = (find(B_MASTER), find(B_DB)) else {
                return err("the server vault is incomplete (no key file or database). Was creating it interrupted?");
            };
            if seen.get(B_MASTER) != Some(&master.etag) || !self.cache.join("master.key.enc").exists() {
                let etag = self.remote.blob_get(B_MASTER, &self.cache.join("master.key.enc"))?;
                seen.insert(B_MASTER.into(), etag);
            }
            let db_file = self.cache.join("vfp.db");
            if seen.get(B_DB) != Some(&db.etag) || !db_file.exists() {
                let sealed = self.cache.join("vfp.db.enc");
                let etag = self.remote.blob_get(B_DB, &sealed)?;
                let mut input = std::io::BufReader::new(std::fs::File::open(&sealed)?);
                let tmp = self.cache.join("vfp.db.new");
                let gen = {
                    let mut out = GenSplit { inner: std::io::BufWriter::new(std::fs::File::create(&tmp)?), head: vec![] };
                    crate::vault::decrypt_stream(&self.dek.subkey("files"), DB_AAD, &mut input, &mut out)
                        .map_err(|_| Error("wrong key for this server, or the database was changed".into()))?;
                    std::io::Write::flush(&mut out)?;
                    let h: [u8; 8] = out.head.as_slice().try_into().map_err(|_| Error("the database blob is damaged".into()))?;
                    u64::from_le_bytes(h)
                };
                drop(input);
                let newest: u64 = seen.get(GEN_KEY).and_then(|v| v.parse().ok()).unwrap_or(0);
                if gen < newest {
                    let _ = std::fs::remove_file(&tmp);
                    let _ = std::fs::remove_file(&sealed);
                    return err("the server sent an OLDER version of your database than this device has seen. It may have been rolled back. Nothing was changed here.");
                }
                seen.insert(GEN_KEY.into(), gen.to_string());
                std::fs::rename(&tmp, &db_file)?;
                let _ = std::fs::remove_file(&sealed);
                seen.insert(B_DB.into(), etag);
            }
            for b in list.iter().filter(|b| b.id.starts_with(B_PREFIX)) {
                seen.insert(b.id.clone(), b.etag.clone());
            }
            self.save_seen(&seen)?;
            Ok(list)
        }

        /// Download the encrypted originals this job needs (all when `only` is None).
        fn fetch_originals(&self, list: &[BlobMeta], only: Option<&str>) -> Result<()> {
            let wanted: Option<String> = match only {
                Some(a) => Some(self.local_ws()?.asset(a)?.orig_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                None => None,
            };
            for b in list.iter().filter(|b| b.id.starts_with(B_PREFIX)) {
                let dest = self.orig_file(&b.id);
                if wanted.as_ref().is_some_and(|w| dest.file_name().map(|n| n.to_string_lossy() != w.as_str()).unwrap_or(true)) {
                    continue;
                }
                if !dest.exists() {
                    self.remote.blob_get(&b.id, &dest)?;
                }
                // mark as used just now, for the least-recently-used rule
                if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&dest) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
            }
            if let Some(w) = &wanted {
                self.trim_cache(list, w);
            }
            Ok(())
        }

        /// Keep the cached originals under a size limit (`VFP_CACHE_MB`, default 4096, 0 = no limit).
        /// Only copies that the server also holds are removed, the one in use stays.
        fn trim_cache(&self, list: &[BlobMeta], keep: &str) {
            let limit: u64 = std::env::var("VFP_CACHE_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(4096) * 1024 * 1024;
            if limit == 0 {
                return;
            }
            let Ok(rd) = std::fs::read_dir(self.cache.join("originals")) else { return };
            let mut files: Vec<(std::time::SystemTime, u64, PathBuf, bool)> = vec![];
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if !name.ends_with(".vfpenc") {
                    continue;
                }
                if let Ok(m) = e.metadata() {
                    let on_server = list.iter().any(|b| b.id == format!("{B_PREFIX}{name}"));
                    files.push((m.modified().unwrap_or(std::time::UNIX_EPOCH), m.len(), e.path(), on_server && name != keep));
                }
            }
            let mut total: u64 = files.iter().map(|f| f.1).sum();
            files.sort_by_key(|f| f.0);
            for (_, size, path, removable) in files {
                if total <= limit {
                    break;
                }
                if removable && std::fs::remove_file(&path).is_ok() {
                    total -= size;
                }
            }
        }

        fn local_ws(&self) -> Result<Workspace> {
            Workspace::open_with(&self.cache, Some(&self.dek))
        }

        /// Send new originals, then the sealed database. A changed server copy stops the upload.
        fn push(&self) -> Result<()> {
            let mut known = self.load_seen();
            if let Ok(rd) = std::fs::read_dir(self.cache.join("originals")) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().to_string();
                    let id = format!("{B_PREFIX}{name}");
                    if !name.ends_with(".vfpenc") || known.contains_key(&id) {
                        continue;
                    }
                    match self.remote.blob_put(&id, &e.path(), None)? {
                        Some(etag) => {
                            known.insert(id, etag);
                        }
                        None => {
                            known.insert(id, String::new()); // somebody else has it already
                        }
                    }
                }
            }
            if !known.contains_key(B_MASTER) {
                if let Some(etag) = self.remote.blob_put(B_MASTER, &self.cache.join("master.key.enc"), None)? {
                    known.insert(B_MASTER.into(), etag);
                }
            }
            let sealed = self.cache.join("vfp.db.enc");
            let mut gen = 0u64;
            {
                let next = known.get(GEN_KEY).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) + 1;
                gen = next;
                let mut input = std::io::Read::chain(std::io::Cursor::new(next.to_le_bytes().to_vec()), std::fs::File::open(self.cache.join("vfp.db"))?);
                let mut out = std::io::BufWriter::new(std::fs::File::create(&sealed)?);
                crate::vault::encrypt_stream(&self.dek.subkey("files"), DB_AAD, &mut input, &mut out)?;
                std::io::Write::flush(&mut out)?;
            }
            let put = self.remote.blob_put(B_DB, &sealed, known.get(B_DB).map(|s| s.as_str()));
            let _ = std::fs::remove_file(&sealed);
            match put? {
                Some(etag) => {
                    known.insert(B_DB.into(), etag);
                    known.insert(GEN_KEY.into(), gen.to_string());
                    self.save_seen(&known)
                }
                None => {
                    // drop our copy so the next call starts from the server's version
                    known.remove(B_DB);
                    let _ = std::fs::remove_file(self.cache.join("vfp.db"));
                    self.save_seen(&known)?;
                    err("another device changed the workspace at the same moment. Your change was NOT saved. Please run it again.")
                }
            }
        }
    }

    impl Backend for SealedBackend {
        fn describe(&self) -> String {
            format!("server {} (zero-knowledge: the server cannot read your files)", self.remote.base)
        }

        fn receivers(&self) -> Result<Vec<ReceiverInfo>> {
            let _g = cache_lock();
            self.pull()?;
            self.local().receivers()
        }

        fn add_receiver(&self, name: &str, note: &str) -> Result<()> {
            let _g = cache_lock();
            self.pull()?;
            self.local().add_receiver(name, note)?;
            self.push()
        }

        fn assets(&self) -> Result<Vec<AssetInfo>> {
            let _g = cache_lock();
            self.pull()?;
            self.local().assets()
        }

        fn add_asset(&self, path: &Path, name: Option<&str>, opts: &PlanOptions) -> Result<(AssetInfo, Plan)> {
            let _g = cache_lock();
            self.pull()?;
            let r = self.local().add_asset(path, name, opts)?;
            self.push()?;
            Ok(r)
        }

        fn mark(&self, asset: &str, receivers: &[String], outdir: &Path, progress: Progress) -> Result<Vec<(String, PathBuf)>> {
            let _g = cache_lock();
            let list = self.pull()?;
            self.fetch_originals(&list, Some(asset))?;
            let r = self.local().mark(asset, receivers, outdir, progress)?;
            self.push()?;
            Ok(r)
        }

        fn trace(&self, leaked: &Path, asset: Option<&str>) -> Result<TraceResult> {
            let _g = cache_lock();
            let list = self.pull()?;
            self.fetch_originals(&list, asset)?;
            self.local().trace(leaked, asset)
        }
    }
}

/// Settings file for server mode: `<home>/server.json` = `{"url": "...", "token": "..."}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerLink {
    pub url: String,
    pub token: String,
}

impl ServerLink {
    pub fn path(home: &Path) -> PathBuf {
        home.join("server.json")
    }
    pub fn load(home: &Path) -> Result<Option<ServerLink>> {
        let p = Self::path(home);
        if !p.exists() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&std::fs::read_to_string(p)?)?))
    }
    pub fn save(&self, home: &Path) -> Result<()> {
        std::fs::create_dir_all(home)?;
        std::fs::write(Self::path(home), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// Check that a server name from the user looks like an http(s) address.
pub fn check_url(url: &str) -> Result<()> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        err("the server address must start with http:// or https://")
    }
}
