//! The local virality analyzer: viral-moment detection, scoring, suggested hooks and automatic
//! timestamps for a video, on this PC.
//!
//! 1. **Transcript** – the video's own captions (from yt-dlp's details) or local Whisper
//!    (FilmCraft's built-in speech recogniser, word timestamps).
//! 2. **Signals** – FFmpeg reads the loudness every half second and the scene cuts.
//! 3. **Candidates** – windows of the asked-for length that start and end on sentences, scored
//!    from speech pace, loudness peaks, cuts and hook words, the best few kept apart.
//! 4. **Vision** – a 2×2 grid of frames of each candidate goes to a vision-language model (a
//!    local Ollama one, e.g. gemma3, or one on NVIDIA NIM), which rates how strong the pictures
//!    are and where the main subject is (for the 9:16 crop).
//! 5. **Ranking** – the workspace's AI (Hermes, Codex, Ollama or NVIDIA) reads all of it and
//!    returns the moments with a 0–100 virality score, its parts, three hooks, the hook moment
//!    and the ranges to keep; without an AI the signal scores rank them.
//!
//! The result is [`autopilot::Segment`]s, so the AI panel reviews them and makes the Reels
//! (into the editor, captions and GPU export as before). A virality score is a prediction, not
//! a guarantee: [`results`] compares the scores of published posts with their real numbers.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::autopilot::{Piece, Segment, Wants};
use crate::{Result, Studio, StudioError, err};

/// A moment's score parts, 0–10 each.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Scores {
    /// How strongly the first seconds stop the scroll.
    pub hook: f64,
    /// Surprise, humour, tension, awe.
    pub emotion: f64,
    /// Makes sense on its own, without the rest of the video.
    pub clarity: f64,
    /// Strong, readable pictures.
    pub visual: f64,
    /// Would people send it on or comment.
    pub share: f64,
}

/// A candidate window before ranking.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Candidate {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// 0–1 from the signals.
    pub signal: f64,
    /// The loudest second (a hook guess).
    pub peak: f64,
    pub visual: Option<Vision>,
    /// Shot changes inside it (seconds), for cutting on shots.
    pub shots: Vec<f64>,
}

/// What the vision model saw.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Vision {
    /// 0–10.
    pub visual: f64,
    /// The main subject's horizontal centre, 0 (left) to 1 (right).
    pub subject_x: f64,
    pub what: String,
}

/// What [`analyze`] found.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Analysis {
    pub duration: f64,
    /// "captions", "whisper" or "none".
    pub transcript: String,
    /// The vision model used, or empty.
    pub vision: String,
    /// The AI that ranked the moments, or "signals".
    pub ranked_by: String,
    pub segments: Vec<Segment>,
    /// Why a step was skipped (no Whisper model, no vision model…), for the review.
    pub notes: Vec<String>,
}

fn ffmpeg(studio: &Studio) -> Result<std::path::PathBuf> {
    crate::ytdlp::ffmpeg(studio).ok_or_else(|| StudioError::Other("FFmpeg is needed for the analyzer (Plugin Settings ▸ Downloads & tools)".into()))
}

fn quiet(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

/// Words grouped into lines of about five seconds (sentence ends and pauses break them).
pub fn lines_from_words(words: &[(f64, f64, String)]) -> Vec<(f64, String)> {
    let mut out: Vec<(f64, String)> = Vec::new();
    let mut last_end = f64::NEG_INFINITY;
    for (start, end, text) in words {
        let gap = start - last_end;
        let close = out.last().is_some_and(|(s, l)| start - s < 5.0 && gap < 0.8 && !l.ends_with(['.', '?', '!']));
        match out.last_mut() {
            Some((_, l)) if close => {
                l.push(' ');
                l.push_str(text);
            }
            _ => out.push((*start, text.clone())),
        }
        last_end = *end;
    }
    out
}

/// Local Whisper (FilmCraft's speech recogniser): the video's words in lines. Needs a build with
/// speech and a downloaded model (`transcript.downloadModel`). Blocking (slow on long videos).
pub fn whisper(video: &Path) -> Result<Vec<(f64, String)>> {
    let mut s = filmcraft_engine::Session::default();
    let imported = s.execute("file.import", json!({"paths": [video.to_string_lossy()]})).map_err(|e| StudioError::Other(e.to_string()))?;
    let id = imported
        .get("items")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_u64)
        .ok_or_else(|| StudioError::Other("the video could not be read for Whisper".into()))?;
    s.execute("transcript.generate", json!({"items": [id], "diarize": false})).map_err(|e| StudioError::Other(format!("Whisper: {e}")))?;
    let t = s
        .project
        .transcripts
        .get(&filmcraft_project::ItemId(id))
        .ok_or_else(|| StudioError::Other("Whisper returned no transcript".into()))?;
    let words: Vec<(f64, f64, String)> = t.words.iter().map(|w| (w.start.seconds(), w.end.seconds(), w.text.clone())).collect();
    Ok(lines_from_words(&words))
}

