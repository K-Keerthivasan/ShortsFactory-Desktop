//! YouTube Shorts through the YouTube Data API v3: an OAuth refresh, a resumable upload session,
//! then the upload, resumed from the last byte YouTube has when interrupted.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::Studio;
use crate::instagram::{PublishError, Published};

const UPLOAD: &str = "https://www.googleapis.com/upload/youtube/v3/videos?uploadType=resumable&part=snippet,status";

fn token(agent: &ureq::Agent, secret: &Value) -> std::result::Result<String, PublishError> {
    let get = |k: &str| secret.get(k).and_then(Value::as_str).unwrap_or_default();
    if get("clientId").is_empty() || get("clientSecret").is_empty() || get("refreshToken").is_empty() {
        return Err(PublishError::Retry("missing YouTube OAuth client or refresh token".into()));
    }
    let r = agent.post("https://oauth2.googleapis.com/token").send_form(&[
        ("client_id", get("clientId")),
        ("client_secret", get("clientSecret")),
        ("refresh_token", get("refreshToken")),
        ("grant_type", "refresh_token"),
    ]);
    match r {
        Ok(r) => r
            .into_json::<Value>()
            .ok()
            .and_then(|v| v.get("access_token").and_then(Value::as_str).map(str::to_string))
            .ok_or_else(|| PublishError::Retry("YouTube OAuth refresh returned no token".into())),
        Err(ureq::Error::Status(c, _)) => Err(PublishError::Retry(format!("YouTube OAuth refresh failed: HTTP {c}"))),
        Err(e) => Err(PublishError::Retry(format!("YouTube OAuth connection failed: {e}"))),
    }
}

fn valid(url: &str) -> std::result::Result<&str, PublishError> {
    let host = url.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or("");
    if host.ends_with("googleapis.com") { Ok(url) } else { Err(PublishError::Retry("unexpected YouTube upload host".into())) }
}

/// A YouTube API failure with Google's reason (`quotaExceeded`, `uploadLimitExceeded`, …), which
/// the upload queue reads to wait instead of failing the post.
fn api_error(what: &str, e: ureq::Error) -> PublishError {
    match e {
        ureq::Error::Status(code, r) => {
            let v: Value = r.into_json().unwrap_or(Value::Null);
            let reason = v.pointer("/error/errors/0/reason").and_then(Value::as_str).unwrap_or("");
            let message = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("no message");
            PublishError::Retry(format!("{what} failed (HTTP {code} {reason}): {}", message.chars().take(200).collect::<String>()))
        }
        ureq::Error::Transport(t) => PublishError::Retry(format!("{what} failed: YouTube is not reachable ({:?})", t.kind())),
    }
}

/// The offset after `bytes=0-N`, from a 308 response's Range header.
fn range_end(r: &ureq::Response) -> u64 {
    r.header("range").and_then(|h| h.rsplit('-').next()).and_then(|n| n.trim().parse::<u64>().ok()).map_or(0, |n| n + 1)
}

/// Uploads the Short. With `publish_at` (UTC, RFC 3339) it is uploaded Private and YouTube itself
/// makes it Public at that time (YouTube Studio shows it as Scheduled).
#[allow(clippy::too_many_arguments)]
pub fn publish(
    studio: &Studio,
    job: i64,
    channel: i64,
    secret: &Value,
    file: &Path,
    title: &str,
    description: &str,
    tags: &[String],
    publish_at: Option<&str>,
) -> Published {
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(30)).timeout(Duration::from_secs(1800)).build();
    let mut state = studio.remote_state(job)?;
    if let Some(id) = state.get("publishedId").and_then(Value::as_str) {
        return Ok((id.to_string(), Some(format!("https://youtu.be/{id}"))));
    }
    let access = token(&agent, secret)?;
    let auth = format!("Bearer {access}");
    let size = std::fs::metadata(file).map_err(|e| PublishError::Retry(format!("cannot read the video: {e}")))?.len();
    let done = |v: Value, state: &mut Value| -> Published {
        let id = v.get("id").and_then(Value::as_str).ok_or_else(|| PublishError::Reconcile("YouTube did not return a video id".into()))?.to_string();
        state["phase"] = json!("PUBLISHED");
        state["publishedId"] = json!(id);
        studio.checkpoint(job, channel, state)?;
        Ok((id.clone(), Some(format!("https://youtu.be/{id}"))))
    };
    let mut offset = 0;
    match state.get("uploadUrl").and_then(Value::as_str).map(str::to_string) {
        None => {
            studio.checkpoint(job, channel, &json!({"phase": "INITIALIZING"}))?;
            let privacy = secret.get("privacyStatus").and_then(Value::as_str).filter(|p| ["public", "unlisted", "private"].contains(p)).unwrap_or("public");
            let status = match publish_at {
                Some(at) => json!({"privacyStatus": "private", "publishAt": at, "selfDeclaredMadeForKids": false}),
                None => json!({"privacyStatus": privacy, "selfDeclaredMadeForKids": false}),
            };
            let body = json!({
                "snippet": {"title": title.chars().take(100).collect::<String>(), "description": description.chars().take(5000).collect::<String>(), "tags": tags.iter().take(30).collect::<Vec<_>>(), "categoryId": "22"},
                "status": status
            });
            let r = agent
                .post(UPLOAD)
                .set("Authorization", &auth)
                .set("X-Upload-Content-Length", &size.to_string())
                .set("X-Upload-Content-Type", "video/mp4")
                .send_json(body)
                .map_err(|e| api_error("YouTube upload session creation", e))?;
            let location = r.header("location").ok_or_else(|| PublishError::Retry("YouTube returned no upload session".into()))?.to_string();
            state = json!({"phase": "CREATED", "uploadUrl": location, "offset": 0});
            studio.checkpoint(job, channel, &state)?;
        }
        Some(url) => {
            let r = agent.put(valid(&url)?).set("Authorization", &auth).set("Content-Range", &format!("bytes */{size}")).send_bytes(&[]);
            match r {
                Ok(r) => return done(r.into_json().unwrap_or(Value::Null), &mut state),
                Err(ureq::Error::Status(308, r)) => offset = range_end(&r),
                Err(e) => return Err(PublishError::Retry(format!("YouTube upload status query failed: {e}"))),
            }
            state["phase"] = json!("UPLOADING");
            state["offset"] = json!(offset);
            studio.checkpoint(job, channel, &state)?;
        }
    }
    let url = state.get("uploadUrl").and_then(Value::as_str).unwrap_or_default().to_string();
    let mut f = std::fs::File::open(file).map_err(|e| PublishError::Retry(e.to_string()))?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| PublishError::Retry(e.to_string()))?;
    let mut rest = Vec::new();
    f.read_to_end(&mut rest).map_err(|e| PublishError::Retry(e.to_string()))?;
    let last = size.saturating_sub(1);
    let r = agent
        .put(valid(&url)?)
        .set("Authorization", &auth)
        .set("Content-Type", "video/mp4")
        .set("Content-Range", &format!("bytes {offset}-{last}/{size}"))
        .send_bytes(&rest);
    match r {
        Ok(r) => done(r.into_json().unwrap_or(Value::Null), &mut state),
        Err(ureq::Error::Status(308, r)) => {
            state["phase"] = json!("UPLOADING");
            state["offset"] = json!(range_end(&r));
            studio.checkpoint(job, channel, &state)?;
            Err(PublishError::Retry("YouTube upload incomplete; the next retry resumes it".into()))
        }
        Err(e) => Err(api_error("YouTube video upload", e)),
    }
}
