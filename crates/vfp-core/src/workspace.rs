//! Workspace: the SQLite database and the master key.
//!
//! Same layout and schema as the Python version, so either program can open the other's workspace:
//!
//! ```text
//! master.key   secret. Without it nobody can trace a leak. Back it up. Never share it.
//! vfp.db       receivers, assets, issued copies
//! originals/   private copies of the unmarked files (tracing needs them)
//! ```
//!
//! An encrypted workspace also has `vault.json`. Then `master.key.enc` replaces `master.key`,
//! originals are `originals/<random>.vfpenc`, and names, notes, hashes and paths in the database are
//! sealed (receiver and asset names are found through a keyed hash). Open it with [`Workspace::open_with`].

use crate::error::{err, Error, Result};
use crate::keys::derive_seed;
use crate::securetemp::SecureTemp;
use crate::vault::{self, Dek, VaultFile, VAULT_FILE};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

const MASTER_ENC: &str = "master.key.enc";
const ENC: &str = "enc:";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS receivers (
    id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, note TEXT DEFAULT '', created REAL);
CREATE TABLE IF NOT EXISTS assets (
    id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL, kind TEXT NOT NULL,
    orig_path TEXT, sha256 TEXT, params TEXT, m INTEGER, max_receivers INTEGER,
    n_colluders INTEGER, eps REAL, created REAL);
CREATE TABLE IF NOT EXISTS issues (
    id INTEGER PRIMARY KEY, asset_id INTEGER NOT NULL, receiver_id INTEGER NOT NULL,
    user_index INTEGER NOT NULL, out_path TEXT, created REAL,
    UNIQUE(asset_id, receiver_id), UNIQUE(asset_id, user_index));
";

#[derive(Clone, Debug)]
pub struct Receiver {
    pub id: i64,
    pub name: String,
    pub note: String,
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub orig_path: PathBuf,
    /// File name of the original (for its extension)
    pub file_name: String,
    pub sha256: String,
    pub params: serde_json::Value,
    pub m: usize,
    pub max_receivers: u64,
    pub n_colluders: u64,
    pub eps: f64,
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub user_index: u64,
    pub out_path: String,
    pub name: String,
}

pub struct Workspace {
    pub home: PathBuf,
    master: Vec<u8>,
    db: Connection,
    dek: Option<Dek>,
}

/// A readable copy of an original. For an encrypted workspace it lives in a private temp folder
/// and is wiped when this value is dropped.
pub struct PlainOriginal {
    path: PathBuf,
    _tmp: Option<SecureTemp>,
}

impl PlainOriginal {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.is_ascii() {
        return err("master.key is not valid hex");
    }
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|_| Error("master.key is not valid hex".into())))
        .collect()
}

/// Read the master key. Make a new one if the file is missing.
fn load_or_create_key(path: &Path) -> Result<Vec<u8>> {
    if path.exists() {
        return from_hex(&std::fs::read_to_string(path)?);
    }
    let mut key = vec![0u8; 32];
    getrandom::getrandom(&mut key)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, to_hex(&key))?;
    Ok(key)
}

fn id_from(key: &str) -> i64 {
    if !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) { key.parse().unwrap_or(-1) } else { -1 }
}

fn rand_hex(n: usize) -> Result<String> {
    let mut b = vec![0u8; n];
    getrandom::getrandom(&mut b)?;
    Ok(to_hex(&b))
}

fn connect(db_path: &Path, encrypted: bool) -> Result<Connection> {
    let c = Connection::open(db_path)?;
    if encrypted {
        // deleted rows must not leave readable text in free pages
        c.execute_batch("PRAGMA secure_delete=ON;")?;
    }
    Ok(c)
}

fn write_master_enc(home: &Path, dek: &Dek, master: &[u8]) -> Result<()> {
    let sealed = vault::seal(&dek.subkey("master"), b"master", master)?;
    let tmp = home.join("master.key.enc.tmp");
    std::fs::write(&tmp, to_hex(&sealed))?;
    std::fs::rename(tmp, home.join(MASTER_ENC))?;
    Ok(())
}

