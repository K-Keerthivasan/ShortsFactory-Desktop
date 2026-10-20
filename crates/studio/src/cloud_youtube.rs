//! YouTube Shorts booked in the cloud, so they upload while this PC is off.
//!
//! [`schedule`] books a render on a YouTube channel at a time: the cloud worker uploads it up to
//! a day and a half ahead with YouTube's own publish time, within the Google project's daily
//! quota. A render already in the cloud Instagram schedule reuses that stored video; otherwise
//! the video is uploaded to the cloud once, for YouTube only. The local publication becomes
//! `CLOUD` (the local YouTube queue leaves it alone) until [`sync`] sees the cloud upload it.
//! [`move_queue`] hands every waiting local YouTube job to the cloud.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use crate::{Result, Studio, StudioError, clock, cloud, err};

/// A local status: the cloud uploads this publication.
pub const CLOUD: &str = "CLOUD";
/// The cloud wants YouTube times comfortably ahead.
const LEAD_MS: i64 = 10 * 60_000;

fn field<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or_default()
}

/// The cloud channel matching a local YouTube account, connecting it (with the local key) if
/// the cloud does not have it yet.
pub fn cloud_account(studio: &Studio, local_account: i64) -> Result<String> {
    let (_, a) = studio.account(local_account)?;
    if a.platform != "YOUTUBE" {
        return err("choose a YouTube account");
    }
    let accounts = cloud::call(studio, "youtubeAccounts", json!({}))?;
    if let Some(found) = accounts.as_array().and_then(|l| l.iter().find(|c| field(c, "channel_id") == a.external_id)) {
        return Ok(field(found, "id").to_string());
    }
    connect(studio, local_account)
}

/// Gives the cloud this account's YouTube key (to the user's own backend, encrypted there).
/// Returns the cloud channel id.
pub fn connect(studio: &Studio, local_account: i64) -> Result<String> {
    let (_, a) = studio.account(local_account)?;
    if a.platform != "YOUTUBE" {
        return err("choose a YouTube account");
    }
    let secret = studio.secret(&a.credentials_reference)?;
    let get = |k: &str| secret.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    if get("clientId").is_empty() || get("clientSecret").is_empty() || get("refreshToken").is_empty() {
        return err("this YouTube account has no saved key; connect YouTube again");
    }
    let privacy = studio.youtube_privacy(local_account).unwrap_or_else(|_| "public".into());
    let saved = cloud::call(
        studio,
        "connectYoutube",
        json!({
            "clientId": get("clientId"), "clientSecret": get("clientSecret"), "refreshToken": get("refreshToken"), "privacy": privacy,
            // At most this many Shorts publish on a day of this PC's time zone.
            "dailyLimit": per_day(studio), "timezone": cloud::local_timezone(),
        }),
    )?;
    if field(&saved, "channel_id") != a.external_id {
        return err(format!("the key posts to “{}”, not this account's channel", field(&saved, "channel_name")));
    }
    Ok(field(&saved, "id").to_string())
}

/// The cloud Instagram post of this render whose stored video can be reused.
fn cloud_post(studio: &Studio, content: &str) -> Result<Option<String>> {
    let mut offset = 0;
    for _ in 0..10 {
        let page = cloud::call(studio, "listSchedules", json!({"offset": offset}))?;
        for item in page.get("items").and_then(Value::as_array).into_iter().flatten() {
            let p = item.get("post").unwrap_or(&Value::Null);
            if field(p, "local_content_id") == content
                && ["scheduled", "processing", "publishing", "published"].contains(&field(p, "status"))
                && p.get("storage_deleted_at").is_none_or(Value::is_null)
            {
                return Ok(Some(field(p, "id").to_string()));
            }
        }
        match page.get("nextOffset").and_then(Value::as_i64) {
            Some(n) => offset = n,
            None => break,
        }
    }
    Ok(None)
}