/// Can this build run Whisper, and is a model there? Panels ask every frame, so the answer is
/// kept for ten seconds (a download clears it).
pub fn whisper_ready() -> std::result::Result<(), String> {
    static CACHE: std::sync::Mutex<Option<(std::time::Instant, std::result::Result<(), String>)>> = std::sync::Mutex::new(None);
    let mut c = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if WHISPER_FRESH.swap(false, std::sync::atomic::Ordering::SeqCst) {
        *c = None;
    }
    if let Some((_, r)) = c.as_ref().filter(|(at, _)| at.elapsed() < Duration::from_secs(10)) {
        return r.clone();
    }
    let r = whisper_ready_now();
    *c = Some((std::time::Instant::now(), r.clone()));
    r
}

static WHISPER_FRESH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn whisper_ready_now() -> std::result::Result<(), String> {
    let mut s = filmcraft_engine::Session::default();
    let models = s.execute("transcript.models", json!({})).map_err(|e| e.to_string())?;
    if !filmcraft_engine::transcript::speech_available() {
        return Err("this build has no Whisper".into());
    }
    let any = models.get("models").and_then(Value::as_array).is_some_and(|m| m.iter().any(|x| x.get("installed").and_then(Value::as_bool) == Some(true)));
    if any { Ok(()) } else { Err("no Whisper model downloaded yet".into()) }
}

/// Downloads the default Whisper model (about 290 MB, once). Blocking.
pub fn download_whisper() -> Result<String> {
    let mut s = filmcraft_engine::Session::default();
    let r = s.execute("transcript.downloadModel", json!({})).map_err(|e| StudioError::Other(e.to_string()))?;
    WHISPER_FRESH.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(r.get("model").and_then(Value::as_str).unwrap_or("whisper-base").to_string())
}

/// Loudness every half second, in dBFS (FFmpeg decodes mono 8 kHz).
pub fn loudness(studio: &Studio, video: &Path) -> Result<Vec<f64>> {
    let o = quiet(
        Command::new(ffmpeg(studio)?)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(video)
            .args(["-vn", "-ac", "1", "-ar", "8000", "-f", "s16le", "-"]),
    )
    .output()?;
    let samples: Vec<f64> = o.stdout.chunks_exact(2).map(|b| f64::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0).collect();
    Ok(samples
        .chunks(4000)
        .map(|c| {
            let rms = (c.iter().map(|x| x * x).sum::<f64>() / c.len().max(1) as f64).sqrt();
            20.0 * rms.max(1e-6).log10()
        })
        .collect())
}

/// Black stretches (FFmpeg's blackdetect: at least 0.2 s nearly black), as (start, end).
pub fn black_spans(studio: &Studio, video: &Path) -> Result<Vec<(f64, f64)>> {
    let o = quiet(
        Command::new(ffmpeg(studio)?)
            .args(["-hide_banner", "-i"])
            .arg(video)
            .args(["-an", "-vf", "scale=160:-2,blackdetect=d=0.2:pix_th=0.10", "-f", "null", "-"]),
    )
    .output()?;
    let text = String::from_utf8_lossy(&o.stderr);
    Ok(text
        .lines()
        .filter(|l| l.contains("black_start:"))
        .filter_map(|l| {
            let get = |k: &str| l.split(k).nth(1)?.split_whitespace().next()?.parse::<f64>().ok();
            Some((get("black_start:")?, get("black_end:")?))
        })
        .collect())
}

