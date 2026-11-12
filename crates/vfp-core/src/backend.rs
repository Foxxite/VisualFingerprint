//! One interface for "where the workspace lives".
//!
//! `LocalBackend`: the workspace is a folder on this device.
//! `RemoteBackend` (feature `remote`): the workspace is on a `vfp-server`. The device sends only the
//! files it needs to send. Originals and the secret key stay on the server.
//!
//! The command line and the apps talk to `Backend` only. They look the same in both modes.

use crate::error::{err, Result};
use crate::pipeline::{self, Plan, PlanOptions, TraceResult};
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
}

impl LocalBackend {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        LocalBackend { home: home.into() }
    }
    fn ws(&self) -> Result<Workspace> {
        Workspace::open(&self.home)
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
pub use remote::RemoteBackend;

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
