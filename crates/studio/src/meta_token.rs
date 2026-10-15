//! Instagram (Facebook Login) access tokens of local accounts: when they expire, and making
//! them last.
//!
//! - [`Studio::token_info`] asks Meta (`debug_token`) whether an account's token is valid and
//!   when it expires.
//! - [`Studio::extend_account_token`] exchanges a still-valid token for a long-lived one
//!   (`fb_exchange_token`, about 60 days) and, when the Instagram account is linked to a Facebook
//!   Page, swaps that for the Page's token, which does not expire. The new token is checked against
//!   the Instagram account before it replaces the old one; on any failure the old one is kept.
//!
//! Tokens travel only to graph.facebook.com over HTTPS, are never logged, and never appear in an
//! error: Meta's requests carry them in the URL, so network errors are reported without it.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, Studio, StudioError, err};

/// What Meta says about a token.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenInfo {
    pub valid: bool,
    /// When it stops working (UTC ISO); None when it never expires.
    pub expires_at: Option<String>,
    /// "USER" or "PAGE".
    pub kind: String,
}

impl TokenInfo {
    /// "valid until 2026-12-13 14:02 (59 days)", "valid, never expires", "expired".
    pub fn describe(&self) -> String {
        if !self.valid {
            return "expired or revoked: paste a fresh token, Save it, then press Extend token within the hour".into();
        }
        match self.expires_at.as_deref() {
            None => format!("valid, never expires ({} token)", self.kind.to_lowercase()),
            Some(at) => {
                let (left, _) = crate::cloud::token_health(Some(at));
                let local = crate::clock::parse_iso(at)
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| at.to_string());
                format!("valid until {local} ({left})")
            }
        }
    }
}

/// A network failure without its URL (which would carry the token).
fn net(e: ureq::Error) -> StudioError {
    match e {
        ureq::Error::Status(code, r) => {
            let v: Value = r.into_json().unwrap_or(Value::Null);
            let m = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("no message");
            // Meta's messages name the problem, never the token.
            StudioError::Other(format!("Meta refused it (HTTP {code}): {}", m.chars().take(200).collect::<String>()))
        }
        ureq::Error::Transport(t) => StudioError::Other(format!("Meta is not reachable ({:?})", t.kind())),
    }
}

fn graph(studio: &Studio) -> Result<String> {
    let v = studio.var("META_GRAPH_VERSION").unwrap_or("v26.0");
    if !(v.starts_with('v') && v[1..].split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))) {
        return err("invalid META_GRAPH_VERSION");
    }
    Ok(format!("https://graph.facebook.com/{v}"))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).redirects(0).build()
}

