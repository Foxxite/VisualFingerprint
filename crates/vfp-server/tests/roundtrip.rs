//! Start a real server, then use it through `RemoteBackend` the way a second device would.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use vfp_core::backend::{Backend, RemoteBackend};
use vfp_core::pipeline::PlanOptions;

const BIN: &str = env!("CARGO_BIN_EXE_vfp-server");

struct Server {
    child: Child,
    url: String,
    admin: String,
    data: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start(data: &Path, max_mb: u64) -> Server {
    start_with(data, max_mb, &["--plain"])
}

fn start_with(data: &Path, max_mb: u64, extra: &[&str]) -> Server {
    let port = free_port();
    let mut child = Command::new(BIN)
        .args(["--data", data.to_str().unwrap(), "serve", "--bind", &format!("127.0.0.1:{port}"), "--max-upload-mb", &max_mb.to_string()])
        .args(extra)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut admin = String::new();
    for l in lines.by_ref() {
        let l = l.unwrap();
        if l.trim().starts_with("vfp_") {
            admin = l.trim().to_string();
        }
        if l.contains("listening") {
            break;
        }
    }
    assert!(!admin.is_empty(), "no admin token printed");
    std::thread::spawn(move || for _ in lines {}); // keep the pipe drained
    Server { child, url: format!("http://127.0.0.1:{port}"), admin, data: data.to_path_buf() }
}

fn token_cmd(data: &Path, args: &[&str]) -> String {
    let out = Command::new(BIN).args(["--data", data.to_str().unwrap(), "token"]).args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn make_photo(path: &Path) {
    let (w, h) = (640u32, 480u32);
    let mut s = 12345u64;
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let n = ((s >> 33) % 24) as f32;
        let base = 100.0 + 60.0 * ((x as f32 / 37.0).sin() * (y as f32 / 23.0).cos());
        let v = (base + n + if (x / 80 + y / 60) % 2 == 0 { 30.0 } else { 0.0 }).clamp(0.0, 255.0) as u8;
        image::Rgb([v, (v as f32 * 0.9) as u8, (v as f32 * 0.8) as u8])
    });
    img.save(path).unwrap();
}

