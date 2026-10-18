//! Thumbnails for every platform from a video's best moment.
//!
//! [`best_moment`] samples frames across the video with FFmpeg and scores each for sharpness,
//! colour, contrast and exposure (skipping the opening and the end card), or looks only near a
//! suggested time. [`make`] takes that frame, enhances it (contrast, colour, sharpening) and
//! writes one JPEG per [`Size`]: where the frame's shape differs, a blurred, darkened copy of it
//! fills the background, and an optional headline is set in bold type. [`ensure`] keeps a set
//! for each library render (its moment beside them in `cover-<render>.json`), which publishing
//! uploads: YouTube gets the image, Instagram the moment (`thumb_offset`).

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Result, Studio, StudioError, err};

/// The shapes social platforms show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Size {
    /// 1280×720: YouTube videos, Facebook and LinkedIn link previews.
    Landscape,
    /// 1080×1920: Shorts, Reels, TikTok, Stories.
    Vertical,
    /// 1080×1350: Instagram and Facebook feed.
    Portrait,
    /// 1080×1080: X, LinkedIn, Facebook square.
    Square,
}

impl Size {
    pub const ALL: [Size; 4] = [Size::Landscape, Size::Vertical, Size::Portrait, Size::Square];
    pub fn pixels(self) -> (u32, u32) {
        match self {
            Size::Landscape => (1280, 720),
            Size::Vertical => (1080, 1920),
            Size::Portrait => (1080, 1350),
            Size::Square => (1080, 1080),
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Size::Landscape => "landscape",
            Size::Vertical => "vertical",
            Size::Portrait => "portrait",
            Size::Square => "square",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Size::Landscape => "YouTube 16:9",
            Size::Vertical => "Shorts / Reels / TikTok 9:16",
            Size::Portrait => "Feed 4:5",
            Size::Square => "Square 1:1",
        }
    }
}

/// A render's thumbnails: the moment (seconds into the video) and a file per size.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Covers {
    pub at: f64,
    pub headline: String,
    pub files: Vec<(Size, PathBuf)>,
}

impl Covers {
    pub fn file(&self, size: Size) -> Option<&Path> {
        self.files.iter().find(|(s, f)| *s == size && f.is_file()).map(|(_, f)| f.as_path())
    }
}

fn ffmpeg(studio: &Studio) -> Result<PathBuf> {
    crate::ytdlp::ffmpeg(studio).ok_or_else(|| StudioError::Other("FFmpeg is needed for thumbnails (Plugin Settings ▸ Downloads & tools)".into()))
}

