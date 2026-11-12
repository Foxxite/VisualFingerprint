//! High-level actions: register a file, mark copies for receivers, trace a leak.
//! The command line and the apps call these functions.
//!
//! Images and audio. Video returns a clear error until it is ported.

use crate::audio::common::{auto_nslots as audio_auto_nslots, AudioParams};
use crate::audio::ffio::probe_audio;
use crate::audio::{detect_audio, embed_audio};
use crate::error::{err, Result};
use crate::image::common::{usable_slots, ImageParams, DEFAULT_SLOTS};
use crate::image::detect::detect_image;
use crate::image::embed::embed_image;
use crate::imgio::{read_image, sha256_file, write_image};
use crate::tardos::{accusation_threshold, required_length, supported_colluders, TardosCode};
use crate::workspace::{Asset, Workspace};
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};

const IMAGE_EXT: [&str; 7] = ["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff"];
const VIDEO_EXT: [&str; 6] = ["mp4", "mov", "mkv", "webm", "avi", "m4v"];
const AUDIO_EXT: [&str; 7] = ["mp3", "wav", "flac", "m4a", "ogg", "opus", "aac"];

fn ext_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default()
}

pub fn kind_of(path: &Path) -> Result<&'static str> {
    let e = ext_of(path);
    if IMAGE_EXT.contains(&e.as_str()) {
        Ok("image")
    } else if VIDEO_EXT.contains(&e.as_str()) {
        Ok("video")
    } else if AUDIO_EXT.contains(&e.as_str()) {
        Ok("audio")
    } else {
        err(format!("unsupported file type: .{e}"))
    }
}

fn need_supported(kind: &str) -> Result<()> {
    if kind == "image" || kind == "audio" {
        Ok(())
    } else {
        err(format!("{kind} files are not supported by the Rust core yet (images and audio only). Use the Python version."))
    }
}

/// Python's `Path(name).stem`.
fn stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

/// Keep letters, digits, `_`, `.` and `-`. Any other run of characters becomes one `_`.
pub fn safe_name(s: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in s.chars() {
        if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
            if gap {
                out.push('_');
                gap = false;
            }
            out.push(c);
        } else {
            gap = true;
        }
    }
    if gap {
        out.push('_');
    }
    let t = out.trim_matches('_');
    if t.is_empty() { "receiver".to_string() } else { t.to_string() }
}

// ---------------------------------------------------------------- register
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub kind: String,
    pub m: usize,
    pub params: serde_json::Value,
    pub guaranteed_colluders: u64,
    pub notes: Vec<String>,
}

pub struct PlanOptions {
    pub max_receivers: u64,
    pub eps: f64,
    pub strength: Option<f32>,
    pub slots: Option<usize>,
    pub n_colluders: u64,
}

impl Default for PlanOptions {
    fn default() -> Self {
        PlanOptions { max_receivers: 1000, eps: 1e-3, strength: None, slots: None, n_colluders: 3 }
    }
}

/// Decide how the file will carry the code. `slots` means symbols per image.
pub fn plan_file(path: &Path, o: &PlanOptions) -> Result<Plan> {
    let kind = kind_of(path)?;
    need_supported(kind)?;
    let mut notes = vec![];
    let target = required_length(o.max_receivers, o.n_colluders, o.eps) as usize;
    let (params, m) = if kind == "image" {
        let img = read_image(path)?;
        let want = o.slots.unwrap_or(DEFAULT_SLOTS.max(target));
        let n = usable_slots(img.h, img.w, want);
        if n < want {
            notes.push(format!(
                "Image ({}x{}) holds {} symbols (wanted {}). {}",
                img.w, img.h, n, want, if n < target { "Tracing is weaker." } else { "" }
            ));
        }
        let strength = o.strength.unwrap_or(ImageParams::default().strength);
        (json!({"nslots": n, "strength": strength}), n)
    } else {
        let mut p = AudioParams::default();
        if let Some(s) = o.strength {
            p.alpha = s as f64;
        }
        let dur = probe_audio(path)?.duration;
        let n = o.slots.unwrap_or_else(|| audio_auto_nslots(dur, &p, target));
        p.nslots = n;
        let m = p.capacity(dur);
        if n > AudioParams::default().nslots {
            notes.push(format!(
                "Short audio: packing {n} symbols per segment ({:.0} per second) to reach {m} symbols.",
                n as f64 / p.seg_seconds
            ));
        }
        (serde_json::to_value(&p)?, m)
    };
    let g = supported_colluders(m as u64, o.max_receivers, o.eps);
    if g == 0 {
        notes.push(
            "Enough for one leaker. The theory does not guarantee tracing a group of colluders with this many \
             symbols. (The theory is cautious; tests on real files did better.) Use a longer or larger file, \
             fewer receivers, or a looser false-accusation rate for a stronger guarantee."
                .into(),
        );
    }
    Ok(Plan { kind: kind.into(), m, params, guaranteed_colluders: g, notes })
}