/// Books `render` on the local YouTube `account` at `at_iso` in the cloud. Blocking: it may
/// upload the video. Returns the cloud Short.
pub fn schedule(studio: &Studio, channel: i64, render: i64, account: i64, at_iso: &str) -> Result<Value> {
    let (owner, a) = studio.account(account)?;
    if owner != channel || a.platform != "YOUTUBE" || !a.enabled {
        return err("that is not an enabled YouTube account of this workspace");
    }
    let at = clock::parse_iso(at_iso).ok_or_else(|| StudioError::Other("unreadable schedule time".into()))?;
    if at < clock::now_ms() + LEAD_MS {
        return err("cloud YouTube times must be at least ten minutes ahead; use Publish now for sooner");
    }
    let existing: Option<String> = studio
        .connect()?
        .query_row("SELECT status FROM publishing_jobs WHERE render_id = ?1 AND social_account_id = ?2", params![render, account], |r| r.get(0))
        .optional()?;
    match existing.as_deref() {
        Some("PUBLISHED") => return err("this video is already on that YouTube channel"),
        Some("UPLOADING") => return err("this video is uploading to YouTube from this PC right now"),
        _ => {}
    }
    let when = clock::iso(at);
    let content = format!("{channel}:{render}");
    let youtube = cloud_account(studio, account)?;
    let (title, description, tags) = crate::short_text(studio, render)?;
    let text = json!({"youtubeAccountId": youtube, "localContentId": content, "title": title, "description": description, "tags": tags, "scheduledFor": when});
    let mut short = Value::Null;
    if let Some(post) = cloud_post(studio, &content)? {
        let mut body = text.clone();
        body["scheduledPostId"] = json!(post);
        match cloud::call(studio, "scheduleYoutube", body) {
            Ok(v) => short = v,
            // Its video was cleaned up meanwhile: upload it for YouTube instead.
            Err(e) if e.to_string().contains("no longer stored") => {}
            Err(e) => return Err(e),
        }
    }
    if short.is_null() {
        short = upload(studio, channel, render, text)?;
    }
    // A repeated booking returns the earlier Short: move it to the chosen time.
    if field(&short, "status") == "scheduled" && clock::parse_iso(field(&short, "scheduled_for")) != Some(at) {
        short = cloud::call(studio, "rescheduleYoutube", json!({"id": field(&short, "id"), "scheduledFor": when}))?;
    }
    if !["scheduled", "processing", "uploaded"].contains(&field(&short, "status")) {
        return err(format!("the cloud did not book it ({})", field(&short, "status")));
    }
    if let Err(e) = send_thumbnail(studio, render, account, &short) {
        log::warn!("thumbnail for the cloud Short of render {render}: {e}");
    }
    let state = json!({"cloudYoutube": field(&short, "id")}).to_string();
    let now = clock::now_iso();
    studio.connect()?.execute(
        "INSERT INTO publishing_jobs (created_at, updated_at, channel_id, render_id, social_account_id, platform, status, scheduled_at, remote_state_json)
         VALUES (?1, ?1, ?2, ?3, ?4, 'YOUTUBE', ?5, ?6, ?7)
         ON CONFLICT(render_id, social_account_id) DO UPDATE SET status = ?5, scheduled_at = ?6, remote_state_json = ?7, last_error = NULL, updated_at = ?1",
        params![now, channel, render, account, CLOUD, when, state],
    )?;
    Ok(short)
}

/// Gives a cloud Short its thumbnail: uploaded with it while it waits (the worker sets it after
/// the video), or set from this PC with the channel's own key once it is on YouTube.
pub fn send_thumbnail(studio: &Studio, render: i64, account: i64, short: &Value) -> Result<()> {
    let covers = crate::thumbnails::ensure(studio, render)?;
    let size = if crate::long_video(studio, render) { crate::thumbnails::Size::Landscape } else { crate::thumbnails::Size::Vertical };
    let image = covers.file(size).ok_or_else(|| StudioError::Other("the thumbnail file is missing".into()))?.to_path_buf();
    match field(short, "status") {
        "uploaded" => {
            let (_, a) = studio.account(account)?;
            let secret = studio.secret(&a.credentials_reference)?;
            crate::youtube::set_thumbnail(&secret, field(short, "youtube_video_id"), &image).map_err(StudioError::Other)
        }
        "scheduled" | "uploading" => {
            let bytes = std::fs::metadata(&image)?.len();
            let r = cloud::call(studio, "youtubeThumbnail", json!({"id": field(short, "id"), "fileSize": bytes}))?;
            let url = r.get("uploadUrl").and_then(Value::as_str).unwrap_or_default();
            let path = r.get("path").and_then(Value::as_str).unwrap_or_default();
            let s = r.get("short").unwrap_or(&Value::Null);
            let project = cloud::project_url(studio)?;
            if !path.starts_with("reels/yt/thumbs/") || !cloud::valid_upload_target(url, &project, field(s, "storage_provider"), field(s, "storage_bucket"), path) {
                return err("unexpected cloud thumbnail destination");
            }
            cloud::upload_as(url, &image, bytes, "image/jpeg")
        }
        other => err(format!("the Short is {other}")),
    }
}

