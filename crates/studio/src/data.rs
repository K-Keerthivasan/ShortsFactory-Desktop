//! The shared studio database: channels, brands, media, articles, posts, renders, accounts and
//! publishing jobs. Writes use the same tables, states and JSON shapes the earlier app does.

use std::path::{Path, PathBuf};

use filmcraft_reel::{Brand, Card, PostLink, ReelInfo};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::clock::{now_iso, now_ms, parse_iso};
use crate::{Result, Studio, StudioError, err};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: i64,
    pub slug: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaRow {
    pub id: i64,
    pub label: String,
    /// "VIDEO", "IMAGE", "LOGO" or "AUDIO".
    pub kind: String,
    pub path: PathBuf,
    pub duration: Option<f64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Article {
    pub id: i64,
    pub title: String,
    pub site: String,
    pub url: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderRow {
    pub id: i64,
    pub status: String,
    pub path: Option<PathBuf>,
    pub thumbnail: Option<PathBuf>,
    pub duration: Option<f64>,
    pub approved: bool,
    pub demo: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryPost {
    pub story_id: i64,
    pub script_id: Option<i64>,
    pub title: String,
    pub status: String,
    pub kind: String,
    pub created_at: String,
    pub render: Option<RenderRow>,
    /// The last publication: platform, status, link.
    pub published: Vec<(String, String, Option<String>)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub platform: String,
    pub display_name: String,
    pub external_id: String,
    pub enabled: bool,
    pub status: String,
    /// The secret file's name inside the secrets folder (never its contents).
    pub credentials_reference: String,
}

const SETTINGS_DEFAULT_PLATFORMS: &[&str] = &[];

impl Studio {
    pub fn rename_post(&self, channel: i64, story: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 180 {
            return err("Use a name of 1–180 characters");
        }
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        if tx.execute(
            "UPDATE story_clusters SET canonical_story_title = ?1, updated_at = ?2 WHERE id = ?3 AND channel_id = ?4 AND status != 'REJECTED'",
            params![name, now_iso(), story, channel],
        )? != 1
        {
            return err("Post not found in this workspace");
        }
        self.audit(&tx, channel, "reel studio post renamed", Some(story), json!({"name": name}))?;
        tx.commit()?;
        Ok(())
    }

    /// Hide a post without deleting media, renders, scripts or publication history.
    pub fn archive_post(&self, channel: i64, story: i64) -> Result<String> {
        let mut c = self.connect()?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let status: String = tx
            .query_row("SELECT status FROM story_clusters WHERE id = ?1 AND channel_id = ?2 AND status != 'REJECTED'", params![story, channel], |r| r.get(0))
            .optional()?
            .ok_or_else(|| StudioError::Other("Post not found in this workspace".into()))?;
        let active: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM renders r WHERE r.story_id = ?1 AND r.channel_id = ?2 AND (r.status IN ('QUEUED','RENDERING','RUNNING') OR EXISTS(SELECT 1 FROM publishing_jobs p WHERE p.render_id = r.id AND p.status NOT IN ('PUBLISHED','FAILED','CANCELLED'))))", params![story, channel], |r| r.get(0))?;
        if active {
            return err("Finish or cancel this post's active rendering/publishing jobs before archiving");
        }
        tx.execute("UPDATE story_clusters SET status = 'REJECTED', updated_at = ?1 WHERE id = ?2 AND channel_id = ?3", params![now_iso(), story, channel])?;
        self.audit(&tx, channel, "reel studio post archived", Some(story), json!({"previousStatus": status}))?;
        tx.commit()?;
        Ok(status)
    }

    pub fn restore_post(&self, channel: i64, story: i64) -> Result<()> {
        let mut c = self.connect()?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let details: String = tx
            .query_row(
                "SELECT details FROM audit_logs WHERE channel_id = ?1 AND entity_id = ?2 AND action = 'reel studio post archived' ORDER BY id DESC LIMIT 1",
                params![channel, story],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StudioError::Other("No archive record found for this post".into()))?;
        let details: Value = serde_json::from_str(&details).map_err(|e| StudioError::Other(e.to_string()))?;
        let status = details
            .get("previousStatus")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && *s != "REJECTED")
            .ok_or_else(|| StudioError::Other("Archive record has no previous status".into()))?;
        if tx.execute(
            "UPDATE story_clusters SET status = ?1, updated_at = ?2 WHERE id = ?3 AND channel_id = ?4 AND status = 'REJECTED'",
            params![status, now_iso(), story, channel],
        )? != 1
        {
            return err("Archived post not found in this workspace");
        }
        self.audit(&tx, channel, "reel studio post restored", Some(story), json!({}))?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_ai_provider(&self, provider: &str) -> Result<()> {
        // Codex is this app's alone: the shared setting (read by the earlier app) is left as is.
        let mut own = self.app_settings();
        own.ai_provider = (provider == "codex").then(|| provider.to_string());
        self.save_app_settings(&own)?;
        if provider == "codex" {
            return Ok(());
        }
        if !matches!(provider, "hermes" | "ollama") {
            return err("Choose Hermes, Codex or Ollama");
        }
        let mut c = self.connect()?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let stored: Option<String> = tx.query_row("SELECT json FROM application_settings WHERE key = 'general'", [], |r| r.get(0)).optional()?;
        let mut settings = match stored {
            Some(text) => serde_json::from_str::<Value>(&text).map_err(|e| StudioError::Other(format!("General settings are unreadable: {e}")))?,
            None => json!({}),
        };
        let Some(object) = settings.as_object_mut() else { return err("General settings must be an object") };
        object.insert("aiProvider".into(), json!(provider));
        tx.execute(
            "INSERT INTO application_settings (key, json) VALUES ('general', ?1) ON CONFLICT(key) DO UPDATE SET json = excluded.json",
            [settings.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Discover readable videos under this workspace's assets, without following links.
    /// Returns (added, unreadable); bounded so a malformed directory tree cannot run forever.
    pub fn scan_media(&self, channel: i64) -> Result<(usize, usize)> {
        let c = self.connect()?;
        let ch = self.channel(&c, channel)?;
        if ch.slug.is_empty() || !ch.slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return err("Workspace has an invalid folder slug");
        }
        let root = self.storage.join("channels").join(&ch.slug).join("assets");
        let mut pending = vec![(root, 0usize)];
        let (mut added, mut unreadable, mut visited) = (0, 0, 0usize);
        while let Some((folder, depth)) = pending.pop() {
            if !folder.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(folder)? {
                visited = visited.saturating_add(1);
                if visited > 10_000 {
                    return err("Scan stopped at 10,000 entries; split this workspace's assets into smaller folders");
                }
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_symlink() {
                    continue;
                }
                let path = entry.path();
                if kind.is_dir() {
                    if depth < 8 {
                        pending.push((path, depth + 1));
                    }
                    continue;
                }
                let ext = path.extension().and_then(|s| s.to_str()).unwrap_or_default().to_ascii_lowercase();
                if !kind.is_file() || !matches!(ext.as_str(), "mp4" | "mov" | "mkv" | "webm" | "avi" | "m4v" | "mts" | "m2ts") {
                    continue;
                }
                let rel = self.relative_storage(&path)?;
                let exists: bool =
                    c.query_row("SELECT EXISTS(SELECT 1 FROM media_assets WHERE file_path = ?1 AND channel_id = ?2)", params![rel, channel], |r| r.get(0))?;
                if exists {
                    continue;
                }
                let Ok((duration, width, height)) = crate::probe(&path) else {
                    unreadable += 1;
                    continue;
                };
                let label = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                c.execute("INSERT INTO media_assets (created_at, updated_at, channel_id, file_path, asset_type, label, duration, width, height, source) VALUES (?1, ?1, ?2, ?3, 'VIDEO', ?4, ?5, ?6, ?7, 'Workspace scan')", params![now_iso(), channel, rel, label, duration, width, height])?;
                added += 1;
            }
        }
        Ok((added, unreadable))
    }

    /// Create an isolated channel with settings compatible with the earlier desktop app.
    pub fn create_workspace(&self, slug: &str, name: &str) -> Result<Channel> {
        let name = name.trim();
        if name.is_empty()
            || name.chars().count() > 100
            || slug.is_empty()
            || slug.len() > 60
            || slug.split('-').any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
        {
            return err("Use a name of 1–100 characters and a slug with lowercase letters, digits and single hyphens (up to 60 characters)");
        }
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let now = now_iso();
        tx.execute("INSERT INTO channels (created_at, updated_at, slug, name) VALUES (?1, ?1, ?2, ?3)", params![now, slug, name])?;
        let id = tx.last_insert_rowid();
        let brand = Brand { channel_name: name.into(), ..Default::default() };
        let settings = serde_json::to_value(&brand).map_err(|e| StudioError::Other(e.to_string()))?;
        tx.execute("INSERT INTO channel_settings (channel_id, json) VALUES (?1, ?2)", params![id, settings.to_string()])?;
        for folder in ["articles", "assets/clips", "assets/brand", "audio", "captions", "renders", "thumbnails", "temp"] {
            std::fs::create_dir_all(self.storage.join("channels").join(slug).join(folder))?;
        }
        tx.commit()?;
        Ok(Channel { id, slug: slug.into(), name: name.into() })
    }

    /// Merge editable brand fields without replacing publishing, AI or scheduling settings.
    pub fn save_brand(&self, channel: i64, brand: &Brand) -> Result<()> {
        if filmcraft_reel::find(&brand.default_layout).is_none() || !brand.card_title_size.is_finite() || !(18.0..=140.0).contains(&brand.card_title_size) {
            return err("Choose a valid template and a headline size between 18 and 140");
        }
        for color in [&brand.primary_color, &brand.secondary_color, &brand.background, &brand.card_background, &brand.band_color, &brand.card_text_color] {
            if color.len() != 7 || !color.starts_with('#') || !color.bytes().skip(1).all(|b| b.is_ascii_hexdigit()) {
                return err("Brand colours must use #RRGGBB");
            }
        }
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        self.channel(&tx, channel)?;
        let stored: Option<String> = tx.query_row("SELECT json FROM channel_settings WHERE channel_id = ?1", [channel], |r| r.get(0)).optional()?;
        let mut settings: Value = match stored {
            Some(s) => serde_json::from_str(&s).map_err(|e| StudioError::Other(format!("Settings are unreadable: {e}")))?,
            None => json!({}),
        };
        let Some(obj) = settings.as_object_mut() else { return err("Settings must be an object") };
        let fields = serde_json::to_value(brand).map_err(|e| StudioError::Other(e.to_string()))?;
        if let Some(fields) = fields.as_object() {
            for (key, value) in fields {
                if key != "channelName" && key != "logoPath" {
                    obj.insert(key.clone(), value.clone());
                }
            }
        }
        tx.execute(
            "INSERT INTO channel_settings (channel_id, json) VALUES (?1, ?2) ON CONFLICT(channel_id) DO UPDATE SET json = excluded.json",
            params![channel, settings.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Makes an image the workspace's logo: copies it into the workspace's brand folder, adds it
    /// to the library as a LOGO asset (once) and points the channel's settings at it.
    pub fn set_logo(&self, channel: i64, file: &Path) -> Result<i64> {
        let ext = file.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        if !["png", "jpg", "jpeg", "webp"].contains(&ext.as_str()) || !file.is_file() {
            return err("Choose a PNG, JPEG or WebP image");
        }
        let mut c = self.connect()?;
        let ch = self.channel(&c, channel)?;
        let dir = self.storage.join("channels").join(&ch.slug).join("assets").join("brand");
        std::fs::create_dir_all(&dir)?;
        let name: String = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| format!("logo.{ext}"));
        let target = dir.join(&name);
        if std::fs::canonicalize(file).ok() != std::fs::canonicalize(&target).ok() {
            std::fs::copy(file, &target)?;
        }
        let rel = self.relative_storage(&target)?;
        let tx = c.transaction()?;
        let now = now_iso();
        let existing: Option<i64> =
            tx.query_row("SELECT id FROM media_assets WHERE file_path = ?1 AND channel_id = ?2", params![rel, channel], |r| r.get(0)).optional()?;
        let id = match existing {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO media_assets (created_at, updated_at, channel_id, file_path, asset_type, label, tags) VALUES (?1, ?1, ?2, ?3, 'LOGO', ?4, '[\"logo\"]')",
                    params![now, channel, rel, name],
                )?;
                tx.last_insert_rowid()
            }
        };
        let stored: Option<String> = tx.query_row("SELECT json FROM channel_settings WHERE channel_id = ?1", [channel], |r| r.get(0)).optional()?;
        let mut settings: Value = stored.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}));
        let Some(obj) = settings.as_object_mut() else { return err("Settings must be an object") };
        obj.insert("logoAssetId".into(), json!(id));
        tx.execute(
            "INSERT INTO channel_settings (channel_id, json) VALUES (?1, ?2) ON CONFLICT(channel_id) DO UPDATE SET json = excluded.json",
            params![channel, settings.to_string()],
        )?;
        self.audit(&tx, channel, "logo changed", Some(id), json!({"file": rel}))?;
        tx.commit()?;
        Ok(id)
    }

    pub fn connect(&self) -> Result<Connection> {
        let c = Connection::open(&self.database)?;
        c.busy_timeout(std::time::Duration::from_secs(10))?;
        c.pragma_update(None, "foreign_keys", "ON")?;
        Ok(c)
    }

    pub fn channels(&self) -> Result<Vec<Channel>> {
        let c = self.connect()?;
        let mut q = c.prepare("SELECT id, slug, name FROM channels WHERE enabled = 1 ORDER BY id")?;
        let rows = q.query_map([], |r| Ok(Channel { id: r.get(0)?, slug: r.get(1)?, name: r.get(2)? }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    fn channel(&self, c: &Connection, id: i64) -> Result<Channel> {
        c.query_row("SELECT id, slug, name FROM channels WHERE id = ?1", [id], |r| Ok(Channel { id: r.get(0)?, slug: r.get(1)?, name: r.get(2)? }))
            .optional()?
            .ok_or_else(|| StudioError::Other(format!("channel {id} not found")))
    }

    /// The channel's stored settings JSON (brand, platforms, AI prompt…).
    pub fn settings_json(&self, channel: i64) -> Result<Value> {
        let c = self.connect()?;
        let json: Option<String> = c.query_row("SELECT json FROM channel_settings WHERE channel_id = ?1", [channel], |r| r.get(0)).optional()?;
        Ok(json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_else(|| json!({})))
    }

    /// The channel's brand, with its name and the logo resolved to a file.
    pub fn brand(&self, channel: i64) -> Result<Brand> {
        let c = self.connect()?;
        let ch = self.channel(&c, channel)?;
        let settings = self.settings_json(channel)?;
        let mut brand: Brand = serde_json::from_value(settings.clone()).unwrap_or_default();
        brand.channel_name = ch.name;
        brand.logo_path = None;
        if let Some(logo) = settings.get("logoAssetId").and_then(Value::as_i64) {
            let row: Option<(String, Option<i64>)> =
                c.query_row("SELECT file_path, channel_id FROM media_assets WHERE id = ?1", [logo], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
            if let Some((path, owner)) = row
                && owner.is_none_or(|o| o == channel)
                && let Ok(file) = self.resolve_storage(&path)
                && file.is_file()
            {
                brand.logo_path = Some(file.to_string_lossy().into_owned());
            }
        }
        Ok(brand)
    }

    /// Platforms the channel publishes to (`INSTAGRAM`, `YOUTUBE`).
    pub fn enabled_platforms(&self, channel: i64) -> Result<Vec<String>> {
        let s = self.settings_json(channel)?;
        Ok(s.get("enabledPlatforms")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_else(|| SETTINGS_DEFAULT_PLATFORMS.iter().map(|s| (*s).to_string()).collect()))
    }

    /// The channel's media library (and shared media), newest first; only files that exist.
    pub fn media(&self, channel: i64) -> Result<Vec<MediaRow>> {
        let c = self.connect()?;
        let mut q = c.prepare(
            "SELECT id, COALESCE(label, ''), asset_type, file_path, duration, width, height, created_at FROM media_assets
             WHERE channel_id = ?1 OR channel_id IS NULL ORDER BY id DESC",
        )?;
        let rows = q.query_map([channel], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get::<_, String>(7)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, label, kind, path, duration, width, height, created_at) = row?;
            let Ok(file) = self.resolve_storage(&path) else { continue };
            if !file.is_file() {
                continue;
            }
            let label = if label.is_empty() { file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default() } else { label };
            out.push(MediaRow { id, label, kind, path: file, duration, width, height, created_at });
        }
        Ok(out)
    }

    /// Extracted articles to write a card from, newest first.
    pub fn articles(&self, channel: i64, limit: usize) -> Result<Vec<Article>> {
        let c = self.connect()?;
        let mut q = c.prepare(
            "SELECT id, title, COALESCE(site_name, ''), url, text FROM articles
             WHERE channel_id = ?1 AND text IS NOT NULL AND length(text) > 0 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows =
            q.query_map(params![channel, limit as i64], |r| Ok(Article { id: r.get(0)?, title: r.get(1)?, site: r.get(2)?, url: r.get(3)?, text: r.get(4)? }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn article(&self, channel: i64, id: i64) -> Result<Article> {
        let c = self.connect()?;
        c.query_row(
            "SELECT id, title, COALESCE(site_name, ''), url, COALESCE(text, '') FROM articles WHERE id = ?1 AND channel_id = ?2",
            params![id, channel],
            |r| Ok(Article { id: r.get(0)?, title: r.get(1)?, site: r.get(2)?, url: r.get(3)?, text: r.get(4)? }),
        )
        .optional()?
        .ok_or_else(|| StudioError::Other("article not found in this channel".into()))
    }

    pub fn accounts(&self, channel: i64) -> Result<Vec<Account>> {
        let c = self.connect()?;
        let mut q = c.prepare(
            "SELECT id, platform, display_name, external_account_id, enabled, status, credentials_reference FROM social_accounts WHERE channel_id = ?1 ORDER BY platform",
        )?;
        let rows = q.query_map([channel], |r| {
            Ok(Account {
                id: r.get(0)?,
                platform: r.get(1)?,
                display_name: r.get(2)?,
                external_id: r.get(3)?,
                enabled: r.get::<_, i64>(4)? != 0,
                status: r.get(5)?,
                credentials_reference: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
    pub fn account(&self, id: i64) -> Result<(i64, Account)> {
        let c = self.connect()?;
        c.query_row(
            "SELECT channel_id, id, platform, display_name, external_account_id, enabled, status, credentials_reference FROM social_accounts WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    Account {
                        id: r.get(1)?,
                        platform: r.get(2)?,
                        display_name: r.get(3)?,
                        external_id: r.get(4)?,
                        enabled: r.get::<_, i64>(5)? != 0,
                        status: r.get(6)?,
                        credentials_reference: r.get(7)?,
                    },
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StudioError::Other("publishing account not found".into()))
    }

    /// Reads an account's secret file. Only a publisher calls this; the values are never logged.
    pub(crate) fn secret(&self, reference: &str) -> Result<Value> {
        let ok = !reference.is_empty()
            && reference.ends_with(".json")
            && reference.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.');
        if !ok || reference.contains("..") {
            return err("invalid credential reference");
        }
        let text = std::fs::read_to_string(self.secrets.join(reference))
            .map_err(|_| StudioError::Other("account credentials missing; connect the account first".into()))?;
        serde_json::from_str(&text).map_err(|_| StudioError::Other("account credentials are unreadable".into()))
    }
    /// The channel's posts, newest first, each with its newest script and render.
    pub fn library(&self, channel: i64, limit: usize) -> Result<Vec<LibraryPost>> {
        self.library_filtered(channel, limit, false)
    }

    pub fn archived_library(&self, channel: i64, limit: usize) -> Result<Vec<LibraryPost>> {
        self.library_filtered(channel, limit, true)
    }

    fn library_filtered(&self, channel: i64, limit: usize, archived: bool) -> Result<Vec<LibraryPost>> {
        let c = self.connect()?;
        let mut q = c.prepare(
            "SELECT id, canonical_story_title, status, kind, created_at FROM story_clusters
             WHERE channel_id = ?1 AND ((?3 = 0 AND status != 'REJECTED') OR (?3 = 1 AND status = 'REJECTED' AND EXISTS(SELECT 1 FROM audit_logs a WHERE a.entity_id = story_clusters.id AND a.channel_id = ?1 AND a.action = 'reel studio post archived'))) ORDER BY id DESC LIMIT ?2",
        )?;
        let stories: Vec<(i64, String, String, String, String)> = q
            .query_map(params![channel, limit.min(10_000) as i64, archived], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let mut out = Vec::new();
        for (story, title, status, kind, created_at) in stories {
            let script_id: Option<i64> =
                c.query_row("SELECT id FROM scripts WHERE story_id = ?1 ORDER BY version DESC LIMIT 1", [story], |r| r.get(0)).optional()?;
            let render = c
                .query_row(
                    "SELECT id, status, file_path, thumbnail_path, duration, approved_at, demo, created_at FROM renders WHERE story_id = ?1 ORDER BY id DESC LIMIT 1",
                    [story],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, Option<String>>(3)?,
                            r.get::<_, Option<f64>>(4)?,
                            r.get::<_, Option<String>>(5)?,
                            r.get::<_, i64>(6)?,
                            r.get::<_, String>(7)?,
                        ))
                    },
                )
                .optional()?
                .map(|(id, status, file, thumb, duration, approved, demo, created_at)| RenderRow {
                    id,
                    status,
                    path: file.and_then(|f| self.resolve_storage(&f).ok()),
                    thumbnail: thumb.and_then(|f| self.resolve_storage(&f).ok()),
                    duration,
                    approved: approved.is_some(),
                    demo: demo != 0,
                    created_at,
                });
            let mut published = Vec::new();
            if let Some(r) = &render {
                let mut p = c.prepare("SELECT platform, status, remote_url FROM publishing_jobs WHERE render_id = ?1 ORDER BY id")?;
                published = p.query_map([r.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<_, _>>()?;
            }
            out.push(LibraryPost { story_id: story, script_id, title, status, kind, created_at, render, published });
        }
        Ok(out)
    }

    /// The newest script of a post, as stored.
    pub fn script(&self, script_id: i64, channel: i64) -> Result<Value> {
        let c = self.connect()?;
        let json: String = c
            .query_row("SELECT json FROM scripts WHERE id = ?1 AND channel_id = ?2", params![script_id, channel], |r| r.get(0))
            .optional()?
            .ok_or_else(|| StudioError::Other("post not found in this channel".into()))?;
        serde_json::from_str(&json).map_err(|e| StudioError::Other(e.to_string()))
    }

    fn audit(&self, c: &Connection, channel: i64, action: &str, entity: Option<i64>, details: Value) -> Result<()> {
        let now = now_iso();
        c.execute(
            "INSERT INTO audit_logs (created_at, updated_at, channel_id, action, entity_id, details) VALUES (?1, ?1, ?2, ?3, ?4, ?5)",
            params![now, channel, action, entity, details.to_string()],
        )?;
        Ok(())
    }

    /// The media row of a footage file, if the library has one.
    fn media_id_for(&self, c: &Connection, channel: i64, file: &Path) -> Result<Option<(i64, Option<f64>)>> {
        let Ok(rel) = self.relative_storage(file) else { return Ok(None) };
        Ok(c.query_row(
            "SELECT id, duration FROM media_assets WHERE (file_path = ?1 OR replace(file_path, '\\', '/') LIKE '%' || ?1) AND asset_type = 'VIDEO' AND (channel_id = ?2 OR channel_id IS NULL) LIMIT 1",
            params![rel, channel],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
    }

    /// Makes the Reel a library post (or a new version of the post it already is) and adds the
    /// exported MP4 as its render. Returns the post and the render id.
    pub fn send_to_library(
        &self,
        reel: &ReelInfo,
        video: &Path,
        thumbnail: Option<&Path>,
        footage: Option<(&Path, f64, f64)>,
        duration: f64,
        approve: bool,
    ) -> Result<(PostLink, i64)> {
        let channel = reel.channel_id.ok_or_else(|| StudioError::Other("choose the channel this Reel belongs to".into()))?;
        if !video.is_file() {
            return err("the exported video was not found");
        }
        let mut c = self.connect()?;
        let ch = self.channel(&c, channel)?;
        let clip = match footage {
            Some((file, start, len)) => self.media_id_for(&c, channel, file)?.map(|(id, _)| {
                json!({"assetId": id, "crop": {"scale": 1, "x": 0, "y": 0, "scaleY": null, "rotate": 0, "pitch": 0, "yaw": 0, "flipH": false, "flipV": false, "anchorX": 0, "anchorY": 0},
                       "trimStart": start.max(0.0), "trimEnd": start.max(0.0) + len.max(0.04), "originalAudio": true})
            }),
            None => None,
        };
        let script = script_json(reel, clip, duration);
        let tx = c.transaction()?;
        let now = now_iso();
        let (story, version) = match reel.post.filter(|p| p.channel_id == channel) {
            Some(p) => {
                let exists: Option<i64> =
                    tx.query_row("SELECT id FROM story_clusters WHERE id = ?1 AND channel_id = ?2", params![p.story_id, channel], |r| r.get(0)).optional()?;
                match exists {
                    Some(s) => {
                        let v: i64 = tx.query_row("SELECT COALESCE(MAX(version), 0) FROM scripts WHERE story_id = ?1", [s], |r| r.get(0))?;
                        (s, v + 1)
                    }
                    None => (new_story(&tx, channel, reel, &now)?, 1),
                }
            }
            None => (new_story(&tx, channel, reel, &now)?, 1),
        };
        tx.execute(
            "INSERT INTO scripts (created_at, updated_at, channel_id, story_id, version, json, status, demo) VALUES (?1, ?1, ?2, ?3, ?4, ?5, 'DRAFT', 0)",
            params![now, channel, story, version, script.to_string()],
        )?;
        let script_id = tx.last_insert_rowid();
        // The video and its thumbnail move into the post's folder in storage.
        let dir = self.storage.join("channels").join(&ch.slug).join(format!("story-{story}"));
        std::fs::create_dir_all(&dir)?;
        let stamp = now_ms();
        let target = dir.join(format!("render-reelstudio-{stamp}.mp4"));
        std::fs::copy(video, &target)?;
        // An editable native project accompanies exports from Reel Studio.
        let project = video.with_extension("fcproj");
        if project.is_file() {
            std::fs::copy(project, target.with_extension("fcproj"))?;
        }
        let thumb = match thumbnail.filter(|t| t.is_file()) {
            Some(t) => {
                let ext = t.extension().and_then(|e| e.to_str()).unwrap_or("png");
                let to = dir.join(format!("thumbnail-reelstudio-{stamp}.{ext}"));
                std::fs::copy(t, &to)?;
                Some(self.relative_storage(&to)?)
            }
            None => None,
        };
        let rel = self.relative_storage(&target)?;
        let status = if approve { "APPROVED" } else { "RENDERED" };
        tx.execute(
            "INSERT INTO renders (created_at, updated_at, channel_id, story_id, script_id, status, file_path, thumbnail_path, duration, props_json, approved_at, demo)
             VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)",
            params![now, channel, story, script_id, status, rel, thumb, duration, json!({"source": "reelstudio", "template": reel.template}).to_string(), approve.then(|| now.clone())],
        )?;
        let render_id = tx.last_insert_rowid();
        tx.execute("UPDATE story_clusters SET status = ?1, updated_at = ?2 WHERE id = ?3", params![if approve { "APPROVED" } else { "RENDERED" }, now, story])?;
        self.audit(&tx, channel, "reel studio render added", Some(story), json!({"scriptId": script_id, "renderId": render_id}))?;
        tx.commit()?;
        Ok((PostLink { channel_id: channel, story_id: story, script_id }, render_id))
    }

    /// A review-ready post for an imported clip: the card and the whole clip, no render yet.
    pub fn create_draft(&self, channel: i64, card: &Card, asset: i64, duration: f64, source: &str) -> Result<PostLink> {
        let reel = ReelInfo { template: String::new(), channel_id: Some(channel), card: card.clone(), ..Default::default() };
        let clip = json!({"assetId": asset, "crop": {"scale": 1, "x": 0, "y": 0, "scaleY": null, "rotate": 0, "pitch": 0, "yaw": 0, "flipH": false, "flipV": false, "anchorX": 0, "anchorY": 0},
                          "trimStart": 0, "trimEnd": null, "originalAudio": true});
        let mut script = script_json(&reel, Some(clip), duration);
        script["layout"] = Value::Null;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let now = now_iso();
        let story = new_story(&tx, channel, &reel, &now)?;
        tx.execute("UPDATE articles SET canonical_url = ?1 WHERE id = (SELECT primary_article_id FROM story_clusters WHERE id = ?2)", params![source, story])?;
        tx.execute(
            "INSERT INTO scripts (created_at, updated_at, channel_id, story_id, version, json, status, demo) VALUES (?1, ?1, ?2, ?3, 1, ?4, 'DRAFT', 0)",
            params![now, channel, story, script.to_string()],
        )?;
        let script_id = tx.last_insert_rowid();
        self.audit(&tx, channel, "imported video", Some(story), json!({"assetId": asset}))?;
        tx.commit()?;
        Ok(PostLink { channel_id: channel, story_id: story, script_id })
    }

    pub fn approve(&self, render: i64) -> Result<()> {
        let c = self.connect()?;
        let now = now_iso();
        let n = c.execute("UPDATE renders SET status = 'APPROVED', approved_at = ?1, updated_at = ?1 WHERE id = ?2 AND status IN ('RENDERED', 'APPROVED') AND file_path IS NOT NULL", params![now, render])?;
        if n == 0 {
            return err("only a finished render can be approved");
        }
        c.execute(
            "UPDATE story_clusters SET status = 'APPROVED', updated_at = ?1 WHERE id = (SELECT story_id FROM renders WHERE id = ?2)",
            params![now, render],
        )?;
        Ok(())
    }

    fn approved_render(&self, c: &Connection, render: i64, channel: i64) -> Result<(PathBuf, i64, i64)> {
        let row: Option<(String, Option<String>, Option<String>, i64, i64, i64)> = c
            .query_row(
                "SELECT status, approved_at, file_path, demo, story_id, script_id FROM renders WHERE id = ?1 AND channel_id = ?2",
                params![render, channel],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .optional()?;
        let Some((status, approved, file, demo, story, script)) = row else { return err("render not found in this channel") };
        if status != "APPROVED" || approved.is_none() {
            return err("approve the rendered video first");
        }
        if demo != 0 {
            return err("demo renders cannot be published");
        }
        let file = file.ok_or_else(|| StudioError::Other("the render has no file".into()))?;
        Ok((self.resolve_storage(&file)?, story, script))
    }

    fn check_account(&self, c: &Connection, channel: i64, account: i64) -> Result<Account> {
        let (owner, a) = self.account(account)?;
        if owner != channel || !a.enabled {
            return err("cross-channel or disabled publishing account");
        }
        if !self.enabled_platforms(channel)?.contains(&a.platform) {
            return err("enable this platform in the channel's settings first");
        }
        let _ = c;
        Ok(a)
    }

    /// Schedules an approved render on accounts, the way the earlier app does: the job is picked
    /// up when due by whichever publisher runs (the earlier app's worker, or Reel Studio's).
    pub fn schedule(&self, channel: i64, render: i64, accounts: &[i64], at_iso: &str) -> Result<usize> {
        let at = parse_iso(at_iso).ok_or_else(|| StudioError::Other("unreadable schedule time".into()))?;
        let mut c = self.connect()?;
        let (_, story, _) = self.approved_render(&c, render, channel)?;
        let checked: Vec<Account> = accounts.iter().map(|a| self.check_account(&c, channel, *a)).collect::<Result<_>>()?;
        let tx = c.transaction()?;
        let now = now_iso();
        let when = crate::clock::iso(at);
        let mut n = 0;
        for a in checked {
            n += tx.execute(
                "INSERT OR IGNORE INTO publishing_jobs (created_at, updated_at, channel_id, render_id, social_account_id, platform, status, scheduled_at) VALUES (?1, ?1, ?2, ?3, ?4, ?5, 'SCHEDULED', ?6)",
                params![now, channel, render, a.id, a.platform, when],
            )?;
        }
        if n > 0 {
            tx.execute("UPDATE story_clusters SET status = 'SCHEDULED', updated_at = ?1 WHERE id = ?2", params![now, story])?;
            self.audit(&tx, channel, "changed schedule", Some(render), json!({"accountIds": accounts, "scheduledAt": when}))?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// Claims a publication for this process: an existing scheduled job that is due, or a new job
    /// made for "publish now". The claim is one atomic update to `UPLOADING`, a state the earlier
    /// app's queue never picks up, so a video is never published twice.
    pub fn claim(&self, channel: i64, render: i64, account: i64) -> Result<i64> {
        let mut c = self.connect()?;
        self.approved_render(&c, render, channel)?;
        let a = self.check_account(&c, channel, account)?;
        let tx = c.transaction()?;
        let now = now_iso();
        let existing: Option<(i64, String)> = tx
            .query_row("SELECT id, status FROM publishing_jobs WHERE render_id = ?1 AND social_account_id = ?2", params![render, account], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()?;
        let id = match existing {
            Some((_, s)) if s == "PUBLISHED" => return err("this video is already published on that account"),
            Some((_, s)) if s == "NEEDS_RECONCILIATION" => return err("this publication needs checking on the platform before it is retried"),
            Some((id, _)) => {
                let n = tx.execute(
                    "UPDATE publishing_jobs SET status = 'UPLOADING', started_at = COALESCE(started_at, ?1), attempt_count = attempt_count + 1, updated_at = ?1
                     WHERE id = ?2 AND status IN ('SCHEDULED', 'PUBLISH_FAILED', 'FAILED')",
                    params![now, id],
                )?;
                if n == 0 {
                    return err("this publication is already in progress");
                }
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO publishing_jobs (created_at, updated_at, channel_id, render_id, social_account_id, platform, status, scheduled_at, started_at, attempt_count)
                     VALUES (?1, ?1, ?2, ?3, ?4, ?5, 'UPLOADING', ?1, ?1, 1)",
                    params![now, channel, render, account, a.platform],
                )?;
                tx.last_insert_rowid()
            }
        };
        tx.commit()?;
        Ok(id)
    }

    /// Records a publication checkpoint (resumable remote state).
    pub fn checkpoint(&self, job: i64, channel: i64, state: &Value) -> Result<()> {
        let c = self.connect()?;
        let now = now_iso();
        c.execute("UPDATE publishing_jobs SET remote_state_json = ?1, updated_at = ?2 WHERE id = ?3", params![state.to_string(), now, job])?;
        let summary = json!({"remoteId": state.get("remoteId"), "offset": state.get("offset"), "publishedId": state.get("publishedId")});
        c.execute(
            "INSERT INTO publishing_attempts (created_at, updated_at, channel_id, publishing_job_id, action, result_json) VALUES (?1, ?1, ?2, ?3, ?4, ?5)",
            params![now, channel, job, state.get("phase").and_then(Value::as_str).unwrap_or("CHECKPOINT"), summary.to_string()],
        )?;
        Ok(())
    }
    pub fn remote_state(&self, job: i64) -> Result<Value> {
        let c = self.connect()?;
        let s: Option<String> = c.query_row("SELECT remote_state_json FROM publishing_jobs WHERE id = ?1", [job], |r| r.get(0))?;
        Ok(s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({})))
    }
    pub fn finish_publication(&self, job: i64, outcome: std::result::Result<(String, Option<String>), (bool, String)>) -> Result<()> {
        let c = self.connect()?;
        let now = now_iso();
        let (channel, render): (i64, i64) =
            c.query_row("SELECT channel_id, render_id FROM publishing_jobs WHERE id = ?1", [job], |r| Ok((r.get(0)?, r.get(1)?)))?;
        match outcome {
            Ok((remote, url)) => {
                c.execute(
                    "UPDATE publishing_jobs SET status = 'PUBLISHED', remote_id = ?1, remote_url = ?2, published_at = ?3, last_error = NULL, updated_at = ?3 WHERE id = ?4",
                    params![remote, url, now, job],
                )?;
                c.execute(
                    "UPDATE story_clusters SET status = 'PUBLISHED', updated_at = ?1 WHERE id = (SELECT story_id FROM renders WHERE id = ?2)",
                    params![now, render],
                )?;
                self.audit(&c, channel, "published video", Some(job), json!({"remoteId": remote}))?;
            }
            Err((reconcile, message)) => {
                c.execute(
                    "UPDATE publishing_jobs SET status = ?1, last_error = ?2, updated_at = ?3 WHERE id = ?4",
                    params![if reconcile { "NEEDS_RECONCILIATION" } else { "PUBLISH_FAILED" }, message, now, job],
                )?;
            }
        }
        Ok(())
    }

    /// Is the earlier app's worker running (a heartbeat in the last two minutes)? While it is,
    /// it owns scheduled publications and Reel Studio leaves them to it.
    pub fn pipeline_online(&self) -> Result<bool> {
        let c = self.connect()?;
        let seen: Option<String> =
            c.query_row("SELECT MAX(last_seen_at) FROM worker_heartbeats WHERE kind = 'pipeline'", [], |r| r.get(0)).optional()?.flatten();
        Ok(seen.and_then(|s| parse_iso(&s)).is_some_and(|t| now_ms() - t < 120_000))
    }

    /// Scheduled publications that are due: (job, channel, render, account).
    pub fn due_publications(&self) -> Result<Vec<(i64, i64, i64, i64)>> {
        let c = self.connect()?;
        let mut q = c.prepare("SELECT id, channel_id, render_id, social_account_id, scheduled_at FROM publishing_jobs WHERE status = 'SCHEDULED'")?;
        let now = now_ms();
        let rows: Vec<(i64, i64, i64, i64, String)> =
            q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<std::result::Result<_, _>>()?;
        Ok(rows.into_iter().filter(|r| parse_iso(&r.4).is_some_and(|t| t <= now)).map(|r| (r.0, r.1, r.2, r.3)).collect())
    }

    /// The caption a render publishes with.
    pub fn caption_for(&self, render: i64) -> Result<String> {
        let c = self.connect()?;
        let json: String = c.query_row("SELECT s.json FROM renders r JOIN scripts s ON s.id = r.script_id WHERE r.id = ?1", [render], |r| r.get(0))?;
        let v: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
        let mut caption = v.get("instagramCaption").and_then(Value::as_str).unwrap_or_default().to_string();
        if let Some(tags) = v.get("hashtags").and_then(Value::as_array) {
            let tags: Vec<String> = tags
                .iter()
                .filter_map(Value::as_str)
                .filter(|t| !caption.contains(*t))
                .map(|t| if t.starts_with('#') { t.to_string() } else { format!("#{t}") })
                .collect();
            if !tags.is_empty() {
                caption = format!("{caption}\n\n{}", tags.join(" "));
            }
        }
        Ok(caption.chars().take(2200).collect())
    }
}

/// A new story for a Reel made in the Studio: the article row every story hangs off, the story
/// and its membership, exactly as a hand-written post is stored.
fn new_story(tx: &rusqlite::Transaction<'_>, channel: i64, reel: &ReelInfo, now: &str) -> Result<i64> {
    let title = card_title(&reel.card);
    let seed = [title.as_str(), reel.card.caption.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n\n");
    let url = format!("manual:reelstudio-{}-{}", now_ms(), std::process::id());
    tx.execute(
        "INSERT INTO articles (created_at, updated_at, channel_id, url, canonical_url, guid, fingerprint, title, site_name, category, text, status)
         VALUES (?1, ?1, ?2, ?3, ?3, NULL, ?4, ?5, 'Manual upload', 'Manual', ?6, 'EXTRACTED')",
        params![now, channel, url, fingerprint(&title), title.chars().take(180).collect::<String>(), seed],
    )?;
    let article = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO story_clusters (created_at, updated_at, channel_id, kind, canonical_story_title, summary, primary_article_id, status)
         VALUES (?1, ?1, ?2, 'MANUAL', ?3, ?4, ?5, 'SCRIPTED')",
        params![now, channel, title.chars().take(180).collect::<String>(), (!reel.card.caption.is_empty()).then(|| reel.card.caption.clone()), article],
    )?;
    let story = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO story_cluster_members (created_at, updated_at, channel_id, cluster_id, article_id) VALUES (?1, ?1, ?2, ?3, ?4)",
        params![now, channel, story, article],
    )?;
    Ok(story)
}

fn card_title(card: &Card) -> String {
    let t: String = card.title.trim().chars().take(400).collect();
    if t.is_empty() { "Reel".into() } else { t }
}

/// The library's headline fingerprint: lower case, letters and digits only, minus stop words,
/// sorted (it groups duplicate stories).
pub fn fingerprint(title: &str) -> String {
    const STOP: &[&str] = &["a", "an", "the", "of", "to", "and", "in", "for", "on", "at", "with", "is"];
    let clean: String = title.to_lowercase().chars().map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' }).collect();
    let mut words: Vec<&str> = clean.split_whitespace().filter(|w| !STOP.contains(w)).collect();
    words.sort_unstable();
    words.join(" ")
}

/// The post's script, in the shape the earlier app reads (its `scriptSchema`).
pub fn script_json(reel: &ReelInfo, clip: Option<Value>, duration: f64) -> Value {
    let card = &reel.card;
    let title = card_title(card);
    let short: String = title.chars().take(80).collect();
    let cap = |s: &str, n: usize| s.chars().take(n).collect::<String>();
    let layout = filmcraft_reel::find(&reel.template).map(|t| t.id).map(Value::from).unwrap_or(Value::Null);
    let badge = card.badge.trim();
    json!({
        "contentKind": "REEL",
        "card": {"title": title, "caption": cap(&card.caption, 200), "attribution": cap(&card.attribution, 160)},
        "clip": clip,
        "layout": layout,
        "bullets": card.bullets.iter().take(4).map(|b| cap(b, 200)).collect::<Vec<_>>(),
        "badge": if badge.is_empty() { Value::Null } else { Value::from(cap(badge, 24)) },
        "tuning": {"topHeight": null, "bottomHeight": null, "titleSize": null, "captionSize": null, "clipFit": null, "bandStyle": null, "bandColor": null, "markPosition": null, "align": null, "showHandle": null, "clipRadius": null},
        "shortHeadline": short,
        "durationTargetSeconds": if duration.is_finite() { duration.clamp(1.0, 180.0) } else { 30.0 },
        "sourceLabel": if card.attribution.trim().is_empty() { "Original video".to_string() } else { cap(&card.attribution, 160) },
        "instagramCaption": cap(if card.instagram_caption.trim().is_empty() { &title } else { &card.instagram_caption }, 2200),
        "hashtags": card.hashtags,
        "cta": if card.cta.trim().is_empty() { Value::Null } else { Value::from(cap(&card.cta, 250)) },
        "topic": "",
    })
}
