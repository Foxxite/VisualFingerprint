//! Video primitives against the Python reference (tests/golden/golden.json, "video").

use serde_json::Value;
use std::path::PathBuf;
use vfp_core::image::plane::{block_valid, embed_plane, make_layout, plane_stats};
use vfp_core::image::warp::{resize_area_u8, resize_cubic};
use vfp_core::video::common::{auto_nslots, canon_size, capacity, effective_strength, seg_frames, VideoParams};
use vfp_core::video::ffio::VideoInfo;
use vfp_core::video::sync::find_offsets;

fn golden() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/golden/golden.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("run tools/dump_golden.py first")).unwrap()
}
fn u64s(v: &Value) -> u64 {
    v.as_str().map(|s| s.parse().unwrap()).unwrap_or_else(|| v.as_u64().unwrap())
}
fn f32s(v: &Value) -> Vec<f32> {
    v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect()
}
fn f64s(v: &Value) -> Vec<f64> {
    v.as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect()
}
fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

fn info(n: usize, fps: &str) -> VideoInfo {
    let (num, den) = match fps.split_once('/') {
        Some((a, b)) => (a.parse().unwrap(), b.parse().unwrap()),
        None => (fps.parse().unwrap(), 1),
    };
    VideoInfo {
        path: PathBuf::from("x"),
        width: 1280,
        height: 720,
        fps_num: num,
        fps_den: den,
        nframes: n,
        duration: n as f64 / (num as f64 / den as f64),
        has_audio: false,
        audio_codec: None,
        color: vec![],
    }
}

#[test]
fn sizes_and_capacity() {
    let g = golden();
    let v = &g["video"];
    let p = VideoParams::default();
    for c in v["canon"].as_array().unwrap() {
        let (w, h) = (c["w"].as_u64().unwrap() as usize, c["h"].as_u64().unwrap() as usize);
        let (cw, ch) = canon_size(w, h);
        assert_eq!([cw as u64, ch as u64], [c["canon"][0].as_u64().unwrap(), c["canon"][1].as_u64().unwrap()], "{w}x{h}");
        assert!((effective_strength(w, h, &p) as f64 - c["strength"].as_f64().unwrap()).abs() < 1e-5);
    }
    for c in v["capacity"].as_array().unwrap() {
        let i = info(c["nframes"].as_u64().unwrap() as usize, c["fps"].as_str().unwrap());
        assert_eq!(seg_frames(&i, &p) as u64, c["seg_frames"].as_u64().unwrap());
        assert_eq!(capacity(&i, &p, None) as u64, c["cap"].as_u64().unwrap());
        assert_eq!(auto_nslots(&i, &p, 2455) as u64, c["auto"].as_u64().unwrap());
        assert_eq!(capacity(&i, &p, Some(2.0)) as u64, c["cap_max"].as_u64().unwrap());
    }
}