/// Thumbnails for every Short this PC booked in the cloud (made first where missing). Blocking.
pub fn send_thumbnails(studio: &Studio) -> Result<Vec<String>> {
    let jobs: Vec<(i64, i64, String)> = {
        let c = studio.connect()?;
        let mut q = c.prepare(
            "SELECT render_id, social_account_id, COALESCE(remote_state_json, '{}') FROM publishing_jobs WHERE platform = 'YOUTUBE' AND status IN (?1, 'PUBLISHED') AND remote_state_json LIKE '%cloudYoutube%'",
        )?;
        q.query_map([CLOUD], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<std::result::Result<_, _>>()?
    };
    let mut shorts: HashMap<String, Value> = HashMap::new();
    let mut offset = 0;
    for _ in 0..10 {
        let page = cloud::call(studio, "listYoutube", json!({"offset": offset}))?;
        for s in page.get("items").and_then(Value::as_array).into_iter().flatten() {
            shorts.insert(field(s, "id").to_string(), s.clone());
        }
        match page.get("nextOffset").and_then(Value::as_i64) {
            Some(n) => offset = n,
            None => break,
        }
    }
    let mut log = Vec::new();
    for (render, account, state) in jobs {
        let id = serde_json::from_str::<Value>(&state).ok().map(|v| field(&v, "cloudYoutube").to_string()).unwrap_or_default();
        let Some(short) = shorts.get(&id) else { continue };
        if field(short, "thumbnail_status") == "set" || short.get("thumbnail_path").is_some_and(|p| !p.is_null()) && field(short, "status") != "uploaded" {
            continue;
        }
        log.push(match send_thumbnail(studio, render, account, short) {
            Ok(()) => format!("render {render}: thumbnail sent"),
            Err(e) => format!("render {render}: no thumbnail ({e})"),
        });
    }
    Ok(log)
}

/// A YouTube-only cloud upload of the render's video.
fn upload(studio: &Studio, channel: i64, render: i64, mut body: Value) -> Result<Value> {
    let rel: String = studio
        .connect()?
        .query_row("SELECT file_path FROM renders WHERE id = ?1 AND channel_id = ?2", params![render, channel], |r| r.get(0))
        .map_err(|_| StudioError::Other("the render has no video file".into()))?;
    let file = studio.resolve_storage(&rel)?;
    let size = std::fs::metadata(&file)?.len();
    body["fileSize"] = json!(size);
    body["mimeType"] = json!("video/mp4");
    let reserved = cloud::call(studio, "createYoutubeUpload", body)?;
    let short = reserved.get("short").cloned().unwrap_or(Value::Null);
    let id = field(&short, "id").to_string();
    if id.is_empty() {
        return err("the cloud returned no booking");
    }
    if let Some(url) = reserved.get("uploadUrl").and_then(Value::as_str) {
        let project = cloud::project_url(studio)?;
        if field(&short, "local_content_id") != format!("{channel}:{render}")
            || short.get("file_size").and_then(Value::as_u64) != Some(size)
            || !cloud::valid_upload_target(url, &project, field(&short, "storage_provider"), field(&short, "storage_bucket"), field(&short, "storage_path"))
        {
            return err("unexpected cloud upload destination");
        }
        cloud::upload(url, &file, size)?;
        return cloud::call(studio, "finalizeYoutubeUpload", json!({"id": id}));
    }
    Ok(short)
}

/// Shorts a channel publishes per day (`YOUTUBE_PER_DAY`, default 3).
pub fn per_day(studio: &Studio) -> usize {
    studio.var("YOUTUBE_PER_DAY").and_then(|v| v.parse().ok()).filter(|n| (1..=20).contains(n)).unwrap_or(3)
}

/// The day's publishing times (`YOUTUBE_SLOTS`, local `HH:MM` list, default 10:00, 15:00, 20:00).
fn slots(studio: &Studio) -> Vec<chrono::NaiveTime> {
    let raw = studio.var("YOUTUBE_SLOTS").unwrap_or("10:00,15:00,20:00").to_string();
    let mut v: Vec<chrono::NaiveTime> = raw.split(',').filter_map(|s| chrono::NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()).collect();
    v.sort();
    if v.is_empty() {
        v = ["10:00", "15:00", "20:00"].iter().filter_map(|s| chrono::NaiveTime::parse_from_str(s, "%H:%M").ok()).collect();
    }
    v
}

/// Keeps every channel to its Shorts per day: a day with more has its latest ones that are not
/// public yet moved to the first free publishing times after it (on YouTube too, for ones
/// already uploaded, with the channel's own key). Returns a line per move. Blocking.
pub fn rebalance(studio: &Studio) -> Result<Vec<String>> {
    use chrono::{Local, TimeZone};
    let limit = per_day(studio);
    let times = slots(studio);
    let shorts = list(studio)?;
    let accounts = cloud::call(studio, "youtubeAccounts", json!({}))?;
    let now = clock::now_ms();
    let local_day = |iso: &str| clock::parse_iso(iso).and_then(|t| Local.timestamp_millis_opt(t).single()).map(|d| d.date_naive());
    let mut log = Vec::new();
    for account in accounts.as_array().into_iter().flatten() {
        let aid = field(account, "id");
        let mut by_day: std::collections::BTreeMap<chrono::NaiveDate, Vec<Value>> = std::collections::BTreeMap::new();
        for s in shorts.iter().filter(|s| field(s, "youtube_account_id") == aid && ["uploading", "scheduled", "processing", "uploaded"].contains(&field(s, "status"))) {
            if let Some(d) = local_day(field(s, "scheduled_for")) {
                by_day.entry(d).or_default().push(s.clone());
            }
        }
        for list in by_day.values_mut() {
            list.sort_by_key(|s| field(s, "scheduled_for").to_string());
        }
        // Too many on a day: the latest that can still move (time ahead, not mid-upload).
        let movable = |s: &Value| {
            clock::parse_iso(field(s, "scheduled_for")).is_some_and(|t| t > now + 30 * 60_000) && ["scheduled", "uploaded"].contains(&field(s, "status"))
        };
        let mut extra: Vec<Value> = Vec::new();
        for list in by_day.values_mut() {
            while list.len() > limit {
                let Some(i) = list.iter().rposition(|s| movable(s)) else { break };
                extra.push(list.remove(i));
            }
        }
        if extra.is_empty() {
            continue;
        }
        extra.sort_by_key(|s| field(s, "scheduled_for").to_string());
        let local = studio
            .channels()?
            .iter()
            .flat_map(|c| studio.accounts(c.id).unwrap_or_default())
            .find(|a| a.platform == "YOUTUBE" && a.external_id == field(account, "channel_id"));
        let mut day = by_day.keys().next().copied().unwrap_or_else(|| Local::now().date_naive());
        for s in extra {
            // The first free publishing time on or after the Short's own day.
            let own = local_day(field(&s, "scheduled_for")).unwrap_or(day);
            day = day.max(own);
            let at = loop {
                let used: Vec<String> = by_day.get(&day).map(|l| l.iter().map(|x| field(x, "scheduled_for").to_string()).collect()).unwrap_or_default();
                let free = (used.len() < limit)
                    .then(|| {
                        times.iter().find_map(|t| {
                            let when = Local.from_local_datetime(&day.and_time(*t)).single()?;
                            let iso = clock::iso(when.timestamp_millis());
                            (when.timestamp_millis() > now + 60 * 60_000 && !used.iter().any(|u| clock::parse_iso(u) == Some(when.timestamp_millis()))).then_some(iso)
                        })
                    })
                    .flatten();
                match free {
                    Some(iso) => break iso,
                    None => day = day.succ_opt().unwrap_or(day),
                }
            };
            let id = field(&s, "id").to_string();
            let result = (|| -> Result<()> {
                if field(&s, "status") == "uploaded" {
                    let a = local.as_ref().ok_or_else(|| StudioError::Other("this PC has no key for the channel".into()))?;
                    let secret = studio.secret(&a.credentials_reference)?;
                    crate::youtube::set_publish_at(&secret, field(&s, "youtube_video_id"), &at).map_err(StudioError::Other)?;
                    cloud::call(studio, "rescheduleYoutube", json!({"id": id, "scheduledFor": at, "movedOnYoutube": true}))?;
                } else {
                    cloud::call(studio, "rescheduleYoutube", json!({"id": id, "scheduledFor": at}))?;
                }
                // The calendar on this PC follows.
                studio.connect()?.execute(
                    "UPDATE publishing_jobs SET scheduled_at = ?1, updated_at = ?2 WHERE platform = 'YOUTUBE' AND remote_state_json LIKE ?3",
                    params![at, clock::now_iso(), format!("%{id}%")],
                )?;
                Ok(())
            })();
            let what = field(&s, "local_content_id").to_string();
            match result {
                Ok(()) => {
                    log.push(format!("{what}: moved from {} to {}", clock::local_time(field(&s, "scheduled_for")), clock::local_time(&at)));
                    let mut moved = s.clone();
                    moved["scheduled_for"] = json!(at);
                    by_day.entry(day).or_default().push(moved);
                }
                Err(e) => log.push(format!("{what}: not moved ({e})")),
            }
        }
    }
    Ok(log)
}

/// Every cloud Short (newest time first), as the cloud lists them.
pub fn list(studio: &Studio) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    let mut offset = 0;
    for _ in 0..20 {
        let page = cloud::call(studio, "listYoutube", json!({"offset": offset}))?;
        out.extend(page.get("items").and_then(Value::as_array).cloned().unwrap_or_default());
        match page.get("nextOffset").and_then(Value::as_i64) {
            Some(n) => offset = n,
            None => break,
        }
    }
    Ok(out)
}

