//! Workspace and pipeline: register, mark, trace, on a synthetic image in a temp folder.

use std::path::{Path, PathBuf};
use vfp_core::image::Bgr;
use vfp_core::imgio::{read_image, write_image};
use vfp_core::pipeline::{add_asset, kind_of, mark_for_receiver, report_text, safe_name, trace_leak, PlanOptions};
use vfp_core::rng::Pcg64;
use vfp_core::workspace::Workspace;

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vfp-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Textured test picture: smooth noise plus random rectangles.
fn picture(h: usize, w: usize) -> Bgr {
    let mut rng = Pcg64::from_seed(&[4242]);
    let mut data = vec![0u8; h * w * 3];
    let cell = 16;
    let lat: Vec<f64> = (0..((h / cell) + 2) * ((w / cell) + 2) * 3).map(|_| rng.next_f64()).collect();
    let lw = w / cell + 2;
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let (cx, cy) = (x / cell, y / cell);
                let v = lat[(cy * lw + cx) * 3 + c];
                data[3 * (y * w + x) + c] = (40.0 + 170.0 * v + (rng.next_f64() - 0.5) * 24.0) as u8;
            }
        }
    }
    Bgr::new(h, w, data)
}

fn setup(name: &str) -> (PathBuf, Workspace, PathBuf) {
    let root = temp(name);
    let ws = Workspace::init(root.join("ws")).unwrap();
    for n in ["alice", "bob", "carol"] {
        ws.add_receiver(n, "").unwrap();
    }
    let src = root.join("pic.png");
    write_image(&src, &picture(480, 640)).unwrap();
    (root, ws, src)
}

fn opts() -> PlanOptions {
    PlanOptions { max_receivers: 20, ..Default::default() }
}

#[test]
fn register_mark_trace() {
    let (root, ws, src) = setup("flow");
    let (asset, plan) = add_asset(&ws, &src, Some("pic"), &opts()).unwrap();
    assert_eq!((asset.kind.as_str(), asset.id), ("image", 1));
    assert_eq!(plan.m, asset.m);

    // the private copy must survive deleting the user's file
    std::fs::remove_file(&src).unwrap();
    assert!(asset.orig_path.exists());

    let out = root.join("out");
    let mut files = vec![];
    for n in ["alice", "bob", "carol"] {
        files.push(mark_for_receiver(&ws, &asset, n, &out).unwrap());
    }
    assert!(files[2].ends_with("pic__carol.png"));
    // asking again gives the same code index (idempotent)
    assert_eq!(ws.issue(asset.id, ws.receiver("bob").unwrap().id, None).unwrap(), 1);
    assert_eq!(ws.issues(asset.id).unwrap().len(), 3);

    // plain copy
    let r = trace_leak(&ws, &files[1], None).unwrap();
    assert_eq!(r.flagged, vec!["bob"], "{r:?}");

    // re-compressed copy (JPEG q60)
    let jpg = root.join("leak.jpg");
    {
        let img = read_image(&files[2]).unwrap();
        let mut rgb = image::RgbImage::new(img.w as u32, img.h as u32);
        for (i, px) in rgb.pixels_mut().enumerate() {
            *px = image::Rgb([img.data[3 * i + 2], img.data[3 * i + 1], img.data[3 * i]]);
        }
        let f = std::fs::File::create(&jpg).unwrap();
        image::codecs::jpeg::JpegEncoder::new_with_quality(f, 60).encode_image(&rgb).unwrap();
    }
    let r = trace_leak(&ws, &jpg, Some("pic")).unwrap();
    assert_eq!(r.flagged, vec!["carol"], "{r:?}");
    assert!(r.ranking[0].1 > r.threshold);
    assert!(report_text(&r, &jpg).unwrap().contains("RESULT: LEAKER: carol"));
}

#[test]
fn unmarked_original_accuses_nobody() {
    let (root, ws, src) = setup("clean");
    let (asset, _) = add_asset(&ws, &src, None, &opts()).unwrap();
    mark_for_receiver(&ws, &asset, "alice", &root.join("out")).unwrap();
    mark_for_receiver(&ws, &asset, "bob", &root.join("out")).unwrap();
    let r = trace_leak(&ws, &src, None).unwrap();
    assert!(r.flagged.is_empty(), "the original must not accuse anyone: {r:?}");
    assert!(r.verdict.starts_with("INCONCLUSIVE"));
}

#[test]
fn workspace_persists_and_looks_up() {
    let (root, ws, src) = setup("persist");
    add_asset(&ws, &src, Some("pic"), &opts()).unwrap();
    let home = ws.home.clone();
    let seed = ws.tardos_seed(1);
    drop(ws);
    let ws = Workspace::open(&home).unwrap(); // reopen: same key, same data
    assert_eq!(ws.tardos_seed(1), seed);
    assert_eq!(ws.receivers().unwrap().len(), 3);
    assert_eq!(ws.receiver("2").unwrap().name, "bob"); // by id
    assert!(ws.receiver("nobody").is_err());
    assert_eq!(ws.asset("pic").unwrap().id, ws.asset("1").unwrap().id);
    assert!(ws.add_receiver("alice", "").is_err(), "names are unique");
    assert!(Workspace::open(root.join("missing")).is_err());
}

#[test]
fn names_and_kinds() {
    assert_eq!(safe_name("Alice B."), "Alice_B.");
    assert_eq!(safe_name("  a/b\\c  "), "a_b_c");
    assert_eq!(safe_name("???"), "receiver");
    assert_eq!(safe_name("Zoë"), "Zoë");
    assert_eq!(kind_of(Path::new("a.JPG")).unwrap(), "image");
    assert_eq!(kind_of(Path::new("a.mp4")).unwrap(), "video");
    assert_eq!(kind_of(Path::new("a.flac")).unwrap(), "audio");
    assert!(kind_of(Path::new("a.xyz")).is_err());
}

#[test]
fn video_and_audio_say_not_yet() {
    let root = temp("video");
    let f = root.join("clip.mp4");
    std::fs::write(&f, b"not really a video").unwrap();
    let e = vfp_core::pipeline::plan_file(&f, &PlanOptions::default()).unwrap_err();
    assert!(e.to_string().contains("not supported by the Rust core yet"), "{e}");
}
