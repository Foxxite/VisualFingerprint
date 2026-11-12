//! Collusion test on real media (Rust version of `vfp.eval.collusion_eval`).
//!
//! Marks copies for N receivers, lets random groups of colluders combine their copies,
//! then traces the result through the normal pipeline.
//! Success = at least one colluder is accused AND no innocent receiver is accused.
//!
//!   cargo run --release --example collusion -- image FILE [--receivers 12] [--trials 6] [--sizes 3,4]
//!   cargo run --release --example collusion -- audio FILE
//!   cargo run --release --example collusion -- video FILE

use std::path::{Path, PathBuf};
use std::time::Instant;
use vfp_core::audio::ffio::{decode_audio, encode_audio, probe_audio};
use vfp_core::image::Bgr;
use vfp_core::imgio::{read_image, write_image};
use vfp_core::pipeline::{add_asset, mark_for_receiver, trace_leak, PlanOptions};
use vfp_core::rng::Pcg64;
use vfp_core::video::ffio::{decode_yuv, probe, Encoder};
use vfp_core::workspace::Workspace;

// ------------------------------------------------------------------ image
const IMAGE_STRATEGIES: [&str; 6] = ["average", "min", "max", "median", "pixel-mix", "tile-mix 32px"];

fn mix_image(strategy: &str, arrs: &[Bgr], rng: &mut Pcg64) -> Bgr {
    let (h, w, c) = (arrs[0].h, arrs[0].w, arrs.len());
    let mut out = arrs[0].clone();
    let n = h * w * 3;
    match strategy {
        "average" | "min" | "max" | "median" => {
            let mut vals = vec![0u8; c];
            for i in 0..n {
                for (k, a) in arrs.iter().enumerate() {
                    vals[k] = a.data[i];
                }
                out.data[i] = match strategy {
                    "average" => (vals.iter().map(|&v| v as f64).sum::<f64>() / c as f64).round_ties_even() as u8,
                    "min" => *vals.iter().min().unwrap(),
                    "max" => *vals.iter().max().unwrap(),
                    _ => {
                        vals.sort_unstable();
                        let m = if c % 2 == 1 { vals[c / 2] as f64 } else { (vals[c / 2 - 1] as f64 + vals[c / 2] as f64) / 2.0 };
                        m.round_ties_even() as u8
                    }
                };
            }
        }
        "pixel-mix" => {
            for p in 0..h * w {
                let k = rng.interval_pub(c as u64 - 1) as usize;
                out.data[3 * p..3 * p + 3].copy_from_slice(&arrs[k].data[3 * p..3 * p + 3]);
            }
        }
        _ => {
            for ty in (0..h).step_by(32) {
                for tx in (0..w).step_by(32) {
                    let k = rng.interval_pub(c as u64 - 1) as usize;
                    for y in ty..(ty + 32).min(h) {
                        for x in tx..(tx + 32).min(w) {
                            let i = 3 * (y * w + x);
                            out.data[i..i + 3].copy_from_slice(&arrs[k].data[i..i + 3]);
                        }
                    }
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------------ audio
const AUDIO_STRATEGIES: [&str; 3] = ["average", "interleave 0.25-3s", "min-abs"];

fn mix_audio(strategy: &str, xs: &[Vec<Vec<f32>>], sr: u32, rng: &mut Pcg64) -> Vec<Vec<f32>> {
    let (ch, n, c) = (xs[0].len(), xs[0][0].len(), xs.len());
    let mut out = vec![vec![0f32; n]; ch];
    match strategy {
        "average" => {
            for k in 0..ch {
                for i in 0..n {
                    out[k][i] = xs.iter().map(|x| x[k][i]).sum::<f32>() / c as f32;
                }
            }
        }
        "min-abs" => {
            for k in 0..ch {
                for i in 0..n {
                    out[k][i] = xs.iter().map(|x| x[k][i]).min_by(|a, b| a.abs().partial_cmp(&b.abs()).unwrap()).unwrap();
                }
            }
        }
        _ => {
            let fade = (0.02 * sr as f64) as usize;
            let mut pos = 0usize;
            let mut prev: Option<Vec<Vec<f32>>> = None;
            while pos < n {
                let ln = ((0.25 + rng.next_f64() * 2.75) * sr as f64) as usize;
                let k = rng.interval_pub(c as u64 - 1) as usize;
                let end = (pos + ln).min(n);
                for q in 0..ch {
                    let mut seg: Vec<f32> = xs[k][q][pos..end].to_vec();
                    if let Some(p) = &prev {
                        if seg.len() > fade {
                            for i in 0..fade {
                                let ramp = i as f32 / (fade - 1) as f32;
                                seg[i] = p[q][i] * (1.0 - ramp) + seg[i] * ramp;
                            }
                        }
                    }
                    out[q][pos..end].copy_from_slice(&seg);
                }
                prev = if pos + ln + fade <= n { Some((0..ch).map(|q| xs[k][q][pos + ln..pos + ln + fade].to_vec()).collect()) } else { None };
                pos += ln;
            }
        }
    }
    out
}

// ------------------------------------------------------------------ video
const VIDEO_STRATEGIES: [&str; 3] = ["average", "interleave 8-45 frames", "frame-mix"];

fn mix_video(strategy: &str, paths: &[PathBuf], rng: &mut Pcg64, out: &Path) {
    let info = probe(&paths[0]).unwrap();
    let mut readers: Vec<_> = paths.iter().map(|p| decode_yuv(&probe(p).unwrap(), None, 0.0, None).unwrap()).collect();
    let c = paths.len();
    let mut enc = Encoder::new(out, &info, 23, "fast", None, None).unwrap();
    let (mut cur, mut left) = (0usize, 0i64);
    loop {
        let frames: Vec<_> = readers.iter_mut().map(|r| r.next()).collect();
        if frames.iter().any(|f| f.is_none()) {
            break;
        }
        let frames: Vec<_> = frames.into_iter().map(|f| f.unwrap()).collect();
        if strategy == "average" {
            let avg = |sel: &dyn Fn(&vfp_core::video::ffio::Frame) -> &Vec<u8>| -> Vec<u8> {
                (0..sel(&frames[0]).len())
                    .map(|i| (frames.iter().map(|f| sel(f)[i] as f64).sum::<f64>() / c as f64).round_ties_even() as u8)
                    .collect()
            };
            enc.write(&avg(&|f| &f.y), &avg(&|f| &f.u), &avg(&|f| &f.v)).unwrap();
        } else {
            if strategy == "frame-mix" {
                cur = rng.interval_pub(c as u64 - 1) as usize;
            } else if left <= 0 {
                cur = rng.interval_pub(c as u64 - 1) as usize;
                left = 8 + rng.interval_pub(36) as i64; // 8..=44
            }
            left -= 1;
            let f = &frames[cur];
            enc.write(&f.y, &f.u, &f.v).unwrap();
        }
    }
    enc.finish().unwrap();
}

fn arg<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (kind, file) = match (args.first(), args.get(1)) {
        (Some(k), Some(f)) => (k.as_str(), PathBuf::from(f)),
        _ => {
            eprintln!("usage: collusion {{image|audio|video}} FILE [--receivers N] [--trials N] [--sizes 2,3] [--strategies a,b] [--max-receivers N] [--eps X] [--slots N] [--work DIR] [--seed N]");
            std::process::exit(2);
        }
    };
    let n_recv: usize = arg(&args, "--receivers").map_or(12, |v| v.parse().unwrap());
    let trials: usize = arg(&args, "--trials").map_or(6, |v| v.parse().unwrap());
    let sizes: Vec<usize> = arg(&args, "--sizes").unwrap_or("2,3").split(',').map(|s| s.trim().parse().unwrap()).collect();
    let seed: u64 = arg(&args, "--seed").map_or(1, |v| v.parse().unwrap());
    let max_receivers: u64 = arg(&args, "--max-receivers").map_or(1000, |v| v.parse().unwrap());
    let eps: f64 = arg(&args, "--eps").map_or(1e-3, |v| v.parse().unwrap());
    let slots: Option<usize> = arg(&args, "--slots").map(|v| v.parse().unwrap());
    let work = PathBuf::from(arg(&args, "--work").map(str::to_string).unwrap_or_else(|| std::env::temp_dir().join(format!("vfp_coll_{}", std::process::id())).to_string_lossy().to_string()));
    let _ = std::fs::remove_dir_all(work.join("ws"));
    let _ = std::fs::remove_dir_all(work.join("copies"));
    std::fs::create_dir_all(&work).unwrap();

    let ws = Workspace::init(work.join("ws")).unwrap();
    let names: Vec<String> = (1..=n_recv).map(|i| format!("R{i:02}")).collect();
    for n in &names {
        ws.add_receiver(n, "").unwrap();
    }
    let opts = PlanOptions { max_receivers, eps, slots, ..Default::default() };
    let (asset, plan) = add_asset(&ws, &file, Some("asset"), &opts).unwrap();
    println!(
        "{}: {}, {} symbols, guaranteed colluder group size {} (for {max_receivers} receivers, false-accusation {eps})",
        file.file_name().unwrap().to_string_lossy(), plan.kind, plan.m,
        if plan.guaranteed_colluders > 0 { plan.guaranteed_colluders.to_string() } else { "none (single leaker only)".into() }
    );
    let t = Instant::now();
    let copies_dir = work.join("copies");
    let copy: Vec<PathBuf> = names.iter().map(|n| mark_for_receiver(&ws, &asset, n, &copies_dir, None).unwrap()).collect();
    println!("made {} marked copies in {:.0}s\n", copy.len(), t.elapsed().as_secs_f64());

    let all: Vec<&str> = match kind {
        "image" => IMAGE_STRATEGIES.to_vec(),
        "audio" => AUDIO_STRATEGIES.to_vec(),
        _ => VIDEO_STRATEGIES.to_vec(),
    };
    let wanted: Option<Vec<String>> = arg(&args, "--strategies").map(|s| s.split(',').map(|x| x.trim().to_lowercase()).collect());
    let strategies: Vec<(usize, &str)> =
        all.iter().cloned().enumerate().filter(|(_, s)| wanted.as_ref().is_none_or(|w| w.iter().any(|x| s.to_lowercase().contains(x)))).collect();
    let ext = match kind { "image" => "jpg", "audio" => "mp3", _ => "mp4" };

    println!("{:<24}{:>6}{:>7}{:>11}{:>11}{:>11}{:>10}{:>9}", "strategy", "group", "trials", ">=1 caught", "all caught", "avg caught", "false acc", "success");
    let (mut tot_ok, mut tot_n, mut tot_fa) = (0, 0, 0);
    for (si, strat) in strategies {
        for &c in &sizes {
            let (mut ok, mut any, mut all_c, mut fa, mut caught_sum) = (0, 0, 0, 0, 0usize);
            for tr in 0..trials {
                let mut rng = Pcg64::from_seed(&[seed, c as u64, tr as u64, si as u64]);
                let order = rng.permutation(names.len());
                let guilty: Vec<usize> = order[..c].iter().map(|&i| i as usize).collect();
                let out = work.join(format!("leak_{si}_{c}_{tr}.{ext}"));
                let paths: Vec<PathBuf> = guilty.iter().map(|&g| copy[g].clone()).collect();
                match kind {
                    "image" => {
                        let arrs: Vec<Bgr> = paths.iter().map(|p| read_image(p).unwrap()).collect();
                        write_image(&out, &mix_image(strat, &arrs, &mut rng)).unwrap();
                    }
                    "audio" => {
                        let sr = probe_audio(&paths[0]).unwrap().sr;
                        let mut xs: Vec<Vec<Vec<f32>>> = paths.iter().map(|p| decode_audio(p, None, None).unwrap()).collect();
                        let n = xs.iter().map(|x| x[0].len()).min().unwrap();
                        for x in xs.iter_mut() {
                            for ch in x.iter_mut() {
                                ch.truncate(n);
                            }
                        }
                        encode_audio(&mix_audio(strat, &xs, sr, &mut rng), sr, &out).unwrap();
                    }
                    _ => mix_video(strat, &paths, &mut rng, &out),
                }
                let r = trace_leak(&ws, &out, Some("asset")).unwrap();
                let guilty_names: Vec<&String> = guilty.iter().map(|&g| &names[g]).collect();
                let hit = r.flagged.iter().filter(|n| guilty_names.contains(n)).count();
                let wrong = r.flagged.iter().filter(|n| !guilty_names.contains(n)).count();
                any += (hit > 0) as usize;
                all_c += (hit == c) as usize;
                caught_sum += hit;
                fa += (wrong > 0) as usize;
                ok += (hit > 0 && wrong == 0) as usize;
            }
            println!(
                "{:<24}{:>6}{:>7}{:>8}/{:<2}{:>8}/{:<2}{:>11.2}{:>7}/{:<2}{:>8.0}%",
                strat, c, trials, any, trials, all_c, trials, caught_sum as f64 / trials as f64, fa, trials, 100.0 * ok as f64 / trials as f64
            );
            tot_ok += ok;
            tot_n += trials;
            tot_fa += fa;
        }
    }
    println!("\noverall: success {tot_ok}/{tot_n}, trials with a false accusation: {tot_fa}");
}
