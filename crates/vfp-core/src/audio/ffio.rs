//! FFmpeg pipes for audio: probe, decode to float samples, encode by file extension.
//!
//! `ffmpeg` and `ffprobe` must be on PATH (desktop). Phones will need another decoder.

use crate::error::{err, Error, Result};
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub sr: u32,
    pub channels: usize,
    pub duration: f64,
}

fn tool_missing(tool: &str, e: std::io::Error) -> Error {
    Error(format!("cannot run {tool}: {e}. Install FFmpeg and put it on PATH (or set VFP_FFMPEG and VFP_FFPROBE)."))
}

pub fn probe_audio(path: &Path) -> Result<AudioInfo> {
    let out = crate::tools::ffprobe()
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=sample_rate,channels,duration:format=duration", "-of", "json"])
        .arg(path)
        .output()
        .map_err(|e| tool_missing("ffprobe", e))?;
    if !out.status.success() {
        return err(format!("cannot read audio {}: {}", path.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    let j: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let s = j["streams"].get(0).ok_or_else(|| Error(format!("no audio stream in {}", path.display())))?;
    let num = |v: &serde_json::Value| v.as_str().and_then(|x| x.parse::<f64>().ok());
    let duration = num(&s["duration"]).or_else(|| num(&j["format"]["duration"])).ok_or_else(|| Error("unknown audio duration".into()))?;
    Ok(AudioInfo {
        sr: num(&s["sample_rate"]).unwrap_or(0.0) as u32,
        channels: s["channels"].as_u64().unwrap_or(0) as usize,
        duration,
    })
}

/// Float samples, one `Vec` per channel. Optionally resample (`sr`) or change channel count.
pub fn decode_audio(path: &Path, sr: Option<u32>, channels: Option<usize>) -> Result<Vec<Vec<f32>>> {
    let ch = match channels {
        Some(c) => c,
        None => probe_audio(path)?.channels,
    };
    let mut cmd = crate::tools::ffmpeg();
    cmd.args(["-v", "error", "-i"]).arg(path).args(["-vn", "-ac", &ch.to_string()]);
    if let Some(sr) = sr {
        cmd.args(["-ar", &sr.to_string()]);
    }
    cmd.args(["-f", "f32le", "-acodec", "pcm_f32le", "-"]);
    let out = cmd.output().map_err(|e| tool_missing("ffmpeg", e))?;
    if !out.status.success() {
        return err(format!("ffmpeg could not decode {}: {}", path.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    let n = out.stdout.len() / (4 * ch);
    let mut chans = vec![Vec::with_capacity(n); ch];
    for (i, b) in out.stdout[..n * ch * 4].chunks_exact(4).enumerate() {
        chans[i % ch].push(f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    }
    Ok(chans)
}

/// Output codec options by file extension.
fn encoder_args(ext: &str) -> Result<&'static [&'static str]> {
    Ok(match ext {
        "wav" => &["-c:a", "pcm_s24le"],
        "flac" => &["-c:a", "flac"],
        "mp3" => &["-c:a", "libmp3lame", "-b:a", "320k"],
        "m4a" => &["-c:a", "aac", "-b:a", "256k"],
        "ogg" => &["-c:a", "libopus", "-b:a", "160k"],
        other => return err(format!("cannot write audio as .{other}")),
    })
}

pub fn encode_audio(x: &[Vec<f32>], sr: u32, dst: &Path) -> Result<()> {
    let ext = dst.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
    let codec = encoder_args(&ext)?;
    let mut child = crate::tools::ffmpeg()
        .args(["-y", "-v", "error", "-f", "f32le", "-ar", &sr.to_string(), "-ac", &x.len().to_string(), "-i", "-"])
        .args(codec)
        .arg(dst)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| tool_missing("ffmpeg", e))?;
    let n = x.first().map_or(0, |c| c.len());
    let mut raw = Vec::with_capacity(n * x.len() * 4);
    for i in 0..n {
        for ch in x {
            raw.extend_from_slice(&ch[i].to_le_bytes());
        }
    }
    child.stdin.take().expect("piped stdin").write_all(&raw)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return err(format!("ffmpeg could not write {}: {}", dst.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}
