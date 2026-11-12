//! FFmpeg / ffprobe pipes: probe, decode raw frames, encode raw frames.
//!
//! `ffmpeg` and `ffprobe` must be on PATH (desktop). Phones will need another route.

use crate::error::{err, Error, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub path: PathBuf,
    /// Even, after rotation.
    pub width: usize,
    pub height: usize,
    /// Frame rate as a reduced fraction.
    pub fps_num: u64,
    pub fps_den: u64,
    /// Estimate when the container has no count.
    pub nframes: usize,
    pub duration: f64,
    pub has_audio: bool,
    pub audio_codec: Option<String>,
    /// ffmpeg option name -> value (colorspace, color_primaries, color_trc, color_range).
    pub color: Vec<(String, String)>,
}

impl VideoInfo {
    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den as f64
    }

    /// The frame rate as ffmpeg writes it ("30" or "30000/1001").
    pub fn fps_arg(&self) -> String {
        if self.fps_den == 1 { self.fps_num.to_string() } else { format!("{}/{}", self.fps_num, self.fps_den) }
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn parse_fraction(s: &str) -> Option<(u64, u64)> {
    let (n, d) = match s.split_once('/') {
        Some((n, d)) => (n.trim().parse::<u64>().ok()?, d.trim().parse::<u64>().ok()?),
        None => (s.trim().parse::<u64>().ok()?, 1),
    };
    if d == 0 || n == 0 {
        return None;
    }
    let g = gcd(n, d);
    Some((n / g, d / g))
}

fn tool_missing(tool: &str, e: std::io::Error) -> Error {
    Error(format!("cannot run {tool}: {e}. Install FFmpeg and put it on PATH."))
}

pub fn probe(path: &Path) -> Result<VideoInfo> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-show_format", "-of", "json"])
        .arg(path)
        .output()
        .map_err(|e| tool_missing("ffprobe", e))?;
    if !out.status.success() {
        return err(format!("cannot read video {}: {}", path.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    let j: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let streams = j["streams"].as_array().cloned().unwrap_or_default();
    let v = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .ok_or_else(|| Error(format!("no video stream in {}", path.display())))?;
    let a = streams.iter().find(|s| s["codec_type"] == "audio");
    let (mut w, mut h) = (v["width"].as_u64().unwrap_or(0) as usize, v["height"].as_u64().unwrap_or(0) as usize);
    let mut rot = 0i64;
    for sd in v["side_data_list"].as_array().into_iter().flatten() {
        if let Some(r) = sd["rotation"].as_i64().or_else(|| sd["rotation"].as_f64().map(|f| f as i64)) {
            rot = r;
        }
    }
    if rot.abs() % 180 == 90 {
        std::mem::swap(&mut w, &mut h);
    }
    let fps_str = |k: &str| v[k].as_str().and_then(parse_fraction);
    let (fps_num, fps_den) = fps_str("avg_frame_rate")
        .or_else(|| fps_str("r_frame_rate"))
        .ok_or_else(|| Error("unknown frame rate".into()))?;
    let fps = fps_num as f64 / fps_den as f64;
    let num = |x: &serde_json::Value| x.as_str().and_then(|s| s.parse::<f64>().ok());
    let mut dur = num(&v["duration"]).or_else(|| num(&j["format"]["duration"]));
    let n = match v["nb_frames"].as_str().and_then(|s| s.parse::<usize>().ok()) {
        Some(n) => {
            dur = dur.or(Some(n as f64 / fps));
            n
        }
        None => (dur.unwrap_or(0.0) * fps).round_ties_even() as usize,
    };
    let mut color = vec![];
    for (key, opt) in [("color_space", "colorspace"), ("color_primaries", "color_primaries"), ("color_transfer", "color_trc"), ("color_range", "color_range")] {
        if let Some(val) = v[key].as_str().filter(|s| *s != "unknown") {
            color.push((opt.to_string(), val.to_string()));
        }
    }
    Ok(VideoInfo {
        path: path.to_path_buf(),
        width: w - w % 2,
        height: h - h % 2,
        fps_num,
        fps_den,
        nframes: n,
        duration: dur.unwrap_or(0.0),
        has_audio: a.is_some(),
        audio_codec: a.and_then(|s| s["codec_name"].as_str().map(str::to_string)),
        color,
    })
}

/// One decoded frame: luma (h x w) and the two half-size chroma planes.
pub struct Frame {
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// Streams frames out of ffmpeg. Dropping it stops ffmpeg.
pub struct FrameReader {
    child: Child,
    out: ChildStdout,
    sizes: (usize, usize),
}

impl Iterator for FrameReader {
    type Item = Frame;
    fn next(&mut self) -> Option<Frame> {
        let (ysz, csz) = self.sizes;
        let mut raw = vec![0u8; ysz + 2 * csz];
        self.out.read_exact(&mut raw).ok()?;
        let v = raw.split_off(ysz + csz);
        let u = raw.split_off(ysz);
        Some(Frame { y: raw, u, v })
    }
}

impl Drop for FrameReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Decode frames as yuv420p. `start_seconds` seeks; `max_frames` stops early.
pub fn decode_yuv(info: &VideoInfo, max_seconds: Option<f64>, start_seconds: f64, max_frames: Option<usize>) -> Result<FrameReader> {
    let (w, h) = (info.width, info.height);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error"]);
    if start_seconds > 0.0 {
        cmd.args(["-ss", &format!("{start_seconds:.6}")]);
    }
    cmd.arg("-i").arg(&info.path);
    if let Some(s) = max_seconds {
        cmd.args(["-t", &s.to_string()]);
    }
    if let Some(n) = max_frames {
        cmd.args(["-frames:v", &n.to_string()]);
    }
    cmd.args(["-an", "-vf", &format!("crop={w}:{h}:0:0"), "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"]);
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| tool_missing("ffmpeg", e))?;
    let out = child.stdout.take().expect("piped stdout");
    Ok(FrameReader { child, out, sizes: (w * h, (w / 2) * (h / 2)) })
}

/// All frames as small gray thumbnails: `n` rows of `tw*th` values. For time sync.
pub fn decode_thumbs(info: &VideoInfo, size: (usize, usize)) -> Result<Vec<Vec<f32>>> {
    let (tw, th) = size;
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&info.path)
        .args(["-an", "-vf", &format!("scale={tw}:{th}:flags=area"), "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .output()
        .map_err(|e| tool_missing("ffmpeg", e))?;
    if !out.status.success() {
        return err(format!("ffmpeg could not read {}: {}", info.path.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout.chunks_exact(tw * th).map(|c| c.iter().map(|&b| b as f32).collect()).collect())
}

/// Pipe raw yuv420p frames into x264. Copies (or re-encodes) the audio of `audio_from`.
pub struct Encoder {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Encoder {
    pub fn new(dst: &Path, info: &VideoInfo, crf: i32, preset: &str, audio_from: Option<&Path>, max_seconds: Option<f64>) -> Result<Self> {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-y", "-v", "error", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-s", &format!("{}x{}", info.width, info.height), "-r", &info.fps_arg(), "-i", "-"]);
        let use_audio = audio_from.is_some() && info.has_audio;
        if use_audio {
            cmd.arg("-i").arg(audio_from.unwrap()).args(["-map", "0:v", "-map", "1:a:0"]);
        }
        cmd.args(["-c:v", "libx264", "-crf", &crf.to_string(), "-preset", preset, "-pix_fmt", "yuv420p"]);
        for (k, v) in &info.color {
            cmd.arg(format!("-{k}")).arg(v);
        }
        if use_audio {
            let copy_ok = matches!(info.audio_codec.as_deref(), Some("aac" | "mp3" | "ac3"));
            if copy_ok {
                cmd.args(["-c:a", "copy"]);
            } else {
                cmd.args(["-c:a", "aac", "-b:a", "192k"]);
            }
        }
        if let Some(s) = max_seconds {
            cmd.args(["-t", &s.to_string()]);
        }
        cmd.args(["-movflags", "+faststart"]).arg(dst);
        let mut child = cmd.stdin(Stdio::piped()).spawn().map_err(|e| tool_missing("ffmpeg", e))?;
        let stdin = child.stdin.take();
        Ok(Encoder { child, stdin })
    }

    pub fn write(&mut self, y: &[u8], u: &[u8], v: &[u8]) -> Result<()> {
        let s = self.stdin.as_mut().expect("encoder is open");
        for part in [y, u, v] {
            s.write_all(part).map_err(|e| Error(format!("ffmpeg stopped while writing frames: {e}")))?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        drop(self.stdin.take());
        let status = self.child.wait()?;
        if !status.success() {
            return err(format!("ffmpeg encode failed ({status})"));
        }
        Ok(())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            drop(self.stdin.take());
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
