//! Klap (klap.app), an optional paid provider: it turns a video link into finished shorts with
//! its own virality scores, for a comparison with the local analyzer or a faster turnkey result.
//! `KLAP_API_KEY` (and optionally `KLAP_API_URL`) in Plugin Settings ▸ AI. Each task uses the
//! account's Klap credits. Klap's projects carry a score and its explanation but no source times,
//! so its clips come back as finished videos to download, not as editable ranges.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, Studio, StudioError, err};

/// One short Klap made.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Clip {
    pub id: String,
    pub folder_id: String,
    pub name: String,
    /// 0–100 (Klap reports 0–1).
    pub score: f64,
    pub explanation: String,
}

pub fn configured(studio: &Studio) -> bool {
    studio.var("KLAP_API_KEY").is_some()
}

fn base(studio: &Studio) -> String {
    studio.var("KLAP_API_URL").unwrap_or("https://api.klap.app/v2").trim_end_matches('/').to_string()
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(60)).build()
}

fn call(studio: &Studio, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let key = studio.var("KLAP_API_KEY").ok_or_else(|| StudioError::Other("set KLAP_API_KEY (Plugin Settings ▸ AI) to use Klap".into()))?;
    let url = format!("{}{path}", base(studio));
    let req = agent().request(method, &url).set("Authorization", &format!("Bearer {key}"));
    let res = match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    };
    match res {
        Ok(r) => r.into_json().map_err(|e| StudioError::Other(format!("Klap returned an unreadable reply: {e}"))),
        Err(ureq::Error::Status(code, r)) => {
            let t = r.into_string().unwrap_or_default();
            err(format!("Klap request failed (HTTP {code}): {}", t.chars().take(300).collect::<String>()))
        }
        Err(e) => err(format!("Klap is not reachable: {e}")),
    }
}

/// Polls `path` while its `status` is "processing", up to `limit`.
fn wait(studio: &Studio, path: &str, limit: Duration, say: &dyn Fn(&str)) -> Result<Value> {
    let started = std::time::Instant::now();
    loop {
        let v = call(studio, "GET", path, None)?;
        match v.get("status").and_then(Value::as_str) {
            Some("processing") | None if started.elapsed() < limit => {
                say(&format!("Klap is working ({} min)", started.elapsed().as_secs() / 60));
                std::thread::sleep(Duration::from_secs(20));
            }
            Some("error") => return err("Klap could not process it"),
            Some("processing") | None => return err("Klap is still working; try again later"),
            _ => return Ok(v),
        }
    }
}

/// Sends a public video link (e.g. YouTube) to Klap and returns its shorts, best first.
/// Blocking (minutes).
pub fn shorts(studio: &Studio, url: &str, max_clips: usize, max_seconds: f64, say: &dyn Fn(&str)) -> Result<Vec<Clip>> {
    if !url.starts_with("https://") {
        return err("Klap needs a public https link to the video");
    }
    let task = call(
        studio,
        "POST",
        "/tasks/video-to-shorts",
        Some(json!({
            "source_video_url": url, "language": "en",
            "max_duration": max_seconds.clamp(10.0, 180.0).round(), "max_clip_count": max_clips.clamp(1, 20),
            "editing_options": {"intro_title": false},
        })),
    )?;
    let id = task.get("id").and_then(Value::as_str).ok_or_else(|| StudioError::Other("Klap returned no task".into()))?.to_string();
    let done = wait(studio, &format!("/tasks/{id}"), Duration::from_secs(45 * 60), say)?;
    let folder = done.get("output_id").and_then(Value::as_str).ok_or_else(|| StudioError::Other("Klap returned no result folder".into()))?.to_string();
    let projects = call(studio, "GET", &format!("/projects/{folder}"), None)?;
    let mut out: Vec<Clip> = projects
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| {
            let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            Clip {
                id: s("id"),
                folder_id: if s("folder_id").is_empty() { folder.clone() } else { s("folder_id") },
                name: s("name"),
                score: (p.get("virality_score").and_then(Value::as_f64).unwrap_or(0.0) * 100.0).round(),
                explanation: s("virality_score_explanation"),
            }
        })
        .collect();
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(out)
}

/// Exports one of Klap's shorts and downloads the MP4 into `dir`. Blocking (minutes).
pub fn download(studio: &Studio, clip: &Clip, dir: &Path, say: &dyn Fn(&str)) -> Result<PathBuf> {
    let started = call(studio, "POST", &format!("/projects/{}/{}/exports", clip.folder_id, clip.id), Some(json!({})))?;
    let id = started.get("id").and_then(Value::as_str).ok_or_else(|| StudioError::Other("Klap returned no export".into()))?.to_string();
    let done = wait(studio, &format!("/projects/{}/{}/exports/{id}", clip.folder_id, clip.id), Duration::from_secs(30 * 60), say)?;
    let src = done.get("src_url").and_then(Value::as_str).filter(|u| u.starts_with("https://")).ok_or_else(|| StudioError::Other("Klap's export has no file".into()))?;
    std::fs::create_dir_all(dir)?;
    let safe: String = clip.name.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).take(60).collect();
    let file = dir.join(format!("klap - {} - {}.mp4", safe.trim(), clip.score));
    let r = ureq::AgentBuilder::new().timeout(Duration::from_secs(900)).build().get(src).call().map_err(|e| StudioError::Other(format!("downloading Klap's clip: {e}")))?;
    let mut out = std::fs::File::create(&file)?;
    std::io::copy(&mut r.into_reader(), &mut out)?;
    Ok(file)
}