fn read_master_enc(home: &Path, dek: &Dek) -> Result<Vec<u8>> {
    let sealed = from_hex(&std::fs::read_to_string(home.join(MASTER_ENC))?)?;
    vault::open(&dek.subkey("master"), b"master", &sealed)
}

/// Where the workspace stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// No vault: everything is in clear text.
    None,
    /// `vault.json` exists: a key is needed.
    Vault,
}

impl Workspace {
    pub fn protection(home: &Path) -> Protection {
        if home.join(VAULT_FILE).exists() { Protection::Vault } else { Protection::None }
    }

    pub fn is_encrypted(&self) -> bool {
        self.dek.is_some()
    }

    /// Open a plain workspace. An encrypted one gives a "locked" error. Use [`Workspace::open_with`].
    pub fn open(home: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(home, None)
    }

    pub fn open_with(home: impl AsRef<Path>, dek: Option<&Dek>) -> Result<Self> {
        let home = home.as_ref().to_path_buf();
        let db_path = home.join("vfp.db");
        if !db_path.exists() {
            return err(format!("No workspace at {}. Run: vfp init", home.display()));
        }
        let vault = Self::protection(&home) == Protection::Vault;
        let (master, dek) = match (vault, dek) {
            (true, None) => return err("This workspace is locked. It needs the passphrase (the key)."),
            (true, Some(d)) => {
                if !home.join(MASTER_ENC).exists() {
                    return err("This workspace has a vault but no master.key.enc. Run: vfp vault migrate");
                }
                (read_master_enc(&home, d).map_err(|_| Error("wrong key for this workspace".into()))?, Some(d.clone()))
            }
            (false, Some(_)) => return err("This workspace has no vault, but a key was given."),
            (false, None) => (load_or_create_key(&home.join("master.key"))?, None),
        };
        Ok(Workspace { db: connect(&db_path, dek.is_some())?, home, master, dek })
    }

    /// Make the folder, the key and the database (safe to call on an existing workspace).
    pub fn init(home: impl AsRef<Path>) -> Result<Self> {
        let home = home.as_ref();
        std::fs::create_dir_all(home.join("originals"))?;
        load_or_create_key(&home.join("master.key"))?;
        Connection::open(home.join("vfp.db"))?.execute_batch(SCHEMA)?;
        Self::open(home)
    }

    /// Make a new encrypted workspace. Writes `vault.json` too, so the folder is complete.
    pub fn init_encrypted(home: impl AsRef<Path>, vault: &VaultFile, dek: &Dek) -> Result<Self> {
        let home = home.as_ref();
        if home.join("vfp.db").exists() || home.join("master.key").exists() || home.join(VAULT_FILE).exists() {
            return err("This folder already holds a workspace.");
        }
        std::fs::create_dir_all(home.join("originals"))?;
        let mut master = vec![0u8; 32];
        getrandom::getrandom(&mut master)?;
        write_master_enc(home, dek, &master)?;
        connect(&home.join("vfp.db"), true)?.execute_batch(SCHEMA)?;
        vault.save(&home.join(VAULT_FILE))?;
        Self::open_with(home, Some(dek))
    }

    /// Private temp root for decrypted files.
    pub fn temp_root(&self) -> PathBuf {
        self.home.join("tmp")
    }

    // ---- seeds: all derived from the master key; nothing secret lives in the database
    pub fn tardos_seed(&self, asset_id: i64) -> u64 {
        derive_seed(&self.master, &["asset", &asset_id.to_string(), "tardos"])
    }

    pub fn layout_seed(&self, asset_id: i64) -> u64 {
        derive_seed(&self.master, &["asset", &asset_id.to_string(), "layout"])
    }

