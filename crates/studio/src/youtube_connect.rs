//! Connecting a YouTube channel to a workspace: Google's OAuth for installed apps.
//!
//! [`start`] opens a listener on a free local port (127.0.0.1 only) and returns Google's consent
//! URL for the browser. [`finish`] waits for Google to send the browser back to that port with a
//! code, exchanges it for a refresh token, reads which channel it is, then keeps the token in the
//! workspace's own key file and adds (or updates) the workspace's YouTube account. The client ID and
//! secret are the user's own Google Cloud "Desktop app" client (YOUTUBE_CLIENT_ID /
//! YOUTUBE_CLIENT_SECRET). Tokens are never logged or returned.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use crate::{Result, Studio, StudioError, err};

const SCOPES: &str = "https://www.googleapis.com/auth/youtube.upload https://www.googleapis.com/auth/youtube.readonly";

/// A sign-in waiting for Google to send the browser back.
pub struct Pending {
    listener: TcpListener,
    redirect: String,
    state: String,
    client_id: String,
    client_secret: String,
    channel: i64,
    /// The consent page to open in the browser.
    pub url: String,
    /// Set to stop waiting (the Cancel button).
    pub cancel: Arc<AtomicBool>,
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An unguessable value that ties Google's reply to this sign-in.
fn random_state() -> String {
    use std::hash::{BuildHasher, Hasher};
    (0..4)
        .map(|i| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
            h.write_u32(std::process::id());
            h.write_u8(i);
            format!("{:016x}", h.finish())
        })
        .collect()
}

/// The workspace's Google client, from YOUTUBE_CLIENT_ID / YOUTUBE_CLIENT_SECRET.
pub(crate) fn client(studio: &Studio) -> Result<(String, String)> {
    let id = studio.var("YOUTUBE_CLIENT_ID").map(str::trim).filter(|v| !v.is_empty());
    let secret = studio.var("YOUTUBE_CLIENT_SECRET").map(str::trim).filter(|v| !v.is_empty());
    match (id, secret) {
        // A common slip: the client ID pasted into the secret field too.
        (Some(i), Some(s)) if s == i || s.ends_with(".apps.googleusercontent.com") => {
            err("The Google client secret field holds the client ID. Copy the Client secret (it starts with GOCSPX-) from your Desktop client and Save")
        }
        (Some(i), Some(s)) => Ok((i.to_string(), s.to_string())),
        _ => err("Add your Google OAuth client (YOUTUBE_CLIENT_ID and YOUTUBE_CLIENT_SECRET) in Plugin Settings > Accounts & keys first"),
    }
}

/// Asks Google whether the client ID and secret belong together: a refresh with a dummy token
/// answers `invalid_grant` for a good client and `invalid_client` for a bad one. Returns the
/// client ID's short form. Blocking.
pub fn check_client(studio: &Studio) -> Result<String> {
    let (id, secret) = client(studio)?;
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(15)).build();
    let short = format!("…{}", id.split('.').next().unwrap_or(&id).chars().rev().take(8).collect::<String>().chars().rev().collect::<String>());
    match agent.post("https://oauth2.googleapis.com/token").send_form(&[
        ("client_id", id.as_str()),
        ("client_secret", secret.as_str()),
        ("refresh_token", "health-check"),
        ("grant_type", "refresh_token"),
    ]) {
        Ok(_) => Ok(short),
        Err(ureq::Error::Status(_, r)) => {
            let v: Value = r.into_json().unwrap_or(Value::Null);
            match v.get("error").and_then(Value::as_str).unwrap_or_default() {
                "invalid_grant" => Ok(short),
                "invalid_client" => err("Google does not accept this client ID and secret together; copy both from the same Desktop client"),
                "unauthorized_client" => err("this Google client may not refresh sign-ins; use a Desktop app client"),
                other => err(format!("Google answered {other}")),
            }
        }
        Err(e) => err(format!("Google is not reachable ({e})")),
    }
}