fn quiet(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

/// The video's length, from FFmpeg's report.
pub fn duration(studio: &Studio, video: &Path) -> Result<f64> {
    let o = quiet(Command::new(ffmpeg(studio)?).args(["-hide_banner", "-i"]).arg(video)).output()?;
    let text = String::from_utf8_lossy(&o.stderr);
    let d = text.split("Duration: ").nth(1).and_then(|r| r.split(',').next()).unwrap_or("");
    let parts: Vec<f64> = d.trim().split(':').filter_map(|p| p.parse().ok()).collect();
    match parts.as_slice() {
        [h, m, s] => Ok(h * 3600.0 + m * 60.0 + s),
        _ => err("FFmpeg could not read the video's length"),
    }
}

/// Side of the square each sampled frame is scaled to for scoring.
const SAMPLE: usize = 128;

/// How good a frame looks as a thumbnail: sharp, colourful, contrasty, well exposed. `rgb` is a
/// SAMPLE×SAMPLE RGB24 frame. Unnormalised parts: (sharpness, colourfulness, contrast, exposure 0..1).
pub fn frame_score(rgb: &[u8]) -> (f64, f64, f64, f64) {
    let n = SAMPLE * SAMPLE;
    if rgb.len() < n * 3 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mut luma = vec![0f64; n];
    let (mut rg_s, mut rg_q, mut yb_s, mut yb_q) = (0.0, 0.0, 0.0, 0.0);
    for i in 0..n {
        let (r, g, b) = (f64::from(rgb[i * 3]), f64::from(rgb[i * 3 + 1]), f64::from(rgb[i * 3 + 2]));
        luma[i] = 0.299 * r + 0.587 * g + 0.114 * b;
        let (rg, yb) = (r - g, 0.5 * (r + g) - b);
        rg_s += rg;
        rg_q += rg * rg;
        yb_s += yb;
        yb_q += yb * yb;
    }
    let nf = n as f64;
    let (rg_m, yb_m) = (rg_s / nf, yb_s / nf);
    let colour = ((rg_q / nf - rg_m * rg_m).max(0.0) + (yb_q / nf - yb_m * yb_m).max(0.0)).sqrt() + 0.3 * (rg_m * rg_m + yb_m * yb_m).sqrt();
    let mean = luma.iter().sum::<f64>() / nf;
    let contrast = (luma.iter().map(|y| (y - mean).powi(2)).sum::<f64>() / nf).sqrt();
    // Variance of the Laplacian: edges in focus.
    let mut lap = Vec::with_capacity(n);
    for y in 1..SAMPLE - 1 {
        for x in 1..SAMPLE - 1 {
            let c = y * SAMPLE + x;
            lap.push(4.0 * luma[c] - luma[c - 1] - luma[c + 1] - luma[c - SAMPLE] - luma[c + SAMPLE]);
        }
    }
    let lm = lap.iter().sum::<f64>() / lap.len() as f64;
    let sharp = lap.iter().map(|v| (v - lm).powi(2)).sum::<f64>() / lap.len() as f64;
    // Best between dark and blown out.
    let exposure = (1.0 - ((mean - 125.0).abs() / 125.0)).clamp(0.0, 1.0);
    (sharp, colour, contrast, exposure)
}

/// Picks the best of scored frames: each part relative to the best frame's, then weighted.
pub fn pick(scores: &[(f64, (f64, f64, f64, f64))]) -> Option<f64> {
    let max = |f: fn(&(f64, f64, f64, f64)) -> f64| scores.iter().map(|(_, s)| f(s)).fold(1e-9, f64::max);
    let (ms, mc, mk) = (max(|s| s.0), max(|s| s.1), max(|s| s.2));
    scores
        .iter()
        .filter(|(_, s)| s.3 > 0.15)
        .map(|(t, s)| (*t, 0.40 * (s.0 / ms).sqrt() + 0.30 * s.1 / mc + 0.15 * s.2 / mk + 0.15 * s.3))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, _)| t)
}

/// The best moment for a thumbnail, in seconds. With `near`, only around that time (±4 s).
pub fn best_moment(studio: &Studio, video: &Path, near: Option<f64>) -> Result<f64> {
    let total = duration(studio, video)?;
    let (from, to) = match near {
        Some(t) => ((t - 4.0).max(0.0), (t + 4.0).min(total)),
        // Past the opening (titles animate in) and before the end card.
        None => ((total * 0.06).max(0.8).min(total * 0.5), total * 0.9),
    };
    let span = (to - from).max(0.5);
    let count = if near.is_some() { 16.0 } else { (span / 1.5).clamp(12.0, 48.0) };
    let o = quiet(
        Command::new(ffmpeg(studio)?)
            .args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{from:.3}"), "-t", &format!("{span:.3}"), "-i"])
            .arg(video)
            .args(["-an", "-vf", &format!("fps={:.6},scale={SAMPLE}:{SAMPLE}", count / span), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]),
    )
    .output()?;
    let frame = SAMPLE * SAMPLE * 3;
    let scores: Vec<(f64, (f64, f64, f64, f64))> =
        o.stdout.chunks_exact(frame).enumerate().map(|(i, f)| (from + (i as f64 + 0.5) * span / count, frame_score(f))).collect();
    pick(&scores).map(|t| t.clamp(0.0, (total - 0.05).max(0.0))).ok_or_else(|| StudioError::Other("no usable frame was found for a thumbnail".into()))
}

/// The bold font for headlines, written once beside the app's tools.
fn font(studio: &Studio) -> Result<PathBuf> {
    static INTER_BOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-Bold.ttf");
    let path = crate::ytdlp::tools_dir(studio).join("Inter-Bold.ttf");
    if std::fs::metadata(&path).map(|m| m.len()).ok() != Some(INTER_BOLD.len() as u64) {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        std::fs::write(&path, INTER_BOLD)?;
    }
    Ok(path)
}