    // ---- sealed database fields
    fn seal_json(&self, label: &str, v: &serde_json::Value) -> Result<String> {
        let d = self.dek.as_ref().expect("encrypted workspace");
        Ok(format!("{ENC}{}", to_hex(&vault::seal(&d.subkey("db-fields"), label.as_bytes(), v.to_string().as_bytes())?)))
    }

    fn open_json(&self, label: &str, text: &str) -> Result<serde_json::Value> {
        let d = self.dek.as_ref().expect("encrypted workspace");
        let body = text.strip_prefix(ENC).ok_or_else(|| Error("database field is not sealed. Run: vfp vault migrate".into()))?;
        let plain = vault::open(&d.subkey("db-fields"), label.as_bytes(), &from_hex(body)?)?;
        Ok(serde_json::from_slice(&plain)?)
    }

    /// The text stored in the `name` column (the name itself, or its keyed hash).
    fn name_key(&self, name: &str) -> String {
        match &self.dek {
            Some(d) => vault::blind_index(&d.subkey("db-index"), name),
            None => name.to_string(),
        }
    }

    fn receiver_from(&self, id: i64, name: String, note: String) -> Result<Receiver> {
        if self.dek.is_none() {
            return Ok(Receiver { id, name, note });
        }
        let v = self.open_json("receiver", &note)?;
        Ok(Receiver { id, name: v["name"].as_str().unwrap_or_default().into(), note: v["note"].as_str().unwrap_or_default().into() })
    }

    // ---- receivers
    pub fn add_receiver(&self, name: &str, note: &str) -> Result<i64> {
        let (col_name, col_note) = if self.dek.is_some() {
            (self.name_key(name), self.seal_json("receiver", &serde_json::json!({"name": name, "note": note}))?)
        } else {
            (name.to_string(), note.to_string())
        };
        self.db
            .execute("INSERT INTO receivers(name, note, created) VALUES (?1,?2,?3)", params![col_name, col_note, now()])
            .map_err(|_| Error(format!("cannot add receiver '{name}': the name may already exist")))?;
        Ok(self.db.last_insert_rowid())
    }

    pub fn receivers(&self) -> Result<Vec<Receiver>> {
        let mut st = self.db.prepare("SELECT id, name, COALESCE(note,'') FROM receivers ORDER BY id")?;
        let rows: Vec<(i64, String, String)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<_, _>>()?;
        rows.into_iter().map(|(i, n, t)| self.receiver_from(i, n, t)).collect()
    }