fn audio_params(a: &Asset) -> AudioParams {
    serde_json::from_value(a.params.clone()).unwrap_or_default()
}

fn image_params(a: &Asset) -> ImageParams {
    ImageParams {
        nslots: a.params["nslots"].as_u64().unwrap_or(256) as usize,
        strength: a.params["strength"].as_f64().unwrap_or(3.0) as f32,
    }
}

pub fn add_asset(ws: &Workspace, path: &Path, name: Option<&str>, o: &PlanOptions) -> Result<(Asset, Plan)> {
    let plan = plan_file(path, o)?;
    let default_name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let name = name.unwrap_or(&default_name);
    let aid = ws.add_asset(name, &plan.kind, path, &sha256_file(path)?, &plan.params, plan.m, o.max_receivers, o.n_colluders, o.eps)?;
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let dest = ws.home.join("originals").join(format!("{aid}_{file}"));
    std::fs::copy(path, &dest)?;
    ws.set_asset_path(aid, &dest)?;
    Ok((ws.asset(&aid.to_string())?, plan))
}

// ---------------------------------------------------------------- mark
fn code_for(ws: &Workspace, a: &Asset) -> TardosCode {
    TardosCode::create(a.m, ws.tardos_seed(a.id), a.n_colluders)
}

/// Write one marked copy for one receiver. Returns the output path.
pub fn mark_for_receiver(ws: &Workspace, a: &Asset, receiver: &str, outdir: &Path) -> Result<PathBuf> {
    need_supported(&a.kind)?;
    std::fs::create_dir_all(outdir)?;
    let rec = ws.receiver(receiver)?;
    let ui = ws.issue(a.id, rec.id, None)?;
    let word = code_for(ws, a).codeword(ui);
    let mut ext = ext_of(&a.orig_path);
    if a.kind == "image" && ext == "webp" {
        ext = "png".into(); // keep the copy lossless
    }
    let out = outdir.join(format!("{}__{}.{}", safe_name(stem(&a.name)), safe_name(&rec.name), ext));
    if a.kind == "image" {
        let marked = embed_image(&read_image(&a.orig_path)?, &word, ws.layout_seed(a.id), &image_params(a))?;
        write_image(&out, &marked)?;
    } else {
        embed_audio(&a.orig_path, &out, &word, ws.layout_seed(a.id), &audio_params(a))?;
    }
    ws.issue(a.id, rec.id, Some(&out))?;
    Ok(out)
}

// ---------------------------------------------------------------- trace
#[derive(Debug, Clone, Serialize)]
pub struct TraceResult {
    pub asset: String,
    pub kind: String,
    pub ranking: Vec<(String, f64)>,
    pub threshold: f64,
    pub flagged: Vec<String>,
    pub verdict: String,
    pub warnings: Vec<String>,
    pub evidence: serde_json::Value,
}

/// Plain-text report to keep with the leaked file.
pub fn report_text(r: &TraceResult, leaked: &Path) -> Result<String> {
    let mut lines = vec![
        "VisualFingerprint trace report".to_string(),
        format!("Date:            {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")),
        format!("Leaked file:     {}", leaked.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
        format!("SHA-256:         {}", sha256_file(leaked)?),
        format!("Matched asset:   {} ({})", r.asset, r.kind),
        format!("Threshold:       {:.1} (score a receiver must beat to be accused)", r.threshold),
        format!("Evidence:        {}", r.evidence),
        String::new(),
        format!("RESULT: {}", r.verdict),
    ];
    lines.extend(r.warnings.iter().map(|w| format!("WARNING: {w}")));
    lines.push(String::new());
    lines.push("Rank  Receiver                        Score".into());
    for (i, (n, s)) in r.ranking.iter().enumerate() {
        let mark = if *s > r.threshold { "   above threshold" } else { "" };
        lines.push(format!("{:>4}  {:<30}{:>8.1}{}", i + 1, n, s, mark));
    }
    lines.push(String::new());
    lines.push("A score is statistical evidence, not proof. Keep this report with the leaked file.".into());
    Ok(lines.join("\n"))
}

