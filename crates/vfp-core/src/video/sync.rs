//! Find which original frame each leaked frame came from (time offset, frame rate).

/// Zero mean and unit-ish spread per row, like `_norm` in Python.
fn norm(t: &[Vec<f32>]) -> Vec<Vec<f32>> {
    t.iter()
        .map(|r| {
            let n = r.len() as f32;
            let mean = r.iter().sum::<f32>() / n;
            let var = r.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
            let sd = var.sqrt() + 1.0;
            r.iter().map(|v| (v - mean) / sd).collect()
        })
        .collect()
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = s.len();
    if n % 2 == 1 { s[n / 2] } else { 0.5 * (s[n / 2 - 1] + s[n / 2]) }
}

/// Candidate offsets, best first: (tau, distance). Leaked frame i maps to original frame
/// `round(i / fps_l * fps_o) + tau`. Thumbnails can be fooled by slow scenes or heavy edits,
/// so the caller confirms the winner with the watermark itself.
pub fn find_offsets(sig_o: &[Vec<f32>], fps_o: f64, sig_l: &[Vec<f32>], fps_l: f64) -> Vec<(i64, f64)> {
    const N_SAMPLES: usize = 64;
    const TOP: usize = 4;
    const MIN_GAP: i64 = 4;
    let (so, sl) = (norm(sig_o), norm(sig_l));
    let (no, nl) = (so.len() as i64, sl.len());
    if nl == 0 || no == 0 {
        return vec![(0, 1.0)];
    }
    // evenly spread leaked frames (np.linspace(0, nl-1, n).astype(int), unique)
    let n = nl.min(N_SAMPLES);
    let step = if n > 1 { (nl - 1) as f64 / (n - 1) as f64 } else { 0.0 }; // as NumPy computes it
    let mut idx: Vec<usize> = (0..n).map(|i| if n > 1 && i == n - 1 { nl - 1 } else { (i as f64 * step) as usize }).collect();
    idx.dedup();
    let base: Vec<f64> = idx.iter().map(|&i| i as f64 / fps_l * fps_o).collect();
    let sls: Vec<&Vec<f32>> = idx.iter().map(|&i| &sl[i]).collect();
    let bmax = base.iter().cloned().fold(f64::MIN, f64::max);
    let bmin = base.iter().cloned().fold(f64::MAX, f64::min);
    let (lo, hi) = ((-bmax) as i64, (no as f64 - bmin) as i64);

    let (mut taus, mut dists) = (vec![], vec![]);
    for tau in lo..hi {
        let (mut ok, mut acc, mut cnt) = (0usize, 0f64, 0usize);
        for (k, b) in base.iter().enumerate() {
            let j = (b + tau as f64).round_ties_even() as i64;
            if j >= 0 && j < no {
                ok += 1;
                let (a, o) = (sls[k], &so[j as usize]);
                acc += a.iter().zip(o).map(|(x, y)| ((x - y) * (x - y)) as f64).sum::<f64>();
                cnt += a.len();
            }
        }
        if (ok as f64) / (base.len() as f64) < 0.6 {
            continue;
        }
        taus.push(tau);
        dists.push(acc / cnt as f64);
    }
    if taus.is_empty() {
        return vec![(0, 1.0)];
    }
    let med = median(&dists) + 1e-9;
    let mut order: Vec<usize> = (0..taus.len()).collect();
    order.sort_by(|&a, &b| dists[a].partial_cmp(&dists[b]).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<(i64, f64)> = vec![];
    for k in order {
        if out.iter().all(|(t, _)| (taus[k] - t).abs() >= MIN_GAP) {
            out.push((taus[k], dists[k] / med));
        }
        if out.len() >= TOP {
            break;
        }
    }
    if let Some(z) = taus.iter().position(|&t| t == 0) {
        if out.iter().all(|(t, _)| *t != 0) {
            out.push((0, dists[z] / med));
        }
    }
    out
}
