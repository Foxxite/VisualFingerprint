//! Video through the whole pipeline: register, mark, re-encode / trim with FFmpeg, trace.
//! Skipped when `ffmpeg` is not on PATH (or lacks libx264).

use std::path::{Path, PathBuf};
use std::process::Command;
use vfp_core::pipeline::{add_asset, mark_for_receiver, trace_leak, PlanOptions};
use vfp_core::video::ffio::probe;
use vfp_core::workspace::Workspace;

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg").args(["-y", "-v", "error"]).args(args).output().map(|o| o.status.success()).unwrap_or(false)
}

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vfp-video-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// 6 s, 640x360, 25 fps: moving test pattern with grain so there is texture to hide the mark in.
fn make_source(path: &Path) -> bool {
    ffmpeg(&[
        "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=25:duration=6",
        "-vf", "noise=alls=14:allf=t,format=yuv420p",
        "-c:v", "libx264", "-crf", "14", path.to_str().unwrap(),
    ])
}

#[test]
fn mark_reencode_trim_trace() {
    let root = temp("flow");
    let src = root.join("clip.mp4");
    if !make_source(&src) {
        eprintln!("skipped: ffmpeg with libx264 is not available");
        return;
    }
    let info = probe(&src).unwrap();
    assert_eq!((info.width, info.height, info.nframes), (640, 360, 150));

    let ws = Workspace::init(root.join("ws")).unwrap();
    for n in ["alice", "bob", "carol"] {
        ws.add_receiver(n, "").unwrap();
    }
    let (asset, plan) = add_asset(&ws, &src, Some("clip"), &PlanOptions { max_receivers: 20, ..Default::default() }).unwrap();
    assert_eq!(asset.kind, "video");
    assert!(plan.notes.iter().any(|n| n.starts_with("Short video")), "{:?}", plan.notes);

    let out = root.join("out");
    let copies: Vec<PathBuf> = ["alice", "bob", "carol"].iter().map(|n| mark_for_receiver(&ws, &asset, n, &out, None).unwrap()).collect();
    let marked = probe(&copies[1]).unwrap();
    assert_eq!((marked.width, marked.height), (640, 360));
    assert!((marked.nframes as i64 - 150).abs() <= 1);

    // plain copy
    let r = trace_leak(&ws, &copies[1], None).unwrap();
    assert_eq!(r.flagged, vec!["bob"], "{r:?}");

    // re-encoded at a much lower quality
    let low = root.join("low.mp4");
    assert!(ffmpeg(&["-i", copies[2].to_str().unwrap(), "-c:v", "libx264", "-crf", "30", "-an", low.to_str().unwrap()]));
    let r = trace_leak(&ws, &low, Some("clip")).unwrap();
    assert_eq!(r.flagged, vec!["carol"], "{r:?}");

    // trimmed at the start (the time offset must be found)
    let cut = root.join("cut.mp4");
    assert!(ffmpeg(&["-i", copies[0].to_str().unwrap(), "-ss", "1.2", "-c:v", "libx264", "-crf", "26", "-an", cut.to_str().unwrap()]));
    let r = trace_leak(&ws, &cut, None).unwrap();
    assert_eq!(r.flagged, vec!["alice"], "{r:?}");
    assert_eq!(r.evidence["time_offset_frames"].as_i64().unwrap(), 30, "{}", r.evidence);

    // scaled down to 480x270
    let small = root.join("small.mp4");
    assert!(ffmpeg(&["-i", copies[1].to_str().unwrap(), "-vf", "scale=480:270", "-c:v", "libx264", "-crf", "24", "-an", small.to_str().unwrap()]));
    let r = trace_leak(&ws, &small, None).unwrap();
    assert_eq!(r.flagged, vec!["bob"], "{r:?}");

    // the unmarked original accuses nobody
    let r = trace_leak(&ws, &src, None).unwrap();
    assert!(r.flagged.is_empty(), "{r:?}");
}
