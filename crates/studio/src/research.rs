//! Research and fact-checking for what the AI writes, with any provider (a local LLM too): the
//! app looks the subject up on the internet itself and hands the model what it found, so the
//! model never needs internet access of its own.
//!
//! Sources: the video's own page (yt-dlp's description and link), Steam's store (games: developer,
//! publisher, release date), Wikipedia, and the web through the Brave Search API
//! (`BRAVE_API_KEY`; without it the web search is skipped). `RESEARCH=off` turns it all off.
//!
//! After writing, [`fact_check`] checks every claim of the card against the sources and what the
//! video itself says or shows; unsupported claims are removed and the change is noted on the card
//! (`fact_check`), with the sources used (`sources`). It works in small steps (judge each claim,
//! rewrite, then the app checks the unsupported names are gone) so small local models do it well.

use std::time::Duration;

use filmcraft_reel::Card;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, Studio};

/// One thing the research found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// "video", "steam", "wikipedia" or "web".
    pub kind: String,
    pub title: String,
    pub url: String,
    pub text: String,
}

/// What the research starts from.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    /// The video's or article's title (or the first line of the notes).
    pub title: String,
    pub uploader: String,
    /// The video page's description.
    pub description: String,
    /// The video page's link.
    pub url: String,
}

impl Hints {
    /// From yt-dlp's metadata of a video.
    pub fn from_meta(meta: &Value) -> Hints {
        let s = |k: &str| meta.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Hints {
            title: s("title"),
            uploader: meta.get("uploader").or_else(|| meta.get("channel")).and_then(Value::as_str).unwrap_or("").to_string(),
            description: s("description"),
            url: meta.get("webpage_url").or_else(|| meta.get("original_url")).and_then(Value::as_str).unwrap_or("").to_string(),
        }
    }
}

