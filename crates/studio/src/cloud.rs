//! Cloud publishing through the Shorts Factory Supabase backend: schedules that publish while the
//! PC is off. Same contract as the earlier app (`cloud/docs/desktop-integration.md`): every call
//! is a POST to the `publishing-api` edge function; a Reel is reserved (`createUpload`), its MP4 is
//! uploaded straight to the signed storage URL (Backblaze B2 or Supabase Storage), then
//! `finalizeUploadAndSchedule` confirms it. Video bytes never pass through the edge function.
//!
//! Sign-in uses the credentials already in the library's `.env` (`CLOUD_SUPABASE_URL`,
//! `CLOUD_SUPABASE_ANON_KEY`, `CLOUD_AUTH_EMAIL`, `CLOUD_AUTH_PASSWORD`); the session lives in memory.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};

use crate::clock::{now_ms, parse_iso};
use crate::{Result, Studio, StudioError, err};

#[derive(Clone, Debug)]
struct Session {
    url: String,
    anon: String,
    access: String,
    refresh: String,
    expires_ms: i64,
}

fn session_slot() -> std::sync::MutexGuard<'static, Option<Session>> {
    static S: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(PoisonError::into_inner)
}
/// Idempotency keys of reservations in flight, per render: a retry after a timeout reuses the
/// key, so the cloud never makes a second schedule for one Reel.
fn keys() -> std::sync::MutexGuard<'static, HashMap<i64, String>> {
    static K: OnceLock<Mutex<HashMap<i64, String>>> = OnceLock::new();
    K.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner)
}

fn agent(secs: u64) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).timeout(Duration::from_secs(secs)).redirects(0).build()
}

/// Is cloud publishing configured in `.env`?
pub fn configured(studio: &Studio) -> bool {
    ["CLOUD_SUPABASE_URL", "CLOUD_SUPABASE_ANON_KEY", "CLOUD_AUTH_EMAIL", "CLOUD_AUTH_PASSWORD"].iter().all(|k| studio.var(k).is_some())
}

fn valid_project_url(url: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|h| h.strip_suffix(".supabase.co"))
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'))
}

fn auth(url: &str, anon: &str, grant: &str, body: Value) -> Result<Session> {
    let r = agent(20)
        .post(&format!("{url}/auth/v1/token?grant_type={grant}"))
        .set("apikey", anon)
        .send_json(body)
        .map_err(|_| StudioError::Other("cloud sign-in failed: check the cloud credentials in the .env file and the internet connection".into()))?;
    let v: Value = r.into_json().map_err(|e| StudioError::Other(e.to_string()))?;
    let get = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    Ok(Session {
        url: url.to_string(),
        anon: anon.to_string(),
        access: get("access_token").ok_or_else(|| StudioError::Other("cloud sign-in returned no session".into()))?,
        refresh: get("refresh_token").unwrap_or_default(),
        expires_ms: now_ms() + v.get("expires_in").and_then(Value::as_i64).unwrap_or(3600) * 1000,
    })
}

fn active(studio: &Studio) -> Result<Session> {
    if !configured(studio) {
        return err("cloud publishing is not configured: add the CLOUD_* settings to the library's .env file");
    }
    let url = studio.var("CLOUD_SUPABASE_URL").unwrap_or_default().trim().trim_end_matches('/').to_string();
    if !valid_project_url(&url) {
        return err("CLOUD_SUPABASE_URL must be https://PROJECT.supabase.co");
    }
    let anon = studio.var("CLOUD_SUPABASE_ANON_KEY").unwrap_or_default().trim().to_string();
    let mut slot = session_slot();
    if let Some(s) = slot.as_ref().filter(|s| s.url == url && s.anon == anon) {
        if s.expires_ms > now_ms() + 60_000 {
            return Ok(s.clone());
        }
        if let Ok(fresh) = auth(&url, &anon, "refresh_token", json!({"refresh_token": s.refresh})) {
            *slot = Some(fresh.clone());
            return Ok(fresh);
        }
    }
    let email = studio.var("CLOUD_AUTH_EMAIL").unwrap_or_default();
    let password = studio.var("CLOUD_AUTH_PASSWORD").unwrap_or_default();
    let fresh = auth(&url, &anon, "password", json!({"email": email, "password": password}))?;
    *slot = Some(fresh.clone());
    Ok(fresh)
}

/// One `publishing-api` action.
pub fn call(studio: &Studio, action: &str, mut body: Value) -> Result<Value> {
    let s = active(studio)?;
    if !body.is_object() {
        body = json!({});
    }
    body["action"] = json!(action);
    match agent(60)
        .post(&format!("{}/functions/v1/publishing-api", s.url))
        .set("Authorization", &format!("Bearer {}", s.access))
        .set("apikey", &s.anon)
        .send_json(body)
    {
        Ok(r) => r.into_json().map_err(|e| StudioError::Other(e.to_string())),
        Err(ureq::Error::Status(code, r)) => {
            let v: Value = r.into_json().unwrap_or(Value::Null);
            err(v.get("error").and_then(Value::as_str).map_or_else(|| format!("cloud request failed ({code})"), str::to_string))
        }
        Err(_) => err("connection required: the cloud has not confirmed this operation; retry with the same request"),
    }
}

