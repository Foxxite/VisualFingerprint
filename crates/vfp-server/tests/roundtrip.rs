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
    let port = free_port();
    let mut child = Command::new(BIN)
        .args(["--data", data.to_str().unwrap(), "serve", "--bind", &format!("127.0.0.1:{port}"), "--max-upload-mb", &max_mb.to_string()])
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