pub fn enabled(studio: &Studio) -> bool {
    studio.var("RESEARCH").is_none_or(|v| !v.eq_ignore_ascii_case("off"))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).user_agent("ShortsFactoryDesktop/1.0 (research)").build()
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The subject of a video title: "Brushland - Official Release Date Trailer | PC Gaming Show"
/// → "Brushland".
pub fn subject(title: &str) -> String {
    let mut t = title.to_string();
    // Bracketed tags: (4K), [Official], 【…】.
    for (a, b) in [('(', ')'), ('[', ']'), ('【', '】')] {
        while let (Some(i), Some(j)) = (t.find(a), t.find(b)) {
            if j <= i {
                break;
            }
            t.replace_range(i..j + b.len_utf8(), " ");
        }
    }
    let first = t.split(['|', '｜', '–', '—']).next().unwrap_or(&t).to_string();
    let first = first.split(" - ").next().unwrap_or(&first).to_string();
    const NOISE: &[&str] =
        &["official", "trailer", "teaser", "gameplay", "reveal", "announcement", "announce", "launch", "release", "date", "4k", "hd", "1080p", "60fps", "cinematic", "demo", "overview"];
    let words: Vec<&str> = first.split_whitespace().collect();
    // Trailing noise words go; the name itself stays even when it is one of them ("The Game").
    let mut end = words.len();
    while end > 1 && NOISE.contains(&norm(words[end - 1]).as_str()) {
        end -= 1;
    }
    words[..end].join(" ").trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

fn clip(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= n { s } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

/// Strips HTML tags (Steam's descriptions, Brave's snippets).
fn plain(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => {
                tag = false;
                out.push(' ');
            }
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

/// The game on Steam whose name is the subject.
fn steam(q: &str) -> Option<Source> {
    let a = agent();
    let found: Value = a.get("https://store.steampowered.com/api/storesearch/").query("term", q).query("l", "english").query("cc", "US").call().ok()?.into_json().ok()?;
    let want = norm(q);
    let item = found.get("items")?.as_array()?.iter().find(|i| {
        let n = norm(i.get("name").and_then(Value::as_str).unwrap_or(""));
        !n.is_empty() && (n == want || n.starts_with(&format!("{want} ")) || want.starts_with(&format!("{n} ")))
    })?;
    let id = item.get("id")?.as_u64()?;
    let details: Value = a.get("https://store.steampowered.com/api/appdetails").query("appids", &id.to_string()).query("l", "english").call().ok()?.into_json().ok()?;
    let d = details.get(id.to_string())?.get("data")?;
    let list = |k: &str| d.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let genres = d.get("genres").and_then(Value::as_array).map(|a| a.iter().filter_map(|g| g.get("description")?.as_str()).collect::<Vec<_>>().join(", ")).unwrap_or_default();
    let date = d.get("release_date").and_then(|r| r.get("date")).and_then(Value::as_str).unwrap_or("");
    let soon = d.get("release_date").and_then(|r| r.get("coming_soon")).and_then(Value::as_bool).unwrap_or(false);
    let name = d.get("name").and_then(Value::as_str).unwrap_or(q);
    let text = format!(
        "{name} (Steam). Developer: {}. Publisher: {}. Release: {}{}. Genres: {genres}. {}",
        list("developers"),
        list("publishers"),
        if date.is_empty() { "not announced" } else { date },
        if soon { " (coming soon)" } else { "" },
        clip(&plain(d.get("short_description").and_then(Value::as_str).unwrap_or("")), 600)
    );
    Some(Source { kind: "steam".into(), title: format!("{name} on Steam"), url: format!("https://store.steampowered.com/app/{id}/"), text })
}

/// The Wikipedia article on the subject (only when its title matches).
fn wikipedia(q: &str) -> Option<Source> {
    let a = agent();
    let found: Value = a.get("https://en.wikipedia.org/w/rest.php/v1/search/page").query("q", q).query("limit", "3").call().ok()?.into_json().ok()?;
    // The exact name, or the name of a work ("Brushland (video game)"), never a namesake
    // ("Brushland tinamou", a bird).
    let want = q.trim().to_lowercase();
    let page = found.get("pages")?.as_array()?.iter().find(|p| {
        let t = p.get("title").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
        t == want
            || t.strip_prefix(&want).and_then(|r| r.trim().strip_prefix('(')).is_some_and(|r| ["video game", "game", "film", "tv series", "series", "band", "album", "company"].iter().any(|k| r.starts_with(k)))
    })?;
    let key = page.get("key")?.as_str()?;
    let sum: Value = a.get(&format!("https://en.wikipedia.org/api/rest_v1/page/summary/{key}")).call().ok()?.into_json().ok()?;
    let extract = sum.get("extract")?.as_str()?;
    let title = sum.get("title").and_then(Value::as_str).unwrap_or(key);
    let url = sum.pointer("/content_urls/desktop/page").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("https://en.wikipedia.org/wiki/{key}"));
    Some(Source { kind: "wikipedia".into(), title: format!("{title} (Wikipedia)"), url, text: clip(extract, 900) })
}

/// The web through Brave Search: the top results' snippets.
fn brave(key: &str, q: &str) -> std::result::Result<Vec<Source>, String> {
    let v: Value = agent()
        .get("https://api.search.brave.com/res/v1/web/search")
        .set("Accept", "application/json")
        .set("X-Subscription-Token", key)
        .query("q", q)
        .query("count", "5")
        .query("extra_snippets", "true")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(c, _) => format!("Brave Search refused the request (HTTP {c}): check BRAVE_API_KEY"),
            e => format!("Brave Search: {e}"),
        })?
        .into_json()
        .map_err(|e| e.to_string())?;
    Ok(v.pointer("/web/results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(5)
        .filter_map(|r| {
            let mut text = plain(r.get("description").and_then(Value::as_str).unwrap_or(""));
            for x in r.get("extra_snippets").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).take(3) {
                text.push(' ');
                text.push_str(&plain(x));
            }
            Some(Source { kind: "web".into(), title: plain(r.get("title")?.as_str()?), url: r.get("url")?.as_str()?.to_string(), text: clip(&text, 700) })
        })
        .collect())
}

/// Looks the subject up. Never fails: what could not be reached is left out (and said in `notes`).
pub fn gather(studio: &Studio, h: &Hints, notes: &mut Vec<String>) -> Vec<Source> {
    let mut out = Vec::new();
    if !h.description.trim().is_empty() || !h.url.is_empty() {
        out.push(Source {
            kind: "video".into(),
            title: format!("{} (the video's page{})", if h.title.is_empty() { "Video" } else { &h.title }, if h.uploader.is_empty() { String::new() } else { format!(", by {}", h.uploader) }),
            url: h.url.clone(),
            text: clip(&h.description, 1500),
        });
    }
    if !enabled(studio) {
        return out;
    }
    let q = subject(&h.title);
    if q.chars().count() < 2 {
        return out;
    }
    if let Some(s) = steam(&q) {
        out.push(s);
    }
    if let Some(s) = wikipedia(&q) {
        out.push(s);
    }
    match studio.var("BRAVE_API_KEY").filter(|k| !k.trim().is_empty()) {
        Some(key) => {
            // The subject, with the uploader to tell apart things of the same name.
            let query = if h.uploader.is_empty() || norm(&h.uploader) == norm(&q) { q.clone() } else { format!("{q} {}", h.uploader) };
            match brave(key.trim(), &query) {
                Ok(r) => out.extend(r.into_iter().filter(|r| !out.iter().any(|o: &Source| o.url == r.url)).collect::<Vec<_>>()),
                Err(e) => notes.push(e),
            }
        }
        None => notes.push("Web search is off: add BRAVE_API_KEY in Plugin Settings ▸ AI".into()),
    }
    out
}

