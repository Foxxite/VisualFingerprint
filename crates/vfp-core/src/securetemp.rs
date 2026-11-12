//! Short-lived private folder for a decrypted file.
//!
//! The folder is private to the user (mode 0700, files 0600 on Unix). It is wiped and removed when the
//! value is dropped. After a crash, [`sweep`] removes what is left. On an SSD an overwrite is not
//! guaranteed to destroy the data, so use full-disk encryption on the machine too.

use crate::error::Result;
use std::fs::File;
use std::path::{Path, PathBuf};

pub struct SecureTemp(PathBuf);

fn rand_name() -> Result<String> {
    let mut b = [0u8; 8];
    getrandom::getrandom(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

impl SecureTemp {
    /// Make a new private folder inside `root`.
    pub fn new(root: &Path) -> Result<Self> {
        let dir = root.join(rand_name()?);
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(SecureTemp(dir))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Create a private file inside the folder.
    pub fn create(&self, name: &str) -> Result<(PathBuf, File)> {
        let p = self.0.join(name);
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let f = o.open(&p)?;
        Ok((p, f))
    }
}

fn wipe_dir(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() {
                if let Ok(len) = p.metadata().map(|m| m.len()) {
                    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(&p) {
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
            }
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

impl Drop for SecureTemp {
    fn drop(&mut self) {
        wipe_dir(&self.0);
    }
}

/// Remove every temp folder under `root` (call at start-up, when no request is running).
pub fn sweep(root: &Path) {
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                wipe_dir(&e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn folder_is_removed_on_drop_and_swept() {
        let root = std::env::temp_dir().join(format!("vfp_st_{}", std::process::id()));
        let p;
        {
            let t = SecureTemp::new(&root).unwrap();
            let (fp, mut f) = t.create("a.bin").unwrap();
            f.write_all(b"secret").unwrap();
            drop(f);
            assert!(fp.exists());
            p = t.path().to_path_buf();
        }
        assert!(!p.exists());
        let t2 = SecureTemp::new(&root).unwrap();
        std::mem::forget(t2); // simulate a crash
        sweep(&root);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }
}
