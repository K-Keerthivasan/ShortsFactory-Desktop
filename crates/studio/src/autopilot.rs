//! The AI workflow from a link to Reels: what the video says (its chapters and captions, from
//! the details yt-dlp already read), the parts of it that stand alone as Reels, and a card for
//! each — chosen by the workspace's AI in the channel's voice. The editor then builds one Reel
//! per segment from the same downloaded file, and nothing is published before it is approved.
//!
//! With [`Wants::hook`] the AI also edits each part: the ranges worth keeping, in playback order,
//! led by a short hook (the part's strongest moment, shown first as a teaser), leaving out dead
//! air, filler and repeated takes. [`assemble`] joins those ranges into one clip with FFmpeg, and
//! the Reel is built from it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use filmcraft_reel::Card;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, Studio, StudioError};

/// How many Reels to make from one video.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// The AI decides: one Reel, or several when the video has several stories.
    #[default]
    Auto,
    Single,
    Multiple,
    /// One long 16:9 video: the whole video edited tighter, opening with a hook.
    Long,
}

/// What to ask for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Wants {
    pub mode: Mode,
    pub max_reels: usize,
    pub min_seconds: f64,
    pub max_seconds: f64,
    /// Extra direction for the AI ("focus on the gameplay reveal").
    pub notes: String,
    /// Open each Reel with a hook and cut the filler out.
    pub hook: bool,
}

impl Default for Wants {
    fn default() -> Self {
        Wants { mode: Mode::Auto, max_reels: 3, min_seconds: 15.0, max_seconds: 60.0, notes: String::new(), hook: true }
    }
}

/// One Reel to make: a part of the source and its card.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub card: Card,
    /// Why the AI chose it (shown for approval).
    pub why: String,
    /// The ranges to play, in order, hook first; empty plays `start`..`end` as it is.
    pub pieces: Vec<Piece>,
    /// The clip [`assemble`] made from `pieces`.
    pub file: Option<PathBuf>,
    /// The analyzer's virality prediction, 0–100 (not a guarantee of views).
    pub score: Option<f64>,
    pub scores: Option<crate::virality::Scores>,
    /// Suggested opening lines for the on-screen text.
    pub hooks: Vec<String>,
    /// The main subject's horizontal centre (0–1): the edit is cropped to 9:16 around it.
    pub focus_x: Option<f64>,
}

/// A range of the source, in seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Piece {
    pub start: f64,
    pub end: f64,
}

impl Segment {
    /// How long the Reel plays.
    pub fn seconds(&self) -> f64 {
        if self.pieces.is_empty() { (self.end - self.start).max(0.0) } else { self.pieces.iter().map(|p| (p.end - p.start).max(0.0)).sum() }
    }
}

/// The longest hook: a Reel's, and a long video's.
const HOOK_SECONDS: f64 = 5.0;
const LONG_HOOK_SECONDS: f64 = 15.0;

const LONG_INSTRUCTION: &str = "You are editing a long video into ONE tighter long video for YouTube (16:9). Return exactly one part: \"start\" is where the useful content begins and \"end\" where it ends. \"keep\" lists the ranges to play, in playback order. The FIRST range is the cold-open hook: the most gripping 5 to 15 seconds of the whole video (a result, a reveal, a strong claim), shown first so viewers stay; it may repeat later. After it, list the rest in time order, leaving out dead air, filler words, false starts, repeated takes, long greetings, tangents that do not serve the story, sponsor segments, and calls to like and subscribe; keep every step the story needs and end right after the payoff. Every range begins and ends on a sentence, lasts at least two seconds, and the ranges after the hook do not overlap. Use the transcript times. Write the card as the video's title and description. If there is no transcript, return an empty \"keep\" list.";

const HOOK_INSTRUCTION: &str = "Also edit each part like a short-form editor. \"keep\" lists the ranges to play, in playback order. The FIRST range is the hook: the single most striking 1 to 5 seconds of the part (a reveal, a punchline, a surprising line or the peak of the action), usually taken from later in the part and shown first as a teaser so viewers stay; it may repeat later. After the hook, list the rest of the part in time order, leaving out dead air, filler words, false starts, repeated takes, greetings, intros, outros and calls to subscribe, and ending right after the payoff. Every range lies between the part's start and end, begins and ends on a sentence or shot, lasts at least one second, and the ranges after the hook do not overlap. Use the transcript times. If there is no transcript, or the part needs no edit, return an empty list.";