/// The sources as the model reads them, numbered from 1.
pub fn for_model(sources: &[Source]) -> Value {
    json!(sources.iter().enumerate().map(|(i, s)| json!({"n": i + 1, "kind": s.kind, "title": s.title, "url": s.url, "text": s.text})).collect::<Vec<_>>())
}

/// How the writer uses the sources.
pub const RULE: &str = "SOURCES are pages the app found about this subject (the video's own page, Steam, Wikipedia, the web). Use a fact from them only when it clearly concerns the same thing as the video (same name, same maker); never mix up things with similar names; prefer the video's own page, then Steam and Wikipedia, then the web. When sources disagree, leave the fact out. Never state a fact that neither the notes nor the sources support.";

const JUDGE: &str = "You are a fact-checker for a short social video post. List every factual claim in the CARD (title, caption, instagramCaption, hashtags) as a separate short item: each name, maker, studio, publisher, platform, date, price, number, event or quote is its own claim. Opinions, the channel's voice, descriptions of mood and calls to action are not claims. For each claim: \"supported\" is true only when the NOTES (what the video itself says and shows) or a SOURCE clearly state it about the same subject; \"source\" is that source's number (0 for the notes, -1 when unsupported).";

const REWRITE: &str = "Rewrite this social video post so it no longer says any of the REMOVE claims. Change as little as possible: keep every other word, the voice, the length and the language. Take a claim out, or say it more generally without the unsupported detail; never replace it with another fact. Drop hashtags that carry a removed claim.";

fn judge_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "claims": {"type": "array", "items": {
                "type": "object",
                "properties": {"claim": {"type": "string"}, "supported": {"type": "boolean"}, "source": {"type": "integer"}},
                "required": ["claim", "supported", "source"],
                "additionalProperties": false
            }}
        },
        "required": ["claims"],
        "additionalProperties": false
    })
}

fn rewrite_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": {"type": "string", "minLength": 1, "maxLength": 400},
            "caption": {"type": "string", "maxLength": 200},
            "instagramCaption": {"type": "string", "maxLength": 2200},
            "hashtags": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["title", "caption", "instagramCaption", "hashtags"],
        "additionalProperties": false
    })
}

fn card_json(card: &Card) -> Value {
    json!({"title": card.title, "caption": card.caption, "instagramCaption": card.instagram_caption, "hashtags": card.hashtags})
}

/// The specific words of unsupported claims (names, platforms, dates, numbers) that neither the
/// sources nor the notes contain: they must not stay in the post.
pub fn foreign_terms(claims: &[String], sources: &[Source], notes: &str) -> Vec<String> {
    // Words that start a sentence with a capital without naming anything.
    const COMMON: &[&str] = &[
        "the", "a", "an", "in", "on", "at", "of", "and", "or", "to", "for", "by", "with", "from", "is", "are", "was", "it", "its", "this", "that", "these", "new", "now",
        "out", "launches", "launch", "launching", "comes", "coming", "hits", "game", "made", "developed", "published", "released", "available", "set", "features",
        "play", "players", "you", "your", "we", "our", "they", "their", "there", "here", "every", "each", "all", "only", "first", "soon", "today", "watch",
    ];
    let known = format!("{} {}", notes, sources.iter().map(|s| format!("{} {}", s.title, s.text)).collect::<Vec<_>>().join(" ")).to_lowercase();
    let apostrophe = |c: char| c == '\'' || c == '\u{2019}';
    let mut out: Vec<String> = Vec::new();
    for c in claims {
        for w in c.split(|ch: char| !(ch.is_alphanumeric() || apostrophe(ch))) {
            let w = w.trim_end_matches("'s").trim_end_matches("\u{2019}s").trim_matches(apostrophe);
            let specific = w.chars().next().is_some_and(char::is_uppercase) || w.chars().any(|ch| ch.is_ascii_digit());
            if w.chars().count() < 2 || !specific || COMMON.contains(&w.to_lowercase().as_str()) {
                continue;
            }
            if !known.contains(&w.to_lowercase()) && !out.iter().any(|o| o.eq_ignore_ascii_case(w)) {
                out.push(w.to_string());
            }
        }
    }
    out
}