/// Starts connecting a YouTube channel to `channel` (a workspace): open `url` in the browser.
pub fn start(studio: &Studio, channel: i64) -> Result<Pending> {
    let (client_id, client_secret) = client(studio)?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let state = random_state();
    let url = format!(
        "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=select_account%20consent&state={}",
        encode(&client_id),
        encode(&redirect),
        encode(SCOPES),
        state
    );
    Ok(Pending { listener, redirect, state, client_id, client_secret, channel, url, cancel: Arc::new(AtomicBool::new(false)) })
}

/// Waits (up to five minutes, or until cancelled) for the browser to come back, then saves the
/// account. Returns the channel's name. Blocking: call it off the UI thread.
pub fn finish(studio: &Studio, p: Pending) -> Result<String> {
    p.listener.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(300);
    let (code, mut browser) = loop {
        if p.cancel.load(Ordering::Relaxed) {
            return err("cancelled");
        }
        if Instant::now() > deadline {
            return err("Google sign-in timed out; press Connect YouTube again");
        }
        let (mut stream, _) = match p.listener.accept() {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut first = String::new();
        BufReader::new(&stream).read_line(&mut first)?;
        // "GET /?state=…&code=…&scope=… HTTP/1.1"
        let query = first.split_whitespace().nth(1).and_then(|p| p.split_once('?')).map(|(_, q)| q.to_string()).unwrap_or_default();
        let field = |k: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| decode(v)));
        // Browsers also ask for /favicon.ico; only the reply with our state counts.
        if field("state").as_deref() != Some(p.state.as_str()) {
            reply(&mut stream, "Waiting for Google…", "");
            continue;
        }
        if let Some(e) = field("error") {
            reply(&mut stream, "YouTube was not connected.", "Google sign-in was not completed.");
            return err(format!("Google sign-in was not completed ({e})"));
        }
        match field("code") {
            // The browser waits for the real result, sent once the account is saved.
            Some(c) if !c.is_empty() => break (c, stream),
            _ => {
                reply(&mut stream, "YouTube was not connected.", "Google returned no sign-in code.");
                return err("Google returned no sign-in code");
            }
        }
    };
    let result = complete(studio, &p, &code);
    match &result {
        Ok(name) => reply(&mut browser, "YouTube is connected.", &format!("Channel: {name}")),
        Err(e) => reply(&mut browser, "YouTube was not connected.", &e.to_string()),
    }
    result
}

/// The browser's answer page.
fn reply(stream: &mut std::net::TcpStream, msg: &str, detail: &str) {
    let html = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Shorts Factory Desktop</title><body style=\"font-family:sans-serif;padding:40px\"><h2>{}</h2><p>{}</p><p>You can close this tab and return to Shorts Factory Desktop.</p>",
        html(msg),
        html(detail)
    );
    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
}

