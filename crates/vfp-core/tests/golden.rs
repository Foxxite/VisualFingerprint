//! Compare the Rust port with the Python reference (tests/golden/golden.json).
//! Regenerate the file with `python tools/dump_golden.py`.

use serde_json::Value;
use vfp_core::image::common::{block_dct, grid, layout, pad8, usable_slots, ImageParams};
use vfp_core::image::detect::detect_aligned;
use vfp_core::image::embed::embed_image;
use vfp_core::image::{color::to_ycrcb, Bgr};
use vfp_core::keys::derive_seed;
use vfp_core::perceptual::block_mask;
use vfp_core::tardos::{accusation_threshold, required_length, supported_colluders, TardosCode};

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
fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * (1.0 + b.abs())
}

#[test]
fn key_derivation() {
    let g = golden();
    let key = hex(g["master_key_hex"].as_str().unwrap());
    for c in g["derive_seed"].as_array().unwrap() {
        let labels: Vec<&str> = c["labels"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
        assert_eq!(derive_seed(&key, &labels), u64s(&c["seed"]), "{labels:?}");
    }
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

#[test]
fn tardos_codes_and_scores() {
    for t in golden()["tardos"].as_array().unwrap() {
        let (seed, len, c) = (u64s(&t["seed"]), t["length"].as_u64().unwrap() as usize, t["n_colluders"].as_u64().unwrap());
        let code = TardosCode::create(len, seed, c);
        for (a, b) in code.p.iter().zip(f64s(&t["p"])) {
            assert!(close(*a, b, 1e-12), "p differs: {a} vs {b}");
        }
        let words = t["codewords"].as_array().unwrap();
        for (j, w) in words.iter().enumerate() {
            let want: Vec<u8> = w.as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
            assert_eq!(code.codeword(j as u64), want, "codeword {j}");
        }
        let y: Vec<u8> = t["y"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
        let conf = f64s(&t["confidence"]);
        for (j, want) in f64s(&t["scores_plain"]).iter().enumerate() {
            assert!(close(code.score(j as u64, &y, None), *want, 1e-9));
        }
        for (j, want) in f64s(&t["scores_weighted"]).iter().enumerate() {
            assert!(close(code.score(j as u64, &y, Some(&conf)), *want, 1e-9));
        }
        let n = t["n_users"].as_u64().unwrap();
        assert!(close(accusation_threshold(n, len, 1e-3, None), t["threshold_plain"].as_f64().unwrap(), 1e-9));
        assert!(close(accusation_threshold(n, len, 1e-3, Some(&conf)), t["threshold_weighted"].as_f64().unwrap(), 1e-9));
    }
}

#[test]
fn code_length_helpers() {
    let g = golden();
    for c in g["required_length"].as_array().unwrap() {
        let got = required_length(c["n"].as_u64().unwrap(), c["c"].as_u64().unwrap(), c["eps"].as_f64().unwrap());
        assert_eq!(got, c["m"].as_u64().unwrap());
    }
    for c in g["supported_colluders"].as_array().unwrap() {
        let got = supported_colluders(c["m"].as_u64().unwrap(), c["n"].as_u64().unwrap(), c["eps"].as_f64().unwrap());
        assert_eq!(got, c["c"].as_u64().unwrap());
    }
    for c in g["usable_slots"].as_array().unwrap() {
        let got = usable_slots(c["h"].as_u64().unwrap() as usize, c["w"].as_u64().unwrap() as usize, c["wanted"].as_u64().unwrap() as usize);
        assert_eq!(got as u64, c["n"].as_u64().unwrap());
    }
}

#[test]
fn image_layout_matches_numpy() {
    for l in golden()["image_layout"].as_array().unwrap() {
        let (nbh, nbw) = (l["nbh"].as_u64().unwrap() as usize, l["nbw"].as_u64().unwrap() as usize);
        let (slot, pat) = layout(nbh, nbw, u64s(&l["seed"]), l["nslots"].as_u64().unwrap() as usize);
        let want: Vec<u32> = l["slot"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect();
        assert_eq!(slot, want, "slot map");
        let rows = l["w_first_rows"].as_array().unwrap();
        for (r, row) in rows.iter().enumerate() {
            for (k, v) in row.as_array().unwrap().iter().enumerate() {
                assert_eq!(pat[r * 16 + k] as f64, v.as_f64().unwrap(), "pattern [{r}][{k}]");
            }
        }
    }
}

fn test_image(h: usize, w: usize) -> Bgr {
    let mut data = vec![0u8; h * w * 3];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let i = (y as usize * w + x as usize) * 3;
            data[i] = ((x * 7 + y * 13 + ((x * y) % 31) * 5) % 256) as u8;
            data[i + 1] = ((x * 3 + y * 5 + ((x + y) % 17) * 11) % 256) as u8;
            data[i + 2] = ((x * 11 + y * 2 + ((x ^ y) % 23) * 7) % 256) as u8;
        }
    }
    Bgr::new(h, w, data)
}

#[test]
fn image_dct_and_mask() {
    let g = golden();
    let d = &g["image_dct"];
    let img = test_image(64, 96);
    let (y8, _, _) = to_ycrcb(&img);
    let y: Vec<f32> = y8.iter().map(|&v| v as f32).collect();
    let (plane, ph, pw) = pad8(&y, 64, 96);
    assert_eq!(grid(64, 96), (8, 12));
    let coef = block_dct(&plane, ph, pw);
    let want0: Vec<f64> = d["coef_block00"].as_array().unwrap().iter().flat_map(f64s).collect();
    for (i, w) in want0.iter().enumerate() {
        assert!((coef[0][i] as f64 - w).abs() < 1e-3, "coef[{i}]");
    }
    let mask: Vec<f64> = d["mask"].as_array().unwrap().iter().flat_map(|r| f64s(r)).collect();
    for (b, c) in coef.iter().enumerate() {
        assert!((block_mask(c) as f64 - mask[b]).abs() < 1e-3, "mask block {b}");
    }
}

#[test]
fn image_embed_and_detect() {
    let g = golden();
    let e = &g["image_embed"];
    let img = test_image(64, 96);
    let want_img: Vec<u8> = e["image"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
    assert_eq!(img.data, want_img, "test image must match the Python one");

    let p = ImageParams { nslots: 8, strength: 3.0 };
    let sym: Vec<u8> = e["symbols"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
    let marked = embed_image(&img, &sym, 4242, &p).unwrap();
    let want: Vec<u8> = e["marked"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
    let diffs: Vec<i32> = marked.data.iter().zip(&want).map(|(a, b)| (*a as i32 - *b as i32).abs()).collect();
    let max = *diffs.iter().max().unwrap();
    let wrong = diffs.iter().filter(|&&d| d != 0).count();
    println!("embed: max diff {max}, {wrong} of {} bytes differ", diffs.len());
    assert!(max <= 1, "max pixel difference {max}");
    assert!(wrong * 100 < diffs.len(), "more than 1% of bytes differ: {wrong}");

    let det = detect_aligned(&marked, &img, None, 4242, &p);
    let d = &g["image_detect"];
    for (a, b) in det.z.iter().zip(f64s(&d["z"])) {
        assert!((a - b).abs() < 0.5, "z {a} vs {b}");
    }
    let bits: Vec<u8> = d["bits"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u8).collect();
    assert_eq!(det.bits, bits);
    assert_eq!(det.bits, sym);
}
