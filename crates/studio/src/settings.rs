//! Plugin settings: Shorts Factory's own preferences (`Editor/shorts-factory.json`), the library's
//! `.env` keys (cloud, AI, tool paths) edited in place, account keys, connection checks and the
//! publishing calendar.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, Studio, StudioError, err};

/// Shorts Factory Desktop's own settings, beside the editor's.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    /// Where yt-dlp downloads go. None: the workspace's clips folder (added to its library).
    pub download_dir: Option<PathBuf>,
    /// Queue: approve renders automatically when they reach the library.
    pub queue_auto_approve: bool,
    /// Show the start-up connection check.
    pub startup_check: bool,
    /// An AI provider only this app has ("codex"), kept here because the earlier app's settings
    /// accept Hermes, Ollama and mock only. None: the shared library setting.
    pub ai_provider: Option<String>,
    /// Workspace clips marked done in Create (media asset ids; this app's own marks).
    pub done_clips: Vec<i64>,
    /// The YouTube queue rests until then after Google reported the daily quota used up.
    pub youtube_paused_until: Option<String>,
}

impl Studio {
    fn app_settings_file(&self) -> PathBuf {
        self.root.join("Editor").join("shorts-factory.json")
    }
    pub fn app_settings(&self) -> AppSettings {
        std::fs::read_to_string(self.app_settings_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(AppSettings {
            download_dir: None,
            queue_auto_approve: true,
            startup_check: true,
            ai_provider: None,
            done_clips: Vec::new(),
            youtube_paused_until: None,
        })
    }
    pub fn save_app_settings(&self, s: &AppSettings) -> Result<()> {
        let file = self.app_settings_file();
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(s).map_err(|e| StudioError::Other(e.to_string()))?)?;
        std::fs::rename(tmp, file)?;
        Ok(())
    }

    /// Writes `values` into the library's `.env`, keeping every other line (and comment) as it
    /// was; an empty value removes the key. Only the keys Shorts Factory manages are accepted.
    pub fn write_env(&mut self, values: &[(&str, &str)]) -> Result<()> {
        for (k, v) in values {
            if !EDITABLE_ENV.contains(k) {
                return err(format!("{k} is not a setting Shorts Factory edits"));
            }
            if v.contains('\n') || v.contains('\r') {
                return err(format!("{k} must be one line"));
            }
        }
        let file = self.root.join(".env");
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        for (k, v) in values {
            let at = lines.iter().position(|l| l.trim_start().split_once('=').is_some_and(|(key, _)| key.trim() == *k));
            let line = format!("{k}={v}");
            match (at, v.is_empty()) {
                (Some(i), true) => {
                    lines.remove(i);
                }
                (Some(i), false) => {
                    if let Some(l) = lines.get_mut(i) {
                        *l = line;
                    }
                }
                (None, false) => lines.push(line),
                (None, true) => {}
            }
            if v.is_empty() {
                self.env.remove(*k);
            } else {
                self.env.insert((*k).to_string(), (*v).to_string());
            }
        }
        let tmp = file.with_extension("env.tmp");
        std::fs::write(&tmp, lines.join("\n") + "\n")?;
        std::fs::rename(tmp, file)?;
        Ok(())
    }

