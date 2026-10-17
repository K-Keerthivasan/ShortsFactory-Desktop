//! Durable publication updates. Acknowledged results stay acknowledged across app restarts.
use crate::{Result, Studio, cloud};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock, PoisonError};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub message: String,
    pub link: Option<String>,
    pub ok: bool,
    pub at: String,
    pub read: bool,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    observed: BTreeMap<String, String>,
    events: Vec<Notification>,
}
pub struct Updates {
    pub notifications: Vec<Notification>,
    pub cloud_posts: Vec<Value>,
    pub cloud_error: bool,
}
struct Observation {
    key: String,
    status: String,
    title: String,
    at: String,
    media: Option<String>,
    link: Option<String>,
    cloud: bool,
    /// A YouTube upload waiting for its publish time (UTC ISO).
    youtube_at: Option<String>,
}
fn guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(PoisonError::into_inner)
}
fn path(studio: &Studio) -> std::path::PathBuf {
    studio.root.join("Editor").join("publishing-updates.json")
}
fn load(studio: &Studio) -> Result<State> {
    match std::fs::read(path(studio)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|_| crate::StudioError::Other("Publication update history is unreadable; it was preserved.".into()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e.into()),
    }
}
fn save(studio: &Studio, state: &State) -> Result<()> {
    let p = path(studio);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(state).map_err(|e| crate::StudioError::Other(e.to_string()))?;
    let temp = p.with_extension("json.tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(temp, p)?;
    Ok(())
}
fn observe(state: &mut State, row: Observation) {
    let old = state.observed.insert(row.key.clone(), row.status.clone());
    let status = row.status.to_ascii_lowercase();
    let ok = status == "published";
    let uploaded = row.cloud && status == "scheduled" && (old.as_deref() == Some("uploading") || old.is_none());
    let failed = matches!(status.as_str(), "failed" | "publish_failed" | "needs_reconciliation");
    if (!ok && !failed && !uploaded) || old.as_deref() == Some(row.status.as_str()) {
        return;
    }
    if old.is_none() && crate::clock::parse_iso(&row.at).is_none_or(|t| t < crate::clock::now_ms() - 86_400_000) {
        return;
    }
    let id = if uploaded {
        format!("{}:uploaded", row.key)
    } else if ok {
        row.media.as_ref().map(|m| format!("published:{m}")).unwrap_or_else(|| format!("{}:published", row.key))
    } else {
        format!("{}:{}", row.key, row.at)
    };
    if state.events.iter().any(|e| e.id == id) {
        return;
    }
    state.events.push(Notification {
        id,
        title: row.title.chars().take(200).collect(),
        message: if uploaded {
            "Uploaded to cloud and scheduled".to_string()
        } else if ok && let Some(at) = &row.youtube_at {
            format!("Uploaded to YouTube; YouTube publishes it at {}", crate::clock::local_time(at))
        } else if ok {
            if row.cloud { "Published by the cloud worker" } else { "Published successfully" }.to_string()
        } else {
            "Publishing failed or needs attention. Check the Calendar for details.".to_string()
        },
        link: row.link.filter(|s| {
            s.starts_with("https://www.instagram.com/")
                || s.starts_with("https://instagram.com/")
                || s.starts_with("https://www.youtube.com/")
                || s.starts_with("https://youtu.be/")
        }),
        ok: ok || uploaded,
        at: row.at,
        read: false,
    });
    if state.events.len() > 200 {
        state.events.drain(..state.events.len() - 200);
    }
}

pub fn refresh(studio: &Studio) -> Result<Updates> {
    let c = studio.connect()?;
    let mut statement=c.prepare("SELECT p.id,p.status,s.canonical_story_title,p.updated_at,p.remote_id,p.remote_url,p.platform,p.scheduled_at FROM publishing_jobs p JOIN renders r ON r.id=p.render_id JOIN story_clusters s ON s.id=r.story_id ORDER BY p.id DESC LIMIT 500")?;
    let mut observations: Vec<Observation> = statement
        .query_map([], |r| {
            Ok(Observation {
                key: format!("local:{}", r.get::<_, i64>(0)?),
                status: r.get(1)?,
                title: r.get(2)?,
                at: r.get(3)?,
                media: r.get(4)?,
                link: r.get(5)?,
                cloud: false,
                // Uploaded to YouTube with a later publish time: YouTube publishes it then.
                youtube_at: {
                    let platform: String = r.get(6)?;
                    let at: Option<String> = r.get(7)?;
                    at.filter(|a| platform == "YOUTUBE" && crate::clock::parse_iso(a).is_some_and(|t| t > crate::clock::now_ms()))
                },
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    let mut posts = Vec::new();
    let mut cloud_error = false;
    if cloud::configured(studio) {
        for page in 0..5 {
            match cloud::call(studio, "listSchedules", json!({"offset":page*100})) {
                Ok(value) => {
                    let items = value.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                    let count = items.len();
                    for item in items {
                        if let Some(post) = item.get("post") {
                            posts.push(post.clone());
                        }
                    }
                    if count < 100 {
                        break;
                    }
                }
                Err(_) => {
                    cloud_error = true;
                    break;
                }
            }
        }
    }
    for post in &posts {
        let text = |key: &str| post.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        if text("id").is_empty() {
            continue;
        }
        observations.push(Observation {
            key: format!("cloud:{}", text("id")),
            status: text("status"),
            title: text("title"),
            at: text("updated_at"),
            media: post.get("instagram_media_id").and_then(Value::as_str).map(str::to_string),
            link: post.get("instagram_permalink").and_then(Value::as_str).map(str::to_string),
            cloud: true,
            youtube_at: None,
        });
    }
    let _lock = guard();
    let mut state = load(studio)?;
    for row in observations {
        observe(&mut state, row);
    }
    // Bound the cache without dropping the durable notification history.
    while state.observed.len() > 1000 {
        if let Some(key) = state.observed.keys().next().cloned() {
            state.observed.remove(&key);
        } else {
            break;
        }
    }
    save(studio, &state)?;
    let notifications = state.events.into_iter().rev().collect();
    Ok(Updates { notifications, cloud_posts: posts, cloud_error })
}
/// Removes updates from the history (all of them when `ids` is None). What was already seen
/// stays recorded, so a cleared update does not come back on the next refresh.
pub fn clear(studio: &Studio, ids: Option<&[String]>) -> Result<()> {
    let _lock = guard();
    let mut state = load(studio)?;
    match ids {
        Some(ids) => state.events.retain(|e| !ids.contains(&e.id)),
        None => state.events.clear(),
    }
    save(studio, &state)
}
pub fn acknowledge(studio: &Studio, ids: &[String]) -> Result<()> {
    let _lock = guard();
    let mut state = load(studio)?;
    for event in &mut state.events {
        if ids.contains(&event.id) {
            event.read = true;
        }
    }
    save(studio, &state)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acknowledgements_survive_disk_reload() {
        let root = std::env::temp_dir().join(format!("filmcraft-notification-test-{}-{}", std::process::id(), crate::clock::now_ms()));
        let studio = Studio {
            database: root.join("unused.db"),
            storage: root.join("storage"),
            secrets: root.join("secrets"),
            env: Default::default(),
            root: root.clone(),
        };
        let mut state = State::default();
        observe(&mut state, row("local:1", "PUBLISHED"));
        save(&studio, &state).unwrap();
        let reopened = load(&studio).unwrap();
        assert!(!reopened.events[0].read);
        acknowledge(&studio, &[reopened.events[0].id.clone()]).unwrap();
        assert!(load(&studio).unwrap().events[0].read);
        std::fs::remove_file(path(&studio)).unwrap();
        std::fs::remove_dir(root.join("Editor")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn cleared_updates_do_not_come_back() {
        let root = std::env::temp_dir().join(format!("filmcraft-notification-clear-{}-{}", std::process::id(), crate::clock::now_ms()));
        let studio = Studio {
            database: root.join("unused.db"),
            storage: root.join("storage"),
            secrets: root.join("secrets"),
            env: Default::default(),
            root: root.clone(),
        };
        let mut state = State::default();
        observe(&mut state, row("local:1", "PUBLISHED"));
        observe(&mut state, row("local:2", "PUBLISHED"));
        save(&studio, &state).unwrap();
        let before = load(&studio).unwrap().events;
        assert!(!before.is_empty());
        clear(&studio, Some(std::slice::from_ref(&before[0].id))).unwrap();
        assert_eq!(load(&studio).unwrap().events.len(), before.len() - 1);
        clear(&studio, None).unwrap();
        let mut reopened = load(&studio).unwrap();
        assert!(reopened.events.is_empty());
        // The same publications observed again (the next refresh) report nothing new.
        observe(&mut reopened, row("local:1", "PUBLISHED"));
        observe(&mut reopened, row("local:2", "PUBLISHED"));
        assert!(reopened.events.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }
    fn row(key: &str, status: &str) -> Observation {
        Observation {
            key: key.into(),
            status: status.into(),
            title: "Reel".into(),
            at: crate::now_iso(),
            media: Some("123".into()),
            link: None,
            cloud: key.starts_with("cloud"),
            youtube_at: None,
        }
    }
    #[test]
    fn closed_app_completion_is_reported_once_and_local_cloud_results_are_deduplicated() {
        let mut state = State::default();
        observe(&mut state, row("cloud:1", "processing"));
        let bytes = serde_json::to_vec(&state).unwrap();
        let mut reopened: State = serde_json::from_slice(&bytes).unwrap();
        observe(&mut reopened, row("cloud:1", "published"));
        assert_eq!(reopened.events.len(), 1);
        reopened.events[0].read = true;
        observe(&mut reopened, row("local:3", "PUBLISHED"));
        observe(&mut reopened, row("cloud:1", "published"));
        assert_eq!(reopened.events.len(), 1);
        assert!(reopened.events[0].read);
    }
    #[test]
    fn a_youtube_upload_waiting_for_its_time_says_so() {
        let mut state = State::default();
        let mut r = row("local:9", "PUBLISHED");
        r.youtube_at = Some(crate::clock::iso(crate::clock::now_ms() + 3_600_000));
        observe(&mut state, r);
        assert!(state.events[0].message.starts_with("Uploaded to YouTube; YouTube publishes it at"), "{}", state.events[0].message);
    }
    #[test]
    fn processing_is_not_reported_as_success_and_failure_is_visible() {
        let mut state = State::default();
        observe(&mut state, row("cloud:1", "processing"));
        assert!(state.events.is_empty());
        observe(&mut state, row("cloud:1", "failed"));
        assert_eq!(state.events.len(), 1);
        assert!(!state.events[0].ok);
    }
    #[test]
    fn verified_cloud_upload_and_publication_are_distinct_updates() {
        let mut state = State::default();
        observe(&mut state, row("cloud:2", "uploading"));
        assert!(state.events.is_empty());
        observe(&mut state, row("cloud:2", "scheduled"));
        assert_eq!(state.events.len(), 1);
        assert!(state.events[0].message.contains("Uploaded"));
        observe(&mut state, row("cloud:2", "published"));
        assert_eq!(state.events.len(), 2);
        assert!(state.events[1].message.contains("Published"));
    }
}