/// How many of this PC's YouTube publications the cloud is still to upload.
pub fn waiting(studio: &Studio) -> Result<usize> {
    let n: i64 = studio.connect()?.query_row("SELECT COUNT(*) FROM publishing_jobs WHERE platform = 'YOUTUBE' AND status = ?1", [CLOUD], |r| r.get(0))?;
    Ok(n as usize)
}

/// Hands every waiting local YouTube job to the cloud, keeping its time (a time already past,
/// or too close, moves to shortly ahead). Blocking. Returns a line per job.
pub fn move_queue(studio: &Studio) -> Result<Vec<String>> {
    let jobs: Vec<(i64, i64, i64, String)> = {
        let c = studio.connect()?;
        let mut q = c.prepare(
            "SELECT channel_id, render_id, social_account_id, scheduled_at FROM publishing_jobs WHERE platform = 'YOUTUBE' AND status = 'SCHEDULED' ORDER BY scheduled_at",
        )?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<std::result::Result<_, _>>()?
    };
    let soonest = clock::now_ms() + 2 * LEAD_MS;
    let mut log = Vec::new();
    for (channel, render, account, at) in jobs {
        let at = clock::iso(clock::parse_iso(&at).unwrap_or(soonest).max(soonest));
        match schedule(studio, channel, render, account, &at) {
            Ok(_) => log.push(format!("render {render}: in the cloud for {}", clock::local_time(&at))),
            Err(e) => log.push(format!("render {render}: stays on this PC ({e})")),
        }
    }
    Ok(log)
}

