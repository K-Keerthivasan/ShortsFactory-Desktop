//! Reel Studio's link to the channel library: the data folder the earlier Shorts Factory app
//! keeps (`%APPDATA%\Shorts Factory Desktop`), opened in place.
//!
//! Nothing is copied or migrated. Channels, their brand settings and logos, media, articles,
//! posts, renders, publishing accounts and their keys stay where they are; this crate reads them
//! and writes new posts, renders and publications into the same database, in the same shape, so
//! the earlier app (and its automation) keep working alongside.
//!
//! Keys are never logged, shown or copied: a publisher reads the account's secret file when it
//! needs it and sends the token only to the platform it belongs to.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod ai;
mod clock;
pub mod cloud;
pub mod commands;
pub mod data;
pub mod instagram;
pub mod settings;
pub mod youtube;
pub mod ytdlp;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use clock::{iso as iso_ms, local_in, local_to_utc_iso, now_iso, parse_iso as clock_ms};
pub use data::{Account, Article, Channel, LibraryPost, MediaRow, RenderRow};

/// Everything that can go wrong talking to the library, the AI or a platform.
#[derive(Debug, thiserror::Error)]
pub enum StudioError {
    #[error("the studio data folder was not found at {0}")]
    NoData(String),
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}
pub type Result<T> = std::result::Result<T, StudioError>;
pub(crate) fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(StudioError::Other(msg.into()))
}

/// The data folder and its settings.
#[derive(Clone, Debug)]
pub struct Studio {
    pub root: PathBuf,
    pub database: PathBuf,
    pub storage: PathBuf,
    pub secrets: PathBuf,
    /// `.env` in the data folder, under the process environment (which wins).
    pub env: HashMap<String, String>,
}

impl Studio {
    /// The data folder: `SHORTS_DESKTOP_DATA`, else `%APPDATA%\Shorts Factory Desktop`.
    pub fn locate() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("SHORTS_DESKTOP_DATA").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("Shorts Factory Desktop"))
    }

    pub fn open(root: &Path) -> Result<Studio> {
        let database = root.join("data").join("studio.db");
        if !database.is_file() {
            return Err(StudioError::NoData(database.display().to_string()));
        }
        let mut env = read_env(&root.join(".env"));
        for (k, v) in std::env::vars() {
            env.insert(k, v);
        }
        let storage = env.get("STORAGE_PATH").map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(|| root.join("storage"));
        let secrets = env.get("SECRETS_PATH").map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(|| root.join("secrets"));
        Ok(Studio { root: root.to_path_buf(), database, storage, secrets, env })
    }

    /// Opens the default data folder.
    pub fn open_default() -> Result<Studio> {
        let root = Self::locate().ok_or_else(|| StudioError::NoData("(no APPDATA)".into()))?;
        Self::open(&root)
    }

    pub fn var(&self, key: &str) -> Option<&str> {
        self.env.get(key).map(String::as_str).filter(|v| !v.trim().is_empty())
    }

    /// A storage-relative path (`channels/jkr/…`, forward slashes) as a path on disk, refusing
    /// anything that would leave storage.
    pub fn resolve_storage(&self, rel: &str) -> Result<PathBuf> {
        let rel = rel.replace('\\', "/");
        let p = Path::new(&rel);
        if p.is_absolute() || rel.contains(':') {
            // Older rows stored absolute paths; keep the part from `channels/` on.
            return match rel.find("/channels/") {
                Some(i) => self.resolve_storage(rel.get(i + 1..).unwrap_or_default()),
                None => err("stored media path is outside storage"),
            };
        }
        if rel.split('/').any(|c| c == "..") {
            return err("unsafe storage path");
        }
        Ok(self.storage.join(rel))
    }

    /// The storage-relative form of a file inside storage.
    pub fn relative_storage(&self, file: &Path) -> Result<String> {
        let rel = file.strip_prefix(&self.storage).map_err(|_| StudioError::Other("file is outside storage".into()))?;
        Ok(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
    }
}

/// `KEY=value` lines; `#` comments and blank lines skipped, surrounding quotes removed.
pub fn read_env(path: &Path) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else { return out };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        let v = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))).unwrap_or(v);
        out.insert(k.trim().to_string(), v.to_string());
    }
    out
}

#[cfg(test)]
mod tests;

