//! The AI workflow from a link to Reels: what the video says (its chapters and captions, from
//! the details yt-dlp already read), the parts of it that stand alone as Reels, and a card for
//! each — chosen by the workspace's AI in the channel's voice. The editor then builds one Reel
//! per segment from the same downloaded file, and nothing is published before it is approved.

use std::time::Duration;

use filmcraft_reel::Card;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, Studio};

/// How many Reels to make from one video.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// The AI decides: one Reel, or several when the video has several stories.
    #[default]
    Auto,
    Single,
    Multiple,
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
}

impl Default for Wants {
    fn default() -> Self {
        Wants { mode: Mode::Auto, max_reels: 3, min_seconds: 15.0, max_seconds: 60.0, notes: String::new() }
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
}

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
                        "why": {"type": "string", "maxLength": 300}
                    },
                    "required": ["start", "end", "title", "caption", "attribution", "instagramCaption", "hashtags", "why"],
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
    w.max_seconds = finite(w.max_seconds, 60.0).clamp(3.0, 600.0);
    w.min_seconds = finite(w.min_seconds, 15.0).clamp(1.0, w.max_seconds);
    if duration.is_finite() && duration > 0.0 {
        w.min_seconds = w.min_seconds.min(duration);
    }
    w.max_reels = match w.mode {
        Mode::Single => 1,
        _ => w.max_reels.clamp(1, 20),
    };
    w
}

fn ask(studio: &Studio, channel: i64, meta: &Value, duration: f64, w: &Wants, lines: &[(f64, String)]) -> Result<Vec<Segment>> {
    let how = match w.mode {
        Mode::Single => "Choose the ONE best part.".to_string(),
        Mode::Multiple => format!("Choose up to {} parts, each a different story or moment.", w.max_reels),
        Mode::Auto => format!("Choose one part, or up to {} when the video holds several separate stories or moments worth a Reel each.", w.max_reels),
    };
    let instruction = [
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
    ]
    .join(" ");
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
        });
    }
    Ok(out)
}

/// Keeps the parts inside the video, of a usable length, in order, without overlaps, and no
/// more than asked for.
pub fn clean(mut segments: Vec<Segment>, duration: f64, w: &Wants) -> Vec<Segment> {
    let total = if duration.is_finite() && duration > 0.0 { duration } else { f64::INFINITY };
    segments.retain(|s| s.start.is_finite() && s.end.is_finite());
    for s in &mut segments {
        s.start = s.start.clamp(0.0, total);
        s.end = s.end.clamp(0.0, total).min(s.start + w.max_seconds);
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
    out
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
            Some(Segment { start: a, end: b, card: card(format!("{title}: {name}")), why: format!("Chapter “{name}”") })
        })
        .collect();
    let chapters = clean(chapters, duration, w);
    if !chapters.is_empty() {
        return chapters;
    }
    let total = if duration.is_finite() && duration > 0.0 { duration } else { w.max_seconds };
    let n = if w.mode == Mode::Single { 1 } else { ((total / w.max_seconds).floor() as usize).clamp(1, w.max_reels) };
    (0..n)
        .map(|i| {
            let start = i as f64 * w.max_seconds;
            let t = if n == 1 { title.clone() } else { format!("{title} (part {})", i + 1) };
            Segment { start, end: (start + w.max_seconds).min(total), card: card(t), why: "Even split (no AI choice)".into() }
        })
        .collect()
}