/// Ranges without the black stretches in them (pieces shorter than half a second dropped).
pub fn without(pieces: &[Piece], black: &[(f64, f64)]) -> Vec<Piece> {
    let mut out = Vec::new();
    for p in pieces {
        let mut parts = vec![*p];
        for (a, b) in black {
            parts = parts
                .into_iter()
                .flat_map(|x| {
                    if *b <= x.start || *a >= x.end {
                        vec![x]
                    } else {
                        vec![Piece { start: x.start, end: a.max(x.start) }, Piece { start: b.min(x.end), end: x.end }]
                    }
                })
                .filter(|x| x.end - x.start >= 0.5)
                .collect();
        }
        out.extend(parts);
    }
    out
}

/// Scene-cut times (FFmpeg's scene score over 0.35).
pub fn cuts(studio: &Studio, video: &Path) -> Result<Vec<f64>> {
    let o = quiet(
        Command::new(ffmpeg(studio)?)
            .args(["-hide_banner", "-i"])
            .arg(video)
            .args(["-an", "-vf", "scale=160:-2,select='gt(scene,0.35)',showinfo", "-f", "null", "-"]),
    )
    .output()?;
    let text = String::from_utf8_lossy(&o.stderr);
    Ok(text.split("pts_time:").skip(1).filter_map(|r| r.split_whitespace().next()?.parse().ok()).collect())
}

/// Words that tend to open strong short videos.
const HOOK_WORDS: &[&str] = &[
    "never", "secret", "nobody", "why", "how", "what", "wait", "insane", "crazy", "first", "only", "biggest", "worst", "best", "actually",
    "shocking", "finally", "imagine", "watch", "look", "this is", "you won't", "can't believe", "?", "!",
];

/// Candidate windows of `min`..`max` seconds starting and ending on transcript lines (or, with
/// no transcript, on scene cuts or every few seconds), scored from the signals, best first,
/// apart from each other.
pub fn candidates(lines: &[(f64, String)], loud: &[f64], cuts: &[f64], duration: f64, w: &Wants, keep: usize) -> Vec<Candidate> {
    let (min, max) = (w.min_seconds.max(3.0), w.max_seconds.max(w.min_seconds.max(3.0)));
    // Where a window may start: lines, else cuts, else every 5 s.
    let starts: Vec<f64> = if !lines.is_empty() {
        lines.iter().map(|(t, _)| *t).collect()
    } else if cuts.len() > 3 {
        std::iter::once(0.0).chain(cuts.iter().copied()).collect()
    } else {
        (0..((duration / 5.0) as usize).max(1)).map(|i| i as f64 * 5.0).collect()
    };
    let ends: Vec<f64> = if lines.is_empty() { starts.iter().copied().chain(std::iter::once(duration)).collect() } else { lines.iter().skip(1).map(|(t, _)| *t).chain(std::iter::once(duration)).collect() };
    let loud_at = |a: f64, b: f64| -> (f64, f64, f64) {
        let (i, j) = (((a * 2.0) as usize).min(loud.len()), ((b * 2.0) as usize).min(loud.len()));
        let part = &loud[i..j.max(i)];
        if part.is_empty() {
            return (-60.0, -60.0, a);
        }
        let mean = part.iter().sum::<f64>() / part.len() as f64;
        let (k, peak) = part.iter().enumerate().fold((0, f64::MIN), |acc, (k, v)| if *v > acc.1 { (k, *v) } else { acc });
        (mean, peak, a + k as f64 / 2.0)
    };
    let overall = if loud.is_empty() { -30.0 } else { loud.iter().sum::<f64>() / loud.len() as f64 };
    let mut all: Vec<Candidate> = Vec::new();
    for &s in &starts {
        let Some(&e) = ends.iter().filter(|&&e| e - s >= min && e - s <= max).max_by(|a, b| a.total_cmp(b)) else { continue };
        let text: String = lines.iter().filter(|(t, _)| *t >= s && *t < e).map(|(_, l)| l.as_str()).collect::<Vec<_>>().join(" ");
        let first: String = lines.iter().filter(|(t, _)| *t >= s && *t < s + 4.0).map(|(_, l)| l.to_lowercase()).collect::<Vec<_>>().join(" ");
        let words = text.split_whitespace().count() as f64;
        let pace = (words / (e - s) / 3.0).min(1.0); // ~3 words a second is brisk
        let (mean, peak, peak_t) = loud_at(s, e);
        let energy = ((mean - overall + 6.0) / 12.0).clamp(0.0, 1.0);
        let punch = ((peak - mean) / 15.0).clamp(0.0, 1.0);
        let cut_rate = (cuts.iter().filter(|c| **c >= s && **c < e).count() as f64 / (e - s) * 5.0).min(1.0);
        let hook = (HOOK_WORDS.iter().filter(|h| first.contains(*h)).count() as f64 / 2.0).min(1.0);
        let signal = if lines.is_empty() { 0.45 * energy + 0.3 * punch + 0.25 * cut_rate } else { 0.25 * pace + 0.2 * energy + 0.15 * punch + 0.15 * cut_rate + 0.25 * hook };
        let shots: Vec<f64> = cuts.iter().copied().filter(|c| *c > s && *c < e).take(60).collect();
        all.push(Candidate { start: s, end: e, text: text.chars().take(1500).collect(), signal, peak: peak_t, visual: None, shots });
    }
    all.sort_by(|a, b| b.signal.total_cmp(&a.signal));
    let mut out: Vec<Candidate> = Vec::new();
    for c in all {
        if out.iter().any(|o| c.start < o.end && o.start < c.end) {
            continue;
        }
        out.push(c);
        if out.len() >= keep {
            break;
        }
    }
    out
}