/// Timestamped caption lines (seconds, text) of a video, from the captions yt-dlp lists in its
/// details: the video's own subtitles first, else the automatic ones; English, else the video's
/// language. Empty when there are none or they can't be read.
pub fn transcript(meta: &Value) -> Vec<(f64, String)> {
    let lang = meta.get("language").and_then(Value::as_str).unwrap_or("en").to_string();
    for key in ["subtitles", "automatic_captions"] {
        let Some(tracks) = meta.get(key).and_then(Value::as_object) else { continue };
        let pick = tracks
            .get("en")
            .or_else(|| tracks.get("en-orig"))
            .or_else(|| tracks.iter().find(|(k, _)| k.starts_with("en-")).map(|(_, v)| v))
            .or_else(|| tracks.get(&lang))
            .or_else(|| tracks.get(&format!("{lang}-orig")));
        let Some(url) = pick
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|f| f.get("ext").and_then(Value::as_str) == Some("json3")))
            .and_then(|f| f.get("url"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if !url.starts_with("https://") {
            continue;
        }
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build();
        let Some(body) = agent.get(url).call().ok().and_then(|r| r.into_json::<Value>().ok()) else { continue };
        let lines = json3_lines(&body);
        if !lines.is_empty() {
            return lines;
        }
    }
    Vec::new()
}

/// YouTube's json3 caption events as lines, merged into about five-second blocks.
pub fn json3_lines(body: &Value) -> Vec<(f64, String)> {
    let mut out: Vec<(f64, String)> = Vec::new();
    for e in body.get("events").and_then(Value::as_array).into_iter().flatten() {
        let t = e.get("tStartMs").and_then(Value::as_f64).unwrap_or(0.0) / 1000.0;
        let text: String = e.get("segs").and_then(Value::as_array).into_iter().flatten().filter_map(|s| s.get("utf8").and_then(Value::as_str)).collect();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some((start, line)) if t - *start < 5.0 && line.len() < 300 => {
                line.push(' ');
                line.push_str(&text);
            }
            _ => out.push((t, text)),
        }
    }
    out
}

fn clock(t: f64) -> String {
    let t = t.max(0.0) as u64;
    format!("{}:{:02}", t / 60, t % 60)
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "segments": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "start": {"type": "number"},
                        "end": {"type": "number"},
                        "title": {"type": "string", "minLength": 1, "maxLength": 400},
                        "caption": {"type": "string", "maxLength": 200},
                        "attribution": {"type": "string", "maxLength": 160},
                        "instagramCaption": {"type": "string", "maxLength": 2200},
                        "hashtags": {"type": "array", "items": {"type": "string"}},
                        "why": {"type": "string", "maxLength": 300},
                        "keep": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {"start": {"type": "number"}, "end": {"type": "number"}},
                                "required": ["start", "end"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["start", "end", "title", "caption", "attribution", "instagramCaption", "hashtags", "why", "keep"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["segments"],
        "additionalProperties": false
    })
}

/// The segments to make from a downloaded video, each with its card. The AI chooses from the
/// video's details, chapters and captions; when it can't (no provider, mock, a bad answer) the
/// chapters or an even split are used, so the workflow always ends with something to review.
/// The second value says why the fallback was used. Blocking.
pub fn plan(studio: &Studio, channel: i64, meta: &Value, duration: f64, wants: &Wants) -> (Vec<Segment>, Option<String>) {
    let wants = tidy(wants, duration);
    let lines = transcript(meta);
    match ask(studio, channel, meta, duration, &wants, &lines) {
        Ok(segments) => {
            let s = clean(segments, duration, &wants);
            if s.is_empty() { (fallback(meta, duration, &wants), Some("the AI chose no usable part".into())) } else { (s, None) }
        }
        Err(e) => (fallback(meta, duration, &wants), Some(e.to_string())),
    }
}

fn tidy(w: &Wants, duration: f64) -> Wants {
    let mut w = w.clone();
    let finite = |v: f64, d: f64| if v.is_finite() && v > 0.0 { v } else { d };
    w.max_seconds = if w.mode == Mode::Long { finite(w.max_seconds, 900.0).clamp(30.0, 14_400.0) } else { finite(w.max_seconds, 60.0).clamp(3.0, 600.0) };
    w.min_seconds = finite(w.min_seconds, 15.0).clamp(1.0, w.max_seconds);
    if duration.is_finite() && duration > 0.0 {
        w.min_seconds = w.min_seconds.min(duration);
    }
    w.max_reels = match w.mode {
        Mode::Single | Mode::Long => 1,
        _ => w.max_reels.clamp(1, 20),
    };
    w
}

