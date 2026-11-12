//! Audio through the whole pipeline: register, mark, re-encode with FFmpeg, trace.
//! Skipped when `ffmpeg` is not on PATH.

use std::path::PathBuf;
use std::process::Command;
use vfp_core::audio::ffio::{decode_audio, encode_audio, probe_audio};
use vfp_core::pipeline::{add_asset, mark_for_receiver, trace_leak, PlanOptions};
use vfp_core::workspace::Workspace;

fn have_ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok() && Command::new("ffprobe").arg("-version").output().is_ok()
}

fn temp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("vfp-audio-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// 20 s of tones plus noise, 22.05 kHz stereo.
fn make_source(path: &std::path::Path) {
    let sr = 22050u32;
    let n = 20 * sr as usize;
    let mut seed = 12345u64;
    let mut noise = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
    };
    let tau = std::f64::consts::TAU;
    let left: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f64 / sr as f64;
            (0.25 * (tau * 220.0 * t).sin() * (0.6 + 0.4 * (tau * 0.7 * t).sin()) + 0.15 * (tau * 1500.0 * t).sin() + 0.1 * noise()) as f32
        })
        .collect();
    let right: Vec<f32> = left.iter().map(|v| v * 0.8 + 0.02 * noise() as f32).collect();
    encode_audio(&[left, right], sr, path).unwrap();
}

fn ffmpeg(args: &[&str]) {
    let o = Command::new("ffmpeg").args(["-y", "-v", "error"]).args(args).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

#[test]
fn mark_reencode_trace() {
    if !have_ffmpeg() {
        eprintln!("skipped: ffmpeg not found");
        return;
    }
    let root = temp("flow");
    let ws = Workspace::init(root.join("ws")).unwrap();
    for n in ["alice", "bob", "carol"] {
        ws.add_receiver(n, "").unwrap();
    }
    let src = root.join("tune.wav");
    make_source(&src);
    let info = probe_audio(&src).unwrap();
    assert_eq!((info.sr, info.channels), (22050, 2));

    let opts = PlanOptions { max_receivers: 20, ..Default::default() };
    let (asset, plan) = add_asset(&ws, &src, Some("tune"), &opts).unwrap();
    assert_eq!(asset.kind, "audio");
    assert!(plan.notes.iter().any(|n| n.starts_with("Short audio")), "{:?}", plan.notes);

    let out = root.join("out");
    let copies: Vec<PathBuf> = ["alice", "bob", "carol"].iter().map(|n| mark_for_receiver(&ws, &asset, n, &out).unwrap()).collect();
    assert!(copies[0].ends_with("tune__alice.wav"));

    // the marked file is the same length and stays close to the original
    let (a, b) = (decode_audio(&src, None, None).unwrap(), decode_audio(&copies[1], None, None).unwrap());
    assert_eq!((a.len(), a[0].len()), (b.len(), b[0].len()));
    let err: f64 = a[0].iter().zip(&b[0]).map(|(x, y)| ((x - y) as f64).powi(2)).sum();
    let sig: f64 = a[0].iter().map(|x| (*x as f64).powi(2)).sum();
    let snr = 10.0 * (sig / err).log10();
    println!("SNR of the mark: {snr:.1} dB");
    assert!(snr > 20.0, "mark too loud: {snr}");

    // plain copy
    let r = trace_leak(&ws, &copies[1], None).unwrap();
    assert_eq!(r.flagged, vec!["bob"], "{r:?}");

    // MP3 128k, then trimmed and resampled
    let mp3 = root.join("leak.mp3");
    ffmpeg(&["-i", copies[2].to_str().unwrap(), "-c:a", "libmp3lame", "-b:a", "128k", mp3.to_str().unwrap()]);
    let r = trace_leak(&ws, &mp3, Some("tune")).unwrap();
    assert_eq!(r.flagged, vec!["carol"], "{r:?}");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);

    let cut = root.join("cut.wav");
    ffmpeg(&["-i", copies[0].to_str().unwrap(), "-ss", "4", "-t", "11", "-ar", "16000", cut.to_str().unwrap()]);
    let r = trace_leak(&ws, &cut, None).unwrap();
    assert_eq!(r.flagged, vec!["alice"], "{r:?}");

    // the unmarked original accuses nobody
    let r = trace_leak(&ws, &src, None).unwrap();
    assert!(r.flagged.is_empty(), "{r:?}");
}

#[test]
fn speed_change_accuses_nobody() {
    if !have_ffmpeg() {
        return;
    }
    let root = temp("speed");
    let ws = Workspace::init(root.join("ws")).unwrap();
    ws.add_receiver("alice", "").unwrap();
    ws.add_receiver("bob", "").unwrap();
    let src = root.join("tune.wav");
    make_source(&src);
    let (asset, _) = add_asset(&ws, &src, Some("tune"), &PlanOptions { max_receivers: 20, ..Default::default() }).unwrap();
    let a = mark_for_receiver(&ws, &asset, "alice", &root.join("out")).unwrap();
    mark_for_receiver(&ws, &asset, "bob", &root.join("out")).unwrap();
    let fast = root.join("fast.wav");
    ffmpeg(&["-i", a.to_str().unwrap(), "-af", "atempo=1.03", fast.to_str().unwrap()]);
    let r = trace_leak(&ws, &fast, None).unwrap();
    assert!(r.flagged.is_empty(), "a speed change must not accuse anyone: {r:?}");
}
