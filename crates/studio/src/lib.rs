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
pub mod autopilot;
mod clock;
pub mod cloud;
pub mod commands;
pub mod data;
pub mod instagram;
mod instagram_cloud;
pub mod meta_token;
pub mod notifications;
pub mod settings;
pub mod workflow;
pub mod cloud_youtube;
pub mod thumbnails;
pub mod writing;
pub mod youtube;
pub mod youtube_connect;
pub mod youtube_queue;
pub mod ytdlp;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub use clock::{iso as iso_ms, local_in, local_to_utc_iso, now_iso, parse_iso as clock_ms};

/// Where the app keeps a project's own files (renders, their thumbnails, downloads): beside the
/// project file, in `<project folder>/<project name> Media/<part>`. A project that has not been
/// saved has no folder yet, so nothing is written for it.
pub fn project_folder(project_file: Option<&str>, part: &str) -> Result<PathBuf> {
    let Some(dir) = project_folder_path(project_file, part) else {
        return err("Save the project first (File > Save As): renders and downloads are kept in the project's folder");
    };
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// [`project_folder`] without creating it (to show where files will go).
pub fn project_folder_path(project_file: Option<&str>, part: &str) -> Option<PathBuf> {
    let file = project_file.map(Path::new).filter(|f| f.parent().is_some_and(|d| !d.as_os_str().is_empty()))?;
    let name = file.file_stem().and_then(|s| s.to_str()).unwrap_or("Project");
    Some(file.parent()?.join(format!("{name} Media")).join(part))
}
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

/// A render's YouTube title, description and tags: the Short's own title if one was written, else
/// the headline; the workspace's YouTube description template around the post's caption. YouTube refuses < and > in titles and
/// descriptions, and #Shorts marks the vertical video as a Short.
pub(crate) fn short_text(studio: &Studio, render: i64) -> Result<(String, String, Vec<String>)> {
    let script = studio
        .connect()?
        .query_row("SELECT s.json FROM renders r JOIN scripts s ON s.id = r.script_id WHERE r.id = ?1", [render], |r| r.get::<_, String>(0))
        .map_err(StudioError::from)?;
    let v: serde_json::Value = serde_json::from_str(&script).unwrap_or_default();
    let title = v
        .get("youtubeTitle")
        .and_then(serde_json::Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| v.get("shortHeadline").and_then(serde_json::Value::as_str))
        .unwrap_or("Reel")
        .to_string();
    let tags: Vec<String> = v
        .get("hashtags")
        .and_then(serde_json::Value::as_array)
        .map(|a| a.iter().filter_map(|t| t.as_str()).map(|t| t.trim_start_matches('#').to_string()).filter(|t| !t.is_empty()).collect())
        .unwrap_or_default();
    let clean = |s: &str| s.replace(['<', '>'], "");
    // The workspace's YouTube description template (Plugin Settings ▸ Writing).
    let description = clean(&studio.youtube_description(render)?);
    Ok((clean(&title).chars().take(100).collect(), description, tags))
}

/// Is the render a long video (a library post of kind VIDEO, 16:9) rather than a Reel?
pub fn long_video(studio: &Studio, render: i64) -> bool {
    studio
        .connect()
        .ok()
        .and_then(|c| c.query_row("SELECT s.json FROM renders r JOIN scripts s ON s.id = r.script_id WHERE r.id = ?1", [render], |r| r.get::<_, String>(0)).ok())
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .is_some_and(|v| v.get("contentKind").and_then(serde_json::Value::as_str) == Some("VIDEO"))
}

pub(crate) fn run_job(studio: &Studio, job: i64, channel: i64, render: i64, account: i64) -> Result<Option<String>> {
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
                let cloud_pending = studio.remote_state(job)?.get("cloudKey").is_some();
                if cloud_pending || (cloud::configured(studio) && instagram::can_replace_upload(studio, job, &a.external_id, token)?) {
                    return instagram_cloud::publish(studio, job, channel, render, &a.external_id, &caption);
                }
                // The cover is the thumbnail's moment (Instagram takes a frame of the video).
                let cover = thumbnails::ensure(studio, render).ok().map(|c| (c.at * 1000.0).round() as u64);
                let result = instagram::publish(studio, job, channel, &a.external_id, token, &file, &caption, cover);
                if matches!(result, Err(instagram::PublishError::Retry(_)))
                    && cloud::configured(studio)
                    && instagram::can_replace_upload(studio, job, &a.external_id, token).unwrap_or(false)
                {
                    instagram_cloud::publish(studio, job, channel, render, &a.external_id, &caption)
                } else {
                    result
                }
            }
            "YOUTUBE" => {
                let (title, description, tags) = short_text(studio, render)?;
                // A job scheduled for later is uploaded now and published by YouTube at its time.
                let publish_at = studio
                    .connect()?
                    .query_row("SELECT scheduled_at FROM publishing_jobs WHERE id = ?1", [job], |r| r.get::<_, String>(0))
                    .ok()
                    .and_then(|s| clock::parse_iso(&s))
                    .filter(|t| *t > clock::now_ms() + 60_000)
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
                let published = youtube::publish(studio, job, channel, &secret, &file, &title, &description, &tags, publish_at.as_deref());
                if let Ok((id, _)) = &published {
                    youtube::thumbnail_once(studio, job, channel, &secret, id, render);
                }
                published
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
/// What [`schedule_everywhere`] did: Instagram (and other) jobs left for their time, and each
/// YouTube account's upload (the video link, or why it failed).
#[derive(Debug, Default)]
pub struct Scheduled {
    pub waiting: usize,
    pub youtube: Vec<(i64, std::result::Result<Option<String>, String>)>,
}

/// Schedules an approved render on `accounts` at `at_iso`. Instagram jobs wait in the library
/// for their time (this app or the earlier one posts them); YouTube does its own scheduling, so
/// those are uploaded now as Private with that publish time, and YouTube publishes them even if
/// this PC is off. Blocking: the YouTube uploads happen here.
pub fn schedule_everywhere(studio: &Studio, channel: i64, render: i64, accounts: &[i64], at_iso: &str) -> Result<Scheduled> {
    let at = clock::parse_iso(at_iso).ok_or_else(|| StudioError::Other("unreadable schedule time".into()))?;
    let mut youtube = Vec::new();
    let mut other = Vec::new();
    for a in accounts {
        let (_, account) = studio.account(*a)?;
        if account.platform == "YOUTUBE" { youtube.push(*a) } else { other.push(*a) }
    }
    if !youtube.is_empty() && at < clock::now_ms() + 5 * 60_000 {
        return err("YouTube schedules must be at least five minutes ahead; use Publish now for sooner");
    }
    let waiting = if other.is_empty() { 0 } else { studio.schedule(channel, render, &other, at_iso)? };
    let when = clock::iso(at);
    let mut out = Scheduled { waiting, youtube: Vec::new() };
    for a in youtube {
        let result = (|| -> Result<Option<String>> {
            let job = studio.claim(channel, render, a)?;
            // The job keeps the chosen time: the calendar shows it, and no worker posts it again.
            studio.connect()?.execute("UPDATE publishing_jobs SET scheduled_at = ?1 WHERE id = ?2", rusqlite::params![when, job])?;
            run_job(studio, job, channel, render, a)
        })();
        out.youtube.push((a, result.map_err(|e| e.to_string())));
    }
    Ok(out)
}

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