/// Exchanges the sign-in code for a refresh token, reads the channel and saves the account.
fn complete(studio: &Studio, p: &Pending, code: &str) -> Result<String> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build();
    let net = |what: &str, e: ureq::Error| -> StudioError {
        match e {
            ureq::Error::Status(c, r) => {
                let v: Value = r.into_json().unwrap_or(Value::Null);
                let m = v.get("error_description").or_else(|| v.pointer("/error/message")).and_then(Value::as_str).unwrap_or("no message");
                StudioError::Other(format!("{what} failed (HTTP {c}): {}", m.chars().take(200).collect::<String>()))
            }
            ureq::Error::Transport(t) => StudioError::Other(format!("{what} failed: Google is not reachable ({:?})", t.kind())),
        }
    };
    let tokens: Value = agent
        .post("https://oauth2.googleapis.com/token")
        .send_form(&[
            ("code", code),
            ("client_id", p.client_id.as_str()),
            ("client_secret", p.client_secret.as_str()),
            ("redirect_uri", p.redirect.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .map_err(|e| net("Exchanging the Google sign-in", e))?
        .into_json()
        .map_err(|_| StudioError::Other("Google's answer was unreadable".into()))?;
    let refresh = tokens.get("refresh_token").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or_else(|| {
        StudioError::Other("Google gave no refresh token; remove the app's access at myaccount.google.com/permissions and connect again".into())
    })?;
    let access = tokens.get("access_token").and_then(Value::as_str).unwrap_or_default();
    let me: Value = agent
        .get("https://www.googleapis.com/youtube/v3/channels")
        .set("Authorization", &format!("Bearer {access}"))
        .query("part", "snippet")
        .query("mine", "true")
        .call()
        .map_err(|e| net("Reading your YouTube channel", e))?
        .into_json()
        .map_err(|_| StudioError::Other("Google's answer was unreadable".into()))?;
    let item = me.pointer("/items/0").ok_or_else(|| StudioError::Other("This Google account has no YouTube channel".into()))?;
    let channel_id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let name = item.pointer("/snippet/title").and_then(Value::as_str).unwrap_or("YouTube").to_string();
    save(studio, p.channel, &channel_id, &name, json!({"clientId": p.client_id, "clientSecret": p.client_secret, "refreshToken": refresh, "privacyStatus": "public"}))?;
    Ok(name)
}

/// Keeps the token in the workspace's YouTube key file and adds or updates its account row; the
/// workspace then publishes to YouTube too.
pub(crate) fn save(studio: &Studio, channel: i64, external_id: &str, name: &str, secret: Value) -> Result<i64> {
    if external_id.is_empty() {
        return err("YouTube returned no channel id");
    }
    let mut c = studio.connect()?;
    let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let slug: String = tx.query_row("SELECT slug FROM channels WHERE id = ?1", [channel], |r| r.get(0))?;
    let existing: Option<(i64, String)> = tx
        .query_row(
            "SELECT id, credentials_reference FROM social_accounts WHERE channel_id = ?1 AND platform = 'YOUTUBE' AND external_account_id = ?2",
            params![channel, external_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let slug: String = slug.to_ascii_lowercase().chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' }).collect();
    let reference = existing.as_ref().map(|(_, r)| r.clone()).unwrap_or_else(|| format!("youtube-{slug}.json"));
    let file = studio.credential_path(&reference)?;
    std::fs::create_dir_all(&studio.secrets)?;
    let tmp = file.with_extension("json.tmp");
    std::fs::write(&tmp, secret.to_string())?;
    std::fs::rename(tmp, &file)?;
    let now = crate::now_iso();
    let id = match existing {
        Some((id, _)) => {
            tx.execute(
                "UPDATE social_accounts SET display_name = ?1, enabled = 1, status = 'CONNECTED', last_verified_at = ?2, updated_at = ?2 WHERE id = ?3",
                params![name, now, id],
            )?;
            id
        }
        None => {
            tx.execute(
                "INSERT INTO social_accounts (created_at, updated_at, channel_id, platform, display_name, external_account_id, credentials_reference, enabled, status, last_verified_at)
                 VALUES (?1, ?1, ?2, 'YOUTUBE', ?3, ?4, ?5, 1, 'CONNECTED', ?1)",
                params![now, channel, name, external_id, reference],
            )?;
            tx.last_insert_rowid()
        }
    };
    // The workspace publishes to YouTube from now on (its other settings are kept).
    let settings: Option<String> = tx.query_row("SELECT json FROM channel_settings WHERE channel_id = ?1", [channel], |r| r.get(0)).optional()?;
    let mut v: Value = settings.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_else(|| json!({}));
    if let Some(o) = v.as_object_mut() {
        let list = o.entry("enabledPlatforms").or_insert_with(|| json!([]));
        if let Some(a) = list.as_array_mut()
            && !a.iter().any(|p| p == "YOUTUBE")
        {
            a.push(json!("YOUTUBE"));
        }
    }
    tx.execute(
        "INSERT INTO channel_settings (channel_id, json) VALUES (?1, ?2) ON CONFLICT(channel_id) DO UPDATE SET json = excluded.json",
        params![channel, v.to_string()],
    )?;
    tx.commit()?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_round_trip_and_states_differ() {
        assert_eq!(encode("a b/c?d=é"), "a%20b%2Fc%3Fd%3D%C3%A9");
        assert_eq!(decode("4%2F0Ab-x_y%3D"), "4/0Ab-x_y=");
        assert_eq!(decode("100%"), "100%");
        assert_ne!(random_state(), random_state());
        assert_eq!(random_state().len(), 64);
    }
}