    /// Replaces an account's access token in its key file (other fields kept). The token is
    /// written only to the account's own file and never logged.
    pub fn set_account_token(&self, account: i64, token: &str) -> Result<()> {
        let token = token.trim();
        if token.len() < 20 || token.chars().any(char::is_whitespace) {
            return err("Paste the whole access token");
        }
        let (_, a) = self.account(account)?;
        let file = self.credential_path(&a.credentials_reference)?;
        let mut secret = if file.exists() { self.secret(&a.credentials_reference)? } else { json!({}) };
        if !secret.is_object() {
            return err("Account credentials must be a JSON object; the existing file was kept");
        }
        secret["accessToken"] = json!(token);
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, secret.to_string())?;
        std::fs::rename(tmp, file)?;
        let c = self.connect()?;
        c.execute("UPDATE social_accounts SET status = 'UNVERIFIED', updated_at = ?1 WHERE id = ?2", params![crate::now_iso(), account])?;
        Ok(())
    }

    /// A YouTube account's visibility for new uploads: "public", "unlisted" or "private".
    pub fn youtube_privacy(&self, account: i64) -> Result<String> {
        let (_, a) = self.account(account)?;
        let secret = self.secret(&a.credentials_reference)?;
        Ok(secret.get("privacyStatus").and_then(Value::as_str).filter(|p| ["public", "unlisted", "private"].contains(p)).unwrap_or("public").to_string())
    }

    /// Sets a YouTube account's visibility for new uploads (its other key fields are kept).
    pub fn set_youtube_privacy(&self, account: i64, privacy: &str) -> Result<()> {
        if !["public", "unlisted", "private"].contains(&privacy) {
            return err("visibility is public, unlisted or private");
        }
        let (_, a) = self.account(account)?;
        if a.platform != "YOUTUBE" {
            return err("only YouTube accounts have a visibility");
        }
        let file = self.credential_path(&a.credentials_reference)?;
        let mut secret = self.secret(&a.credentials_reference)?;
        if !secret.is_object() {
            return err("the account's key file is unreadable; reconnect YouTube");
        }
        secret["privacyStatus"] = json!(privacy);
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, secret.to_string())?;
        std::fs::rename(tmp, file)?;
        Ok(())
    }

    /// Checks an account's key against its platform. With `record`, the result is written on the
    /// account (the earlier app reads that status); the automatic start-up check never records.
    pub fn test_account(&self, account: i64, record: bool) -> Result<String> {
        let (_, a) = self.account(account)?;
        let secret = self.secret(&a.credentials_reference)?;
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).build();
        let outcome = match a.platform.as_str() {
            "INSTAGRAM" => {
                let token = secret.get("accessToken").and_then(Value::as_str).unwrap_or_default().trim();
                let version = self.var("META_GRAPH_VERSION").unwrap_or("v26.0");
                match agent
                    .get(&format!("https://graph.facebook.com/{version}/{}", a.external_id))
                    .set("Authorization", &format!("Bearer {token}"))
                    .query("fields", "id,username")
                    .call()
                {
                    Ok(r) => Ok(r
                        .into_json::<Value>()
                        .ok()
                        .and_then(|v| v.get("username").and_then(Value::as_str).map(|u| format!("@{u}")))
                        .unwrap_or_else(|| a.display_name.clone())),
                    Err(ureq::Error::Status(code, r)) => {
                        // Meta's own reason (type, code, subcode, message) carries no secret.
                        let v: Value = r.into_json().unwrap_or(Value::Null);
                        let e = v.get("error");
                        let field = |k: &str| e.and_then(|e| e.get(k)).map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string)).unwrap_or_default();
                        Err(format!(
                            "Instagram refused the key (HTTP {code}, {} {} {}): {}",
                            field("type"),
                            field("code"),
                            field("error_subcode"),
                            field("message").chars().take(200).collect::<String>()
                        ))
                    }
                    Err(e) => Err(format!("Instagram is not reachable: {e}")),
                }
            }
            "YOUTUBE" => {
                let get = |k: &str| secret.get(k).and_then(Value::as_str).unwrap_or_default();
                match agent.post("https://oauth2.googleapis.com/token").send_form(&[
                    ("client_id", get("clientId")),
                    ("client_secret", get("clientSecret")),
                    ("refresh_token", get("refreshToken")),
                    ("grant_type", "refresh_token"),
                ]) {
                    // The key works: say which channel it posts to (personal or a Brand Account).
                    Ok(r) => {
                        let access = r.into_json::<Value>().ok().and_then(|v| v.get("access_token").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
                        let channel = agent
                            .get("https://www.googleapis.com/youtube/v3/channels")
                            .set("Authorization", &format!("Bearer {access}"))
                            .query("part", "snippet")
                            .query("mine", "true")
                            .call()
                            .ok()
                            .and_then(|r| r.into_json::<Value>().ok())
                            .and_then(|v| v.pointer("/items/0/snippet/title").and_then(Value::as_str).map(str::to_string));
                        Ok(channel.map_or_else(|| a.display_name.clone(), |c| format!("YouTube channel “{c}”")))
                    }
                    Err(ureq::Error::Status(code, _)) => Err(format!("Google refused the key (HTTP {code}); reconnect YouTube")),
                    Err(e) => Err(format!("Google is not reachable: {e}")),
                }
            }
            other => Err(format!("{other} accounts cannot be checked")),
        };
        if !record {
            return outcome.map_err(StudioError::Other);
        }
        let c = self.connect()?;
        let now = crate::now_iso();
        match outcome {
            Ok(name) => {
                c.execute("UPDATE social_accounts SET status = 'CONNECTED', last_verified_at = ?1, updated_at = ?1 WHERE id = ?2", params![now, account])?;
                Ok(name)
            }
            Err(e) => {
                c.execute("UPDATE social_accounts SET status = 'ERROR', updated_at = ?1 WHERE id = ?2", params![now, account])?;
                err(e)
            }
        }
    }

    /// Publications in `[from, to)` (ISO), all workspaces or one: (day `YYYY-MM-DD` in UTC,
    /// time, channel, title, platform, status, link).
    pub fn calendar(&self, channel: Option<i64>, from: &str, to: &str) -> Result<Vec<CalendarEntry>> {
        let c = self.connect()?;
        let mut q = c.prepare(
            "SELECT p.scheduled_at, p.channel_id, COALESCE(r.name, s.canonical_story_title), p.platform, p.status, p.remote_url, p.render_id
             FROM publishing_jobs p JOIN renders r ON r.id = p.render_id JOIN story_clusters s ON s.id = r.story_id
             WHERE p.scheduled_at >= ?1 AND p.scheduled_at < ?2 AND (?3 IS NULL OR p.channel_id = ?3) ORDER BY p.scheduled_at",
        )?;
        let rows = q.query_map(params![from, to, channel], |r| {
            Ok(CalendarEntry {
                at: r.get(0)?,
                channel: r.get(1)?,
                title: r.get(2)?,
                platform: r.get(3)?,
                status: r.get(4)?,
                link: r.get(5)?,
                render: r.get(6)?,
                cloud: false,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

/// One publication on the calendar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CalendarEntry {
    pub at: String,
    pub channel: i64,
    pub title: String,
    pub platform: String,
    pub status: String,
    pub link: Option<String>,
    pub render: i64,
    /// Scheduled through the Supabase backend rather than the local queue.
    pub cloud: bool,
}

/// The `.env` keys the Plugin Settings window edits.
pub const EDITABLE_ENV: &[&str] = &[
    "CLOUD_SUPABASE_URL",
    "CLOUD_SUPABASE_ANON_KEY",
    "CLOUD_AUTH_EMAIL",
    "CLOUD_AUTH_PASSWORD",
    "AI_PROVIDER",
    "OLLAMA_HOST",
    "OLLAMA_MODEL",
    "HERMES_BIN",
    "HERMES_MODEL_POST",
    "HERMES_PROVIDER",
    "HERMES_TIMEOUT_MS",
    "AI_FALLBACK",
    "NVIDIA_API_KEY",
    "NVIDIA_MODEL",
    "NVIDIA_VISION_MODEL",
    "VISION_PROVIDER",
    "OLLAMA_VISION_MODEL",
    "KLAP_API_KEY",
    "CODEX_BIN",
    "CODEX_MODEL",
    "YTDLP_PATH",
    "FFMPEG_PATH",
    "META_GRAPH_VERSION",
    "META_APP_ID",
    "YOUTUBE_CLIENT_ID",
    "YOUTUBE_CLIENT_SECRET",
    "YOUTUBE_PER_DAY",
    "YOUTUBE_SLOTS",
    "META_APP_SECRET",
];

/// Results of the start-up connection check.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HealthReport {
    pub lines: Vec<(String, bool)>,
}

/// Checks the AI provider, every workspace's publishing accounts and the cloud. Blocking.
pub fn health_check(studio: &Studio) -> HealthReport {
    let mut out = HealthReport::default();
    let provider = crate::ai::provider(studio);
    let ai = match provider.as_str() {
        "ollama" => {
            let host = crate::ai::ollama_host(studio.var("OLLAMA_HOST"));
            match ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).build().get(&format!("{host}/api/tags")).call() {
                Ok(_) => (format!("AI: Ollama is running at {host}"), true),
                Err(_) => (format!("AI: Ollama is not reachable at {host}"), false),
            }
        }
        "mock" => ("AI: the mock provider is on (no real AI)".to_string(), true),
        other => {
            let (name, var, default) = if other == "codex" { ("Codex", "CODEX_BIN", "codex") } else { ("Hermes", "HERMES_BIN", "hermes") };
            let bin = studio.var(var).unwrap_or(default);
            let found = Path::new(bin).is_file()
                || std::env::var_os("PATH").is_some_and(|p| {
                    std::env::split_paths(&p).any(|d| d.join(bin).is_file() || d.join(format!("{bin}.exe")).is_file() || d.join(format!("{bin}.cmd")).is_file())
                });
            if found { (format!("AI: {name} is installed"), true) } else { (format!("AI: {name} was not found (set {var})"), false) }
        }
    };
    out.lines.push(ai);
    let channels = studio.channels().unwrap_or_default();
    let youtube: Vec<(String, crate::Account)> = channels
        .iter()
        .flat_map(|ch| studio.accounts(ch.id).unwrap_or_default().into_iter().filter(|a| a.enabled && a.platform == "YOUTUBE").map(|a| (ch.name.clone(), a)))
        .collect();
    // The Google sign-in client YouTube connections are made with.
    let google = crate::youtube_connect::client(studio).ok().map(|(id, _)| id);
    if google.is_some() || !youtube.is_empty() {
        out.lines.push(match crate::youtube_connect::check_client(studio) {
            Ok(short) => (format!("Google: sign-in client {short} works (YouTube)"), true),
            Err(e) => (format!("Google: {e}"), false),
        });
    }
    for ch in &channels {
        for a in studio.accounts(ch.id).unwrap_or_default().into_iter().filter(|a| a.enabled) {
            out.lines.push(match studio.test_account(a.id, false) {
                // "YouTube channel “JKR”" already names the platform.
                Ok(name) if a.platform == "YOUTUBE" => (format!("{}: {name} key works", ch.name), true),
                Ok(name) => (format!("{}: {} {name} key works", ch.name, title(&a.platform)), true),
                Err(e) => (format!("{}: {} — {e}", ch.name, title(&a.platform)), false),
            });
        }
    }
    for (ch, a) in &youtube {
        let key_client = studio.secret(&a.credentials_reference).ok().and_then(|s| s.get("clientId").and_then(Value::as_str).map(str::to_string));
        if let (Some(k), Some(g)) = (key_client, google.as_ref())
            && &k != g
        {
            out.lines.push((format!("{ch}: the YouTube key was made with another Google client (it still works; reconnect to use the current one)"), true));
        }
    }
    if crate::cloud::configured(studio) {
        match crate::cloud::call(studio, "accounts", json!({})) {
            Ok(v) => {
                out.lines.push(("Cloud: signed in to Supabase".into(), true));
                for a in v.as_array().cloned().unwrap_or_default() {
                    let name = a.get("account_name").and_then(Value::as_str).unwrap_or("account");
                    let (left, level) = crate::cloud::token_health(a.get("token_expires_at").and_then(Value::as_str));
                    out.lines.push((format!("Cloud @{name}: {left}"), level < 2));
                }
            }
            Err(e) => out.lines.push((format!("Cloud: {e}"), false)),
        }
        // Cloud YouTube: each channel's connection and its queue.
        if let Ok(accounts) = crate::cloud::call(studio, "youtubeAccounts", json!({}))
            && let Some(accounts) = accounts.as_array().filter(|a| !a.is_empty())
        {
            let shorts = crate::cloud::call(studio, "listYoutube", json!({})).ok().and_then(|v| v.get("items").and_then(Value::as_array).cloned()).unwrap_or_default();
            let day_ago = crate::clock::now_ms() - 86_400_000;
            for a in accounts {
                let id = a.get("id").and_then(Value::as_str).unwrap_or_default();
                let name = a.get("channel_name").and_then(Value::as_str).unwrap_or("YouTube");
                let mine: Vec<&Value> = shorts.iter().filter(|s| s.get("youtube_account_id").and_then(Value::as_str) == Some(id)).collect();
                let status = |s: &&Value| s.get("status").and_then(Value::as_str).unwrap_or_default().to_string();
                let waiting = mine.iter().filter(|s| ["scheduled", "processing", "uploading"].contains(&status(s).as_str())).count();
                let today = mine
                    .iter()
                    .filter(|s| s.get("uploaded_at").and_then(Value::as_str).and_then(crate::clock::parse_iso).is_some_and(|t| t > day_ago))
                    .count();
                let failed = mine.iter().filter(|s| status(s) == "failed").count();
                let connected = a.get("connection_status").and_then(Value::as_str) == Some("valid");
                let mut line = format!(
                    "Cloud YouTube “{name}”: {} · {waiting} waiting · {today} of {} uploads in 24 h",
                    if connected { "connected" } else { "reconnect needed" },
                    crate::youtube_queue::DAILY_UPLOADS
                );
                if failed > 0 {
                    line.push_str(&format!(" · {failed} failed"));
                }
                out.lines.push((line, connected && failed == 0));
            }
        }
    }
    out
}

fn title(platform: &str) -> String {
    if platform == "YOUTUBE" {
        return "YouTube".into();
    }
    let mut c = platform.to_lowercase();
    if let Some(f) = c.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    c
}
