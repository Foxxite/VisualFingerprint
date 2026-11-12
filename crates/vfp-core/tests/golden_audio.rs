//! Audio port against the Python reference (tests/golden/golden.json, "audio" and layout parts).

use serde_json::Value;
use vfp_core::audio::align::{find_offset, prep};
use vfp_core::audio::common::{auto_nslots, seg_layout, AudioParams};
use vfp_core::audio::detect::detect_arrays;
use vfp_core::audio::embed::embed_audio_array;

fn golden() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/golden.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("run tools/dump_golden.py first")).unwrap()
}
fn u64s(v: &Value) -> u64 {
    v.as_str().map(|s| s.parse().unwrap()).unwrap_or_else(|| v.as_u64().unwrap())
}
fn f64s(v: &Value) -> Vec<f64> {
    v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect()
}

/// Same integer-only noise as tools/dump_golden.py.
fn hash_noise(n: usize, salt: u64) -> Vec<f64> {
    (0..n as u64)
        .map(|k| {
            let i = (k + salt) & 0xFFFF_FFFF;
            let mut h = (i * 2_654_435_761) & 0xFFFF_FFFF;
            h ^= h >> 15;
            h = (h * 2_246_822_519) & 0xFFFF_FFFF;
            h ^= h >> 13;
            (h as f64 / 4_294_967_296.0) * 2.0 - 1.0
        })
        .collect()
}

fn synth_audio(n: usize, sr: u32) -> Vec<f32> {
    let tau = std::f64::consts::TAU;
    let t = |i: usize| i as f64 / sr as f64;
    let env = |i: usize| 0.6 + 0.4 * (tau * 0.5 * t(i)).sin();
    let noise = hash_noise(n, 0);
    (0..n)
        .map(|i| {
            (0.3 * (tau * 220.0 * t(i)).sin() * env(i)
                + 0.2 * (tau * 1375.0 * t(i) + 1.0).sin()
                + 0.15 * (tau * 3100.0 * t(i) + 2.0).sin() * env(n - 1 - i)
                + 0.1 * noise[i]) as f32
        })
        .collect()
}

fn max_diff(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max)
}

#[test]
fn layout_and_sizes() {
    let g = golden();
    for l in g["audio_layout"].as_array().unwrap() {
        let (nf, nb, ns) = (l["nframes"].as_u64().unwrap() as usize, l["nbins"].as_u64().unwrap() as usize, l["nslots"].as_u64().unwrap() as usize);
        let (w, slot) = seg_layout(u64s(&l["seed"]), l["seg"].as_u64().unwrap(), nf, nb, ns);
        let want_w: Vec<f64> = l["w"].as_array().unwrap().iter().flat_map(f64s).collect();
        let want_s: Vec<u64> = l["slot"].as_array().unwrap().iter().flat_map(|r| r.as_array().unwrap().iter().map(|x| x.as_u64().unwrap())).collect();
        assert_eq!(w.iter().map(|&v| v as f64).collect::<Vec<_>>(), want_w, "pattern");
        assert_eq!(slot.iter().map(|&v| v as u64).collect::<Vec<_>>(), want_s, "slot map");
    }
    let m = &g["audio_misc"];
    let p = AudioParams::default();
    for (sr, want) in m["band"].as_object().unwrap() {
        let (b0, b1) = p.band(sr.parse().unwrap());
        assert_eq!([b0 as u64, b1 as u64], [want[0].as_u64().unwrap(), want[1].as_u64().unwrap()], "band at {sr}");
    }
    for c in m["capacity"].as_array().unwrap() {
        let q = AudioParams { nslots: c["nslots"].as_u64().unwrap() as usize, ..Default::default() };
        assert_eq!(q.capacity(c["dur"].as_f64().unwrap()) as u64, c["m"].as_u64().unwrap());
    }
    for c in m["auto_nslots"].as_array().unwrap() {
        let n = auto_nslots(c["dur"].as_f64().unwrap(), &p, c["target"].as_u64().unwrap() as usize);
        assert_eq!(n as u64, c["n"].as_u64().unwrap(), "auto_nslots {c}");
    }
}

#[test]
fn embed_align_detect_match_python() {
    let a = &golden()["audio"];
    let (sr, secs) = (a["sr"].as_u64().unwrap() as u32, a["secs"].as_u64().unwrap() as usize);
    let seed = u64s(&a["seed"]);
    let p = AudioParams { nslots: a["nslots"].as_u64().unwrap() as usize, ..Default::default() };
    let cap = a["cap"].as_u64().unwrap() as usize;
    assert_eq!(p.capacity(secs as f64), cap);
    let sym: Vec<u8> = a["symbols"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect();

    // the test signal itself
    let x = synth_audio(secs * sr as usize, sr);
    let x_sub: Vec<f64> = x.iter().step_by(97).map(|&v| v as f64).collect();
    assert!(max_diff(&x_sub, &f64s(&a["x_sub"])) < 1e-5, "test signal differs");

    // band-limited, whitened signal (Butterworth + zero-phase filter)
    let pr: Vec<f64> = prep(&x, sr).iter().step_by(101).copied().collect();
    let d = max_diff(&pr, &f64s(&a["prep_sub"]));
    println!("prep: max diff {d:.2e}");
    assert!(d < 1e-4, "prep differs by {d}");

    // embedding
    let y = embed_audio_array(&[x.clone()], sr, &sym, seed, &p).unwrap().remove(0);
    let y_sub: Vec<f64> = y.iter().step_by(97).map(|&v| v as f64).collect();
    let d = max_diff(&y_sub, &f64s(&a["y_sub"]));
    let change = x.iter().zip(&y).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max) as f64;
    println!("embed: max diff {d:.2e}, max change {change:.5} (python {:.5})", a["max_abs_change"].as_f64().unwrap());
    assert!(d < 1e-4, "marked samples differ by {d}");
    assert!((change - a["max_abs_change"].as_f64().unwrap()).abs() < 1e-3);

    // offset search and detection on a quiet, noisy excerpt
    let (start, len) = (a["leak_start"].as_u64().unwrap() as usize, a["leak_len"].as_u64().unwrap() as usize);
    let noise = hash_noise(len, 12345);
    let leak: Vec<f32> = (0..len).map(|i| (0.5 * y[start + i] as f64 + 0.01 * noise[i]) as f32).collect();
    let (off, score) = find_offset(&x, &leak, sr);
    println!("offset {off} score {score:.4}");
    assert_eq!(off, a["offset"].as_i64().unwrap());
    assert!((score - a["offset_score"].as_f64().unwrap()).abs() < 1e-3);

    let det = detect_arrays(&x, &leak, sr, seed, &p, cap);
    let want_z = f64s(&a["z"]);
    let dz = max_diff(&det.z, &want_z);
    println!("detect: max z diff {dz:.3e}, coverage {:.3}, gain {:.4}", det.coverage, det.gain);
    assert!(dz < 0.05, "z differs by {dz}");
    let bits: Vec<u8> = a["bits"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect();
    assert_eq!(det.bits, bits);
    assert!((det.coverage - a["coverage"].as_f64().unwrap()).abs() < 1e-9);
    assert!((det.gain - a["det_gain"].as_f64().unwrap()).abs() < 1e-3);
}