/// The vision model: `VISION_PROVIDER` "ollama" (default, `OLLAMA_VISION_MODEL`, gemma3),
/// "nvidia" (`NVIDIA_API_KEY`, `NVIDIA_VISION_MODEL`) or "off".
pub fn vision_provider(studio: &Studio) -> (String, String) {
    let p = studio.var("VISION_PROVIDER").map(str::to_ascii_lowercase).unwrap_or_else(|| if studio.var("NVIDIA_API_KEY").is_some() { "nvidia".into() } else { "ollama".into() });
    let model = match p.as_str() {
        "nvidia" => studio.var("NVIDIA_VISION_MODEL").unwrap_or("meta/llama-3.2-90b-vision-instruct").to_string(),
        "off" => String::new(),
        _ => studio.var("OLLAMA_VISION_MODEL").unwrap_or("gemma3:latest").to_string(),
    };
    (p, model)
}

/// A grid of frames of `a`..`b` (2×2, or 3×3 for moments over 30 s), as JPEG bytes.
fn grid(studio: &Studio, video: &Path, a: f64, b: f64) -> Result<Vec<u8>> {
    let span = (b - a).max(1.0);
    let (n, tile, width) = if span > 30.0 { (9.0, "3x3", 320) } else { (4.0, "2x2", 384) };
    let o = quiet(
        Command::new(ffmpeg(studio)?)
            .args(["-hide_banner", "-loglevel", "error", "-ss", &format!("{a:.3}"), "-t", &format!("{span:.3}"), "-i"])
            .arg(video)
            .args(["-an", "-vf", &format!("fps={:.5},scale={width}:-2,tile={tile}", n / span), "-frames:v", "1", "-q:v", "4", "-f", "mjpeg", "-"]),
    )
    .output()?;
    if o.stdout.len() < 100 {
        return err("FFmpeg could not take frames for the vision model");
    }
    Ok(o.stdout)
}

const VISION_PROMPT: &str = "These are frames, in reading order, of one moment of a video that may become a vertical short (Reel/Short/TikTok). Rate how strong and readable the pictures are for a scroll-stopping short from 0 to 10 (faces, action, clear subject, contrast count; static slides, darkness or clutter do not). Give the horizontal centre of the main subject from 0 (left edge) to 1 (right edge), for cropping the frame to 9:16. Describe what is happening in one sentence, and name any studio logo, age-rating card, release-date slate or black frame you see. Reply with only JSON: {\"visual\": number, \"subjectX\": number, \"what\": string}.";

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