/// Records what the cloud has done with this PC's `CLOUD` publications: uploaded ones become
/// published (with their video link), failed and cancelled ones say so. Returns how many changed.
pub fn sync(studio: &Studio) -> Result<usize> {
    let waiting: Vec<(i64, String)> = {
        let c = studio.connect()?;
        let mut q = c.prepare("SELECT id, COALESCE(remote_state_json, '{}') FROM publishing_jobs WHERE platform = 'YOUTUBE' AND status = ?1")?;
        q.query_map([CLOUD], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?
    };
    if waiting.is_empty() {
        return Ok(0);
    }
    let mut shorts: HashMap<String, Value> = HashMap::new();
    let mut offset = 0;
    for _ in 0..10 {
        let page = cloud::call(studio, "listYoutube", json!({"offset": offset}))?;
        for s in page.get("items").and_then(Value::as_array).into_iter().flatten() {
            shorts.insert(field(s, "id").to_string(), s.clone());
        }
        match page.get("nextOffset").and_then(Value::as_i64) {
            Some(n) => offset = n,
            None => break,
        }
    }
    let mut changed = 0;
    for (job, state) in waiting {
        let id = serde_json::from_str::<Value>(&state).ok().map(|v| field(&v, "cloudYoutube").to_string()).unwrap_or_default();
        let Some(s) = shorts.get(&id) else { continue };
        match field(s, "status") {
            "uploaded" => {
                let video = field(s, "youtube_video_id").to_string();
                studio.finish_publication(job, Ok((video.clone(), Some(format!("https://youtube.com/shorts/{video}")))))?;
                changed += 1;
            }
            "failed" => {
                studio.finish_publication(job, Err((false, format!("cloud YouTube upload failed: {}", field(s, "last_error")))))?;
                changed += 1;
            }
            "cancelled" => {
                studio.connect()?.execute(
                    "UPDATE publishing_jobs SET status = 'CANCELLED', updated_at = ?1 WHERE id = ?2 AND status = ?3",
                    params![clock::now_iso(), job, CLOUD],
                )?;
                changed += 1;
            }
            _ => {}
        }
    }
    Ok(changed)
}