#[test]
fn layout_embed_and_stats() {
    let g = golden();
    let v = &g["video"];
    let l = &v["layout"];
    let seed = u64s(&l["seed"]);
    let (slot, pat) = make_layout(40, 56, &[seed, 0x7E6, l["seg"].as_u64().unwrap()], 8);
    let want_slot: Vec<u32> = l["slot"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect();
    assert_eq!(slot, want_slot, "slot map (3-part seed)");
    for (r, row) in l["pat_first"].as_array().unwrap().iter().enumerate() {
        for (k, x) in row.as_array().unwrap().iter().enumerate() {
            assert_eq!(pat[r * 16 + k] as f64, x.as_f64().unwrap());
        }
    }

    let y0 = f32s(&v["embed_plane"]["y"]);
    let sym = [1.0f32, -1.0, 1.0, 1.0, -1.0, -1.0, 1.0, -1.0];
    let marked = embed_plane(&y0, 40, 56, &sym, &slot, &pat, 2.1);
    let d = max_diff(&marked, &f32s(&v["embed_plane"]["marked"]));
    println!("embed_plane max diff {d:.2e}");
    assert!(d < 1e-3);

    let s = &v["plane_stats"];
    let leak = f32s(&s["leak"]);
    let mut valid = vec![true; 40 * 56];
    for y in 0..40 {
        for x in 50..56 {
            valid[y * 56 + x] = false;
        }
    }
    let bv = block_valid(&valid, 40, 56);
    let want_bv: Vec<bool> = s["bvalid"].as_array().unwrap().iter().map(|b| b.as_bool().unwrap()).collect();
    assert_eq!(bv, want_bv);
    let (num, den, sig) = plane_stats(&y0, &leak, 40, 56, &bv, &slot, &pat, 2.1, 8);
    for (a, b) in num.iter().zip(f64s(&s["num"])) {
        assert!((a - b).abs() < 1e-2 * (1.0 + b.abs()), "num {a} vs {b}");
    }
    for (a, b) in den.iter().zip(f64s(&s["den"])) {
        assert!((a - b).abs() < 1e-3 * (1.0 + b.abs()), "den {a} vs {b}");
    }
    assert!((sig - s["sigma"].as_f64().unwrap()).abs() < 1e-3, "sigma {sig}");
}

fn thumbs(rows: &Value) -> Vec<Vec<f32>> {
    rows.as_array().unwrap().iter().map(f32s).collect()
}

/// Same integer-only noise as tools/dump_golden.py.
fn hash_noise(n: usize, salt: u64) -> Vec<f32> {
    (0..n as u64)
        .map(|k| {
            let i = (k + salt) & 0xFFFF_FFFF;
            let mut h = (i * 2_654_435_761) & 0xFFFF_FFFF;
            h ^= h >> 15;
            h = (h * 2_246_822_519) & 0xFFFF_FFFF;
            h ^= h >> 13;
            ((h as f64 / 4_294_967_296.0) * 2.0 - 1.0) as f32
        })
        .collect()
}

#[test]
fn time_offsets_match() {
    let g = golden();
    let o = &g["video"]["offsets"];
    let (nf, tau) = (o["nf"].as_u64().unwrap() as usize, o["tau_true"].as_u64().unwrap() as usize);
    let sig_o: Vec<Vec<f32>> = (0..nf).map(|k| hash_noise(576, 1000 + k as u64)).collect();
    let sig_l: Vec<Vec<f32>> = (0..100)
        .map(|k| {
            let n = hash_noise(576, 90000 + k as u64);
            sig_o[k + tau].iter().zip(&n).map(|(a, b)| (*a as f64 + 0.2 * *b as f64) as f32).collect()
        })
        .collect();
    let half: Vec<Vec<f32>> = sig_l.iter().step_by(2).cloned().collect();
    for (label, got) in [("same_fps", find_offsets(&sig_o, 30.0, &sig_l, 30.0)), ("half_fps", find_offsets(&sig_o, 30.0, &half, 15.0))] {
        let want = o[label].as_array().unwrap();
        println!("{label}: {got:?}");
        assert_eq!(got.len(), want.len(), "{label}");
        for (g, w) in got.iter().zip(want) {
            assert_eq!(g.0, w[0].as_i64().unwrap(), "{label} tau");
            assert!((g.1 - w[1].as_f64().unwrap()).abs() < 0.01, "{label} distance {} vs {}", g.1, w[1]);
        }
    }
    let _ = thumbs;
}

#[test]
fn resizers_match_opencv() {
    let g = golden();
    let r = &g["video"]["resize"];
    let src: Vec<u8> = r["src"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
    let area = resize_area_u8(&src, 36, 48, 23, 30);
    let want: Vec<u8> = r["area"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
    let worst = area.iter().zip(&want).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
    println!("area resize: worst difference {worst}");
    assert!(worst <= 1, "area resize differs by {worst}");

    // cubic: the Python input is the top-left 23x30 corner of the plane, scaled
    let small: Vec<f32> = (0..23).flat_map(|y| (0..30).map(move |x| (y, x))).map(|(y, x)| (src[y * 48 + x] as f32 - 100.0) / 7.0).collect();
    let up = resize_cubic(&small, 23, 30, 36, 48);
    let d = max_diff(&up, &f32s(&r["cubic"]));
    println!("cubic resize: max diff {d:.2e}");
    assert!(d < 1e-3, "cubic resize differs by {d}");
}