/// Asks the vision model about one candidate.
pub fn look(studio: &Studio, video: &Path, c: &Candidate) -> Result<Vision> {
    let (provider, model) = vision_provider(studio);
    let image = b64(&grid(studio, video, c.start, c.end)?);
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(180)).build();
    let reply: String = match provider.as_str() {
        "off" => return err("vision is off (VISION_PROVIDER=off)"),
        "nvidia" => {
            let key = studio.var("NVIDIA_API_KEY").ok_or_else(|| StudioError::Other("set NVIDIA_API_KEY for NVIDIA vision".into()))?;
            let body = json!({
                "model": model, "max_tokens": 300, "temperature": 0.2,
                "messages": [{"role": "user", "content": [
                    {"type": "text", "text": VISION_PROMPT},
                    {"type": "image_url", "image_url": {"url": format!("data:image/jpeg;base64,{image}")}}
                ]}]
            });
            let r: Value = agent
                .post("https://integrate.api.nvidia.com/v1/chat/completions")
                .set("Authorization", &format!("Bearer {key}"))
                .send_json(body)
                .map_err(|e| StudioError::Other(format!("NVIDIA vision ({model}): {}", nim_error(e))))?
                .into_json()
                .map_err(|e| StudioError::Other(e.to_string()))?;
            r.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or_default().to_string()
        }
        _ => {
            let host = crate::ai::ollama_host(studio.var("OLLAMA_HOST"));
            let body = json!({"model": model, "stream": false, "format": "json", "options": {"temperature": 0.2},
                "messages": [{"role": "user", "content": VISION_PROMPT, "images": [image]}]});
            let r: Value = agent
                .post(&format!("{host}/api/chat"))
                .send_json(body)
                .map_err(|e| StudioError::Other(format!("Ollama vision ({model}): {e}")))?
                .into_json()
                .map_err(|e| StudioError::Other(e.to_string()))?;
            r.pointer("/message/content").and_then(Value::as_str).unwrap_or_default().to_string()
        }
    };
    let v: Value = serde_json::from_str(crate::ai::extract_json(&reply)?).map_err(|e| StudioError::Other(format!("the vision model's answer is not JSON: {e}")))?;
    Ok(Vision {
        visual: v.get("visual").and_then(Value::as_f64).unwrap_or(5.0).clamp(0.0, 10.0),
        subject_x: v.get("subjectX").and_then(Value::as_f64).unwrap_or(0.5).clamp(0.0, 1.0),
        what: v.get("what").and_then(Value::as_str).unwrap_or_default().chars().take(300).collect(),
    })
}

/// A readable NIM error.
pub(crate) fn nim_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let t = r.into_string().unwrap_or_default();
            format!("HTTP {code}: {}", t.chars().take(200).collect::<String>())
        }
        other => other.to_string(),
    }
}