/// Wraps a headline into at most three lines of about `width` characters.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + word.chars().count() <= width => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    if lines.len() > 3 {
        lines.truncate(3);
        if let Some(l) = lines.last_mut() {
            l.push('…');
        }
    }
    lines
}

/// The filter for one size: fitted frame over its own blurred fill, then the headline.
/// `lines` are text files and the font is `Inter-Bold.ttf`, relative to FFmpeg's working folder
/// (the tools folder), so nothing in the filter needs escaping.
fn size_filter(input: &str, size: Size, lines: &[String], out: &str) -> String {
    let (w, h) = size.pixels();
    let i = size.id();
    let mut f = format!(
        "[{input}]split[a{i}][b{i}];[a{i}]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},boxblur=32:5,eq=brightness=-0.24:saturation=1.35[bg{i}];\
         [b{i}]scale={w}:{h}:force_original_aspect_ratio=decrease[fg{i}];[bg{i}][fg{i}]overlay=(main_w-overlay_w)/2:(main_h-overlay_h)/2"
    );
    if !lines.is_empty() {
        // A dark band behind bold white type with an outline, in the lower part.
        let size_px = (f64::from(w.min(h)) * if size == Size::Landscape { 0.11 } else { 0.085 }).round() as u32;
        let line_h = (f64::from(size_px) * 1.18).round() as u32;
        let block = line_h * lines.len() as u32;
        let top = h.saturating_sub(block + h / 12);
        f.push_str(&format!(",drawbox=x=0:y={}:w=iw:h={}:color=black@0.45:t=fill", top.saturating_sub(line_h / 3), block + line_h * 2 / 3));
        for (n, line) in lines.iter().enumerate() {
            f.push_str(&format!(
                ",drawtext=fontfile=Inter-Bold.ttf:textfile={line}:fontsize={size_px}:fontcolor=white:borderw={}:bordercolor=black@0.85:x=(w-text_w)/2:y={}",
                (size_px / 14).max(2),
                top + line_h * n as u32
            ));
        }
    }
    f.push_str(&format!("[{out}]"));
    f
}