fn iso(secs: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

impl Studio {
    fn meta_app(&self) -> Option<(String, String)> {
        let id = self.var("META_APP_ID").map(str::trim).filter(|v| !v.is_empty())?;
        let secret = self.var("META_APP_SECRET").map(str::trim).filter(|v| !v.is_empty())?;
        Some((id.to_string(), secret.to_string()))
    }

    fn instagram_token(&self, account: i64) -> Result<(crate::Account, String)> {
        let (_, a) = self.account(account)?;
        if a.platform != "INSTAGRAM" {
            return err("only Instagram tokens expire this way");
        }
        let token = self.secret(&a.credentials_reference)?.get("accessToken").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        if token.is_empty() {
            return err("this account has no token yet; paste one first");
        }
        Ok((a, token))
    }

    /// Meta's view of a token. With the app's ID and secret it asks as the app; without them,
    /// as the token itself (which works for the app's own developers).
    fn inspect(&self, token: &str) -> Result<TokenInfo> {
        let by = self.meta_app().map_or_else(|| token.to_string(), |(id, secret)| format!("{id}|{secret}"));
        let reply = agent().get(&format!("{}/debug_token", graph(self)?)).query("input_token", token).query("access_token", &by).call();
        let v: Value = match reply {
            Ok(r) => r.into_json().map_err(|_| StudioError::Other("Meta's answer was unreadable".into()))?,
            // Asking as an expired token itself: Meta refuses with OAuth error 190, which is the
            // answer (expired), not a failure to get one.
            Err(ureq::Error::Status(code, r)) => {
                let v: Value = r.into_json().unwrap_or(Value::Null);
                if v.pointer("/error/code").and_then(Value::as_i64) == Some(190) {
                    return Ok(TokenInfo { valid: false, expires_at: None, kind: "USER".into() });
                }
                let m = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("no message");
                return err(format!("Meta refused it (HTTP {code}): {}", m.chars().take(200).collect::<String>()));
            }
            Err(e) => return Err(net(e)),
        };
        let d = v.get("data").cloned().unwrap_or(Value::Null);
        // The earliest of the token's own expiry and its data-access expiry (0 = never).
        let ends = ["expires_at", "data_access_expires_at"].iter().filter_map(|k| d.get(*k).and_then(Value::as_i64)).filter(|s| *s > 0).min();
        Ok(TokenInfo {
            valid: d.get("is_valid").and_then(Value::as_bool).unwrap_or(false),
            expires_at: ends.and_then(iso),
            kind: d.get("type").and_then(Value::as_str).unwrap_or("USER").to_string(),
        })
    }

    /// Whether an account's token works and when it expires.
    pub fn token_info(&self, account: i64) -> Result<TokenInfo> {
        let (_, token) = self.instagram_token(account)?;
        self.inspect(&token)
    }

    /// Makes an account's token last: long-lived, and the never-expiring Page token when the
    /// Instagram account belongs to a Page. Needs META_APP_ID and META_APP_SECRET. The old token
    /// is kept unless the new one is valid, lasts longer and reaches the Instagram account.
    pub fn extend_account_token(&self, account: i64) -> Result<TokenInfo> {
        let Some((app_id, app_secret)) = self.meta_app() else {
            return err("Extending needs the Meta app's ID and secret: add META_APP_ID and META_APP_SECRET in Plugin Settings ▸ Cloud");
        };
        let (a, token) = self.instagram_token(account)?;
        let before = self.inspect(&token)?;
        if !before.valid {
            return err(
                "This token has already expired, and Meta cannot extend an expired token. Paste a fresh token (Graph API Explorer, same Meta app), Save it, then press Extend straight away",
            );
        }
        if before.expires_at.is_none() {
            return Ok(before);
        }
        let base = graph(self)?;
        let long: Value = agent()
            .get(&format!("{base}/oauth/access_token"))
            .query("grant_type", "fb_exchange_token")
            .query("client_id", &app_id)
            .query("client_secret", &app_secret)
            .query("fb_exchange_token", &token)
            .call()
            .map_err(net)?
            .into_json()
            .map_err(|_| StudioError::Other("Meta's answer was unreadable".into()))?;
        let long = long.get("access_token").and_then(Value::as_str).filter(|t| t.len() > 20).map(str::to_string).ok_or_else(|| {
            StudioError::Other("Meta did not extend this token (a Page token, or one from another Meta app, cannot be exchanged); the old token was kept".into())
        })?;
        // The Page token from a long-lived user token does not expire.
        let page = agent()
            .get(&format!("{base}/me/accounts"))
            .set("Authorization", &format!("Bearer {long}"))
            .query("fields", "access_token,instagram_business_account{id}")
            .query("limit", "100")
            .call()
            .ok()
            .and_then(|r| r.into_json::<Value>().ok())
            .and_then(|v| {
                v.get("data")?.as_array()?.iter().find_map(|p| {
                    (p.pointer("/instagram_business_account/id").and_then(Value::as_str) == Some(a.external_id.as_str()))
                        .then(|| p.get("access_token").and_then(Value::as_str).map(str::to_string))
                        .flatten()
                })
            });
        let chosen = page.unwrap_or(long);
        let after = self.inspect(&chosen)?;
        let longer = match (&after.expires_at, &before.expires_at) {
            (None, _) => true,
            (Some(a), Some(b)) => crate::clock::parse_iso(a) > crate::clock::parse_iso(b),
            (Some(_), None) => false,
        };
        if !after.valid || !longer {
            return err("the extended token was not better than the current one; the old token was kept");
        }
        // It must still reach the Instagram account before it replaces the old one.
        agent()
            .get(&format!("{base}/{}", a.external_id))
            .set("Authorization", &format!("Bearer {chosen}"))
            .query("fields", "id")
            .call()
            .map_err(|e| StudioError::Other(format!("the extended token cannot reach this Instagram account, so the old one was kept: {}", net(e))))?;
        self.set_account_token(account, &chosen)?;
        let c = self.connect()?;
        c.execute(
            "UPDATE social_accounts SET status = 'CONNECTED', last_verified_at = ?1, updated_at = ?1 WHERE id = ?2",
            rusqlite::params![crate::now_iso(), account],
        )?;
        Ok(after)
    }
}

/// The account's Meta token summary, for the panel and MCP (never the token).
pub fn summary(info: &TokenInfo) -> Value {
    json!({"valid": info.valid, "expiresAt": info.expires_at, "kind": info.kind, "text": info.describe()})
}
