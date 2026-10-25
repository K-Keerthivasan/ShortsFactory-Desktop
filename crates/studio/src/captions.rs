//! Captions for recorded or written videos: the words said (local Whisper, word-timed) or a
//! written script (aligned to the speech, or spread over the video when nobody speaks), set as
//! short social captions on a caption track of the open sequence, in a look the channel keeps.
//!
//! The track ("Social captions") is an ordinary caption track: every caption's text and times
//! can be edited afterwards, the look is the track's style, and exports burn it in. Making the
//! captions again replaces that track only. `studio.captions` gives the same to the AI editors
//! (Claude, Codex, Hermes), which can pass corrected words of their own, and writes the words in
//! Remotion's caption format for animated captions.

use std::path::{Path, PathBuf};

use filmcraft_engine::Session;
use filmcraft_engine::time::Tick;
use filmcraft_project::{Caption, CaptionAlign, CaptionAnchor, CaptionFormat, CaptionStyle, CaptionTrack, ClipId, ItemId, MediaRef, TrackId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, StudioError, err};

/// The caption track this tool owns.
pub const TRACK: &str = "Social captions";

/// How captions look and are split.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Look {
    /// The preset it started from.
    pub preset: String,
    pub font: String,
    /// Pixels at 1080 lines (a 1920-line Reel draws it 1.78 times larger).
    pub size: f32,
    /// `#RRGGBB`.
    pub color: String,
    /// Outline width (0 = none).
    pub outline: f32,
    pub outline_color: String,
    pub background: bool,
    /// `#RRGGBBAA`.
    pub background_color: String,
    pub uppercase: bool,
    /// Most words in one caption.
    pub words: usize,
    /// 1 or 2 lines.
    pub lines: usize,
    /// "lower", "middle" or "upper".
    pub position: String,
}

impl Default for Look {
    fn default() -> Self {
        preset("Bold pop").unwrap_or_else(|| Look {
            preset: String::new(),
            font: "Inter".into(),
            size: 46.0,
            color: "#FFFFFF".into(),
            outline: 4.0,
            outline_color: "#000000".into(),
            background: false,
            background_color: "#000000B0".into(),
            uppercase: true,
            words: 3,
            lines: 1,
            position: "lower".into(),
        })
    }
}

/// The built-in looks.
pub const PRESETS: &[&str] = &["Bold pop", "Yellow punch", "Clean box", "Subtitles", "Minimal"];

pub fn preset(name: &str) -> Option<Look> {
    let base = Look {
        preset: name.into(),
        font: "Inter".into(),
        size: 46.0,
        color: "#FFFFFF".into(),
        outline: 4.0,
        outline_color: "#000000".into(),
        background: false,
        background_color: "#000000B0".into(),
        uppercase: true,
        words: 3,
        lines: 1,
        position: "lower".into(),
    };
    Some(match name {
        "Bold pop" => base,
        "Yellow punch" => Look { color: "#FFE14D".into(), size: 52.0, outline: 5.0, words: 2, position: "middle".into(), ..base },
        "Clean box" => Look { size: 34.0, outline: 0.0, background: true, background_color: "#000000B8".into(), uppercase: false, words: 6, ..base },
        "Subtitles" => Look { size: 28.0, outline: 2.0, uppercase: false, words: 14, lines: 2, ..base },
        "Minimal" => Look { size: 32.0, outline: 0.0, uppercase: false, words: 5, ..base },
        _ => return None,
    })
}

/// A word or a caption with its times (seconds, sequence time unless said otherwise).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timed {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

fn colour(s: &str, fallback: [u8; 4]) -> [u8; 4] {
    let h = s.trim().trim_start_matches('#');
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => (|| Some([byte(0)?, byte(2)?, byte(4)?, 255]))().unwrap_or(fallback),
        8 => (|| Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]))().unwrap_or(fallback),
        _ => fallback,
    }
}

impl Look {
    pub fn check(&self) -> Result<()> {
        if !(8.0..=300.0).contains(&self.size) || !(0.0..=40.0).contains(&self.outline) || !(1..=30).contains(&self.words) || !(1..=2).contains(&self.lines) {
            return err("Captions: size 8-300, outline 0-40, 1-30 words and 1-2 lines");
        }
        if !["lower", "middle", "upper"].contains(&self.position.as_str()) {
            return err("Captions: the position is lower, middle or upper");
        }
        Ok(())
    }