/// A Reel's cloud upload destination must be the reservation's own object, on B2 or this
/// project's Storage, over HTTPS without credentials or ports.
pub fn valid_upload_target(url: &str, project: &str, provider: &str, bucket: &str, key: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let (host, path_query) = rest.split_once('/').unwrap_or((rest, ""));
    if host.contains('@') || host.contains(':') {
        return false;
    }
    let path = format!("/{}", path_query.split('?').next().unwrap_or(""));
    match provider {
        "b2" => {
            let region_ok = host
                .strip_prefix("s3.")
                .and_then(|h| h.strip_suffix(".backblazeb2.com"))
                .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
            region_ok && path == format!("/{bucket}/{key}") && path_query.contains("X-Amz-Algorithm=AWS4-HMAC-SHA256")
        }
        "supabase" => format!("https://{host}") == project && path == format!("/storage/v1/object/upload/sign/{bucket}/{key}"),
        _ => false,
    }
}

pub(crate) fn new_key() -> String {
    // A v4-shaped UUID from the clock and a counter: unique per reservation, not a secret.
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut x = (now_ms() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ u64::from(std::process::id()).rotate_left(32)
        ^ N.fetch_add(1, std::sync::atomic::Ordering::Relaxed).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let (a, b) = (next(), next());
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", a >> 32, (a >> 16) & 0xffff, a & 0xfff, (b >> 48) & 0x3fff | 0x8000, b & 0xffff_ffff_ffff)
}

/// The local IANA time zone name (the cloud schedules in it).
pub fn local_timezone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into())
}

/// Schedules an approved render in the cloud: reserve, upload, finalize. Returns the cloud post.
pub fn schedule(studio: &Studio, channel: i64, render: i64, account: &str, caption: &str, scheduled_for_iso: &str, publish_now: bool) -> Result<Value> {
    let key = keys().entry(render).or_insert_with(new_key).clone();
    schedule_with_key(studio, channel, render, account, caption, scheduled_for_iso, publish_now, &key)
}

/// A persisted key lets a local publication resume the same cloud reservation after a restart.
#[allow(clippy::too_many_arguments)]
pub(crate) fn schedule_with_key(
    studio: &Studio,
    channel: i64,
    render: i64,
    account: &str,
    caption: &str,
    scheduled_for_iso: &str,
    publish_now: bool,
    key: &str,
) -> Result<Value> {
    let c = studio.connect()?;
    let (status, approved, file, demo, name): (String, Option<String>, Option<String>, i64, Option<String>) = c
        .query_row(
            "SELECT status, approved_at, file_path, demo, name FROM renders WHERE id = ?1 AND channel_id = ?2",
            rusqlite::params![render, channel],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .map_err(|_| StudioError::Other("render not found in this workspace".into()))?;
    if status != "APPROVED" || approved.is_none() || demo != 0 {
        return err("choose an approved, non-demo render");
    }
    let file = studio.resolve_storage(&file.ok_or_else(|| StudioError::Other("the render has no file".into()))?)?;
    let size = std::fs::metadata(&file)?.len();
    let mut head = [0u8; 12];
    std::fs::File::open(&file)?.read_exact(&mut head)?;
    if !(12..=500 * 1024 * 1024).contains(&size) || head.get(4..8) != Some(b"ftyp") {
        return err("a cloud Reel must be an MP4 of at most 500 MB");
    }
    if parse_iso(scheduled_for_iso).is_none() {
        return err("unreadable schedule time");
    }
    let filename = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "reel.mp4".into());
    let reserved = call(
        studio,
        "createUpload",
        json!({
            "idempotencyKey": key, "instagramAccountId": account, "localContentId": format!("{channel}:{render}"),
            "title": name.unwrap_or_else(|| format!("Render {render}")), "caption": caption.chars().take(2200).collect::<String>(),
            "filename": filename, "mimeType": "video/mp4", "fileSize": size,
            "scheduledFor": scheduled_for_iso, "timezone": local_timezone(), "publishNow": publish_now,
            // The cover: the thumbnail's moment.
            "thumbOffsetMs": crate::thumbnails::ensure(studio, render).ok().map(|c| (c.at * 1000.0).round() as u64),
        }),
    )?;
    let post = reserved.get("post").cloned().unwrap_or(Value::Null);
    let id = post.get("id").and_then(Value::as_str).ok_or_else(|| StudioError::Other("the cloud returned no reservation".into()))?.to_string();
    if let Some(url) = reserved.get("uploadUrl").and_then(Value::as_str).filter(|_| reserved.get("uploaded") != Some(&Value::Bool(true))) {
        let check = call(studio, "getSchedule", json!({"id": id}))?;
        let p = check.get("post").cloned().unwrap_or(Value::Null);
        let field = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
        if field("local_content_id") != format!("{channel}:{render}")
            || p.get("file_size").and_then(Value::as_u64) != Some(size)
            || field("status") != "uploading"
        {
            return err("the cloud reservation does not match this render");
        }
        let project = active(studio)?.url;
        if !valid_upload_target(url, &project, &field("storage_provider"), &field("storage_bucket"), &field("storage_path")) {
            return err("unexpected cloud upload destination");
        }
        upload(url, &file, size)?;
    }
    let done = call(studio, "finalizeUploadAndSchedule", json!({"id": id}))?;
    if done.get("cloudConfirmed") == Some(&Value::Bool(true)) {
        keys().remove(&render);
    }
    Ok(done)
}

