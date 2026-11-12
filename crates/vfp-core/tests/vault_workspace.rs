//! Encrypted workspace: same results as plain, nothing readable on disk, migration, wrong key.

use std::path::{Path, PathBuf};
use vfp_core::image::Bgr;
use vfp_core::imgio::write_image;
use vfp_core::pipeline::{add_asset, mark_for_receiver, trace_leak, PlanOptions};
use vfp_core::rng::Pcg64;
use vfp_core::vault::{KdfParams, Protection, VaultFile};
use vfp_core::workspace::Workspace;

const PASS: &str = "gnarly pelican tuxedo mulch yoyo cabbage lunar";

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vfp-vtest-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn picture(h: usize, w: usize) -> Bgr {
    let mut rng = Pcg64::from_seed(&[77]);
    let data: Vec<u8> = (0..h * w * 3).map(|_| (40.0 + 170.0 * rng.next_f64()) as u8).collect();
    Bgr::new(h, w, data)
}

fn opts() -> PlanOptions {
    PlanOptions { max_receivers: 10, ..Default::default() }
}

fn all_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { all_files(&p, out) } else { out.push(p) }
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn encrypted_flow_leaves_no_plaintext() {
    let root = temp("flow");
    let (vf, dek) = VaultFile::create(PASS, Protection::Protected, KdfParams::TEST).unwrap();
    let home = root.join("ws");
    let ws = Workspace::init_encrypted(&home, &vf, &dek).unwrap();
    assert!(ws.is_encrypted());
    ws.add_receiver("Zelda Fitzgerald", "secret-note-xyz").unwrap();
    ws.add_receiver("bob", "").unwrap();
    let src = root.join("TopSecretPlan.png");
    write_image(&src, &picture(320, 480)).unwrap();
    let (asset, _) = add_asset(&ws, &src, Some("Launch Plan"), &opts()).unwrap();
    assert_eq!(asset.name, "Launch Plan");
    assert_eq!(asset.file_name, "TopSecretPlan.png");
    let out = mark_for_receiver(&ws, &asset, "Zelda Fitzgerald", &root.join("out"), None).unwrap();
    mark_for_receiver(&ws, &asset, "bob", &root.join("out"), None).unwrap();
    let r = trace_leak(&ws, &out, Some("Launch Plan")).unwrap();
    assert_eq!(r.flagged, vec!["Zelda Fitzgerald"], "{r:?}");
    assert_eq!(ws.receivers().unwrap()[0].note, "secret-note-xyz");
    assert_eq!(ws.issues(asset.id).unwrap()[0].name, "Zelda Fitzgerald");

    // temp folder holds nothing
    let tmp = ws.temp_root();
    if tmp.exists() {
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
    }
    drop(ws);

    // nothing readable on disk
    let plain = std::fs::read(&src).unwrap();
    let mut files = vec![];
    all_files(&home, &mut files);
    assert!(!files.iter().any(|f| f.file_name().unwrap() == "master.key"));
    for f in &files {
        let b = std::fs::read(f).unwrap();
        for s in ["Zelda", "secret-note-xyz", "Launch Plan", "TopSecretPlan"] {
            assert!(!contains(&b, s.as_bytes()), "{s} found in {}", f.display());
        }
        assert!(!contains(&b, &plain[plain.len() / 2..plain.len() / 2 + 64]), "plain bytes in {}", f.display());
    }

    // locked, wrong key, right key
    assert!(Workspace::open(&home).err().unwrap().0.contains("locked"));
    let (_, other) = VaultFile::create("another sturdy pelican garden mulch xylophone", Protection::Protected, KdfParams::TEST).unwrap();
    assert!(Workspace::open_with(&home, Some(&other)).is_err());
    let ws = Workspace::open_with(&home, Some(&dek)).unwrap();
    assert_eq!(ws.asset("Launch Plan").unwrap().id, 1);
}

#[test]
fn migration_is_complete_and_repeatable() {
    let root = temp("mig");
    let home = root.join("ws");
    let ws = Workspace::init(&home).unwrap();
    ws.add_receiver("alice", "n1").unwrap();
    ws.add_receiver("bob", "").unwrap();
    let src = root.join("pic.png");
    write_image(&src, &picture(320, 480)).unwrap();
    let (asset, _) = add_asset(&ws, &src, Some("pic"), &opts()).unwrap();
    let out = mark_for_receiver(&ws, &asset, "alice", &root.join("out"), None).unwrap();
    mark_for_receiver(&ws, &asset, "bob", &root.join("out"), None).unwrap();
    drop(ws);

    let (vf, dek) = VaultFile::create(PASS, Protection::Protected, KdfParams::TEST).unwrap();
    Workspace::migrate_to_vault(&home, &vf, &dek).unwrap();
    Workspace::migrate_to_vault(&home, &vf, &dek).unwrap(); // again: no harm
    assert!(!home.join("master.key").exists());
    let mut files = vec![];
    all_files(&home.join("originals"), &mut files);
    assert!(files.iter().all(|f| f.extension().unwrap() == "vfpenc"), "{files:?}");

    let ws = Workspace::open_with(&home, Some(&dek)).unwrap();
    let r = trace_leak(&ws, &out, Some("pic")).unwrap();
    assert_eq!(r.flagged, vec!["alice"], "{r:?}");
    let db = std::fs::read(home.join("vfp.db")).unwrap();
    assert!(!contains(&db, b"alice") && !contains(&db, b"n1\0"));
}