    /// The caption track's style.
    pub fn style(&self) -> CaptionStyle {
        let (anchor, margin) = match self.position.as_str() {
            "middle" => (CaptionAnchor::Middle, 0.0),
            // Above the platforms' buttons and description at the bottom, below the top bar.
            "upper" => (CaptionAnchor::Top, 0.2),
            _ => (CaptionAnchor::Bottom, 0.24),
        };
        CaptionStyle {
            font: if self.font.trim().is_empty() { "Inter".into() } else { self.font.trim().into() },
            size: self.size,
            color: colour(&self.color, [255, 255, 255, 255]),
            background: self.background,
            background_color: colour(&self.background_color, [0, 0, 0, 176]),
            align: CaptionAlign::Center,
            anchor,
            margin,
            line_spacing: 1.12,
            outline: self.outline,
            outline_color: colour(&self.outline_color, [0, 0, 0, 255]),
        }
    }
}

fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Splits timed words into captions: at most `look.words` words (and a line's characters), a new
/// caption after a sentence ends or at a pause. Each caption stays up until the next one when the
/// gap is short, so they do not flicker.
pub fn group(words: &[Timed], look: &Look) -> Vec<Timed> {
    let max_words = look.words.clamp(1, 30);
    let line_chars = if look.words <= 4 { 24 } else { 34 };
    let max_chars = line_chars * look.lines.clamp(1, 2);
    let mut out: Vec<Timed> = Vec::new();
    let mut cur: Vec<&Timed> = Vec::new();
    let flush = |cur: &mut Vec<&Timed>, out: &mut Vec<Timed>| {
        if cur.is_empty() {
            return;
        }
        let mut text = cur.iter().map(|w| w.text.trim()).collect::<Vec<_>>().join(" ");
        if look.uppercase {
            text = text.to_uppercase();
        }
        // Two lines: break at the space nearest the middle.
        if look.lines == 2 && text.chars().count() > line_chars {
            let mid = text.len() / 2;
            if let Some(i) = text.char_indices().filter(|(_, c)| *c == ' ').map(|(i, _)| i).min_by_key(|i| i.abs_diff(mid)) {
                text.replace_range(i..i + 1, "\n");
            }
        }
        out.push(Timed { start: cur[0].start, end: cur[cur.len() - 1].end, text });
        cur.clear();
    };
    for w in words.iter().filter(|w| !w.text.trim().is_empty()) {
        if let Some(last) = cur.last() {
            let chars: usize = cur.iter().map(|x| x.text.trim().chars().count() + 1).sum::<usize>() + w.text.trim().chars().count();
            let sentence = last.text.trim_end().ends_with(['.', '!', '?']);
            if cur.len() >= max_words || chars > max_chars || w.start - last.end > 0.45 || sentence {
                flush(&mut cur, &mut out);
            }
        }
        cur.push(w);
    }
    flush(&mut cur, &mut out);
    for i in 0..out.len() {
        let next = out.get(i + 1).map(|n| n.start);
        let c = &mut out[i];
        c.end = c.end.max(c.start + 0.25);
        if let Some(n) = next {
            if n - c.end < 0.6 {
                c.end = n;
            }
            c.end = c.end.min(n);
        }
    }
    out
}

/// A written script given the speech's timing: script words matched to the words heard take
/// their times; the others share the time between their matched neighbours. Use it when the
/// text is known (a script, corrected names) and the recording says it.
pub fn align_script(speech: &[Timed], script: &str) -> Vec<Timed> {
    let sw: Vec<&str> = script.split_whitespace().collect();
    if speech.is_empty() || sw.is_empty() {
        return Vec::new();
    }
    let (n, m) = (speech.len(), sw.len());
    // Matched (speech index) per script word.
    let mut matched: Vec<Option<usize>> = vec![None; m];
    if n * m <= 6_000_000 {
        let a: Vec<String> = speech.iter().map(|w| norm(&w.text)).collect();
        let b: Vec<String> = sw.iter().map(|w| norm(w)).collect();
        let mut dp = vec![0u16; (n + 1) * (m + 1)];
        let at = |i: usize, j: usize| i * (m + 1) + j;
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                dp[at(i, j)] = if !a[i].is_empty() && a[i] == b[j] { dp[at(i + 1, j + 1)] + 1 } else { dp[at(i + 1, j)].max(dp[at(i, j + 1)]) };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if !a[i].is_empty() && a[i] == b[j] {
                matched[j] = Some(i);
                i += 1;
                j += 1;
            } else if dp[at(i + 1, j)] >= dp[at(i, j + 1)] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    let first = speech[0].start;
    let last = speech[n - 1].end;
    let mut out: Vec<Timed> = Vec::with_capacity(m);
    let mut j = 0;
    while j < m {
        if let Some(i) = matched[j] {
            out.push(Timed { start: speech[i].start, end: speech[i].end, text: sw[j].into() });
            j += 1;
            continue;
        }
        // A run of unmatched script words, between the matched words around it.
        let k = (j..m).find(|&k| matched[k].is_some()).unwrap_or(m);
        let from = out.last().map(|w| w.end).unwrap_or(first);
        let to = if k < m { matched[k].map(|i| speech[i].start).unwrap_or(last) } else { last };
        let (from, to) = if matched.iter().all(Option::is_none) {
            // Nothing matched at all: follow the speech proportionally.
            (first, last)
        } else {
            (from, to.max(from))
        };
        out.extend(spread(&sw[j..k], from, to));
        j = k;
    }
    out
}

/// Words spread over `[start, end]` by their length (a written video with no speech).
pub fn spread(words: &[&str], start: f64, end: f64) -> Vec<Timed> {
    let weights: Vec<f64> = words.iter().map(|w| w.chars().count() as f64 + 2.0).collect();
    let total: f64 = weights.iter().sum::<f64>().max(1.0);
    let span = (end - start).max(0.0);
    let mut t = start;
    words
        .iter()
        .zip(weights)
        .map(|(w, k)| {
            let d = span * k / total;
            let out = Timed { start: t, end: t + d, text: (*w).into() };
            t += d;
            out
        })
        .collect()
}

/// A script with no speech under it: at a reading pace (about 2.6 words a second) from `start`,
/// but never past `end`.
pub fn read_script(script: &str, start: f64, end: f64) -> Vec<Timed> {
    let words: Vec<&str> = script.split_whitespace().collect();
    let natural = words.len() as f64 / 2.6;
    let end = if end > start { end.min(start + natural.max(1.0)).max(start + 0.5) } else { start + natural.max(1.0) };
    spread(&words, start, end)
}

/// Media items heard on the open sequence (audio clips, else video clips) with their files, and
/// whether each has a transcript.
pub fn heard(s: &Session) -> Vec<(u64, PathBuf, bool)> {
    let Some(q) = s.active_sequence() else { return Vec::new() };
    let mut items: Vec<ItemId> = q.audio_tracks.iter().flat_map(|t| t.items.iter().map(|i| i.item)).collect();
    if items.is_empty() {
        items = q.video_tracks.iter().flat_map(|t| t.items.iter().map(|i| i.item)).collect();
    }
    let mut out: Vec<(u64, PathBuf, bool)> = Vec::new();
    for id in items {
        if out.iter().any(|(i, _, _)| *i == id.0) {
            continue;
        }
        let Some(item) = s.project.item(id) else { continue };
        let Some(MediaRef::File { path }) = item.as_media().map(|m| &m.media) else { continue };
        out.push((id.0, PathBuf::from(path), s.project.transcripts.contains_key(&id)));
    }
    out
}

/// Whisper's words for each file (blocking; run it off the UI thread). Words are in media time.
pub fn transcribe(files: &[(u64, PathBuf)], progress: &dyn Fn(&str)) -> Result<Vec<(u64, Vec<Timed>)>> {
    let mut out = Vec::new();
    for (i, (item, path)) in files.iter().enumerate() {
        progress(&format!("Listening to {} ({}/{})", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), i + 1, files.len()));
        let words = crate::virality::whisper_words(path)?;
        out.push((*item, words.into_iter().map(|(start, end, text)| Timed { start, end, text }).collect()));
    }
    Ok(out)
}

/// Gives the media items their transcripts (media time), so the editor's text-based editing
/// works on them too.
pub fn set_transcripts(s: &mut Session, heard: &[(u64, Vec<Timed>)]) -> Result<()> {
    for (item, words) in heard {
        let words: Vec<Value> = words
            .iter()
            .filter(|w| !w.text.trim().is_empty() && w.end > w.start)
            .map(|w| json!({"text": w.text.trim(), "start": Tick::from_seconds_f64(w.start).0, "end": Tick::from_seconds_f64(w.end).0}))
            .collect();
        if words.is_empty() {
            continue;
        }
        s.execute("transcript.set", json!({"item": item, "transcript": {"language": "en", "source": "whisper", "speakers": [], "words": words}}))
            .map_err(|e| StudioError::Other(format!("transcript: {e}")))?;
    }
    Ok(())
}

/// The words heard on the open sequence, in sequence time (through its cuts).
pub fn sequence_speech(s: &Session) -> Vec<Timed> {
    let mut words: Vec<Timed> = filmcraft_engine::transcript::sequence_words(s)
        .into_iter()
        .map(|w| Timed { start: w.start.seconds(), end: w.end.seconds(), text: w.text })
        .collect();
    words.sort_by(|a, b| a.start.total_cmp(&b.start));
    // The same word heard on two tracks is one word.
    words.dedup_by(|b, a| (b.start - a.start).abs() < 0.02 && b.text == a.text);
    words
}

/// Puts the captions on the open sequence's "Social captions" track (replacing it), in one
/// undoable edit. Returns the track and the number of captions.
pub fn place(s: &mut Session, captions: &[Timed], look: &Look) -> Result<Value> {
    let rate = s.sequence_rate();
    let frame = rate.frame_duration();
    let mut cues: Vec<(Tick, Tick, String)> = Vec::new();
    for c in captions {
        let start = rate.snap(Tick::from_seconds_f64(c.start.max(0.0)));
        let start = cues.last().map_or(start, |p| start.max(p.1));
        let end = rate.snap(Tick::from_seconds_f64(c.end)).max(start + frame);
        cues.push((start, end, c.text.clone()));
    }
    let style = look.style();
    let n = cues.len();
    let track = s
        .edit_sequence("Make Captions", |q, ctx, st| {
            q.caption_tracks.retain(|t| t.name != TRACK);
            let tid = TrackId(ctx.alloc());
            let mut t = CaptionTrack::new(tid, TRACK.into(), CaptionFormat::default());
            t.style = style;
            t.captions = cues
                .into_iter()
                .map(|(start, end, text)| Caption { id: ClipId(ctx.alloc()), start, duration: end - start, text, speaker: None, cue_id: None, settings: String::new() })
                .collect();
            q.caption_tracks.insert(0, t);
            st.caption_selection.clear();
            Ok(tid)
        })
        .map_err(|e| StudioError::Other(e.to_string()))?;
    Ok(json!({"track": track.0, "captions": n}))
}

/// Restyles the "Social captions" track without touching its words.
pub fn restyle(s: &mut Session, look: &Look) -> Result<()> {
    let style = look.style();
    s.edit_sequence("Caption Style", |q, _, _| {
        let t = q.caption_tracks.iter_mut().find(|t| t.name == TRACK).ok_or(filmcraft_engine::EngineError::Other("make the captions first".into()))?;
        t.style = style;
        Ok(())
    })
    .map_err(|e| StudioError::Other(e.to_string()))
}

/// The open sequence's length in seconds.
fn sequence_seconds(s: &Session) -> f64 {
    s.active_sequence().map(|q| q.duration().seconds()).unwrap_or(0.0)
}

/// After transcription (or with words given): the captions, from the speech or the script.
pub fn finish(s: &mut Session, heard: &[(u64, Vec<Timed>)], look: &Look, script: Option<&str>) -> Result<Value> {
    look.check()?;
    set_transcripts(s, heard)?;
    let speech = sequence_speech(s);
    let words = match script.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) if speech.is_empty() => read_script(t, 0.0, sequence_seconds(s)),
        Some(t) => align_script(&speech, t),
        None => speech,
    };
    if words.is_empty() {
        return err("No speech was heard on this sequence: write the captions' text (the script) and make them again");
    }
    let captions = group(&words, look);
    place(s, &captions, look)
}