/// Score a leaked file against one asset, or find the matching asset by trying each.
pub fn trace_leak(ws: &Workspace, leaked: &Path, asset: Option<&str>) -> Result<TraceResult> {
    let kind = kind_of(leaked)?;
    if let Some(a) = asset {
        return trace_one(ws, &ws.asset(a)?, leaked);
    }
    let mut best: Option<(f64, TraceResult)> = None;
    for a in ws.assets()? {
        if a.kind != kind || ws.issues(a.id)?.is_empty() {
            continue;
        }
        let r = trace_one(ws, &a, leaked)?;
        let strength = r.ranking.first().map_or(0.0, |x| x.1 / r.threshold);
        if best.as_ref().is_none_or(|b| strength > b.0) {
            best = Some((strength, r));
        }
    }
    best.map(|b| b.1).ok_or_else(|| crate::error::Error(format!("no registered {kind} assets with issued copies")))
}

fn trace_one(ws: &Workspace, a: &Asset, leaked: &Path) -> Result<TraceResult> {
    need_supported(&a.kind)?;
    let issued = ws.issues(a.id)?;
    if issued.is_empty() {
        return err(format!("asset '{}' has no issued copies", a.name));
    }
    let mut warnings = vec![];
    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    let (bits, confidence, cover, evidence) = if a.kind == "image" {
        let (det, reg) = detect_image(&read_image(leaked)?, &read_image(&a.orig_path)?, ws.layout_seed(a.id), &image_params(a));
        if det.valid_fraction < 0.3 {
            warnings.push("Less than 30% of the leaked image overlaps the original.".to_string());
        }
        let ev = json!({"alignment": reg.method, "overlap": r2(det.valid_fraction)});
        (det.bits, det.confidence, det.valid_fraction, ev)
    } else {
        let det = detect_audio(leaked, &a.orig_path, ws.layout_seed(a.id), &audio_params(a), a.m)?;
        if det.align < 0.5 {
            warnings.push(
                "Audio alignment is weak. The file may have been sped up, slowed down, or changed too much.".into(),
            );
        }
        let ev = json!({"offset_samples": det.offset, "alignment_score": r2(det.align), "symbols_with_evidence": r2(det.coverage)});
        (det.bits, det.confidence, det.coverage, ev)
    };

    let code = code_for(ws, a);
    let thr = accusation_threshold(a.max_receivers, a.m, a.eps, Some(&confidence));
    let mut ranking: Vec<(String, f64)> = issued
        .iter()
        .map(|i| (i.name.clone(), code.score(i.user_index, &bits, Some(&confidence))))
        .collect();
    ranking.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    let flagged: Vec<String> = ranking.iter().filter(|(_, s)| *s > thr).map(|(n, _)| n.clone()).collect();
    if flagged.is_empty() && cover < 0.5 {
        warnings.push(
            "Less than half of the code symbols could be read (the copy may be a short excerpt or heavily \
             damaged). This can explain the weak result."
                .into(),
        );
    }
    let verdict = match flagged.len() {
        0 => {
            let mut v = "INCONCLUSIVE: no receiver is above the accusation threshold.".to_string();
            if let Some((n, s)) = ranking.first().filter(|x| x.1 > 0.0) {
                v += &format!(" Leading candidate: {n} (score {s:.0}, threshold {thr:.0}). This is a lead, not grounds to accuse.");
            }
            v
        }
        1 => format!("LEAKER: {}", flagged[0]),
        _ => format!("POSSIBLE COLLUSION: {}", flagged.join(", ")),
    };
    Ok(TraceResult { asset: a.name.clone(), kind: a.kind.clone(), ranking, threshold: thr, flagged, verdict, warnings, evidence })
}