fn ask(studio: &Studio, channel: i64, meta: &Value, duration: f64, w: &Wants, lines: &[(f64, String)]) -> Result<Vec<Segment>> {
    let how = match w.mode {
        Mode::Single => "Choose the ONE best part.".to_string(),
        Mode::Multiple => format!("Choose up to {} parts, each a different story or moment.", w.max_reels),
        Mode::Auto => format!("Choose one part, or up to {} when the video holds several separate stories or moments worth a Reel each.", w.max_reels),
        Mode::Long => format!("The edited video should run at most {:.0} minutes.", w.max_seconds / 60.0),
    };
    let instruction = if w.mode == Mode::Long {
        [crate::ai::voice(studio, channel)?.as_str(), LONG_INSTRUCTION, &how, "For the part write its card:", crate::ai::CARD_INSTRUCTION, "\"why\" says in one line what the edit keeps."]
            .join(" ")
    } else {
        [
            crate::ai::voice(studio, channel)?.as_str(),
            "You are cutting a long video into vertical short videos (Reels).",
            &how,
            &format!(
                "Each part is one continuous range of the video: \"start\" and \"end\" in seconds, between {:.0} and {:.0} seconds long, starting where a sentence or shot starts and ending after the point lands. Parts must not overlap. Use the transcript times.",
                w.min_seconds, w.max_seconds
            ),
            "For each part write its quote card as for a single post:",
            crate::ai::CARD_INSTRUCTION,
            "\"why\" says in one line why this part works on its own.",
            if w.hook { HOOK_INSTRUCTION } else { "Leave \"keep\" empty." },
        ]
        .join(" ")
    };
    let transcript: String = lines.iter().map(|(t, l)| format!("[{}] {l}\n", clock(*t))).collect::<String>().chars().take(40_000).collect();
    let chapters: Vec<Value> = meta
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| json!({"start": c.get("start_time"), "end": c.get("end_time"), "title": c.get("title")}))
        .collect();
    let name = studio.channels()?.into_iter().find(|c| c.id == channel).map(|c| c.name).unwrap_or_default();
    let input = json!({
        "channelName": name,
        "title": meta.get("title"),
        "uploader": meta.get("uploader").or_else(|| meta.get("channel")),
        "description": meta.get("description").and_then(Value::as_str).unwrap_or("").chars().take(3000).collect::<String>(),
        "durationSeconds": duration,
        "chapters": chapters,
        "transcript": if transcript.is_empty() { "(no captions; choose from the title, description and chapters)".to_string() } else { transcript },
        "direction": w.notes,
    });
    let Some(reply) = crate::ai::ask(studio, &instruction, &input, &schema())? else {
        return crate::err("the mock AI provider does not choose parts");
    };
    let uploader = meta.get("uploader").or_else(|| meta.get("channel")).and_then(Value::as_str).unwrap_or("");
    let mut out = Vec::new();
    for s in reply.get("segments").and_then(Value::as_array).into_iter().flatten() {
        let Ok(mut card) = crate::ai::card_from(s) else { continue };
        if card.attribution.is_empty() && !uploader.is_empty() {
            card.attribution = format!("Original: {uploader}");
        }
        out.push(Segment {
            start: s.get("start").and_then(Value::as_f64).unwrap_or(f64::NAN),
            end: s.get("end").and_then(Value::as_f64).unwrap_or(f64::NAN),
            card,
            why: s.get("why").and_then(Value::as_str).unwrap_or("").chars().take(300).collect(),
            pieces: s
                .get("keep")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|k| Some(Piece { start: k.get("start")?.as_f64()?, end: k.get("end")?.as_f64()? }))
                .collect(),
            ..Default::default()
        });
    }
    Ok(out)
}

