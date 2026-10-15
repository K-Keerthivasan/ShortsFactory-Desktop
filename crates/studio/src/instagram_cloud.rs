//! Replace a confirmed empty Meta binary upload through the user's configured cloud publisher.
//! Persist the reservation key before sending bytes; retries follow that reservation only.
use crate::{
    Studio, cloud,
    instagram::{PublishError, Published},
};
use serde_json::{Value, json};
use std::time::Duration;

fn matching_account(accounts: &Value, instagram_id: &str) -> Option<String> {
    let matches: Vec<_> = accounts
        .as_array()?
        .iter()
        .filter(|a| a.get("instagram_user_id").and_then(Value::as_str) == Some(instagram_id))
        .filter_map(|a| a.get("id").and_then(Value::as_str))
        .collect();
    if matches.len() == 1 { matches.first().map(|s| (*s).to_string()) } else { None }
}

fn published(post: &Value) -> Option<(String, Option<String>)> {
    if post.get("status").and_then(Value::as_str) != Some("published") {
        return None;
    }
    let id = post.get("instagram_media_id")?.as_str()?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let link = post
        .get("instagram_permalink")
        .and_then(Value::as_str)
        .filter(|s| s.starts_with("https://www.instagram.com/") || s.starts_with("https://instagram.com/"))
        .map(str::to_string);
    Some((id.to_string(), link))
}

pub(crate) fn publish(studio: &Studio, job: i64, channel: i64, render: i64, instagram_id: &str, caption: &str) -> Published {
    let mut state = studio.remote_state(job)?;
    if state.get("cloudKey").is_none() {
        let accounts = cloud::call(studio, "accounts", json!({}))?;
        let account = matching_account(&accounts, instagram_id).ok_or_else(|| {
            PublishError::Retry("Meta rejected the video transfer. Connect this same Instagram account in Cloud to use the alternate upload route.".into())
        })?;
        state["cloudKey"] = json!(cloud::new_key());
        state["cloudAccount"] = json!(account);
        state["phase"] = json!("CLOUD_UPLOADING");
        studio.checkpoint(job, channel, &state)?;
    }
    let mut response = if let Some(id) = state.get("cloudPostId").and_then(Value::as_str) {
        cloud::call(studio, "getSchedule", json!({"id":id}))?
    } else {
        let key = state.get("cloudKey").and_then(Value::as_str).ok_or_else(|| PublishError::Reconcile("missing cloud reservation key".into()))?;
        let account = state.get("cloudAccount").and_then(Value::as_str).ok_or_else(|| PublishError::Reconcile("missing cloud account".into()))?;
        cloud::schedule_with_key(studio, channel, render, account, caption, &crate::now_iso(), true, key)?
    };
    let id = response
        .pointer("/post/id")
        .and_then(Value::as_str)
        .ok_or_else(|| PublishError::Retry("Cloud has not returned the publication ID; retry resumes the saved reservation.".into()))?
        .to_string();
    state["cloudPostId"] = json!(id);
    state["phase"] = json!("CLOUD_PROCESSING");
    studio.checkpoint(job, channel, &state)?;
    for attempt in 0..120 {
        let post = response.get("post").unwrap_or(&Value::Null);
        if let Some((media, link)) = published(post) {
            state["publishedId"] = json!(media);
            state["phase"] = json!("PUBLISHED");
            studio.checkpoint(job, channel, &state)?;
            return Ok((media, link));
        }
        if matches!(post.get("status").and_then(Value::as_str), Some("failed" | "cancelled" | "expired" | "published")) {
            return Err(PublishError::Reconcile(
                "Cloud publication needs attention; inspect its status in Publish → Cloud. No second publication was created.".into(),
            ));
        }
        if attempt == 119 {
            break;
        }
        std::thread::sleep(Duration::from_secs(5));
        response = cloud::call(studio, "getSchedule", json!({"id":id}))?;
    }
    Err(PublishError::Retry("Cloud is still processing this Reel. Retry checks the same publication; do not create another cloud post.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_matching_never_uses_a_display_name_or_another_workspace() {
        let accounts = json!([{"id":"wrong","instagram_user_id":"22","account_name":"JKR"},{"id":"right","instagram_user_id":"11"}]);
        assert_eq!(matching_account(&accounts, "11").as_deref(), Some("right"));
        assert!(matching_account(&accounts, "33").is_none());
        assert!(matching_account(&json!([{"id":"a","instagram_user_id":"11"},{"id":"b","instagram_user_id":"11"}]), "11").is_none());
    }
    #[test]
    fn only_confirmed_media_is_reported_as_published() {
        assert!(published(&json!({"status":"processing","instagram_media_id":"123"})).is_none());
        assert!(published(&json!({"status":"published"})).is_none());
        assert!(published(&json!({"status":"published","instagram_media_id":""})).is_none());
        assert_eq!(
            published(&json!({"status":"published","instagram_media_id":"123","instagram_permalink":"https://www.instagram.com/reel/example/"})),
            Some(("123".into(), Some("https://www.instagram.com/reel/example/".into())))
        );
    }
}