fn rank_schema() -> Value {
    let range = json!({"type": "object", "properties": {"start": {"type": "number"}, "end": {"type": "number"}}, "required": ["start", "end"], "additionalProperties": false});
    json!({
        "type": "object",
        "properties": {
            "moments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "candidate": {"type": "integer"},
                        "start": {"type": "number"},
                        "end": {"type": "number"},
                        "score": {"type": "number"},
                        "hook": {"type": "number"}, "emotion": {"type": "number"}, "clarity": {"type": "number"}, "visual": {"type": "number"}, "share": {"type": "number"},
                        "hooks": {"type": "array", "items": {"type": "string"}},
                        "keep": {"type": "array", "items": range},
                        "title": {"type": "string"}, "caption": {"type": "string"}, "attribution": {"type": "string"},
                        "instagramCaption": {"type": "string"}, "hashtags": {"type": "array", "items": {"type": "string"}},
                        "why": {"type": "string"}
                    },
                    "required": ["candidate", "start", "end", "score", "hook", "emotion", "clarity", "visual", "share", "hooks", "keep", "title", "caption", "attribution", "instagramCaption", "hashtags", "why"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["moments"],
        "additionalProperties": false
    })
}

const RANK_INSTRUCTION: &str = "You are a short-form video editor scoring candidate moments of a long video (often a game or film trailer) for their potential as vertical Reels/Shorts/TikToks. For each candidate you get its times, transcript (trailers often have little or none: then judge from the shots, the loudness peaks and what the vision model saw), signal score (pace, loudness, cuts, hook words, 0-1), its shot changes and what a vision model saw. Cut on shot changes. In trailers leave out studio logos, age-rating cards, release-date slates, long title cards, black frames and slow fades, unless the title reveal is the payoff; open on the most striking shot. Name characters, games and people only as the title, description or transcript names them, and never guess anyone's gender: use their name or \"they\". Pick up to the asked number of moments, best first. For each: \"candidate\" is its number; \"start\"/\"end\" may tighten it to sentence bounds inside the candidate; score 0-10 each of hook (first seconds stop the scroll), emotion (surprise, humour, tension, awe), clarity (makes sense alone), visual, share (people send it on or comment); \"score\" is the overall 0-100 virality prediction, calibrated (most moments are 40-70; reserve 85+ for rare standouts). \"hooks\" are three different opening lines for the on-screen text, each under 60 characters, in the channel voice. \"keep\" lists the ranges to play in order: first the 1-5 second hook (the strongest moment, shown first as a teaser), then the rest of the moment in time order without dead air, filler or repeats; empty when no edit is needed. Write the card as for a single post. \"why\" names the main reason in one line.";

/// Ranks candidates with the workspace's AI. `None` when there is no AI (mock).
fn rank(studio: &Studio, channel: i64, meta: Option<&Value>, cands: &[Candidate], w: &Wants) -> Result<Option<Vec<Segment>>> {
    let instruction = [crate::ai::voice(studio, channel)?.as_str(), RANK_INSTRUCTION, crate::ai::CARD_INSTRUCTION].join(" ");
    let list: Vec<Value> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| {
            json!({"candidate": i, "start": c.start, "end": c.end, "signal": (c.signal * 100.0).round() / 100.0, "loudestAt": c.peak, "shots": c.shots,
                   "transcript": c.text, "vision": c.visual.as_ref().map(|v| json!({"visual": v.visual, "what": v.what}))})
        })
        .collect();
    let input = json!({
        "title": meta.and_then(|m| m.get("title")),
        "uploader": meta.and_then(|m| m.get("uploader").or_else(|| m.get("channel"))),
        "wanted": w.max_reels, "minSeconds": w.min_seconds, "maxSeconds": w.max_seconds, "direction": w.notes,
        "candidates": list,
    });
    let Some(reply) = crate::ai::ask(studio, &instruction, &input, &rank_schema())? else { return Ok(None) };
    let uploader = meta.and_then(|m| m.get("uploader").or_else(|| m.get("channel"))).and_then(Value::as_str).unwrap_or("");
    let mut out = Vec::new();
    for m in reply.get("moments").and_then(Value::as_array).into_iter().flatten() {
        let Ok(mut card) = crate::ai::card_from(m) else { continue };
        if card.attribution.is_empty() && !uploader.is_empty() {
            card.attribution = format!("Original: {uploader}");
        }
        let f = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let c = m.get("candidate").and_then(Value::as_u64).and_then(|i| cands.get(i as usize));
        let scores = Scores { hook: f("hook"), emotion: f("emotion"), clarity: f("clarity"), visual: f("visual"), share: f("share") };
        out.push(Segment {
            start: f("start"),
            end: f("end"),
            card,
            why: m.get("why").and_then(Value::as_str).unwrap_or("").chars().take(300).collect(),
            pieces: m
                .get("keep")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|k| Some(Piece { start: k.get("start")?.as_f64()?, end: k.get("end")?.as_f64()? }))
                .collect(),
            score: Some(f("score").clamp(0.0, 100.0)),
            scores: Some(scores),
            hooks: m.get("hooks").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(|h| h.chars().take(80).collect()).take(3).collect(),
            focus_x: c.and_then(|c| c.visual.as_ref()).map(|v| v.subject_x),
            ..Default::default()
        });
    }
    Ok(Some(out))
}

/// Without an AI: the signal ranking, the loudest moment as the hook.
fn by_signals(cands: &[Candidate], meta: Option<&Value>, w: &Wants) -> Vec<Segment> {
    let title = meta.and_then(|m| m.get("title")).and_then(Value::as_str).unwrap_or("Clip").to_string();
    cands
        .iter()
        .take(w.max_reels.max(1))
        .enumerate()
        .map(|(i, c)| {
            let v = c.visual.as_ref().map_or(5.0, |v| v.visual);
            let score = (c.signal * 70.0 + v * 3.0).clamp(0.0, 100.0);
            let hook = Piece { start: (c.peak - 1.0).max(c.start), end: (c.peak + 2.0).min(c.end) };
            Segment {
                start: c.start,
                end: c.end,
                card: filmcraft_reel::Card { title: format!("{title} ({})", i + 1), instagram_caption: title.clone(), ..Default::default() },
                why: "Ranked by its signals (no AI answered)".into(),
                pieces: if hook.end - hook.start >= 1.0 { vec![hook, Piece { start: c.start, end: c.end }] } else { Vec::new() },
                score: Some(score.round()),
                scores: Some(Scores { visual: v, ..Default::default() }),
                focus_x: c.visual.as_ref().map(|v| v.subject_x),
                ..Default::default()
            }
        })
        .collect()
}