/// The signed-in cloud project's URL.
pub(crate) fn project_url(studio: &Studio) -> Result<String> {
    Ok(active(studio)?.url)
}

pub(crate) fn upload(url: &str, file: &Path, size: u64) -> Result<()> {
    upload_as(url, file, size, "video/mp4")
}

pub(crate) fn upload_as(url: &str, file: &Path, size: u64, content_type: &str) -> Result<()> {
    let f = std::fs::File::open(file)?;
    match agent(1800).put(url).set("Content-Type", content_type).set("Content-Length", &size.to_string()).send(f) {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(code, _)) => err(format!("video upload not confirmed ({code}); retry the same schedule")),
        Err(e) => err(format!("video upload failed ({e}); retry the same schedule")),
    }
}

/// How long an account's Instagram token has left, and the warning level the earlier app shows
/// (7 days, 3 days, 24 hours, expired).
pub fn token_health(expires_at: Option<&str>) -> (String, u8) {
    let Some(t) = expires_at.and_then(parse_iso) else { return ("expiry unknown".into(), 0) };
    let left = t - now_ms();
    let hours = left / 3_600_000;
    let level = if left <= 0 {
        4
    } else if hours <= 24 {
        3
    } else if hours <= 72 {
        2
    } else if hours <= 7 * 24 {
        1
    } else {
        0
    };
    let text = if left <= 0 {
        "token expired".to_string()
    } else if hours < 48 {
        format!("{hours} hours left")
    } else {
        format!("{} days left", hours / 24)
    };
    (text, level)
}

/// "until 2026-12-13 14:02 (59 days left)" from a backend `expires` / `token_expires_at`.
pub fn expiry_text(expires_at: Option<&str>) -> String {
    let (left, _) = token_health(expires_at);
    match expires_at.and_then(parse_iso).and_then(chrono::DateTime::from_timestamp_millis) {
        Some(d) => format!("until {} ({left})", d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M")),
        None => left,
    }
}

/// Gives a cloud Instagram account a fresh token (`connectInstagram`, which replaces the stored
/// one) and extends it at once (`extendInstagramToken`), so a short-lived token from Graph API
/// Explorer becomes a long-lived one before it can lapse. Returns the new expiry text. If the
/// extension fails, the fresh token stays connected and the error says so.
pub fn replace_token(studio: &Studio, account_id: &str, instagram_user_id: &str, token: &str) -> Result<String> {
    let token = token.trim();
    if token.len() < 20 || token.chars().any(char::is_whitespace) {
        return err("Paste the whole access token");
    }
    if !instagram_user_id.bytes().all(|b| b.is_ascii_digit()) || instagram_user_id.is_empty() {
        return err("this cloud account has no Instagram account ID");
    }
    let connected = call(studio, "connectInstagram", json!({"instagramUserId": instagram_user_id, "accessToken": token}))?;
    match call(studio, "extendInstagramToken", json!({"id": account_id})) {
        Ok(h) => Ok(format!("token replaced and extended, valid {}", expiry_text(h.get("expires").and_then(Value::as_str)))),
        Err(e) => Ok(format!("token replaced, valid {}; extending it failed: {e}", expiry_text(connected.get("token_expires_at").and_then(Value::as_str)))),
    }
}

/// Explicitly reuse this workspace's local credential for the matching cloud account only.
pub fn use_local_token(studio: &Studio, local_account: i64, cloud_account: &str) -> Result<String> {
    let (_, account) = studio.account(local_account)?;
    if account.platform != "INSTAGRAM" {
        return err("choose an Instagram account");
    }
    let accounts = call(studio, "accounts", json!({}))?;
    if !accounts.as_array().is_some_and(|list| {
        list.iter().any(|a| {
            a.get("id").and_then(Value::as_str) == Some(cloud_account)
                && a.get("instagram_user_id").and_then(Value::as_str) == Some(account.external_id.as_str())
        })
    }) {
        return err("the local and cloud accounts do not refer to the same Instagram profile");
    }
    let secret = studio.secret(&account.credentials_reference)?;
    let token = secret
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| StudioError::Other("the local account has no token".into()))?;
    replace_token(studio, cloud_account, &account.external_id, token)
}
