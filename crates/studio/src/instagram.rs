//! Instagram Reels through Meta's Graph API content publishing flow: a resumable REELS container,
//! the binary upload to `rupload.facebook.com`, waiting for processing, then `media_publish`.
//! Every step is checkpointed in the publishing job, so an interrupted upload resumes and a
//! publication whose outcome is unknown is never repeated blindly.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use crate::{Result, Studio, StudioError, err};

/// Why a publication stopped: a retry may work, or the remote state must be checked by a person.
#[derive(Debug)]
pub enum PublishError {
    Retry(String),
    Reconcile(String),
}
impl From<StudioError> for PublishError {
    fn from(e: StudioError) -> Self {
        PublishError::Retry(e.to_string())
    }
}
pub type Published = std::result::Result<(String, Option<String>), PublishError>;

/// Declare the binary media type and exact remaining length for both fresh and resumed uploads.
fn upload_request(agent: &ureq::Agent, url: &str, token: &str, size: u64, offset: u64, remaining: usize) -> ureq::Request {
    agent
        .post(url)
        .set("Authorization", &format!("OAuth {token}"))
        .set("Content-Type", "application/octet-stream")
        .set("Content-Length", &remaining.to_string())
        .set("offset", &offset.to_string())
        .set("file_size", &size.to_string())
}

fn graph(studio: &Studio) -> Result<String> {
    let v = studio.var("META_GRAPH_VERSION").unwrap_or("v26.0");
    let ok = v.strip_prefix('v').is_some_and(|r| r.split('.').count() == 2 && r.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())));
    if !ok {
        return err("invalid META_GRAPH_VERSION");
    }
    Ok(v.to_string())
}

struct Meta {
    version: String,
    account: String,
    token: String,
    agent: ureq::Agent,
}

impl Meta {
    fn request(&self, endpoint: &str, params: &[(&str, &str)], post: bool) -> std::result::Result<Value, PublishError> {
        let url = format!("https://graph.facebook.com/{}/{endpoint}", self.version);
        let auth = format!("Bearer {}", self.token);
        let r = if post {
            self.agent.post(&url).set("Authorization", &auth).send_form(params)
        } else {
            let mut q = self.agent.get(&url).set("Authorization", &auth);
            for (k, v) in params {
                q = q.query(k, v);
            }
            q.call()
        };
        match r {
            Ok(r) => r.into_json().map_err(|e| PublishError::Retry(format!("Meta reply unreadable: {e}"))),
            Err(ureq::Error::Status(code, r)) => {
                let v: Value = r.into_json().unwrap_or(Value::Null);
                let e = v.get("error");
                Err(PublishError::Retry(format!(
                    "Meta HTTP {code}; API code {}, subcode {}. Check account permissions and token validity.",
                    e.and_then(|e| e.get("code")).map_or("unknown".into(), Value::to_string),
                    e.and_then(|e| e.get("error_subcode")).map_or("none".into(), Value::to_string)
                )))
            }
            Err(e) => Err(PublishError::Retry(format!("Meta connection failed: {e}"))),
        }
    }
    fn status(&self, id: &str) -> std::result::Result<Value, PublishError> {
        self.request(id, &[("fields", "id,status_code,video_status")], false)
    }
    fn upload(&self, url: &str, file: &Path, offset: u64) -> std::result::Result<(), PublishError> {
        if !url.starts_with("https://rupload.facebook.com/") {
            return Err(PublishError::Retry("unexpected Meta upload host".into()));
        }
        let data = std::fs::read(file).map_err(|e| PublishError::Retry(format!("cannot read the video: {e}")))?;
        let size = data.len() as u64;
        if size == 0 || offset > size {
            return Err(PublishError::Retry("the video is empty or the upload offset is beyond its end".into()));
        }
        if offset == size {
            return Ok(());
        }
        let body = data.get(offset as usize..).unwrap_or_default();
        let r = upload_request(&self.agent, url, &self.token, size, offset, body.len()).send_bytes(body);
        let (code, text) = match r {
            Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
            Err(ureq::Error::Status(c, r)) => (c, r.into_string().unwrap_or_default()),
            Err(_) => return Err(PublishError::Retry("Meta upload connection failed; no automatic second upload was attempted".into())),
        };
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !(200..300).contains(&code) || v.get("success") != Some(&Value::Bool(true)) {
            let detail: String = text.trim().chars().take(240).collect();
            return Err(PublishError::Retry(format!("Meta binary upload failed: HTTP {code} · {detail}")));
        }
        Ok(())
    }
}