/// Everything at once (blocking: transcribes what has no transcript yet). For the AI editors.
pub fn make(s: &mut Session, look: &Look, script: Option<&str>, words: Option<Vec<Timed>>) -> Result<Value> {
    look.check()?;
    if let Some(words) = words {
        let captions = group(&words, look);
        return place(s, &captions, look);
    }
    let missing: Vec<(u64, PathBuf)> = heard(s).into_iter().filter(|(_, _, has)| !has).map(|(i, p, _)| (i, p)).collect();
    let script_only = script.is_some_and(|t| !t.trim().is_empty()) && heard(s).is_empty();
    let heard = if script_only { Vec::new() } else { transcribe(&missing, &|_| {})? };
    finish(s, &heard, look, script)
}

/// The captions' words in Remotion's caption format (`@remotion/captions`), for animated
/// captions made with Remotion (Flick, the Remotion MCP).
pub fn remotion(words: &[Timed]) -> Value {
    json!(words
        .iter()
        .map(|w| {
            let ms = |t: f64| (t * 1000.0).round() as i64;
            json!({"text": format!(" {}", w.text.trim()), "startMs": ms(w.start), "endMs": ms(w.end), "timestampMs": ms((w.start + w.end) / 2.0), "confidence": Value::Null})
        })
        .collect::<Vec<_>>())
}

/// Writes [`remotion`] for the open sequence's speech (or the script aligned to it) to `file`.
pub fn export_remotion(s: &Session, script: Option<&str>, file: &Path) -> Result<usize> {
    let speech = sequence_speech(s);
    let words = match script.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) if speech.is_empty() => read_script(t, 0.0, sequence_seconds(s)),
        Some(t) => align_script(&speech, t),
        None => speech,
    };
    if words.is_empty() {
        return err("No words: transcribe the sequence (make captions) or give the script");
    }
    if let Some(d) = file.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(file, serde_json::to_string_pretty(&remotion(&words)).unwrap_or_default())?;
    Ok(words.len())
}
