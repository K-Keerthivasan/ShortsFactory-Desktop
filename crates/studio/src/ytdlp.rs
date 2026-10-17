//! Video links into a workspace with yt-dlp, driven directly by the app: read the link's details,
//! download it as MP4 with the chosen options (quality, section, cookies, rate limit, format),
//! move it into the workspace's clips folder, check it is readable and as long as the source says,
//! add it to the library, then let the local AI draft the card.
//!
//! Downloaded footage is kept when the AI step fails, so a retry never downloads it again.
//! yt-dlp is an external program: `YTDLP_PATH`, `<library>\tools\yt-dlp.exe` or `PATH`. Merging
//! separate video and audio streams needs FFmpeg (`FFMPEG_PATH`, or the earlier app's bundled
//! copy); without it, downloads use the best single-file MP4.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use filmcraft_reel::Card;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::clock::now_ms;
use crate::{Result, Studio, StudioError, err};

/// How to download.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Options {
    /// "best", "1080", "720", "480".
    pub quality: String,
    /// A yt-dlp format expression that overrides `quality` when set.
    pub format: String,
    /// "", "chrome", "edge", "firefox", "brave": sign-in cookies for links that need them.
    pub cookies_from: String,
    /// e.g. "5M"; empty is unlimited.
    pub rate_limit: String,
    /// Download only this part of the video (`HH:MM:SS` or seconds); empty is the whole.
    pub section_start: String,
    pub section_end: String,
}
impl Default for Options {
    fn default() -> Self {
        Options {
            quality: "best".into(),
            format: String::new(),
            cookies_from: String::new(),
            rate_limit: String::new(),
            section_start: String::new(),
            section_end: String::new(),
        }
    }
}

/// Where one link's import has got to. Kept by the caller between attempts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Import {
    pub channel_id: Option<i64>,
    pub url: String,
    pub notes: String,
    pub options: Options,
    pub stage: String,
    pub progress: f64,
    pub title: String,
    pub metadata: Option<Value>,
    pub asset_id: Option<i64>,
    pub file: Option<PathBuf>,
    pub duration: Option<f64>,
    pub card: Option<Card>,
    pub post: Option<filmcraft_reel::PostLink>,
    pub error: Option<String>,
    /// Legacy serialized field. Downloads now belong to the workspace, independently of projects.
    #[serde(default)]
    pub save_to: Option<PathBuf>,
    /// Stop after the download (the AI workflow writes a card per segment instead of one draft).
    #[serde(default)]
    pub no_draft: bool,
}

/// yt-dlp's executable, if one is installed.
pub fn binary(studio: &Studio) -> Option<PathBuf> {
    if let Some(p) = studio.var("YTDLP_PATH").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let local = tools_dir(studio).join(if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" });
    if local.is_file() {
        return Some(local);
    }
    let name = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

pub fn tools_dir(studio: &Studio) -> PathBuf {
    studio.root.join("tools")
}

/// FFmpeg for merging streams: `FFMPEG_PATH`, else the earlier app's bundled copy, else PATH.
pub fn ffmpeg(studio: &Studio) -> Option<PathBuf> {
    let name = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let mut candidates: Vec<PathBuf> = studio.var("FFMPEG_PATH").map(PathBuf::from).into_iter().collect();
    candidates.push(tools_dir(studio).join(name));
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("Programs").join("Shorts Factory Desktop").join("resources").join("bin").join(name));
    }
    if let Some(p) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&p).map(|d| d.join(name)));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Downloads the latest official yt-dlp release into the library's tools folder. Only run when
/// the user asks for it.
pub fn install(studio: &Studio) -> Result<PathBuf> {
    let dir = tools_dir(studio);
    std::fs::create_dir_all(&dir)?;
    let (asset, name) = if cfg!(windows) { ("yt-dlp.exe", "yt-dlp.exe") } else { ("yt-dlp", "yt-dlp") };
    let url = format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{asset}");
    let resp = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .get(&url)
        .call()
        .map_err(|e| StudioError::Other(format!("could not download yt-dlp: {e}")))?;
    let target = dir.join(name);
    let part = target.with_extension("part");
    let mut out = std::fs::File::create(&part)?;
    std::io::copy(&mut resp.into_reader(), &mut out)?;
    drop(out);
    std::fs::rename(&part, &target)?;
    Ok(target)
}