#[test]
fn second_device_traces_without_original() {
    let tmp = std::env::temp_dir().join(format!("vfp_srv_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (data, dev_a, dev_b) = (tmp.join("data"), tmp.join("device_a"), tmp.join("device_b"));
    for d in [&dev_a, &dev_b] {
        std::fs::create_dir_all(d).unwrap();
    }
    let srv = start(&data, 8);

    // device A: admin. Registers an original and makes copies.
    let a = RemoteBackend::new(&srv.url, &srv.admin);
    assert_eq!(a.info().unwrap()["role"], "admin");
    for n in ["alice", "bob", "carol"] {
        a.add_receiver(n, "").unwrap();
    }
    let photo = dev_a.join("photo.png");
    make_photo(&photo);
    let (asset, plan) = a.add_asset(&photo, Some("photo"), &PlanOptions { max_receivers: 10, ..Default::default() }).unwrap();
    assert_eq!(asset.kind, "image");
    assert!(plan.m > 0);
    let copies = a.mark("photo", &["alice".into(), "bob".into(), "carol".into()], &dev_a.join("out"), None).unwrap();
    assert_eq!(copies.len(), 3);
    let bob = &copies.iter().find(|c| c.0 == "bob").unwrap().1;
    assert!(bob.exists());

    // device B: has only a trace token and a copy of the leak. No original, no key.
    let trace_tok = token_cmd(&data, &["new", "--role", "trace", "--label", "phone"]).lines().find(|l| l.trim().starts_with("vfp_")).unwrap().trim().to_string();
    let leak = dev_b.join("leaked.png");
    std::fs::copy(bob, &leak).unwrap();
    let b = RemoteBackend::new(&srv.url, &trace_tok);
    let r = b.trace(&leak, None).unwrap();
    assert_eq!(r.asset, "photo");
    assert_eq!(r.ranking[0].0, "bob", "{:?}", r.ranking);
    assert!(r.ranking[0].1 > r.threshold, "{:?} threshold {}", r.ranking, r.threshold);
    assert_eq!(std::fs::read_dir(&dev_b).unwrap().count(), 1, "device B must hold only the leak");

    // the trace-only token may not change anything or download copies
    assert!(b.add_receiver("mallory", "").unwrap_err().0.contains("not allowed"));
    assert!(b.mark("photo", &["alice".into()], &dev_b.join("x"), None).is_err());

    // wrong token
    assert!(RemoteBackend::new(&srv.url, "vfp_nope").receivers().unwrap_err().0.contains("token"));

    // upload over the limit is refused
    let big = dev_b.join("big.png");
    std::fs::write(&big, vec![7u8; 9 << 20]).unwrap();
    assert!(b.trace(&big, None).is_err());

    // a revoked token stops working at once
    let list = token_cmd(&data, &["list"]);
    let id = list.lines().find(|l| l.contains("phone")).unwrap().split_whitespace().next().unwrap().to_string();
    token_cmd(&data, &["revoke", &id]);
    assert!(b.receivers().is_err());

    // the audit log names who did what, and temporary uploads are gone
    let log = std::fs::read_to_string(srv.data.join("audit.log")).unwrap();
    assert!(log.contains("phone") && log.contains("trace"), "{log}");
    assert_eq!(std::fs::read_dir(srv.data.join("uploads")).unwrap().count(), 0);
    drop(srv);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------- encrypted (Protected) server
const PASS: &str = "gnarly pelican tuxedo mulch yoyo cabbage lunar";

fn all_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { all_files(&p, out) } else { out.push(p) }
    }
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// One raw HTTP request. Returns the status code.
fn raw_status(port: &str, token: &str, extra_headers: &str) -> u16 {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(port).unwrap();
    write!(s, "GET /v1/receivers HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n{extra_headers}Connection: close\r\n\r\n").unwrap();
    let mut buf = String::new();
    s.read_to_string(&mut buf).unwrap();
    buf.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[test]
fn protected_server_keeps_nothing_readable() {
    use vfp_core::vault::{KdfParams, Protection, VaultFile};
    let tmp = std::env::temp_dir().join(format!("vfp_enc_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (data, dev) = (tmp.join("data"), tmp.join("dev"));
    std::fs::create_dir_all(&dev).unwrap();
    let srv = start_with(&data, 8, &[]);

    // fresh server: no vault yet
    let anon = RemoteBackend::new(&srv.url, &srv.admin);
    assert_eq!(anon.info().unwrap()["protection"], "uninitialized");
    assert!(anon.receivers().unwrap_err().0.contains("no vault"));
    assert!(anon.vault().unwrap().is_none());

    // the client makes the key and creates the vault
    let (vf, dek) = VaultFile::create(PASS, Protection::Protected, KdfParams::TEST).unwrap();
    let a = RemoteBackend::new(&srv.url, &srv.admin).with_key(Some(dek.clone())).unwrap();
    a.create_vault(&vf).unwrap();
    assert!(a.create_vault(&vf).is_err(), "a second vault must be refused");
    assert_eq!(a.info().unwrap()["protection"], "protected");

    a.add_receiver("Zelda Fitzgerald", "note-for-zelda").unwrap();
    a.add_receiver("bob", "").unwrap();
    let photo = dev.join("TopSecretPhoto.png");
    make_photo(&photo);
    a.add_asset(&photo, Some("Secret Plan"), &PlanOptions { max_receivers: 10, ..Default::default() }).unwrap();
    let copies = a.mark("Secret Plan", &["Zelda Fitzgerald".into(), "bob".into()], &dev.join("out"), None).unwrap();
    assert_eq!(copies.len(), 2);
    let zelda = &copies[0].1;
    assert!(zelda.exists() && std::fs::metadata(zelda).unwrap().len() > 1000);

    // no key, wrong key
    let nokey = RemoteBackend::new(&srv.url, &srv.admin);
    assert!(nokey.receivers().unwrap_err().0.contains("locked"));
    let (_, other) = VaultFile::create("another sturdy pelican garden mulch xylophone", Protection::Protected, KdfParams::TEST).unwrap();
    let wrong = RemoteBackend::new(&srv.url, &srv.admin).with_key(Some(other)).unwrap();
    assert!(wrong.receivers().unwrap_err().0.contains("key"));

    // a second device: trace token, gets vault.json, unlocks with the passphrase, traces
    let tok = token_cmd(&data, &["new", "--role", "trace", "--label", "phone"]).lines().find(|l| l.trim().starts_with("vfp_")).unwrap().trim().to_string();
    let b0 = RemoteBackend::new(&srv.url, &tok);
    let dek_b = b0.vault().unwrap().unwrap().unlock(PASS).unwrap();
    assert!(b0.vault().unwrap().unwrap().unlock("not the passphrase at all").is_err());
    let b = RemoteBackend::new(&srv.url, &tok).with_key(Some(dek_b)).unwrap();
    let r = b.trace(zelda, None).unwrap();
    assert_eq!(r.ranking[0].0, "Zelda Fitzgerald", "{:?}", r.ranking);
    assert!(r.ranking[0].1 > r.threshold);

    // the key is never sent over plain HTTP to another machine
    assert!(RemoteBackend::new("http://example.com", "t").with_key(Some(dek.clone())).is_err());

    // nothing readable in the data folder, audit log has no names, temp folders are empty
    let photo_bytes = std::fs::read(&photo).unwrap();
    let mid = &photo_bytes[photo_bytes.len() / 2..photo_bytes.len() / 2 + 64];
    let mut files = vec![];
    all_files(&data, &mut files);
    assert!(!files.iter().any(|f| f.file_name().unwrap() == "master.key"));
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        for s in ["Zelda", "note-for-zelda", "Secret Plan", "TopSecretPhoto"] {
            assert!(!has(&bytes, s.as_bytes()), "{s} found in {}", f.display());
        }
        assert!(!has(&bytes, mid), "original bytes found in {}", f.display());
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        assert!(!name.contains("Zelda") && !name.contains("zelda") && !name.contains("Secret"), "{name}");
    }
    for d in ["uploads", "tmp"] {
        assert_eq!(std::fs::read_dir(srv.data.join(d)).unwrap().count(), 0, "{d} must be empty");
    }
    drop(srv);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn key_header_needs_https_behind_a_proxy() {
    use vfp_core::vault::{KdfParams, Protection, VaultFile};
    let tmp = std::env::temp_dir().join(format!("vfp_proxy_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let srv = start_with(&tmp.join("data"), 8, &["--trust-proxy"]);
    let host = srv.url.trim_start_matches("http://").to_string();
    let (_, dek) = VaultFile::create(PASS, Protection::Protected, KdfParams::TEST).unwrap();
    let key = format!("X-VFP-Key: {}\r\n", dek.to_hex());
    // behind a trusted proxy, a key without "X-Forwarded-Proto: https" is refused
    assert_eq!(raw_status(&host, &srv.admin, &key), 400);
    // with the header the request is accepted (and the fresh server answers "no vault yet": 503)
    assert_eq!(raw_status(&host, &srv.admin, &format!("{key}X-Forwarded-Proto: https\r\n")), 503);
    // no key header: nothing to protect
    assert_eq!(raw_status(&host, &srv.admin, ""), 503);
    drop(srv);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------- zero-knowledge server
/// One raw HTTP request with a body. Returns (status, body text).
fn raw(addr: &str, method: &str, path: &str, token: &str, headers: &str, body: &[u8]) -> (u16, String) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    write!(s, "{method} {path} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {token}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
    s.write_all(body).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).to_string();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, text)
}

#[test]
fn zero_knowledge_server_never_sees_key_or_files() {
    use vfp_core::backend::SealedBackend;
    use vfp_core::vault::{KdfParams, Protection, VaultFile};
    let tmp = std::env::temp_dir().join(format!("vfp_zk_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let (data, dev) = (tmp.join("data"), tmp.join("dev"));
    std::fs::create_dir_all(&dev).unwrap();
    let srv = start_with(&data, 8, &[]);
    let addr = srv.url.trim_start_matches("http://").to_string();

    // device A creates a zero-knowledge vault. The key is made here and never sent.
    let (vf, dek) = VaultFile::create(PASS, Protection::ZeroKnowledge, KdfParams::TEST).unwrap();
    let a = SealedBackend::create(&srv.url, &srv.admin, &vf, dek.clone(), tmp.join("cache_a")).unwrap();
    let plain = RemoteBackend::new(&srv.url, &srv.admin);
    assert_eq!(plain.info().unwrap()["protection"], "zero-knowledge");
    assert!(SealedBackend::create(&srv.url, &srv.admin, &vf, dek.clone(), tmp.join("cache_x")).is_err(), "second vault refused");

    a.add_receiver("Zelda Fitzgerald", "note-for-zelda").unwrap();
    a.add_receiver("bob", "").unwrap();
    let photo = dev.join("TopSecretPhoto.png");
    make_photo(&photo);
    a.add_asset(&photo, Some("Secret Plan"), &PlanOptions { max_receivers: 10, ..Default::default() }).unwrap();
    let copies = a.mark("Secret Plan", &["Zelda Fitzgerald".into(), "bob".into()], &dev.join("out"), None).unwrap();
    let zelda = copies[0].1.clone();
    let ra = a.trace(&zelda, None).unwrap();
    assert_eq!(ra.ranking[0].0, "Zelda Fitzgerald", "{:?}", ra.ranking);

    // device B: a trace token and the passphrase. A fresh cache. Same verdict.
    let tok = token_cmd(&data, &["new", "--role", "trace", "--label", "phone"]).lines().find(|l| l.trim().starts_with("vfp_")).unwrap().trim().to_string();
    let v = RemoteBackend::new(&srv.url, &tok).vault().unwrap().unwrap();
    assert!(v.unlock("not the passphrase at all").is_err());
    let dek_b = v.unlock(PASS).unwrap();
    let b = SealedBackend::open(&srv.url, &tok, dek_b, tmp.join("cache_b"));
    let rb = b.trace(&zelda, None).unwrap();
    assert_eq!(rb.ranking[0].0, "Zelda Fitzgerald");
    assert_eq!(ra.ranking, rb.ranking, "both devices must give the same verdict");
    assert_eq!(b.receivers().unwrap().len(), 2);
    // a trace-only token cannot change anything
    assert!(b.add_receiver("mallory", "").is_err());

    // a device with the wrong key cannot read the database
    let (_, other) = VaultFile::create("another sturdy pelican garden mulch xylophone", Protection::ZeroKnowledge, KdfParams::TEST).unwrap();
    let wrong = SealedBackend::open(&srv.url, &srv.admin, other, tmp.join("cache_w"));
    assert!(wrong.receivers().unwrap_err().0.contains("wrong key"));

    // device C (admin) sees what A wrote, and its own change reaches A
    let c = SealedBackend::open(&srv.url, &srv.admin, dek.clone(), tmp.join("cache_c"));
    c.add_receiver("dave", "").unwrap();
    assert_eq!(a.receivers().unwrap().len(), 3);

    // the usual routes do not work: the server cannot do the work
    assert!(plain.receivers().unwrap_err().0.contains("zero-knowledge"));

    // the key header is refused outright on blob routes
    let key = format!("X-VFP-Key: {}\r\n", dek.to_hex());
    assert_eq!(raw(&addr, "GET", "/v1/blobs", &srv.admin, &key, b"").0, 400);

    // blob rules: create-only, If-Match, and a precondition is required
    assert_eq!(raw(&addr, "PUT", "/v1/blobs/t-1", &srv.admin, "If-None-Match: *\r\n", b"one").0, 200);
    assert_eq!(raw(&addr, "PUT", "/v1/blobs/t-1", &srv.admin, "If-None-Match: *\r\n", b"two").0, 412);
    assert_eq!(raw(&addr, "PUT", "/v1/blobs/t-1", &srv.admin, "If-Match: stale\r\n", b"two").0, 412);
    assert_eq!(raw(&addr, "PUT", "/v1/blobs/t-1", &srv.admin, "", b"two").0, 428);
    assert_eq!(raw(&addr, "PUT", "/v1/blobs/..%2Fvault.json", &srv.admin, "If-None-Match: *\r\n", b"x").0, 400);
    assert_eq!(raw(&addr, "PUT", "/v1/blobs/t-2", &tok, "If-None-Match: *\r\n", b"x").0, 403);
    let list = raw(&addr, "GET", "/v1/blobs", &srv.admin, "", b"").1;
    let etag = list.split("\"id\":\"t-1\"").nth(1).and_then(|r| r.split("\"etag\":\"").nth(1)).and_then(|r| r.split('"').next()).unwrap().to_string();
    assert_eq!(raw(&addr, "PUT", "/v1/blobs/t-1", &srv.admin, &format!("If-Match: {etag}\r\n"), b"three").0, 200);
    assert!(raw(&addr, "GET", "/v1/blobs/t-1", &srv.admin, "", b"").1.contains("three"), "streamed body is chunked, so only check it is present");

    // nothing readable in the data folder: no names, no original bytes, no key, no passphrase
    let photo_bytes = std::fs::read(&photo).unwrap();
    let mid = &photo_bytes[photo_bytes.len() / 2..photo_bytes.len() / 2 + 64];
    let mut files = vec![];
    all_files(&data, &mut files);
    assert!(!files.iter().any(|f| f.file_name().unwrap() == "master.key"));
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        for s in ["Zelda", "note-for-zelda", "Secret Plan", "TopSecretPhoto", "bob", "dave", &dek.to_hex(), PASS] {
            assert!(!has(&bytes, s.as_bytes()), "{s} found in {}", f.display());
        }
        assert!(!has(&bytes, mid), "original bytes found in {}", f.display());
    }
    // nothing on the server's temp folders either
    for d in ["uploads", "tmp"] {
        if let Ok(rd) = std::fs::read_dir(srv.data.join(d)) {
            assert_eq!(rd.count(), 0, "{d} must be empty");
        }
    }
    drop(srv);
    let _ = std::fs::remove_dir_all(&tmp);
}