/// Keeps the parts inside the video, of a usable length, in order, without overlaps, and no
/// more than asked for.
pub fn clean(mut segments: Vec<Segment>, duration: f64, w: &Wants) -> Vec<Segment> {
    let total = if duration.is_finite() && duration > 0.0 { duration } else { f64::INFINITY };
    segments.retain(|s| s.start.is_finite() && s.end.is_finite());
    // A long video's part spans the whole edit; its kept ranges keep to the length.
    let hull = if w.mode == Mode::Long { f64::INFINITY } else { w.max_seconds };
    for s in &mut segments {
        s.start = s.start.clamp(0.0, total);
        s.end = s.end.clamp(0.0, total).min(s.start + hull);
    }
    segments.retain(|s| s.end - s.start >= w.min_seconds.min(total) - 0.5);
    segments.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out: Vec<Segment> = Vec::new();
    for s in segments {
        if out.last().is_some_and(|p| s.start < p.end - 0.5) {
            continue;
        }
        out.push(s);
    }
    out.truncate(w.max_reels);
    for s in &mut out {
        tidy_pieces(s, w);
    }
    out
}

/// Keeps a usable edit: the hook (at most five seconds) then non-overlapping ranges in time
/// order, all inside the part, no longer in all than the part may be plus the hook. Anything
/// less than a hook and one more range is no edit.
fn tidy_pieces(s: &mut Segment, w: &Wants) {
    let (lo, hi) = (s.start, s.end);
    let mut p: Vec<Piece> = if w.hook {
        s.pieces
            .iter()
            .filter(|p| p.start.is_finite() && p.end.is_finite())
            .map(|p| Piece { start: p.start.clamp(lo, hi), end: p.end.clamp(lo, hi) })
            .filter(|p| p.end - p.start >= 0.5)
            .collect()
    } else {
        Vec::new()
    };
    s.pieces.clear();
    if p.len() < 2 {
        return;
    }
    let mut hook = p.remove(0);
    hook.end = hook.end.min(hook.start + if w.mode == Mode::Long { LONG_HOOK_SECONDS } else { HOOK_SECONDS });
    p.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut rest: Vec<Piece> = Vec::new();
    for x in p {
        if rest.last().is_some_and(|l| x.start < l.end - 0.05) {
            continue;
        }
        rest.push(x);
    }
    let mut room = w.max_seconds;
    rest.retain_mut(|x| {
        if room < 0.5 {
            return false;
        }
        x.end = x.end.min(x.start + room);
        room -= x.end - x.start;
        true
    });
    if rest.is_empty() {
        return;
    }
    s.pieces = std::iter::once(hook).chain(rest).collect();
}

/// [`join_filter`], then a 9:16 crop centred on `focus_x` (0–1) for wide video (a portrait
/// video is kept as it is), with even sides for H.264.
pub fn join_filter_cropped(pieces: &[Piece], audio: bool, focus_x: Option<f64>) -> String {
    let joined = join_filter(pieces, audio);
    let Some(x) = focus_x.filter(|x| x.is_finite()) else { return joined };
    let x = x.clamp(0.0, 1.0);
    // The joined picture is [v]; crop it into [vc] and hand that on as [v].
    let joined = joined.replacen("[v]", "[vj]", 1);
    format!(
        "{joined};[vj]crop='min(iw,ih*9/16)':'ih':'(iw-min(iw,ih*9/16))*{x:.3}':'0',scale='trunc(iw/2)*2':'trunc(ih/2)*2'[v]"
    )
}

/// FFmpeg's filter joining `pieces` of input 0, with or without sound.
pub fn join_filter(pieces: &[Piece], audio: bool) -> String {
    let mut f = String::new();
    let mut ins = String::new();
    for (i, p) in pieces.iter().enumerate() {
        f.push_str(&format!("[0:v]trim=start={:.3}:end={:.3},setpts=PTS-STARTPTS[v{i}];", p.start, p.end));
        ins.push_str(&format!("[v{i}]"));
        if audio {
            f.push_str(&format!("[0:a]atrim=start={:.3}:end={:.3},asetpts=PTS-STARTPTS[a{i}];", p.start, p.end));
            ins.push_str(&format!("[a{i}]"));
        }
    }
    f.push_str(&format!("{ins}concat=n={}:v=1:a={}[v]{}", pieces.len(), u8::from(audio), if audio { "[a]" } else { "" }));
    f
}