/// Uses `file` as the app's FFmpeg: it must run and say it is FFmpeg, then it is copied into the
/// tools folder (where downloads look for it first after FFMPEG_PATH).
pub fn use_ffmpeg(studio: &Studio, file: &Path) -> Result<PathBuf> {
    let mut c = Command::new(file);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    let out = c.args(["-hide_banner", "-version"]).stdin(Stdio::null()).output().map_err(|_| StudioError::Other("that file does not run".into()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !text.starts_with("ffmpeg version") {
        return err("that file is not FFmpeg");
    }
    let dir = tools_dir(studio);
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
    if target != file {
        let part = target.with_extension("part");
        std::fs::copy(file, &part)?;
        std::fs::rename(&part, &target)?;
    }
    Ok(target)
}

/// Run the installed tool, rather than treating the presence of a file as a successful setup.
pub fn version(studio: &Studio) -> Result<String> {
    let out = base(studio, &Options::default())?.arg("--version").stdin(Stdio::null()).output()?;
    if !out.status.success() {
        return err("yt-dlp did not start. Install it again from Downloads.");
    }
    let version = String::from_utf8_lossy(&out.stdout).trim().chars().take(100).collect::<String>();
    if version.is_empty() {
        return err("yt-dlp returned no version");
    }
    Ok(version)
}

fn base(studio: &Studio, o: &Options) -> Result<Command> {
    let bin = binary(studio).ok_or_else(|| StudioError::Other("yt-dlp is not installed. Use \"Install yt-dlp\" in the panel, or set YTDLP_PATH.".into()))?;
    let mut c = Command::new(bin);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    c.args(["--no-playlist", "--no-warnings", "--no-color"]);
    if !o.cookies_from.trim().is_empty() {
        let b = o.cookies_from.trim().to_ascii_lowercase();
        if !["chrome", "edge", "firefox", "brave", "chromium", "opera", "vivaldi"].contains(&b.as_str()) {
            return err("unknown browser for cookies");
        }
        c.args(["--cookies-from-browser", &b]);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // no console window
    }
    Ok(c)
}

/// A link's details (title, duration, uploader, description), without downloading.
pub fn analyze(studio: &Studio, url: &str, o: &Options) -> Result<Value> {
    let out = base(studio, o)?.args(["-J", "--", url]).stdin(Stdio::null()).output()?;
    if !out.status.success() {
        return err(format!("yt-dlp could not read the link: {}", String::from_utf8_lossy(&out.stderr).trim().chars().take(300).collect::<String>()));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| StudioError::Other(format!("yt-dlp details unreadable: {e}")))
}

fn time_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 12 && s.bytes().all(|b| b.is_ascii_digit() || b == b':' || b == b'.')
}

fn seconds(s: &str) -> Option<f64> {
    s.split(':').try_fold(0.0, |acc, part| part.parse::<f64>().ok().map(|v| acc * 60.0 + v))
}

/// The format expression for the options; merged formats need FFmpeg.
pub fn format_for(o: &Options, can_merge: bool) -> String {
    if !o.format.trim().is_empty() {
        return o.format.trim().to_string();
    }
    let cap = match o.quality.as_str() {
        "1080" => "[height<=1080]",
        "720" => "[height<=720]",
        "480" => "[height<=480]",
        _ => "",
    };
    if can_merge { format!("bv*{cap}[ext=mp4]+ba[ext=m4a]/bv*{cap}+ba/b{cap}[ext=mp4]/b{cap}") } else { format!("b{cap}[ext=mp4]/b{cap}") }
}

/// Runs (or resumes) one import. `update` sees every stage; the import is returned whatever
/// happened, with `error` set if it stopped.
pub fn run(studio: &Studio, channel: i64, mut job: Import, update: &dyn Fn(&Import)) -> Import {
    job.channel_id = Some(channel);
    job.error = None;
    if let Err(e) = step(studio, channel, &mut job, update) {
        job.error = Some(e.to_string());
        job.stage = "Failed".into();
    }
    update(&job);
    job
}

fn step(studio: &Studio, channel: i64, job: &mut Import, update: &dyn Fn(&Import)) -> Result<()> {
    let set = |job: &mut Import, stage: &str| {
        job.stage = stage.into();
        update(job);
    };
    if !(job.url.starts_with("http://") || job.url.starts_with("https://")) {
        return err("use an HTTP video link");
    }
    let o = job.options.clone();
    for t in [&o.section_start, &o.section_end] {
        if !t.is_empty() && !time_ok(t) {
            return err("section times are HH:MM:SS or seconds");
        }
    }
    if !o.rate_limit.is_empty() && !o.rate_limit.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.') {
        return err("rate limit looks like 5M or 500K");
    }
    if job.file.is_none() {
        if job.metadata.is_none() {
            set(job, "Reading the link");
            job.metadata = Some(analyze(studio, &job.url, &o)?);
        }
        let meta = job.metadata.clone().unwrap_or(Value::Null);
        if meta.get("_type").and_then(Value::as_str) == Some("playlist") {
            return err("import individual video links, not playlists");
        }
        job.title = meta.get("title").and_then(Value::as_str).unwrap_or("Imported video").chars().take(200).collect();
        let (slug,): (String,) = studio.connect()?.query_row("SELECT slug FROM channels WHERE id = ?1", [channel], |r| Ok((r.get(0)?,)))?;
        // Downloads belong to the workspace. A custom folder is indexed by reference too.
        let dir = studio.app_settings().download_dir.unwrap_or_else(|| studio.storage.join("channels").join(&slug).join("assets").join("clips"));
        std::fs::create_dir_all(&dir)?;
        let stem = format!("import-{}", now_ms());
        // Partial files stay beside the result (same drive, so the finished file is moved, not copied).
        let work = dir.join(".downloading");
        std::fs::create_dir_all(&work)?;
        let template = work.join(format!("{stem}.%(ext)s"));
        let merge = ffmpeg(studio);
        let mut c = base(studio, &o)?;
        c.args([
            "--newline",
            "--progress-template",
            "download:PROGRESS %(progress._percent_str)s",
            "-f",
            &format_for(&o, merge.is_some()),
            "--merge-output-format",
            "mp4",
            "-o",
        ])
        .arg(&template);
        if let Some(f) = &merge {
            c.arg("--ffmpeg-location").arg(f);
        }
        if !o.rate_limit.is_empty() {
            c.args(["-r", &o.rate_limit]);
        }
        let sectioned = !o.section_start.is_empty() || !o.section_end.is_empty();
        if sectioned {
            let a = if o.section_start.is_empty() { "0" } else { &o.section_start };
            let b = if o.section_end.is_empty() { "inf" } else { &o.section_end };
            c.args(["--download-sections", &format!("*{a}-{b}"), "--force-keyframes-at-cuts"]);
        }
        c.args(["--", &job.url]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        set(job, "Downloading");
        let mut child = c.spawn()?;
        let stderr = child.stderr.take();
        let errors = std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut e) = stderr {
                let _ = std::io::Read::read_to_string(&mut e, &mut s);
            }
            s
        });
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(std::result::Result::ok) {
                if let Some(p) = line.strip_prefix("PROGRESS ").and_then(|v| v.trim().trim_end_matches('%').trim().parse::<f64>().ok()) {
                    job.progress = p;
                    update(job);
                }
            }
        }
        let status = child.wait()?;
        let stderr = errors.join().unwrap_or_default();
        if !status.success() {
            // YouTube mostly offers picture and sound as separate streams now; joining them needs FFmpeg.
            if merge.is_none() && stderr.contains("Requested format is not available") {
                return err(
                    "This video has no single file with picture and sound, and FFmpeg (which joins them) was not found. Set it in Plugin Settings > Downloads & tools (Choose FFmpeg), then retry",
                );
            }
            return err(format!("yt-dlp failed: {}", stderr.trim().lines().last().unwrap_or("no message")));
        }
        let downloaded = std::fs::read_dir(&work)?
            .filter_map(std::result::Result::ok)
            .map(|e| e.path())
            .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(stem.as_str()) && p.extension().and_then(|e| e.to_str()) != Some("part"))
            .ok_or_else(|| StudioError::Other("yt-dlp finished but left no video".into()))?;
        set(job, "Checking the video");
        let (duration, _, _) = match crate::probe(&downloaded) {
            Ok(x) => x,
            Err(e) => {
                let _ = std::fs::remove_file(&downloaded);
                return err(format!("the downloaded video is empty or unreadable ({e}); retry or change the options"));
            }
        };
        let source = meta.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
        let expected = if sectioned {
            let a = seconds(&o.section_start).unwrap_or(0.0);
            let b = seconds(&o.section_end).unwrap_or(source);
            (b - a).max(0.0)
        } else {
            source
        };
        if duration.is_nan() || duration <= 0.0 || (expected > 0.0 && (duration - expected).abs() > 5f64.max(expected * 0.05)) {
            let _ = std::fs::remove_file(&downloaded);
            return err("the downloaded video's length differs from the source; review the link or the section");
        }
        let ext = downloaded.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_ascii_lowercase();
        let file = dir.join(format!("{stem}.{ext}"));
        if std::fs::rename(&downloaded, &file).is_err() {
            std::fs::copy(&downloaded, &file)?;
            let _ = std::fs::remove_file(&downloaded);
        }
        // Register both workspace and custom destinations in Create without importing a project item.
        job.file = Some(file);
        job.duration = Some(duration);
    }
    if job.asset_id.is_none()
        && let Some(file) = &job.file
    {
        job.asset_id = Some(studio.register_download(channel, file, &job.title, &job.url)?);
    }
    if job.no_draft {
        set(job, "Downloaded");
        return Ok(());
    }
    if job.card.is_none() {
        set(job, "Writing draft with local AI");
        let meta = job.metadata.clone().unwrap_or(Value::Null);
        let uploader = meta.get("uploader").or_else(|| meta.get("channel")).and_then(Value::as_str).unwrap_or("");
        let text = format!(
            "Title: {}\nUploader: {uploader}\nDescription: {}\n\nNotes: {}",
            job.title,
            meta.get("description").and_then(Value::as_str).unwrap_or("").chars().take(4000).collect::<String>(),
            job.notes
        );
        let mut card = crate::ai::write_card(studio, channel, &text, &job.url)?;
        if card.attribution.is_empty() && !uploader.is_empty() {
            card.attribution = format!("Original: {uploader}");
        }
        job.card = Some(card);
    }
    if job.post.is_none()
        && let (Some(card), Some(asset)) = (job.card.clone(), job.asset_id)
    {
        job.post = Some(studio.create_draft(channel, &card, asset, job.duration.unwrap_or(30.0), &job.url)?);
    }
    set(job, "Ready for review");
    Ok(())
}