/// Makes the thumbnails of `video` at `at` seconds into `dir`, named `<stem>-<size>.jpg`; a
/// headline is set on each when given. Blocking (one FFmpeg run).
pub fn make(studio: &Studio, video: &Path, at: f64, dir: &Path, stem: &str, headline: &str, sizes: &[Size]) -> Result<Vec<(Size, PathBuf)>> {
    if sizes.is_empty() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(dir)?;
    let headline = headline.trim();
    let tools = crate::ytdlp::tools_dir(studio);
    // The headline's lines as text files in a scratch folder of the tools folder.
    let scratch = format!("thumbtext-{}-{}", std::process::id(), crate::clock::now_ms());
    if !headline.is_empty() {
        font(studio)?;
        std::fs::create_dir_all(tools.join(&scratch))?;
    }
    let mut written = std::collections::HashMap::new();
    let mut graph = format!(
        "[0:v]eq=contrast=1.08:saturation=1.22:brightness=0.015,unsharp=5:5:0.6,split={}{}",
        sizes.len(),
        sizes.iter().map(|s| format!("[s{}]", s.id())).collect::<String>()
    );
    for s in sizes {
        let width = if *s == Size::Landscape { 22 } else { 16 };
        let mut files = Vec::new();
        if !headline.is_empty() {
            for (n, line) in wrap(headline, width).into_iter().enumerate() {
                let name = format!("{scratch}/{width}-{n}.txt");
                if !written.contains_key(&name) {
                    std::fs::write(tools.join(&name), &line)?;
                    written.insert(name.clone(), ());
                }
                files.push(name);
            }
        }
        graph.push(';');
        graph.push_str(&size_filter(&format!("s{}", s.id()), *s, &files, &format!("o{}", s.id())));
    }
    let mut c = Command::new(ffmpeg(studio)?);
    c.args(["-y", "-hide_banner", "-loglevel", "error", "-ss", &format!("{:.3}", at.max(0.0)), "-i"]).arg(video).args(["-filter_complex", &graph]);
    let mut out = Vec::new();
    for s in sizes {
        let file = dir.join(format!("{stem}-{}.jpg", s.id()));
        c.args(["-map", &format!("[o{}]", s.id()), "-frames:v", "1", "-q:v", "3"]).arg(&file);
        out.push((*s, file));
    }
    std::fs::create_dir_all(&tools)?;
    let o = quiet(c.current_dir(&tools)).output();
    let _ = std::fs::remove_dir_all(tools.join(&scratch));
    let o = o?;
    if !o.status.success() || out.iter().any(|(_, f)| !f.is_file()) {
        return err(format!("FFmpeg could not make the thumbnails: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("")));
    }
    // YouTube takes at most 2 MB.
    for (_, f) in &out {
        if std::fs::metadata(f).map(|m| m.len()).unwrap_or(0) > 2_000_000 {
            let tmp = f.with_extension("small.jpg");
            let r = quiet(Command::new(ffmpeg(studio)?).args(["-y", "-hide_banner", "-loglevel", "error", "-i"]).arg(f).args(["-q:v", "7"]).arg(&tmp)).output()?;
            if r.status.success() {
                std::fs::rename(&tmp, f)?;
            }
        }
    }
    Ok(out)
}

/// Where a render's thumbnails live: beside its video.
fn render_files(studio: &Studio, render: i64) -> Result<(PathBuf, PathBuf, String)> {
    let rel: String = studio
        .connect()?
        .query_row("SELECT file_path FROM renders WHERE id = ?1", [render], |r| r.get::<_, Option<String>>(0))?
        .ok_or_else(|| StudioError::Other("the render has no video file".into()))?;
    let video = studio.resolve_storage(&rel)?;
    let dir = video.parent().map(Path::to_path_buf).unwrap_or_default().join("thumbnails");
    Ok((video, dir, format!("cover-{render}")))
}

/// A render's thumbnails, if made.
pub fn covers(studio: &Studio, render: i64) -> Option<Covers> {
    let (_, dir, stem) = render_files(studio, render).ok()?;
    let mut c: Covers = std::fs::read_to_string(dir.join(format!("{stem}.json"))).ok().and_then(|t| serde_json::from_str(&t).ok())?;
    c.files.retain(|(_, f)| f.is_file());
    (!c.files.is_empty()).then_some(c)
}

/// Makes a render's thumbnails at `at` (the best moment when `None`), with `headline` (empty for
/// none), replacing earlier ones, and records them as the render's thumbnail.
pub fn make_for_render(studio: &Studio, render: i64, at: Option<f64>, headline: &str) -> Result<Covers> {
    let (video, dir, stem) = render_files(studio, render)?;
    let at = match at {
        Some(t) => t,
        None => best_moment(studio, &video, None)?,
    };
    let files = make(studio, &video, at, &dir, &stem, headline, &Size::ALL)?;
    let covers = Covers { at, headline: headline.trim().to_string(), files };
    std::fs::write(dir.join(format!("{stem}.json")), serde_json::to_string(&covers).map_err(|e| StudioError::Other(e.to_string()))?)?;
    if let Some(v) = covers.file(Size::Vertical).or_else(|| covers.files.first().map(|(_, f)| f.as_path()))
        && let Ok(rel) = studio.relative_storage(v)
    {
        studio.connect()?.execute("UPDATE renders SET thumbnail_path = ?1 WHERE id = ?2", rusqlite::params![rel, render])?;
    }
    Ok(covers)
}

/// The render's thumbnails, made at its best moment the first time. A long video (a library
/// post of kind VIDEO, which has no headline drawn on it) gets its headline set on them.
pub fn ensure(studio: &Studio, render: i64) -> Result<Covers> {
    if let Some(c) = covers(studio, render) {
        return Ok(c);
    }
    let (_, fields) = studio.fields_for(render)?;
    let headline = if crate::long_video(studio, render) { fields.title } else { String::new() };
    make_for_render(studio, render, None, &headline)
}

/// For agents: the shape of [`Covers`].
pub fn describe(c: &Covers) -> serde_json::Value {
    json!({"at": c.at, "headline": c.headline, "files": c.files.iter().map(|(s, f)| json!({"size": s.id(), "label": s.label(), "file": f})).collect::<Vec<_>>()})
}