/// Joins a segment's pieces of `source` into one MP4 beside it (a plain part is cut out as it
/// is). Blocking (an encode).
pub fn assemble(studio: &Studio, source: &Path, seg: &Segment) -> Result<PathBuf> {
    let plain = [Piece { start: seg.start, end: seg.end }];
    let pieces: &[Piece] = if seg.pieces.is_empty() { &plain } else { &seg.pieces };
    if pieces.iter().all(|p| p.end - p.start < 0.1) {
        return crate::err("this part is empty");
    }
    let ffmpeg = crate::ytdlp::ffmpeg(studio).ok_or_else(|| StudioError::Other("FFmpeg is needed for the hook edit (Downloads & tools)".into()))?;
    let stem = source.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "clip".into());
    let out = source.with_file_name(format!("{stem} - edit {:.0}-{:.0}.mp4", seg.start, seg.end));
    let mut last = String::new();
    // With sound first; a source without an audio stream fails that and goes without.
    for audio in [true, false] {
        let mut c = std::process::Command::new(&ffmpeg);
        c.args(["-y", "-hide_banner", "-loglevel", "error", "-i"]).arg(source);
        c.args(["-filter_complex", &join_filter_cropped(pieces, audio, seg.focus_x), "-map", "[v]"]);
        if audio {
            c.args(["-map", "[a]", "-c:a", "aac", "-b:a", "192k"]);
        }
        c.args(["-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart"]).arg(&out);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        let o = c.output()?;
        if o.status.success() && out.is_file() {
            return Ok(out);
        }
        last = String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("").to_string();
    }
    crate::err(format!("FFmpeg could not make the hook edit: {last}"))
}

/// Makes a long video from a segment: its edit (or the plain part) as one MP4, added to the
/// workspace's library as a long video (16:9, kind VIDEO) waiting for approval, with
/// thumbnails that carry its headline. Returns the render. Blocking (an encode).
pub fn make_long(studio: &Studio, channel: i64, source: &Path, seg: &Segment) -> Result<i64> {
    let file = match seg.file.as_ref().filter(|f| f.is_file()) {
        Some(f) => f.clone(),
        None => assemble(studio, source, seg)?,
    };
    let info = filmcraft_reel::ReelInfo { channel_id: Some(channel), card: seg.card.clone(), format: "long".into(), template: String::new(), ..Default::default() };
    let duration = crate::thumbnails::duration(studio, &file).unwrap_or(seg.seconds());
    let (_, render) = studio.send_to_library(&info, &file, None, None, duration, false)?;
    if let Err(e) = crate::thumbnails::ensure(studio, render) {
        log::warn!("thumbnails for the long video {render}: {e}");
    }
    Ok(render)
}

/// Chapters that fit, else even parts from the start.
fn fallback(meta: &Value, duration: f64, w: &Wants) -> Vec<Segment> {
    let title = meta.get("title").and_then(Value::as_str).unwrap_or("Imported video").chars().take(200).collect::<String>();
    let uploader = meta.get("uploader").or_else(|| meta.get("channel")).and_then(Value::as_str).unwrap_or("");
    let card = |t: String| Card {
        title: t.clone(),
        attribution: if uploader.is_empty() { String::new() } else { format!("Original: {uploader}") },
        instagram_caption: t,
        ..Default::default()
    };
    let chapters: Vec<Segment> = meta
        .get("chapters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let (a, b) = (c.get("start_time")?.as_f64()?, c.get("end_time")?.as_f64()?);
            let name = c.get("title").and_then(Value::as_str).unwrap_or("");
            Some(Segment { start: a, end: b, card: card(format!("{title}: {name}")), why: format!("Chapter “{name}”"), ..Default::default() })
        })
        .collect();
    let chapters = clean(chapters, duration, w);
    if !chapters.is_empty() {
        return chapters;
    }
    let total = if duration.is_finite() && duration > 0.0 { duration } else { w.max_seconds };
    if w.mode == Mode::Long {
        // No AI edit: the whole video as it is.
        return vec![Segment { start: 0.0, end: total, card: card(title.clone()), why: "The whole video (no AI edit)".into(), ..Default::default() }];
    }
    let n = if w.mode == Mode::Single { 1 } else { ((total / w.max_seconds).floor() as usize).clamp(1, w.max_reels) };
    (0..n)
        .map(|i| {
            let start = i as f64 * w.max_seconds;
            let t = if n == 1 { title.clone() } else { format!("{title} (part {})", i + 1) };
            Segment { start, end: (start + w.max_seconds).min(total), card: card(t), why: "Even split (no AI choice)".into(), ..Default::default() }
        })
        .collect()
}
