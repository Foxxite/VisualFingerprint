//! Workspace: the SQLite database and the master key.
//!
//! Same layout and schema as the Python version, so either program can open the other's workspace:
//!
//! ```text
//! master.key   secret. Without it nobody can trace a leak. Back it up. Never share it.
//! vfp.db       receivers, assets, issued copies
//! originals/   private copies of the unmarked files (tracing needs them)
//! ```

use crate::error::{err, Error, Result};
use crate::keys::derive_seed;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

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

impl Workspace {
    pub fn open(home: impl AsRef<Path>) -> Result<Self> {
        let home = home.as_ref().to_path_buf();
        let db_path = home.join("vfp.db");
        if !db_path.exists() {
            return err(format!("No workspace at {}. Run: vfp init", home.display()));
        }
        let master = load_or_create_key(&home.join("master.key"))?;
        Ok(Workspace { db: Connection::open(db_path)?, home, master })
    }

    /// Make the folder, the key and the database (safe to call on an existing workspace).
    pub fn init(home: impl AsRef<Path>) -> Result<Self> {
        let home = home.as_ref();
        std::fs::create_dir_all(home.join("originals"))?;
        load_or_create_key(&home.join("master.key"))?;
        Connection::open(home.join("vfp.db"))?.execute_batch(SCHEMA)?;
        Self::open(home)
    }

    // ---- seeds: all derived from the master key; nothing secret lives in the database
    pub fn tardos_seed(&self, asset_id: i64) -> u64 {
        derive_seed(&self.master, &["asset", &asset_id.to_string(), "tardos"])
    }

    pub fn layout_seed(&self, asset_id: i64) -> u64 {
        derive_seed(&self.master, &["asset", &asset_id.to_string(), "layout"])
    }

    // ---- receivers
    pub fn add_receiver(&self, name: &str, note: &str) -> Result<i64> {
        self.db
            .execute("INSERT INTO receivers(name, note, created) VALUES (?1,?2,?3)", params![name, note, now()])
            .map_err(|e| Error(format!("cannot add receiver '{name}': {e}")))?;
        Ok(self.db.last_insert_rowid())
    }

    pub fn receivers(&self) -> Result<Vec<Receiver>> {
        let mut st = self.db.prepare("SELECT id, name, COALESCE(note,'') FROM receivers ORDER BY id")?;
        let rows = st.query_map([], |r| Ok(Receiver { id: r.get(0)?, name: r.get(1)?, note: r.get(2)? }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn receiver(&self, name_or_id: &str) -> Result<Receiver> {
        self.db
            .query_row(
                "SELECT id, name, COALESCE(note,'') FROM receivers WHERE name=?1 OR id=?2",
                params![name_or_id, id_from(name_or_id)],
                |r| Ok(Receiver { id: r.get(0)?, name: r.get(1)?, note: r.get(2)? }),
            )
            .optional()?
            .ok_or_else(|| Error(format!("unknown receiver: {name_or_id}")))
    }

    // ---- assets
    #[allow(clippy::too_many_arguments)]
    pub fn add_asset(
        &self, name: &str, kind: &str, orig_path: &Path, sha256: &str, params: &serde_json::Value,
        m: usize, max_receivers: u64, n_colluders: u64, eps: f64,
    ) -> Result<i64> {
        self.db
            .execute(
                "INSERT INTO assets(name,kind,orig_path,sha256,params,m,max_receivers,n_colluders,eps,created)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![name, kind, orig_path.to_string_lossy(), sha256, params.to_string(), m as i64,
                        max_receivers as i64, n_colluders as i64, eps, now()],
            )
            .map_err(|e| Error(format!("cannot add asset '{name}': {e}")))?;
        Ok(self.db.last_insert_rowid())
    }

    /// Remember where the private copy lives, relative to the workspace.
    pub fn set_asset_path(&self, asset_id: i64, path: &Path) -> Result<()> {
        let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.db.execute("UPDATE assets SET orig_path=?1 WHERE id=?2", params![format!("originals/{file}"), asset_id])?;
        Ok(())
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

    fn asset_from(&self, r: &rusqlite::Row) -> rusqlite::Result<Asset> {
        let stored: String = r.get("orig_path")?;
        let params: String = r.get("params")?;
        Ok(Asset {
            id: r.get("id")?,
            name: r.get("name")?,
            kind: r.get("kind")?,
            orig_path: self.original_path(&stored),
            sha256: r.get::<_, Option<String>>("sha256")?.unwrap_or_default(),
            params: serde_json::from_str(&params).unwrap_or(serde_json::Value::Null),
            m: r.get::<_, i64>("m")? as usize,
            max_receivers: r.get::<_, i64>("max_receivers")? as u64,
            n_colluders: r.get::<_, i64>("n_colluders")? as u64,
            eps: r.get("eps")?,
        })
    }

    pub fn assets(&self) -> Result<Vec<Asset>> {
        let mut st = self.db.prepare("SELECT * FROM assets ORDER BY id")?;
        let rows = st.query_map([], |r| self.asset_from(r))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn asset(&self, name_or_id: &str) -> Result<Asset> {
        self.db
            .query_row("SELECT * FROM assets WHERE name=?1 OR id=?2", params![name_or_id, id_from(name_or_id)], |r| {
                self.asset_from(r)
            })
            .optional()?
            .ok_or_else(|| Error(format!("unknown asset: {name_or_id}")))
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
        let out = out_path.map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
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
            "SELECT i.user_index, COALESCE(i.out_path,''), r.name FROM issues i
             JOIN receivers r ON r.id=i.receiver_id WHERE i.asset_id=?1 ORDER BY i.user_index",
        )?;
        let rows = st.query_map(params![asset_id], |r| {
            Ok(Issue { user_index: r.get::<_, i64>(0)? as u64, out_path: r.get(1)?, name: r.get(2)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}