/// Analyzes a downloaded video: moments with scores, hooks, timestamps and the edit, best first.
/// `say` reports progress. Blocking (Whisper and the vision model take a while).
pub fn analyze(studio: &Studio, channel: i64, video: &Path, meta: Option<&Value>, wants: &Wants, say: &dyn Fn(&str)) -> Result<Analysis> {
    let mut a = Analysis::default();
    let mut w = wants.clone();
    w.max_reels = w.max_reels.clamp(1, 20);
    if !(w.min_seconds.is_finite() && w.max_seconds.is_finite()) || w.max_seconds < 3.0 {
        w.min_seconds = 15.0;
        w.max_seconds = 60.0;
    }
    a.duration = crate::thumbnails::duration(studio, video)?;
    say("Reading the transcript");
    let mut lines = meta.map(crate::autopilot::transcript).unwrap_or_default();
    a.transcript = if lines.is_empty() { "none".into() } else { "captions".into() };
    if lines.is_empty() {
        match whisper_ready() {
            Ok(()) => {
                say("Transcribing with Whisper (local)");
                match whisper(video) {
                    Ok(l) if !l.is_empty() => {
                        lines = l;
                        a.transcript = "whisper".into();
                    }
                    Ok(_) => a.notes.push("Whisper heard no speech".into()),
                    Err(e) => a.notes.push(e.to_string()),
                }
            }
            Err(e) => a.notes.push(format!("No transcript: {e} (AI panel ▸ Download Whisper)")),
        }
    }
    say("Measuring loudness and scene cuts");
    let loud = loudness(studio, video).unwrap_or_default();
    let cuts = cuts(studio, video).unwrap_or_default();
    let black = black_spans(studio, video).unwrap_or_default();
    let mut cands = candidates(&lines, &loud, &cuts, a.duration, &w, (w.max_reels * 3).clamp(4, 10));
    if cands.is_empty() {
        return err("no part of the video fits the asked length");
    }
    let (provider, model) = vision_provider(studio);
    if provider != "off" {
        let n = cands.len();
        for (i, c) in cands.iter_mut().enumerate() {
            say(&format!("Looking at moment {} of {n} ({provider})", i + 1));
            match look(studio, video, c) {
                Ok(v) => c.visual = Some(v),
                Err(e) => {
                    a.notes.push(format!("Vision skipped: {e}"));
                    break;
                }
            }
        }
        if cands.iter().any(|c| c.visual.is_some()) {
            a.vision = format!("{provider} {model}");
        }
    }
    say("Scoring the moments");
    let ranked = match rank(studio, channel, meta, &cands, &w) {
        Ok(Some(s)) if !s.is_empty() => {
            a.ranked_by = crate::ai::provider(studio);
            s
        }
        Ok(_) => {
            a.ranked_by = "signals".into();
            by_signals(&cands, meta, &w)
        }
        Err(e) => {
            a.notes.push(format!("The AI could not score them ({e}); ranked by signals"));
            a.ranked_by = "signals".into();
            by_signals(&cands, meta, &w)
        }
    };
    let mut segs = crate::autopilot::clean(ranked, a.duration, &w);
    // Black frames never play: they come out of the cuts (in play order, after the edit is tidy).
    for s in &mut segs {
        let base = if s.pieces.is_empty() { vec![Piece { start: s.start, end: s.end }] } else { s.pieces.clone() };
        let clean = without(&base, &black);
        if clean != base && !clean.is_empty() {
            s.pieces = clean;
        }
    }
    // Each moment keeps its words, for Flick's scenes.
    for s in &mut segs {
        s.lines = lines
            .iter()
            .enumerate()
            .map(|(i, (t, l))| (*t, lines.get(i + 1).map_or(t + 5.0, |n| n.0.min(t + 8.0)), l.clone()))
            .filter(|(a, b, _)| *b > s.start && *a < s.end)
            .collect();
    }
    segs.sort_by(|x, y| y.score.unwrap_or(0.0).total_cmp(&x.score.unwrap_or(0.0)));
    a.segments = segs;
    Ok(a)
}

/// A published post's prediction and real numbers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PostResult {
    pub title: String,
    pub platform: String,
    pub score: f64,
    pub views: Option<f64>,
    pub likes: Option<f64>,
    pub comments: Option<f64>,
    pub link: String,
}