/// Only an explicitly failed, empty upload is safe to replace with another transport.
fn empty_failed_upload(status: &Value) -> bool {
    status.get("status_code").and_then(Value::as_str) != Some("PUBLISHED")
        && status.pointer("/video_status/uploading_phase/status").and_then(Value::as_str).is_some_and(|s| s.eq_ignore_ascii_case("error"))
        && status.pointer("/video_status/uploading_phase/bytes_transferred").and_then(Value::as_u64) == Some(0)
        && status
            .pointer("/video_status/uploading_phase/errors")
            .and_then(Value::as_array)
            .is_some_and(|errors| errors.iter().any(|e| e.get("code").and_then(Value::as_u64) == Some(1363008)))
}

pub(crate) fn can_replace_upload(studio: &Studio, job: i64, account: &str, token: &str) -> std::result::Result<bool, PublishError> {
    let state = studio.remote_state(job)?;
    if state.get("phase").and_then(Value::as_str) != Some("UPLOADING") || state.get("publishedId").is_some() {
        return Ok(false);
    }
    let Some(remote) = state.get("remoteId").and_then(Value::as_str) else { return Ok(false) };
    let meta = Meta {
        version: graph(studio)?,
        account: account.into(),
        token: token.into(),
        agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).redirects(0).build(),
    };
    Ok(empty_failed_upload(&meta.status(remote)?))
}