/// Publishes an approved render on one account now: claims the job, reads the account's key,
/// uploads, and records the outcome. Blocking; call it off the UI thread. Returns the link.
pub fn publish_now(studio: &Studio, channel: i64, render: i64, account: i64) -> Result<Option<String>> {
    let job = studio.claim(channel, render, account)?;
    run_job(studio, job, channel, render, account)
}

fn run_job(studio: &Studio, job: i64, channel: i64, render: i64, account: i64) -> Result<Option<String>> {
    let outcome = (|| -> instagram::Published {
        let (_, a) = studio.account(account)?;
        let secret = studio.secret(&a.credentials_reference)?;
        let file = {
            let c = studio.connect()?;
            let rel: String = c.query_row("SELECT file_path FROM renders WHERE id = ?1", [render], |r| r.get(0)).map_err(StudioError::from)?;
            studio.resolve_storage(&rel)?
        };
        let caption = studio.caption_for(render)?;
        match a.platform.as_str() {
            "INSTAGRAM" => {
                let token = secret
                    .get("accessToken")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| instagram::PublishError::Retry("missing Meta token; reconnect the account".into()))?;
                instagram::publish(studio, job, channel, &a.external_id, token, &file, &caption)
            }
            "YOUTUBE" => {
                let script = studio
                    .connect()?
                    .query_row("SELECT s.json FROM renders r JOIN scripts s ON s.id = r.script_id WHERE r.id = ?1", [render], |r| r.get::<_, String>(0))
                    .map_err(StudioError::from)?;
                let v: serde_json::Value = serde_json::from_str(&script).unwrap_or_default();
                let title = v.get("shortHeadline").and_then(serde_json::Value::as_str).unwrap_or("Reel").to_string();
                let tags: Vec<String> = v
                    .get("hashtags")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| a.iter().filter_map(|t| t.as_str()).map(|t| t.trim_start_matches('#').to_string()).filter(|t| !t.is_empty()).collect())
                    .unwrap_or_default();
                youtube::publish(studio, job, channel, &secret, &file, &title, &caption, &tags)
            }
            other => Err(instagram::PublishError::Retry(format!("{other} publishing is not supported"))),
        }
    })();
    match outcome {
        Ok((id, link)) => {
            studio.finish_publication(job, Ok((id, link.clone())))?;
            Ok(link)
        }
        Err(instagram::PublishError::Reconcile(m)) => {
            studio.finish_publication(job, Err((true, m.clone())))?;
            err(m)
        }
        Err(instagram::PublishError::Retry(m)) => {
            studio.finish_publication(job, Err((false, m.clone())))?;
            err(m)
        }
    }
}

/// Runs scheduled publications that are due, unless the earlier app's worker is online (it owns
/// them then). Returns what happened to each, for the activity log.
pub fn run_due(studio: &Studio) -> Result<Vec<String>> {
    if studio.pipeline_online()? {
        return Ok(Vec::new());
    }
    let mut log = Vec::new();
    for (job, channel, render, account) in studio.due_publications()? {
        let claimed = {
            let c = studio.connect()?;
            c.execute(
                "UPDATE publishing_jobs SET status = 'UPLOADING', started_at = COALESCE(started_at, ?1), attempt_count = attempt_count + 1, updated_at = ?1 WHERE id = ?2 AND status = 'SCHEDULED'",
                rusqlite::params![now_iso(), job],
            )?
        };
        if claimed == 0 {
            continue;
        }
        match run_job(studio, job, channel, render, account) {
            Ok(link) => log.push(format!("Published scheduled video {render}{}", link.map(|l| format!(": {l}")).unwrap_or_default())),
            Err(e) => log.push(format!("Scheduled video {render} failed: {e}")),
        }
    }
    Ok(log)
}

/// Probes a video file with FilmCraft's own demuxers: (duration seconds, width, height).
pub fn probe(file: &Path) -> Result<(f64, u32, u32)> {
    let reader: filmcraft_media::reader::SharedReader = std::sync::Arc::new(filmcraft_media::reader::FileReader::open(file)?);
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let src = filmcraft_media::reader::open_reader(&name, reader, &filmcraft_codecs::reader_openers(), &filmcraft_codecs::openers())
        .map_err(|e| StudioError::Other(format!("the video is unreadable: {e}")))?;
    let info = src.info();
    let v = info.video.as_ref().ok_or_else(|| StudioError::Other("the file has no picture".into()))?;
    Ok((info.duration.seconds(), v.width, v.height))
}