/// Spearman rank correlation of two equal-length lists (None under 3 pairs).
pub fn spearman(a: &[f64], b: &[f64]) -> Option<f64> {
    if a.len() != b.len() || a.len() < 3 {
        return None;
    }
    let rank = |v: &[f64]| {
        let mut idx: Vec<usize> = (0..v.len()).collect();
        idx.sort_by(|&i, &j| v[i].total_cmp(&v[j]));
        let mut r = vec![0.0; v.len()];
        let mut k = 0;
        while k < idx.len() {
            let mut e = k;
            while e + 1 < idx.len() && v[idx[e + 1]] == v[idx[k]] {
                e += 1;
            }
            let avg = (k + e) as f64 / 2.0 + 1.0;
            for &i in &idx[k..=e] {
                r[i] = avg;
            }
            k = e + 1;
        }
        r
    };
    let (ra, rb) = (rank(a), rank(b));
    let n = a.len() as f64;
    let (ma, mb) = (ra.iter().sum::<f64>() / n, rb.iter().sum::<f64>() / n);
    let cov: f64 = ra.iter().zip(&rb).map(|(x, y)| (x - ma) * (y - mb)).sum();
    let (va, vb): (f64, f64) = (ra.iter().map(|x| (x - ma).powi(2)).sum(), rb.iter().map(|y| (y - mb).powi(2)).sum());
    (va > 0.0 && vb > 0.0).then(|| cov / (va * vb).sqrt())
}

/// Published posts that carry a virality score, with their real views, likes and comments
/// (YouTube with the channel's key; Instagram with the account's token). Blocking.
pub fn results(studio: &Studio, channel: i64) -> Result<(Vec<PostResult>, Option<f64>)> {
    let rows: Vec<(String, String, String, i64, String)> = {
        let c = studio.connect()?;
        let mut q = c.prepare(
            "SELECT s.json, p.platform, p.remote_id, p.social_account_id, COALESCE(p.remote_url, '') FROM publishing_jobs p
             JOIN renders r ON r.id = p.render_id JOIN scripts s ON s.id = r.script_id
             WHERE p.channel_id = ?1 AND p.status = 'PUBLISHED' AND p.remote_id IS NOT NULL",
        )?;
        q.query_map([channel], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<std::result::Result<_, _>>()?
    };
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build();
    let mut out = Vec::new();
    for (script, platform, remote, account, link) in rows {
        let v: Value = serde_json::from_str(&script).unwrap_or(Value::Null);
        let Some(score) = v.get("virality").and_then(Value::as_f64) else { continue };
        let title = v.pointer("/card/title").and_then(Value::as_str).unwrap_or("Post").to_string();
        let mut r = PostResult { title, platform: platform.clone(), score, link, ..Default::default() };
        let (_, a) = studio.account(account)?;
        let secret = studio.secret(&a.credentials_reference).unwrap_or(Value::Null);
        match platform.as_str() {
            "YOUTUBE" => {
                if let Ok(stats) = crate::youtube::statistics(&secret, &remote) {
                    r.views = stats.0;
                    r.likes = stats.1;
                    r.comments = stats.2;
                }
            }
            "INSTAGRAM" => {
                if let Some(token) = secret.get("accessToken").and_then(Value::as_str) {
                    let version = studio.var("META_GRAPH_VERSION").unwrap_or("v23.0").to_string();
                    let get = |path: &str| -> Option<Value> {
                        agent.get(&format!("https://graph.facebook.com/{version}/{path}")).query("access_token", token).call().ok()?.into_json().ok()
                    };
                    if let Some(m) = get(&format!("{remote}?fields=like_count,comments_count")) {
                        r.likes = m.get("like_count").and_then(Value::as_f64);
                        r.comments = m.get("comments_count").and_then(Value::as_f64);
                    }
                    if let Some(i) = get(&format!("{remote}/insights?metric=views")) {
                        r.views = i.pointer("/data/0/values/0/value").and_then(Value::as_f64);
                    }
                }
            }
            _ => {}
        }
        out.push(r);
    }
    let pairs: Vec<(f64, f64)> = out.iter().filter_map(|r| Some((r.score, r.views?))).collect();
    let rho = spearman(&pairs.iter().map(|p| p.0).collect::<Vec<_>>(), &pairs.iter().map(|p| p.1).collect::<Vec<_>>());
    Ok((out, rho))
}