/// Publishes `file` as a Reel on the Instagram account, resuming from `job`'s checkpoints.
pub fn publish(studio: &Studio, job: i64, channel: i64, account_id: &str, token: &str, file: &Path, caption: &str) -> Published {
    let meta = Meta {
        version: graph(studio)?,
        account: account_id.to_string(),
        token: token.to_string(),
        agent: ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(30)).timeout(Duration::from_secs(600)).redirects(0).build(),
    };
    let mut state = studio.remote_state(job)?;
    let phase = |s: &Value| s.get("phase").and_then(Value::as_str).unwrap_or_default().to_string();
    let checkpoint = |s: &Value| studio.checkpoint(job, channel, s);
    if let Some(id) = state.get("publishedId").and_then(Value::as_str) {
        return Ok((id.to_string(), None));
    }
    let mut status: Option<Value> = None;
    if let Some(remote) = state.get("remoteId").and_then(Value::as_str).map(str::to_string) {
        let s = meta.status(&remote)?;
        if s.get("status_code").and_then(Value::as_str) == Some("PUBLISHED") {
            return Err(PublishError::Reconcile(
                "the Instagram container is already published; reconcile its media id, no second publication is attempted".into(),
            ));
        }
        if phase(&state) == "PUBLISHING" {
            return Err(PublishError::Reconcile("the Instagram publish response was interrupted; check the account before retrying".into()));
        }
        if s.pointer("/video_status/uploading_phase/status").and_then(Value::as_str).is_some_and(|x| x.eq_ignore_ascii_case("error")) {
            checkpoint(&json!({"phase": "RESTARTING_FAILED_UPLOAD"}))?;
            state = json!({});
        } else {
            status = Some(s);
        }
    }
    if state.get("remoteId").is_none() {
        if phase(&state) == "INITIALIZING" {
            return Err(PublishError::Reconcile("the container creation outcome is unknown; check Instagram before retrying".into()));
        }
        let mut s = state.clone();
        s["phase"] = json!("INITIALIZING");
        checkpoint(&s)?;
        let created = meta.request(&format!("{}/media", meta.account), &[("media_type", "REELS"), ("upload_type", "resumable"), ("caption", caption)], true)?;
        let id = created.get("id").and_then(Value::as_str).ok_or_else(|| PublishError::Retry("Meta did not return a container id".into()))?.to_string();
        if !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(PublishError::Retry("unexpected Instagram container id".into()));
        }
        let uri = created
            .get("uri")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("https://rupload.facebook.com/ig-api-upload/{}/{id}", meta.version));
        state = json!({"remoteId": id, "uploadUrl": uri, "phase": "CREATED"});
        checkpoint(&state)?;
        status = None;
    }
    let remote = state.get("remoteId").and_then(Value::as_str).unwrap_or_default().to_string();
    let p = phase(&state);
    if p == "CREATED" || p == "UPLOADING" {
        let s = match status.take() {
            Some(s) => s,
            None => meta.status(&remote)?,
        };
        let offset = s.pointer("/video_status/uploading_phase/bytes_transferred").and_then(Value::as_u64).unwrap_or(0);
        state["phase"] = json!("UPLOADING");
        state["offset"] = json!(offset);
        checkpoint(&state)?;
        let url = state.get("uploadUrl").and_then(Value::as_str).unwrap_or_default().to_string();
        meta.upload(&url, file, offset)?;
        state["phase"] = json!("UPLOADED");
        checkpoint(&state)?;
    }
    for i in 0..60 {
        let s = meta.status(&remote)?;
        let code = s.get("status_code").and_then(Value::as_str).unwrap_or_default();
        if code == "FINISHED" {
            break;
        }
        if s.pointer("/video_status/uploading_phase/status").and_then(Value::as_str).is_some_and(|x| x.eq_ignore_ascii_case("error")) {
            return Err(PublishError::Retry("Instagram upload processing failed; a retry creates a fresh container".into()));
        }
        if code == "ERROR" || code == "EXPIRED" {
            return Err(PublishError::Reconcile(format!("Instagram container {code}; inspect the processing result")));
        }
        if i == 59 {
            return Err(PublishError::Retry("Instagram processing timed out; a retry resumes the existing container".into()));
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    state["phase"] = json!("PUBLISHING");
    checkpoint(&state)?;
    let published = meta.request(&format!("{}/media_publish", meta.account), &[("creation_id", &remote)], true)?;
    let id =
        published.get("id").and_then(Value::as_str).ok_or_else(|| PublishError::Reconcile("Meta did not return the published media id".into()))?.to_string();
    state["phase"] = json!("PUBLISHED");
    state["publishedId"] = json!(id);
    checkpoint(&state)?;
    let link = meta.request(&id, &[("fields", "permalink")], false).ok().and_then(|v| v.get("permalink").and_then(Value::as_str).map(str::to_string));
    Ok((id, link))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn cloud_replacement_requires_explicit_empty_failed_upload() {
        let failed =
            json!({"status_code":"IN_PROGRESS","video_status":{"uploading_phase":{"status":"error","bytes_transferred":0,"errors":[{"code":1363008}]}}});
        assert!(empty_failed_upload(&failed));
        for status in [json!({}), json!({"status_code":"PUBLISHED"}), json!({"video_status":{"uploading_phase":{"status":"error"}}})] {
            assert!(!empty_failed_upload(&status));
        }
        for (pointer, value) in [
            ("/status_code", json!("PUBLISHED")),
            ("/video_status/uploading_phase/bytes_transferred", json!(1)),
            ("/video_status/uploading_phase/status", json!("in_progress")),
        ] {
            let mut other = failed.clone();
            *other.pointer_mut(pointer).unwrap() = value;
            assert!(!empty_failed_upload(&other));
        }
    }

    #[test]
    fn binary_upload_wire_format_has_exact_body_type_length_and_resume_headers() {
        let bytes = b"\0raw-video\xff&not=form%20data\r\n";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/upload", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for offset in [0usize, 7] {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") && headers.len() < 8192 {
                    let mut byte = [0u8];
                    stream.read_exact(&mut byte).unwrap();
                    headers.push(byte[0]);
                }
                let text = String::from_utf8(headers).unwrap().to_ascii_lowercase();
                assert!(text.starts_with("post /upload http/1.1\r\n"));
                assert!(text.contains("content-type: application/octet-stream\r\n"));
                assert!(text.contains(&format!("content-length: {}\r\n", bytes.len() - offset)));
                assert!(text.contains(&format!("file_size: {}\r\n", bytes.len())));
                assert!(text.contains(&format!("offset: {offset}\r\n")));
                assert!(text.contains("authorization: oauth test-only\r\n"));
                assert!(!text.contains("transfer-encoding:") && !text.contains("file_url:"));
                let mut body = vec![0u8; bytes.len() - offset];
                stream.read_exact(&mut body).unwrap();
                assert_eq!(&body, &bytes[offset..]);
                let reply = br#"{"success":true}"#;
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", reply.len()).unwrap();
                stream.write_all(reply).unwrap();
            }
        });
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).redirects(0).build();
        for offset in [0usize, 7] {
            let response =
                upload_request(&agent, &url, "test-only", bytes.len() as u64, offset as u64, bytes.len() - offset).send_bytes(&bytes[offset..]).unwrap();
            assert_eq!(response.into_json::<Value>().unwrap()["success"], true);
        }
        worker.join().unwrap();
    }
}