fn has_word(text: &str, w: &str) -> bool {
    let (t, w) = (text.to_lowercase(), w.to_lowercase());
    t.match_indices(&w).any(|(i, _)| {
        let before = t[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = t[i + w.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

/// Drops the sentences that still carry an unsupported word (the last resort).
fn drop_sentences(text: &str, terms: &[String]) -> String {
    let mut out = String::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') {
            if !terms.iter().any(|t| has_word(&cur, t)) {
                out.push_str(&cur);
            }
            cur.clear();
        }
    }
    if !terms.iter().any(|t| has_word(&cur, t)) {
        out.push_str(&cur);
    }
    out.trim().to_string()
}

fn apply(card: &mut Card, v: &Value) {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(|x| x.trim().to_string());
    if let Some(t) = s("title").filter(|t| !t.is_empty()) {
        card.title = t.chars().take(400).collect();
    }
    if let Some(c) = s("caption") {
        card.caption = c.chars().take(200).collect();
    }
    if let Some(c) = s("instagramCaption") {
        card.instagram_caption = c.chars().take(2200).collect();
    }
    if let Some(h) = v.get("hashtags").and_then(Value::as_array) {
        card.hashtags = h.iter().filter_map(Value::as_str).map(str::to_string).take(30).collect();
    }
}

/// Checks the card against the sources and the notes in steps a small local model handles well:
/// it judges each claim on its own, then rewrites without the unsupported ones; the app then makes
/// sure the unsupported names and dates are really gone (a second rewrite, else those sentences
/// and hashtags are dropped). The change and the sources used are recorded on the card. Without
/// an AI (mock) it only records the sources.
pub fn fact_check(studio: &Studio, card: &mut Card, sources: &[Source], notes: &str) -> Result<()> {
    let notes: String = notes.chars().take(12_000).collect();
    let input = json!({"card": card_json(card), "notes": notes, "sources": for_model(sources)});
    let Some(v) = crate::ai::ask(studio, JUDGE, &input, &judge_schema())? else {
        card.sources = sources.iter().map(cite).collect();
        return Ok(());
    };
    let mut claims: Vec<(String, bool, i64)> = v
        .get("claims")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| Some((c.get("claim")?.as_str()?.trim().to_string(), c.get("supported")?.as_bool()?, c.get("source").and_then(Value::as_i64).unwrap_or(-1))))
        .filter(|c| !c.0.is_empty())
        .collect();
    // The model's "supported" is checked too: a claim naming something (a maker, a platform, a
    // month, a number) that no source and nothing in the video mentions is not supported.
    for c in claims.iter_mut().filter(|c| c.1) {
        if !foreign_terms(std::slice::from_ref(&c.0), sources, &notes).is_empty() {
            c.1 = false;
        }
    }
    let used: Vec<usize> = claims.iter().filter(|c| c.1 && c.2 > 0).map(|c| c.2 as usize).collect();
    let picked: Vec<&Source> = sources.iter().enumerate().filter(|(i, _)| used.contains(&(i + 1))).map(|(_, s)| s).collect();
    card.sources = if picked.is_empty() { sources.iter().map(cite).collect() } else { picked.into_iter().map(cite).collect() };
    let unsupported: Vec<String> = claims.iter().filter(|c| !c.1).map(|c| clip(&c.0, 160)).take(10).collect();
    if unsupported.is_empty() {
        card.fact_check = "Checked: every claim is supported".into();
        return Ok(());
    }
    let terms = foreign_terms(&unsupported, sources, &notes);
    let left = |card: &Card| -> Vec<String> {
        let text = format!("{} {} {}", card.title, card.caption, card.instagram_caption);
        terms.iter().filter(|t| has_word(&text, t)).cloned().collect()
    };
    for attempt in 0..2 {
        let mut remove = unsupported.clone();
        if attempt > 0 {
            remove.push(format!("these words must not appear at all: {}", left(card).join(", ")));
        }
        if let Some(v) = crate::ai::ask(studio, REWRITE, &json!({"post": card_json(card), "remove": remove}), &rewrite_schema())? {
            apply(card, &v);
        }
        if left(card).is_empty() {
            break;
        }
    }
    // The last resort: the sentences that still carry them go (a headline with nothing clean to
    // replace it stays, and is flagged).
    let stuck = left(card);
    if !stuck.is_empty() {
        let title = drop_sentences(&card.title, &stuck);
        card.caption = drop_sentences(&card.caption, &stuck);
        if !title.is_empty() {
            card.title = title;
        } else if !card.caption.is_empty() {
            // A one-sentence headline that cannot be kept: the clean supporting line takes its place.
            card.title = std::mem::take(&mut card.caption);
        }
        card.instagram_caption = drop_sentences(&card.instagram_caption, &stuck);
    }
    card.hashtags.retain(|h| !terms.iter().any(|t| has_word(h.trim_start_matches('#'), t)));
    let still = left(card);
    card.fact_check = if still.is_empty() {
        format!("Removed (no source supports it): {}", unsupported.join("; "))
    } else {
        format!("Removed (no source supports it): {}. Still unsupported, please edit: {}", unsupported.join("; "), still.join(", "))
    };
    Ok(())
}

fn cite(s: &Source) -> String {
    if s.url.is_empty() { s.title.clone() } else { format!("{} — {}", s.title, s.url) }
}