    pub fn receiver(&self, name_or_id: &str) -> Result<Receiver> {
        let row: Option<(i64, String, String)> = self
            .db
            .query_row(
                "SELECT id, name, COALESCE(note,'') FROM receivers WHERE name=?1 OR id=?2",
                params![self.name_key(name_or_id), id_from(name_or_id)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (i, n, t) = row.ok_or_else(|| Error(format!("unknown receiver: {name_or_id}")))?;
        self.receiver_from(i, n, t)
    }

    // ---- assets
    #[allow(clippy::too_many_arguments)]
    pub fn add_asset(
        &self, name: &str, kind: &str, orig_path: &Path, sha256: &str, params: &serde_json::Value,
        m: usize, max_receivers: u64, n_colluders: u64, eps: f64,
    ) -> Result<i64> {
        let file = orig_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let (col_name, col_path, col_sha) = if self.dek.is_some() {
            (
                self.name_key(name),
                String::new(),
                self.seal_json("asset", &serde_json::json!({"name": name, "sha256": sha256, "file": file}))?,
            )
        } else {
            (name.to_string(), orig_path.to_string_lossy().to_string(), sha256.to_string())
        };
        self.db
            .execute(
                "INSERT INTO assets(name,kind,orig_path,sha256,params,m,max_receivers,n_colluders,eps,created)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![col_name, kind, col_path, col_sha, params.to_string(), m as i64,
                        max_receivers as i64, n_colluders as i64, eps, now()],
            )
            .map_err(|_| Error(format!("cannot add asset '{name}': the name may already exist")))?;
        Ok(self.db.last_insert_rowid())
    }

    /// Remember where the private copy lives, relative to the workspace.
    pub fn set_asset_path(&self, asset_id: i64, path: &Path) -> Result<()> {
        let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.db.execute("UPDATE assets SET orig_path=?1 WHERE id=?2", params![format!("originals/{file}"), asset_id])?;
        Ok(())
    }

    /// Keep a private copy of an original in `originals/`. Encrypted when the workspace is.
    pub fn store_original(&self, asset_id: i64, src: &Path) -> Result<()> {
        let file = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        std::fs::create_dir_all(self.home.join("originals"))?;
        match &self.dek {
            None => {
                let dest = self.home.join("originals").join(format!("{asset_id}_{file}"));
                std::fs::copy(src, &dest)?;
                self.set_asset_path(asset_id, &dest)
            }
            Some(d) => {
                let dest = self.home.join("originals").join(format!("{}.vfpenc", rand_hex(12)?));
                let tmp = dest.with_extension("vfpenc.tmp");
                {
                    let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
                    let mut input = std::fs::File::open(src)?;
                    vault::encrypt_stream(&d.subkey("files"), format!("original:{asset_id}").as_bytes(), &mut input, &mut out)?;
                    std::io::Write::flush(&mut out)?;
                }
                std::fs::rename(&tmp, &dest)?;
                self.set_asset_path(asset_id, &dest)
            }
        }
    }

    /// A readable copy of the original, for marking or tracing.
    pub fn open_original(&self, a: &Asset) -> Result<PlainOriginal> {
        let Some(d) = &self.dek else {
            return Ok(PlainOriginal { path: a.orig_path.clone(), _tmp: None });
        };
        let tmp = SecureTemp::new(&self.temp_root())?;
        let ext = Path::new(&a.file_name).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        let (path, file) = tmp.create(&if ext.is_empty() { "original".to_string() } else { format!("original.{ext}") })?;
        let mut out = std::io::BufWriter::new(file);
        let mut input = std::io::BufReader::new(
            std::fs::File::open(&a.orig_path).map_err(|_| Error("the encrypted original is missing".into()))?,
        );
        vault::decrypt_stream(&d.subkey("files"), format!("original:{}", a.id).as_bytes(), &mut input, &mut out)?;
        std::io::Write::flush(&mut out)?;
        Ok(PlainOriginal { path, _tmp: Some(tmp) })
    }

    /// Find the private copy of an original. Always look in this workspace's `originals` folder
    /// by file name, so the workspace can move and the tool can run from any folder.
    pub fn original_path(&self, stored: &str) -> PathBuf {
        let name = stored.replace('\\', "/").rsplit('/').next().unwrap_or("").to_string();
        let inside = self.home.join("originals").join(name);
        let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        if inside.exists() {
            return abs(&inside);
        }
        let old = PathBuf::from(stored);
        if old.is_absolute() && old.exists() { old } else { abs(&inside) }
    }

    fn asset_from(&self, r: &rusqlite::Row) -> Result<Asset> {
        let stored: String = r.get("orig_path")?;
        let params: String = r.get("params")?;
        let (mut name, mut sha256): (String, String) = (r.get("name")?, r.get::<_, Option<String>>("sha256")?.unwrap_or_default());
        let orig_path = self.original_path(&stored);
        let mut file_name = orig_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if self.dek.is_some() {
            let v = self.open_json("asset", &sha256)?;
            name = v["name"].as_str().unwrap_or_default().into();
            sha256 = v["sha256"].as_str().unwrap_or_default().into();
            file_name = v["file"].as_str().unwrap_or_default().into();
        }
        Ok(Asset {
            id: r.get("id")?,
            name,
            kind: r.get("kind")?,
            orig_path,
            file_name,
            sha256,
            params: serde_json::from_str(&params).unwrap_or(serde_json::Value::Null),
            m: r.get::<_, i64>("m")? as usize,
            max_receivers: r.get::<_, i64>("max_receivers")? as u64,
            n_colluders: r.get::<_, i64>("n_colluders")? as u64,
            eps: r.get("eps")?,
        })
    }

    pub fn assets(&self) -> Result<Vec<Asset>> {
        let mut st = self.db.prepare("SELECT * FROM assets ORDER BY id")?;
        let mut rows = st.query([])?;
        let mut out = vec![];
        while let Some(r) = rows.next()? {
            out.push(self.asset_from(r)?);
        }
        Ok(out)
    }

    pub fn asset(&self, name_or_id: &str) -> Result<Asset> {
        let mut st = self.db.prepare("SELECT * FROM assets WHERE name=?1 OR id=?2")?;
        let mut rows = st.query(params![self.name_key(name_or_id), id_from(name_or_id)])?;
        match rows.next()? {
            Some(r) => self.asset_from(r),
            None => err(format!("unknown asset: {name_or_id}")),
        }
    }

    // ---- issued copies
    /// This receiver's code index for this asset. Created on first use.
    pub fn issue(&self, asset_id: i64, receiver_id: i64, out_path: Option<&Path>) -> Result<u64> {
        let existing: Option<i64> = self
            .db
            .query_row(
                "SELECT user_index FROM issues WHERE asset_id=?1 AND receiver_id=?2",
                params![asset_id, receiver_id],
                |r| r.get(0),
            )
            .optional()?;
        let mut out = out_path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        if self.dek.is_some() && !out.is_empty() {
            out = self.seal_json("issue", &serde_json::json!({"path": out}))?;
        }
        match existing {
            None => {
                let next: i64 = self.db.query_row(
                    "SELECT COALESCE(MAX(user_index)+1, 0) FROM issues WHERE asset_id=?1",
                    params![asset_id],
                    |r| r.get(0),
                )?;
                self.db.execute(
                    "INSERT INTO issues(asset_id,receiver_id,user_index,out_path,created) VALUES (?1,?2,?3,?4,?5)",
                    params![asset_id, receiver_id, next, out, now()],
                )?;
                Ok(next as u64)
            }
            Some(i) => {
                if !out.is_empty() {
                    self.db.execute(
                        "UPDATE issues SET out_path=?1 WHERE asset_id=?2 AND receiver_id=?3",
                        params![out, asset_id, receiver_id],
                    )?;
                }
                Ok(i as u64)
            }
        }
    }

    pub fn issues(&self, asset_id: i64) -> Result<Vec<Issue>> {
        let mut st = self.db.prepare(
            "SELECT i.user_index, COALESCE(i.out_path,''), r.id, r.name, COALESCE(r.note,'') FROM issues i
             JOIN receivers r ON r.id=i.receiver_id WHERE i.asset_id=?1 ORDER BY i.user_index",
        )?;
        let rows: Vec<(i64, String, i64, String, String)> = st
            .query_map(params![asset_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<std::result::Result<_, _>>()?;
        rows.into_iter()
            .map(|(ui, mut out, rid, name, note)| {
                if self.dek.is_some() && !out.is_empty() {
                    out = self.open_json("issue", &out)?["path"].as_str().unwrap_or_default().to_string();
                }
                Ok(Issue { user_index: ui as u64, out_path: out, name: self.receiver_from(rid, name, note)?.name })
            })
            .collect()
    }

    /// Turn a plain workspace into an encrypted one (or finish an interrupted run).
    /// Safe to run again. Plain originals and `master.key` are removed at the end.
    pub fn migrate_to_vault(home: impl AsRef<Path>, vault_file: &VaultFile, dek: &Dek) -> Result<()> {
        let home = home.as_ref();
        let db_path = home.join("vfp.db");
        if !db_path.exists() {
            return err("no workspace here");
        }
        // 1. sealed master key (the plain one stays until the end)
        if !home.join(MASTER_ENC).exists() {
            write_master_enc(home, dek, &load_or_create_key(&home.join("master.key"))?)?;
        }
        vault_file.save(&home.join(VAULT_FILE))?;
        let ws = Workspace { db: connect(&db_path, true)?, home: home.to_path_buf(), master: vec![], dek: Some(dek.clone()) };
        // 2. originals
        let mut moved: Vec<PathBuf> = vec![];
        let rows: Vec<(i64, String, String, String)> = {
            let mut st = ws.db.prepare("SELECT id, name, COALESCE(orig_path,''), COALESCE(sha256,'') FROM assets")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        for (id, name, stored, sha) in rows {
            if sha.starts_with(ENC) {
                continue; // done in an earlier run
            }
            let plain = ws.original_path(&stored);
            let file = plain.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let dest = home.join("originals").join(format!("{}.vfpenc", rand_hex(12)?));
            {
                let mut out = std::io::BufWriter::new(std::fs::File::create(&dest)?);
                let mut input = std::fs::File::open(&plain).map_err(|e| Error(format!("cannot read original of asset {id}: {e}")))?;
                vault::encrypt_stream(&dek.subkey("files"), format!("original:{id}").as_bytes(), &mut input, &mut out)?;
                std::io::Write::flush(&mut out)?;
            }
            let sealed = ws.seal_json("asset", &serde_json::json!({"name": name, "sha256": sha, "file": file}))?;
            ws.db.execute(
                "UPDATE assets SET name=?1, sha256=?2, orig_path=?3 WHERE id=?4",
                params![ws.name_key(&name), sealed, format!("originals/{}", dest.file_name().unwrap().to_string_lossy()), id],
            )?;
            moved.push(plain);
        }
        // 3. receivers and issue paths
        let recs: Vec<(i64, String, String)> = {
            let mut st = ws.db.prepare("SELECT id, name, COALESCE(note,'') FROM receivers")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        for (id, name, note) in recs {
            if note.starts_with(ENC) {
                continue;
            }
            let sealed = ws.seal_json("receiver", &serde_json::json!({"name": name, "note": note}))?;
            ws.db.execute("UPDATE receivers SET name=?1, note=?2 WHERE id=?3", params![ws.name_key(&name), sealed, id])?;
        }
        let iss: Vec<(i64, String)> = {
            let mut st = ws.db.prepare("SELECT id, COALESCE(out_path,'') FROM issues")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<Vec<_>, _>>()?;
            v
        };
        for (id, out) in iss {
            if out.is_empty() || out.starts_with(ENC) {
                continue;
            }
            let sealed = ws.seal_json("issue", &serde_json::json!({"path": out}))?;
            ws.db.execute("UPDATE issues SET out_path=?1 WHERE id=?2", params![sealed, id])?;
        }
        // 4. remove clear text: old originals, old key, and old text inside the database file
        for p in moved {
            if p.starts_with(home.join("originals")) {
                wipe_file(&p);
            }
        }
        ws.db.execute_batch("VACUUM;")?;
        wipe_file(&home.join("master.key"));
        Ok(())
    }
}

/// Overwrite a file with zeros, then delete it. (Best effort: not reliable on SSDs.)
pub fn wipe_file(p: &Path) {
    if let Ok(len) = p.metadata().map(|m| m.len()) {
        if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(p) {
            use std::io::Write;
            let zeros = vec![0u8; 1 << 16];
            let mut left = len;
            while left > 0 {
                let n = left.min(zeros.len() as u64) as usize;
                if f.write_all(&zeros[..n]).is_err() {
                    break;
                }
                left -= n as u64;
            }
            let _ = f.sync_all();
        }
    }
    let _ = std::fs::remove_file(p);
}
