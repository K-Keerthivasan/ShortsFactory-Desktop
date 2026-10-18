//! Reel Studio panel: everything that turns a sequence into a channel's Reel and gets it out.
//!
//! - **Workspace**: the library's channels (JKR, K2 Digital Media…); their brand, media, articles
//!   and publishing accounts.
//! - **New Reel** from a workspace clip, built with the channel's default template.
//! - **Templates**: the 21 Reel templates; a click rebuilds the Reel in that template (`reel.apply`).
//! - **Card**: headline, caption, attribution, badge, points, call to action, Instagram caption
//!   and hashtags (`reel.setCard`), and **Write with AI** from notes or an article.
//! - **Deliver**: export (Social Vertical 1080×1920), send to the library as the post's render,
//!   approve, publish now or schedule on the workspace's accounts.
//! - **Library**: the workspace's recent posts, their renders and publications.
//!
//! Editing goes through `reel.*` engine commands, so agents drive the same path. The library,
//! AI and platforms are `filmcraft_studio`; anything slow runs on a worker thread and reports
//! back to the activity log. Keys are read only by the publishers and never shown.
//!
//! Automation ids: `reel.channel`, `reel.template.<id>`, `reel.card.<field>`, `reel.write`,
//! `reel.new.<media id>`, `reel.export`, `reel.publish.<render>`, `reel.approve.<render>`.

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Mutex, OnceLock, PoisonError};

use egui::{Rect, RichText, vec2};
use filmcraft_reel::{Brand, Card, PostLink, ReelInfo, TEMPLATES};
use filmcraft_studio::{Account, Article, Channel, LibraryPost, MediaRow, Studio};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::FilmcraftApp;

#[path = "reel_ai.rs"]
mod ai;
#[path = "reel_delivery.rs"]
mod delivery;
#[path = "reel_gaps.rs"]
mod gaps;
#[path = "reel_schedule.rs"]
mod schedule;
pub(crate) fn guard_render(app: &mut FilmcraftApp, command: &str, params: &serde_json::Value) -> bool {
    gaps::command_guard(app, command, params)
}
pub use ai::{AiDraft, show as show_ai};

/// What the panel is editing (UI state; serde so agents can read it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReelDraft {
    pub channel_id: Option<i64>,
    pub card: Card,
    /// What the clip shows, for the AI.
    pub notes: String,
    pub article: Option<i64>,
    pub accounts: Vec<i64>,
    /// Local `YYYY-MM-DD HH:MM`.
    pub schedule_at: String,
    pub approve_on_send: bool,
    pub media_filter: String,
    pub library_filter: String,
    pub library_selected: Option<(i64, i64)>,
    pub library_preview_time: f64,
    pub page: ReelPage,
    pub workspace_name: String,
    pub workspace_slug: String,
    pub brand_edit: Option<Brand>,
    pub rename_story: Option<i64>,
    pub rename_title: String,
    pub show_archived: bool,
    /// Video links for yt-dlp, one per line.
    pub links: String,
    pub link_notes: String,
    pub draft_downloads: bool,
    /// yt-dlp download options.
    pub download: filmcraft_studio::ytdlp::Options,
    /// The cloud Instagram account (Supabase id) schedules go to.
    pub cloud_account: Option<String>,
    pub use_cloud: bool,
    pub instagram_guides: bool,
    /// The card was loaded from this project's Reel info (so a project switch reloads it).
    pub loaded_for: Option<String>,
    /// The AI panel's links and choices.
    pub ai: AiDraft,
    /// Make a Reel (9:16) or a Post (4:5).
    pub format: Format,
    pub design_tab: DesignTab,
    pub publish_tab: PublishTab,
    pub export_tab: ExportTab,
    pub plugin_open: bool,
    pub plugin_tab: PluginTab,
    /// The "this clip already has a Reel" question that is open.
    #[serde(skip)]
    pub reuse: Option<ReusePrompt>,
    pub calendar_month: Option<(i32, u32)>,
    pub calendar_all: bool,
    /// The day picked on the publishing calendar (local `YYYY-MM-DD`).
    pub calendar_day: Option<String>,
    /// The time a Reel is scheduled at on that day (local `HH:MM`).
    pub schedule_time: String,
    /// Library as a grid of thumbnails instead of a list.
    pub library_grid: bool,
    /// Create's workspace clips as a grid of thumbnails.
    pub clips_grid: bool,
    /// Hide workspace clips marked done.
    pub hide_done_clips: bool,
    /// Finished cloud posts cleared from the calendar (ids; hidden here only, nothing is deleted).
    pub cleared_posts: Vec<String>,
    /// Show cleared posts again.
    pub show_cleared: bool,
    /// Plugin Settings fields being edited (never saved in the workspace state).
    #[serde(skip)]
    pub env_drafts: std::collections::BTreeMap<String, String>,
    #[serde(skip)]
    pub token_drafts: std::collections::BTreeMap<i64, String>,
}

/// What to make.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    #[default]
    Reel,
    Post,
}
impl Format {
    pub fn id(self) -> &'static str {
        match self {
            Format::Reel => "reel",
            Format::Post => "post",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::Reel => "Reel",
            Format::Post => "Post",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesignTab {
    #[default]
    Templates,
    Card,
    Ai,
    Caption,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublishTab {
    #[default]
    Now,
    Queue,
    Calendar,
    Cloud,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportTab {
    #[default]
    File,
    Library,
    Publish,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginTab {
    #[default]
    Brands,
    Accounts,
    Ai,
    Cloud,
    Downloads,
}

/// A clip that already has Reels: the question shown before making another.
#[derive(Clone, Debug, PartialEq)]
pub struct ReusePrompt {
    pub name: String,
    pub path: Option<std::path::PathBuf>,
    pub item: Option<u64>,
    pub reels: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardPart {
    Card,
    Caption,
}

/// What happens to a queued Reel once it is rendered.
#[derive(Clone, Debug, PartialEq)]
enum QueueAction {
    LibraryOnly,
    PublishNow(Vec<i64>),
    /// Accounts, UTC ISO time.
    Schedule(Vec<i64>, String),
    /// Book it in the cloud: the cloud account, UTC ISO time, publish at once, and YouTube
    /// accounts that take the same time through YouTube's own scheduling.
    Cloud(String, String, bool, Vec<i64>),
}

struct QueueItem {
    range: Option<(i64, i64)>,
    id: u64,
    sequence: u64,
    name: String,
    info: ReelInfo,
    footage: Option<(std::path::PathBuf, f64, f64)>,
    duration: f64,
    project: std::sync::Arc<filmcraft_project::Project>,
    action: QueueAction,
    status: String,
    render: Option<i64>,
    error: Option<String>,
}

struct Notice {
    text: String,
    ok: bool,
    shown: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReelPage {
    #[default]
    Create,
    Design,
    Publish,
    Library,
    Settings,
}

enum Done {
    CloudQueued(u64, Result<bool, String>, bool),
    Updates(Result<filmcraft_studio::notifications::Updates, String>),
    Preview(std::path::PathBuf, Result<Box<delivery::Preview>, String>),
    CloudAccounts(Result<Vec<serde_json::Value>, String>),
    CloudPosts(Result<Vec<serde_json::Value>, String>),
    Card(Result<Box<Card>, String>, Box<CardTarget>),
    /// The library hand-off finished, for this sequence (a Reel).
    Sent(Result<(PostLink, i64), String>, u64, std::sync::Arc<filmcraft_project::Project>, Option<u64>),
    Message(String),
    Health(filmcraft_studio::settings::HealthReport),
    /// A publication finished: the Reel's name, the link or the error, the queue item.
    Published(String, Result<Option<String>, String>, Option<u64>),
    Notice((String, bool)),
    /// A queued Reel's schedule: its name, what was scheduled where, the time (UTC ISO), the queue item.
    Scheduled(String, Result<filmcraft_studio::Scheduled, String>, String, u64),
    /// A local account's token expiry (text) or why it could not be read; whether to pop it up.
    Token(i64, Result<String, String>, bool),
}

struct CardTarget {
    project: std::sync::Arc<filmcraft_project::Project>,
    sequence: Option<filmcraft_project::ItemId>,
    channel: Option<i64>,
    card: Card,
}

impl CardTarget {
    fn capture(app: &FilmcraftApp) -> Self {
        Self {
            project: app.session.project.clone(),
            sequence: app.session.state.active_sequence,
            channel: app.ui.reel.channel_id,
            card: app.ui.reel.card.clone(),
        }
    }

    fn matches(&self, app: &FilmcraftApp) -> bool {
        std::sync::Arc::ptr_eq(&self.project, &app.session.project)
            && self.sequence == app.session.state.active_sequence
            && self.channel == app.ui.reel.channel_id
            && self.card == app.ui.reel.card
    }
}

struct PendingExport {
    job: u64,
    path: std::path::PathBuf,
    approve: bool,
    info: ReelInfo,
    footage: Option<(std::path::PathBuf, f64, f64)>,
    duration: f64,
    /// The queue item this render belongs to.
    queue: Option<u64>,
    /// The Reel (sequence) rendered.
    sequence: u64,
    project: std::sync::Arc<filmcraft_project::Project>,
    /// When the job first read as finished without a result: the exporter sets `finished` a
    /// moment before it stores the result, so that is waited out before calling it stopped.
    finished_at: Option<f64>,
}

/// Library data and background work. Not UI state: it is rebuilt from the library on demand.
#[derive(Default)]
struct Runtime {
    gap_prompt: Option<gaps::Prompt>,
    clip_preview: Option<(std::path::PathBuf, String, f64)>,
    last_updates: Option<f64>,
    publication_updates: Vec<filmcraft_studio::notifications::Notification>,
    updates_open: bool,
    previews: std::collections::HashMap<std::path::PathBuf, Result<Option<std::sync::Arc<delivery::Preview>>, String>>,
    studio: Option<Result<Studio, String>>,
    channels: Vec<Channel>,
    brand: Option<Brand>,
    media: Vec<MediaRow>,
    articles: Vec<Article>,
    accounts: Vec<Account>,
    library: Vec<LibraryPost>,
    archived_posts: Vec<LibraryPost>,
    loaded_channel: Option<i64>,
    tasks: Vec<(String, Receiver<Done>)>,
    export: Option<PendingExport>,
    log: Vec<String>,
    last_due_check: f64,
    due_running: bool,
    /// MediaDock imports of this session, updated by their worker.
    imports: std::sync::Arc<Mutex<Vec<filmcraft_studio::ytdlp::Import>>>,
    imports_running: bool,
    cloud_accounts: Vec<serde_json::Value>,
    cloud_posts: Vec<serde_json::Value>,
    cloud_error: String,
    cloud_loaded: bool,
    archived: Option<(i64, i64)>,
    health: Option<filmcraft_studio::settings::HealthReport>,
    /// When the start-up popup opened.
    health_popup: Option<f64>,
    startup_checked: bool,
    notices: Vec<Notice>,
    queue: Vec<QueueItem>,
    /// A workspace clip whose New Reel button was pressed.
    pending_start: Option<(String, std::path::PathBuf)>,
    /// The AI panel's links, from download to the parts chosen.
    ai_runs: ai::Runs,
    /// Stops a YouTube sign-in that is waiting for the browser.
    youtube_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// When the YouTube queue last ran (seconds of app time).
    last_youtube_queue: Option<f64>,
    /// When a card field was last typed in (applied half a second later).
    card_typed_at: Option<f64>,
    /// Local accounts' token expiry, as last read from Meta.
    token_info: std::collections::BTreeMap<i64, Result<String, String>>,
}

/// Whether this process may act on the library by itself: the start-up connection check and
/// running due scheduled posts. Only the Shorts Factory app turns it on ([`enable_background`]),
/// so tests and other embedders never call Meta or publish from the user's real library.
static BACKGROUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Unread publishing updates (posted, scheduled, failed), for the header's bell.
pub fn unread_updates() -> usize {
    rt().publication_updates.iter().filter(|n| !n.read).count()
}

/// Opens the publishing updates window (the header's bell).
pub fn open_updates() {
    rt().updates_open = true;
}

/// Lets this process run the start-up check and due scheduled posts. Call once from the app.
pub fn enable_background() {
    BACKGROUND.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn background() -> bool {
    BACKGROUND.load(std::sync::atomic::Ordering::Relaxed)
}

fn rt() -> std::sync::MutexGuard<'static, Runtime> {
    static RT: OnceLock<Mutex<Runtime>> = OnceLock::new();
    RT.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner)
}

fn studio(r: &mut Runtime) -> Option<Studio> {
    if r.studio.is_none() {
        r.studio = Some(Studio::open_default().map_err(|e| e.to_string()));
    }
    r.studio.as_ref().and_then(|s| s.as_ref().ok()).cloned()
}

fn log(r: &mut Runtime, msg: impl Into<String>) {
    let msg = msg.into();
    log::info!("reel studio: {msg}");
    r.log.insert(0, msg);
    r.log.truncate(40);
}

/// Runs `work` on a worker thread; its result arrives in [`poll`].
fn spawn(r: &mut Runtime, label: &str, work: impl FnOnce() -> Done + Send + 'static) {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let out =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|_| Done::Message("the background task stopped unexpectedly".into()));
        let _ = tx.send(out);
    });
    r.tasks.push((label.to_string(), rx));
}

fn load_channel(r: &mut Runtime, channel: i64) {
    if r.loaded_channel != Some(channel) {
        r.previews.clear();
    }
    let Some(s) = studio(r) else { return };
    let result = (|| -> filmcraft_studio::Result<()> {
        if let Ok(folder) = s.workspace_folder(channel) {
            filmcraft_text::fonts::add_source(&filmcraft_text::fonts::DirectorySource { dirs: vec![folder.join("assets/brand/fonts")] });
        }
        r.brand = Some(s.brand(channel)?);
        r.media = s.media(channel)?;
        r.articles = s.articles(channel, 60)?;
        r.accounts = s.accounts(channel)?;
        r.library = s.library(channel, 40)?;
        r.archived_posts = s.archived_library(channel, 40)?;
        Ok(())
    })();
    if let Err(e) = result {
        log(r, format!("Could not read the workspace: {e}"));
    }
    r.loaded_channel = Some(channel);
}

fn reel_info(app: &FilmcraftApp) -> Option<ReelInfo> {
    filmcraft_engine::reel::current(&app.session)
}

/// An item id alone is not a project identity: a new project reuses the same ids.
/// Keep accepting edits and undo/redo in the originating session, but fail closed after
/// opening another project or discarding the originating history snapshot.
fn project_is_open(app: &FilmcraftApp, source: &std::sync::Arc<filmcraft_project::Project>) -> bool {
    std::sync::Arc::ptr_eq(source, &app.session.project)
        || app.session.history.undo.iter().chain(&app.session.history.redo).any(|(_, project)| std::sync::Arc::ptr_eq(source, project))
}

/// The Reel's main footage: its file, source in point and length (seconds).
fn footage(app: &FilmcraftApp, info: &ReelInfo) -> Option<(std::path::PathBuf, f64, f64)> {
    let q = app.session.active_sequence()?;
    let (_, it) = q.find_item(filmcraft_project::ClipId(info.main_clip?))?;
    let item = app.session.project.item(it.item)?;
    let media = item.as_media()?;
    let filmcraft_project::MediaRef::File { path } = &media.media else { return None };
    Some((std::path::PathBuf::from(path), it.source_in.seconds(), it.duration.seconds()))
}

/// Background results and exports finishing; runs every frame, also when the panel is hidden.
pub fn poll(app: &mut FilmcraftApp, now: f64) {
    let mut r = rt();
    let mut finished = Vec::new();
    r.tasks.retain(|(label, rx)| match rx.try_recv() {
        Ok(d) => {
            finished.push((label.clone(), d));
            false
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
        Err(_) => false,
    });
    for (label, d) in finished {
        match d {
            Done::CloudQueued(id, result, now) => {
                match result {
                    Ok(true) => {
                        if let Some(q) = r.queue.iter_mut().find(|q| q.id == id) {
                            q.status = if now { "Cloud processing" } else { "Scheduled in cloud" }.into();
                        }
                        notify(
                            &mut r,
                            if now {
                                "Video uploaded to cloud. Waiting for Instagram publication."
                            } else {
                                "Video uploaded and scheduled in cloud. Safe to close the app."
                            },
                            true,
                        );
                    }
                    Ok(false) => fail_queued(&mut r, Some(id), "Cloud has not confirmed the upload yet; check Calendar before retrying.".into()),
                    Err(e) => {
                        fail_queued(&mut r, Some(id), e.clone());
                        notify(&mut r, format!("Cloud upload failed: {e}"), false);
                    }
                }
                r.last_updates = None;
            }
            Done::Updates(result) => match result {
                Ok(batch) => {
                    for post in &batch.cloud_posts {
                        let content = post.get("local_content_id").and_then(|v| v.as_str()).unwrap_or_default();
                        let status = post.get("status").and_then(|v| v.as_str()).unwrap_or_default();
                        for q in &mut r.queue {
                            if let (Some(ch), Some(render)) = (q.info.channel_id, q.render)
                                && content == format!("{ch}:{render}")
                            {
                                if status == "published" {
                                    q.status = "Published".into();
                                }
                                if status == "failed" {
                                    q.status = "Failed".into();
                                    q.error = Some("Cloud publication failed; check Calendar.".into());
                                }
                            }
                        }
                    }
                    r.updates_open |= batch.notifications.iter().any(|n| !n.read);
                    r.publication_updates = batch.notifications;
                    if !batch.cloud_error {
                        r.cloud_posts = batch.cloud_posts;
                    }
                }
                Err(e) => log(&mut r, format!("Publication updates: {e}")),
            },
            Done::Preview(path, result) => {
                if r.previews.contains_key(&path) {
                    r.previews.insert(path, result.map(|p| Some(std::sync::Arc::from(p))));
                }
            }
            Done::Card(Ok(card), target) => {
                if !target.matches(app) {
                    log(&mut r, "AI writing finished, but the Reel or its text changed. Run Write again on the current Reel.");
                    continue;
                }
                let keep = app.ui.reel.card.clone();
                app.ui.reel.card = Card {
                    badge: keep.badge,
                    bullets: keep.bullets,
                    cta: keep.cta,
                    fonts: keep.fonts,
                    text_styles: keep.text_styles,
                    text_timing: keep.text_timing,
                    ..*card
                };
                log(&mut r, "The AI wrote the card");
                if reel_info(app).is_some() {
                    let c = app.ui.reel.card.clone();
                    if let Err(e) = app.session.execute("reel.setCard", card_params(&c)) {
                        log(&mut r, format!("Card not applied: {e}"));
                    }
                }
            }
            Done::Card(Err(e), _) => log(&mut r, format!("AI writing failed: {e}")),
            Done::Health(h) => {
                if r.health_popup.is_none() {
                    r.health_popup = Some(now);
                }
                r.health_popup = r.health_popup.map(|_| now);
                r.health = Some(h);
            }
            Done::Scheduled(name, result, at, queue) => {
                let when = local_time(&at);
                let (status, error) = match result {
                    Ok(done) => {
                        let ok = done.youtube.iter().filter(|(_, r)| r.is_ok()).count();
                        let failed: Vec<String> = done.youtube.iter().filter_map(|(_, r)| r.as_ref().err().cloned()).collect();
                        let mut parts = Vec::new();
                        if done.waiting > 0 {
                            parts.push(format!("{} account(s) at {when}", done.waiting));
                        }
                        if ok > 0 {
                            parts.push(format!("uploaded to YouTube, published by YouTube at {when}"));
                        }
                        notify(&mut r, format!("Scheduled “{name}”: {}", parts.join("; ")), failed.is_empty());
                        for e in &failed {
                            notify(&mut r, format!("YouTube upload of “{name}” failed: {e}"), false);
                        }
                        (if failed.is_empty() { "Scheduled".to_string() } else { "Scheduled (YouTube failed)".to_string() }, failed.first().cloned())
                    }
                    Err(e) => {
                        notify(&mut r, format!("Scheduling “{name}” failed: {e}"), false);
                        ("Failed".to_string(), Some(e))
                    }
                };
                if let Some(q) = r.queue.iter_mut().find(|q| q.id == queue) {
                    q.status = status;
                    q.error = error;
                }
                if let Some(c) = r.loaded_channel {
                    load_channel(&mut r, c);
                }
            }
            Done::Published(name, Ok(link), queue) => {
                notify(&mut r, format!("Posted “{name}”{}", link.map(|l| format!(": {l}")).unwrap_or_default()), true);
                if let Some(id) = queue
                    && let Some(q) = r.queue.iter_mut().find(|q| q.id == id)
                {
                    q.status = "Published".into();
                }
                if let Some(c) = r.loaded_channel {
                    load_channel(&mut r, c);
                }
            }
            Done::Published(name, Err(e), queue) => {
                notify(&mut r, format!("Posting “{name}” failed: {e}"), false);
                if let Some(id) = queue
                    && let Some(q) = r.queue.iter_mut().find(|q| q.id == id)
                {
                    q.status = "Failed".into();
                    q.error = Some(e);
                }
            }
            Done::Notice((text, ok)) => {
                notify(&mut r, text, ok);
                // A notice can follow a change to the workspace's accounts (a YouTube connection).
                if let Some(c) = r.loaded_channel {
                    load_channel(&mut r, c);
                }
            }
            Done::Token(id, result, tell) => {
                if tell {
                    match &result {
                        Ok(t) => notify(&mut r, format!("Instagram token: {t}"), t.starts_with("valid")),
                        Err(e) => notify(&mut r, format!("Instagram token: {e}"), false),
                    }
                }
                r.token_info.insert(id, result);
                if let Some(c) = r.loaded_channel {
                    load_channel(&mut r, c);
                }
            }
            Done::Sent(Ok((link, render)), sequence, project, queue) => {
                // The post links to the Reel that was rendered, wherever the editor is now.
                if project_is_open(app, &project)
                    && app.session.project.item(filmcraft_project::ItemId(sequence)).is_some()
                    && let Err(e) = app.session.execute("reel.linkPost", json!({"post": link, "sequence": sequence}))
                {
                    log(&mut r, format!("Could not link the post: {e}"));
                }
                log(&mut r, format!("Sent to the library as post {} (render {render})", link.story_id));
                if let Some(id) = queue {
                    queue_sent(&mut r, id, render);
                }
                if let Some(c) = r.loaded_channel {
                    load_channel(&mut r, c);
                }
            }
            Done::Sent(Err(e), _, _, queue) => {
                if let Some(id) = queue
                    && let Some(q) = r.queue.iter_mut().find(|q| q.id == id)
                {
                    q.status = "Failed".into();
                    q.error = Some(e.clone());
                }
                log(&mut r, format!("{label} failed: {e}"));
            }
            Done::CloudAccounts(Ok(a)) => {
                r.cloud_accounts = a;
                r.cloud_error.clear();
            }
            Done::CloudPosts(Ok(p)) => {
                r.cloud_posts = p;
                r.cloud_error.clear();
            }
            Done::CloudAccounts(Err(e)) | Done::CloudPosts(Err(e)) => r.cloud_error = e,
            Done::Message(m) if m == "__imports_done" => {
                r.imports_running = false;
                if let Some(ch) = r.loaded_channel {
                    load_channel(&mut r, ch);
                }
                let failed = r.imports.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|j| j.error.is_some());
                notify(
                    &mut r,
                    if failed {
                        "Some downloads need attention. Saved clips are available in Create."
                    } else {
                        "Downloads finished. Find saved clips in Create → Workspace clips."
                    },
                    !failed,
                );
            }
            Done::Message(m) => {
                if m.starts_with("Cloud") {
                    r.last_updates = None;
                }
                let ok = m.starts_with("Published") || m.starts_with("Scheduled in cloud");
                let failed = m.starts_with("Publishing failed") || m.starts_with("Cloud scheduling failed");
                if ok || failed {
                    notify(&mut r, m, ok);
                } else if !m.trim().is_empty() {
                    log(&mut r, m);
                }
                r.due_running = false;
                if let Some(c) = r.loaded_channel {
                    load_channel(&mut r, c);
                }
            }
        }
    }
    // An export to the library finished: thumbnail, then the library hand-off on a worker.
    if let Some(job) = r.export.as_ref().map(|e| e.job) {
        let state = app.session.jobs.iter().find(|j| j.id == job).map(|j| {
            use std::sync::atomic::Ordering;
            let done = j.progress.finished.load(Ordering::Relaxed);
            (done, j.result.lock().unwrap_or_else(PoisonError::into_inner).clone())
        });
        match state {
            None => {
                r.export = None;
                log(&mut r, "The export job disappeared");
            }
            Some((true, Some(Ok(_)))) => {
                let Some(pending) = r.export.take() else { return };
                let thumb = pending.path.with_extension("png");
                if project_is_open(app, &pending.project) && app.session.state.active_sequence.map(|q| q.0) == Some(pending.sequence) {
                    let _ =
                        app.session.execute("file.exportFrame", json!({"path": thumb.to_string_lossy(), "format": "png", "time": filmcraft_reel::secs(1.0).0}));
                }
                match studio(&mut r) {
                    Some(s) => {
                        log(&mut r, "Export finished; sending to the library…");
                        spawn(&mut r, "Send to library", move || {
                            let f = pending.footage.as_ref().map(|(p, a, b)| (p.as_path(), *a, *b));
                            Done::Sent(
                                s.send_to_library(
                                    &pending.info,
                                    &pending.path,
                                    thumb.is_file().then_some(thumb.as_path()),
                                    f,
                                    pending.duration,
                                    pending.approve,
                                )
                                .map_err(|e| e.to_string()),
                                pending.sequence,
                                pending.project,
                                pending.queue,
                            )
                        });
                    }
                    _ => log(&mut r, "Export finished, but the project is not a Reel of a workspace"),
                }
            }
            Some((true, Some(Err(e)))) => {
                let queue = r.export.take().and_then(|p| p.queue);
                fail_queued(&mut r, queue, format!("the render failed: {e}"));
                log(&mut r, format!("Export failed: {e}"));
            }
            Some((true, None)) => {
                // Usually the result is a moment away; only a job that stays resultless stopped.
                let first = r.export.as_mut().map_or(now, |p| *p.finished_at.get_or_insert(now));
                if now - first > 10.0 {
                    let queue = r.export.take().and_then(|p| p.queue);
                    fail_queued(&mut r, queue, "the render was cancelled or stopped".into());
                    log(&mut r, "The export was cancelled or stopped without a result");
                }
            }
            Some((false, _)) => {}
        }
    }
    // Scheduled publications that are due (when the earlier app's worker is not running).
    if background() && !r.due_running && now - r.last_due_check > 60.0 {
        r.last_due_check = now;
        if let Some(s) = studio(&mut r) {
            r.due_running = true;
            spawn(&mut r, "Scheduled publications", move || match filmcraft_studio::run_due(&s) {
                Ok(lines) if lines.is_empty() => Done::Message(String::new()),
                Ok(lines) => Done::Message(lines.join("\n")),
                Err(e) => Done::Message(format!("Scheduled publications: {e}")),
            });
        }
    }
    // The YouTube queue: Shorts uploaded ahead of their time, within Google's daily quota.
    if background() && r.last_youtube_queue.is_none_or(|last| now - last >= 600.0) && !r.tasks.iter().any(|(label, _)| label == "YouTube queue") {
        r.last_youtube_queue = Some(now);
        if let Some(s) = studio(&mut r) {
            spawn(&mut r, "YouTube queue", move || {
                let mut lines = Vec::new();
                if filmcraft_studio::cloud::configured(&s) {
                    match filmcraft_studio::cloud_youtube::sync(&s) {
                        Ok(0) => {}
                        Ok(n) => lines.push(format!("YouTube: {n} cloud upload(s) recorded")),
                        Err(e) => lines.push(format!("Cloud YouTube: {e}")),
                    }
                }
                match filmcraft_studio::youtube_queue::run(&s) {
                    Ok(l) => lines.extend(l),
                    Err(e) => lines.push(format!("YouTube queue: {e}")),
                }
                Done::Message(lines.join("\n"))
            });
        }
    }
    if background() && r.last_updates.is_none_or(|last| now - last >= 30.0) && !r.tasks.iter().any(|(label, _)| label == "Publication updates") {
        r.last_updates = Some(now);
        if let Some(s) = studio(&mut r) {
            spawn(&mut r, "Publication updates", move || Done::Updates(filmcraft_studio::notifications::refresh(&s).map_err(|e| e.to_string())));
        }
    }
    advance_queue(app, &mut r);
    r.log.retain(|l| !l.is_empty());
}

fn card_params(c: &Card) -> serde_json::Value {
    json!({"reflow":true,"fonts": c.fonts, "textStyles":c.text_styles, "textTiming":c.text_timing, "title": c.title, "caption": c.caption, "attribution": c.attribution, "badge": c.badge, "bullets": c.bullets, "cta": c.cta, "instagramCaption": c.instagram_caption, "hashtags": c.hashtags, "youtubeTitle": c.youtube_title})
}

fn section(ui: &mut egui::Ui, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong()).default_open(open).show(ui, body);
}

/// What every Shorts Factory panel starts from: the library, the workspace (loaded when it
/// changes), the current Reel and its brand.
struct Ctx {
    s: Studio,
    info: Option<ReelInfo>,
    channel: Option<i64>,
    brand: Brand,
}

/// Loads the shared state; draws a hint and returns None when the library is missing.
fn prelude(app: &mut FilmcraftApp, ui: &mut egui::Ui) -> Option<Ctx> {
    let t = app.tokens;
    let mut info = reel_info(app);
    // The card follows the Reel that is open.
    let key = info.as_ref().map(|i| format!("{}:{:?}:{:?}", app.session.project.name, app.session.state.active_sequence, i.main_clip));
    if key.is_some() && key != app.ui.reel.loaded_for {
        if let Some(i) = &info {
            app.ui.reel.card = i.card.clone();
            if i.channel_id.is_some() {
                app.ui.reel.channel_id = i.channel_id;
            }
        }
        app.ui.reel.loaded_for = key;
    }
    let mut r = rt();
    let Some(s) = studio(&mut r) else {
        let why = r.studio.as_ref().and_then(|x| x.as_ref().err()).cloned().unwrap_or_default();
        ui.label(RichText::new("The Shorts Factory library was not found.").color(t.text));
        ui.label(RichText::new(why).small().color(t.text_dim));
        return None;
    };
    if r.channels.is_empty() {
        r.channels = s.channels().unwrap_or_default();
    }
    let channel = app.ui.reel.channel_id.or_else(|| r.channels.first().map(|c| c.id));
    if app.ui.reel.channel_id.is_none() {
        app.ui.reel.channel_id = channel;
    }
    if let Some(c) = channel
        && r.loaded_channel != Some(c)
    {
        app.ui.reel.accounts.clear();
        app.ui.reel.article = None;
        app.ui.reel.cloud_account = None;
        app.ui.reel.rename_story = None;
        load_channel(&mut r, c);
        app.ui.reel.brand_edit = r.brand.clone();
        app.ui.reel.accounts = r.accounts.iter().filter(|a| a.enabled && a.status == "CONNECTED").map(|a| a.id).collect();
        // The open Reel takes the workspace's brand.
        if let (Some(b), Some(_)) = (r.brand.clone(), info.as_ref()) {
            let _ = app.session.execute("reel.setBrand", json!({"brand": b, "channelId": c}));
            info = reel_info(app);
        }
    }
    let brand = r.brand.clone().unwrap_or_default();
    Some(Ctx { s, info, channel, brand })
}

/// A panel's frame: margins and a vertical scroll.
fn frame(ui: &mut egui::Ui, rect: Rect, id: &str, body: impl FnOnce(&mut egui::Ui)) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 8.0))));
    let ui = &mut child;
    ui.spacing_mut().item_spacing.y = 6.0;
    egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show(ui, body);
}

/// The workspace the panel works in, with a pointer to where it is chosen.
fn workspace_line(app: &FilmcraftApp, ui: &mut egui::Ui, ctx: &Ctx) {
    let name = rt().channels.iter().find(|c| Some(c.id) == ctx.channel).map(|c| c.name.clone()).unwrap_or_else(|| "no workspace".into());
    ui.label(RichText::new(format!("Workspace: {name} · change it in Reel Studio")).small().color(app.tokens.text_dim));
}

/// A row of tabs.
fn tab_bar<T: PartialEq + Copy>(app: &mut FilmcraftApp, ui: &mut egui::Ui, current: &mut T, prefix: &str, tabs: &[(T, &str, &str)]) {
    ui.horizontal_wrapped(|ui| {
        for (value, id, label) in tabs {
            let b = ui.selectable_value(current, *value, *label);
            app.auto.add_response(&format!("{prefix}.{id}"), &b, label);
        }
    });
    ui.separator();
}

fn activity(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let t = app.tokens;
    let mut r = rt();
    let unread = r.publication_updates.iter().filter(|n| !n.read).count();
    let updates = ui.button(if unread > 0 { format!("Publishing updates ({unread})") } else { "Publishing updates".into() });
    app.auto.add_response("reel.updates.open", &updates, "Publishing updates");
    if updates.clicked() {
        r.updates_open = true;
    }
    if let Some(pending) = &r.export
        && let Some(job) = app.session.jobs.iter().find(|j| j.id == pending.job)
    {
        let progress = job.to_json();
        let bar = ui.add(egui::ProgressBar::new(job.progress.fraction()).show_percentage());
        app.auto.add_response("reel.render.progress", &bar, "Render progress");
        let timing = ui.label(super::timing_suffix(&progress).trim_start_matches(" · "));
        app.auto.add_response("reel.render.timing", &timing, "Render elapsed time and ETA");
    }
    if !r.tasks.is_empty() || r.export.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(
                RichText::new(r.tasks.iter().map(|(l, _)| l.as_str()).chain(r.export.as_ref().map(|_| "Rendering")).collect::<Vec<_>>().join(" · ")).small(),
            );
        });
    }
    ui.horizontal(|ui| {
        ui.menu_button("Activity", |ui| {
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                if r.log.is_empty() {
                    ui.label("No activity yet.");
                }
                for entry in &r.log {
                    ui.label(entry);
                }
            });
        });
        if let Some(message) = r.log.first() {
            ui.add(egui::Label::new(RichText::new(message).small().color(t.text_dim)).truncate()).on_hover_text(message);
        }
    });
}

// ------------------------------------------------------------------------------------------------
// Reel Studio: workspace, accounts, status
// ------------------------------------------------------------------------------------------------

/// Reel Studio panel: which workspace and accounts the Reels belong to, and how they are doing.
pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    frame(ui, rect, "sf-studio", |ui| {
        let Some(ctx) = prelude(app, ui) else { return };
        let t = app.tokens;
        let mut r = rt();
        let name = r.channels.iter().find(|c| Some(c.id) == ctx.channel).map(|c| c.name.clone()).unwrap_or_else(|| "Choose a workspace".into());
        let mut picked = ctx.channel;
        ui.label(RichText::new("Workspace").strong());
        let combo = egui::ComboBox::from_id_salt("reel-channel").selected_text(&name).width(ui.available_width() - 40.0).show_ui(ui, |ui| {
            for c in &r.channels {
                ui.selectable_value(&mut picked, Some(c.id), &c.name);
            }
        });
        app.auto.add_response("reel.channel", &combo.response, &name);
        if picked != app.ui.reel.channel_id {
            app.ui.reel.channel_id = picked;
        }
        let brand = &ctx.brand;
        ui.horizontal(|ui| {
            for c in [&brand.primary_color, &brand.card_background, &brand.band_color] {
                let col = filmcraft_reel::color(c, [0.5, 0.5, 0.5, 1.0]);
                let (rr, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().rect_filled(rr, 3.0, egui::Color32::from_rgb((col[0] * 255.0) as u8, (col[1] * 255.0) as u8, (col[2] * 255.0) as u8));
            }
            ui.label(
                RichText::new(format!("{}  {}", brand.handle_line(), if brand.logo_path.is_some() { "· logo set" } else { "· no logo" }))
                    .small()
                    .color(t.text_dim),
            );
            if ui.small_button("↻").on_hover_text("Reload the workspace from the library").clicked()
                && let Some(c) = picked
            {
                r.channels = ctx.s.channels().unwrap_or_default();
                load_channel(&mut r, c);
            }
        });
        let accounts = r.accounts.clone();
        let health = r.health.clone();
        drop(r);
        if let Some((msg, level)) = token_warning() {
            let colour = if level >= 3 { egui::Color32::from_rgb(0xff, 0x6b, 0x6b) } else { egui::Color32::from_rgb(0xf5, 0xb0, 0x41) };
            ui.label(RichText::new(format!("⚠ Instagram token {msg}. Extend it in Export ▸ Publish ▸ Cloud.")).small().color(colour));
        }
        ui.separator();
        ui.label(RichText::new("Accounts to publish on").strong());
        if accounts.is_empty() {
            ui.label(RichText::new("No accounts are connected for this workspace. Add keys in Plugin Settings ▸ Accounts.").small().color(t.text_dim));
        }
        for a in &accounts {
            let mut on = app.ui.reel.accounts.contains(&a.id);
            let label = format!("{} · {}", a.platform.to_lowercase(), a.display_name);
            let r = ui.add_enabled(a.enabled, egui::Checkbox::new(&mut on, label));
            app.auto.add_response(&format!("reel.account.{}", a.id), &r, &a.display_name);
            if r.changed() {
                app.ui.reel.accounts.retain(|x| *x != a.id);
                if on {
                    app.ui.reel.accounts.push(a.id);
                }
            }
            let ok = a.status == "CONNECTED";
            ui.label(RichText::new(format!("   {}", a.status.to_lowercase())).small().color(if ok {
                t.text_dim
            } else {
                egui::Color32::from_rgb(0xff, 0x6b, 0x6b)
            }));
        }
        ui.separator();
        ui.label(RichText::new("Connections").strong());
        match &health {
            Some(h) => {
                for (line, ok) in &h.lines {
                    ui.label(RichText::new(format!("{} {line}", if *ok { "✓" } else { "✗" })).small().color(if *ok {
                        t.text_dim
                    } else {
                        egui::Color32::from_rgb(0xff, 0x6b, 0x6b)
                    }));
                }
            }
            None => {
                ui.label(RichText::new("Not checked yet.").small().color(t.text_dim));
            }
        }
        ui.horizontal_wrapped(|ui| {
            let b = ui.button("Check connections");
            app.auto.add_response("reel.health", &b, "Check connections");
            if b.clicked() {
                start_health_check(&ctx.s);
            }
            let b = ui.button("Plugin Settings…");
            app.auto.add_response("reel.pluginSettings", &b, "Plugin Settings");
            if b.clicked() {
                app.ui.reel.plugin_open = true;
            }
            if ui.button("Media folder").clicked() {
                let _ = std::process::Command::new("explorer").arg(&ctx.s.storage).spawn();
            }
        });
        ui.separator();
        activity(app, ui);
    });
}

// ------------------------------------------------------------------------------------------------
// Create: workspace clips, project footage, drag and drop
// ------------------------------------------------------------------------------------------------

/// Starts a Reel from a clip, asking first when that clip already has Reels.
fn start_reel(app: &mut FilmcraftApp, ctx: &Ctx, name: &str, path: Option<std::path::PathBuf>, item: Option<u64>) {
    let query = match (&path, item) {
        (_, Some(i)) => json!({"item": i}),
        (Some(p), None) => json!({"path": p.to_string_lossy()}),
        _ => return,
    };
    let uses = app.session.execute("reel.usesOf", query).ok().and_then(|v| v.get("reels").and_then(|r| r.as_array()).map(Vec::len)).unwrap_or(0);
    let prompt = ReusePrompt { name: name.into(), path, item, reels: uses };
    if uses > 0 {
        app.ui.reel.reuse = Some(prompt);
    } else {
        new_reel_from(app, ctx, &prompt);
    }
}

/// Preview uses an isolated media session and does not import into the edited project.
fn preview_clip(_app: &mut FilmcraftApp, r: &mut Runtime, m: &MediaRow) {
    r.clip_preview = Some((m.path.clone(), m.label.clone(), 0.0));
}

fn clip_preview_window(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some((path, title, mut seconds)) = rt().clip_preview.clone() else { return };
    let mut open = true;
    egui::Window::new("Footage preview").id(egui::Id::new("create-preview")).open(&mut open).default_width(640.0).show(ctx, |ui| {
        ui.strong(title.clone());
        ui.label("Preview only · added to the project when you choose New Reel");
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 320.0), egui::Sense::hover());
        let duration = delivery::preview_in(app, ui, rect, Some(&path), seconds, false, &mut rt()).unwrap_or(0.0);
        let scrub = ui.add(egui::Slider::new(&mut seconds, 0.0..=duration.max(0.01)).suffix(" s"));
        app.auto.add_response("reel.clipPreview.scrub", &scrub, "Preview footage time");
        ui.horizontal(|ui| {
            let play = ui.button("Play with audio");
            app.auto.add_response("reel.clipPreview.play", &play, "Play footage in the system player");
            if play.clicked()
                && let Err(e) = super::menu_dialogs::open_path(app, ctx, &path.to_string_lossy(), false)
            {
                app.ui.status = e;
            }
            let close = ui.button("Close preview");
            app.auto.add_response("reel.clipPreview.close", &close, "Close footage preview");
            if close.clicked() {
                rt().clip_preview = None;
            }
        });
        ui.add(egui::Label::new(path.to_string_lossy()).wrap());
    });
    if !open {
        rt().clip_preview = None;
    } else if let Some(p) = rt().clip_preview.as_mut() {
        p.2 = seconds;
    }
}

fn new_reel_from(app: &mut FilmcraftApp, ctx: &Ctx, p: &ReusePrompt) {
    let format = app.ui.reel.format;
    let mut params = json!({"name": p.name, "template": ctx.brand.default_layout, "brand": ctx.brand, "channelId": ctx.channel, "card": app.ui.reel.card, "format": format.id()});
    match (p.item, &p.path) {
        (Some(i), _) => params["item"] = json!(i),
        (None, Some(path)) => params["paths"] = json!([path.to_string_lossy()]),
        _ => return,
    }
    let mut r = rt();
    match app.session.execute("reel.new", params) {
        Ok(_) => {
            app.show_panel(crate::dock::PanelKind::Design);
            log(&mut r, format!("New {} from {}", format.label(), p.name));
        }
        Err(e) => log(&mut r, format!("Could not start the {}: {e}", format.label())),
    }
}

/// The "this clip already has a Reel" question.
fn reuse_window(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let Some(p) = app.ui.reel.reuse.clone() else { return };
    let mut open = true;
    let mut choice = None;
    egui::Window::new("Make another Reel?").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).open(&mut open).show(
        ctx,
        |ui| {
            ui.label(format!("“{}” already has {} Reel{} in this project.", p.name, p.reels, if p.reels == 1 { "" } else { "s" }));
            ui.label(RichText::new("The video is not imported again either way.").small());
            ui.add_space(6.0);
            if ui.button("New Reel from the same source video").on_hover_text("A fresh Reel with the workspace's default template").clicked() {
                choice = Some(1);
            }
            if ui.button("New version of the current Reel").on_hover_text("Copies the open Reel's cut, card and template as a new post").clicked() {
                choice = Some(2);
            }
            if ui.button("Cancel").clicked() {
                choice = Some(0);
            }
        },
    );
    if !open {
        choice = Some(0);
    }
    match choice {
        Some(1) => {
            app.ui.reel.reuse = None;
            let mut r = rt();
            let Some(s) = studio(&mut r) else { return };
            let brand = r.brand.clone().unwrap_or_default();
            drop(r);
            let c = Ctx { s, info: reel_info(app), channel: app.ui.reel.channel_id, brand };
            new_reel_from(app, &c, &p);
        }
        Some(2) => {
            app.ui.reel.reuse = None;
            match app.session.execute("reel.duplicate", json!({})) {
                Ok(_) => app.show_panel(crate::dock::PanelKind::Design),
                Err(e) => app.ui.status = format!("Open the Reel you want a new version of first: {e}"),
            }
        }
        Some(_) => app.ui.reel.reuse = None,
        None => {}
    }
}

/// Create panel: start a Reel or a Post from workspace clips or project footage.
pub fn show_create(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    // A clip dragged from the Project panel and dropped anywhere on this panel.
    let dropped = ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.any_released()) && super::dragged_project_item(ui).is_some();
    let dropped_item = if dropped { super::dragged_project_item(ui) } else { None };
    frame(ui, rect, "sf-create", |ui| {
        let Some(ctx) = prelude(app, ui) else { return };
        let t = app.tokens;
        workspace_line(app, ui, &ctx);
        ui.horizontal_wrapped(|ui| {
            let b = ui.add_enabled(ctx.channel.is_some(), egui::Button::new("Open folder"));
            app.auto.add_response("reel.create.folder", &b, "Open workspace folder");
            if b.clicked()
                && let Some(channel) = ctx.channel
            {
                match ctx.s.workspace_folder(channel) {
                    Ok(folder) => {
                        if let Err(e) = super::menu_dialogs::open_path(app, ui.ctx(), &folder.to_string_lossy(), false) {
                            app.ui.status = e;
                        }
                    }
                    Err(e) => app.ui.status = e.to_string(),
                }
            }
            let b = ui.button("Download videos");
            app.auto.add_response("reel.create.downloads", &b, "Download videos");
            if b.clicked() {
                app.show_panel(crate::PanelKind::Downloads);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Make a");
            for f in [Format::Reel, Format::Post] {
                let b = ui.selectable_value(&mut app.ui.reel.format, f, f.label());
                app.auto.add_response(&format!("reel.format.{}", f.id()), &b, f.label());
            }
        });
        let drop_zone = egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Drop a clip from the Project panel here").color(t.text_dim));
        });
        if super::dragged_project_item(ui).is_some() && drop_zone.response.rect.contains(ui.input(|i| i.pointer.hover_pos()).unwrap_or_default()) {
            ui.painter().rect_stroke(drop_zone.response.rect, 4.0, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        }
        if let Some(item) = dropped_item {
            let name = app.session.project.item(item).map(|i| i.name.clone()).unwrap_or_else(|| "clip".into());
            start_reel(app, &ctx, &name, None, Some(item.0));
        }
        section(ui, "Footage already in this project", false, |ui| {
            let mut items: Vec<(u64, String, f64)> = app
                .session
                .project
                .items
                .values()
                .filter(|i| i.as_media().is_some_and(|m| m.info.video.is_some()))
                .map(|i| (i.id.0, i.name.clone(), i.duration().seconds()))
                .collect();
            items.sort_by(|a, b| a.1.cmp(&b.1));
            if items.is_empty() {
                ui.label(RichText::new("No video in this project yet: import with File ▸ Import, or use a workspace clip below.").small().color(t.text_dim));
            }
            for (id, name, secs) in items {
                ui.horizontal(|ui| {
                    let preview = ui.small_button("Preview");
                    app.auto.add_response(&format!("reel.previewItem.{id}"), &preview, "Preview footage");
                    if preview.clicked() {
                        match app.session.execute("source.open", json!({"item": id})) {
                            Ok(_) => app.show_panel(crate::PanelKind::Source),
                            Err(e) => app.ui.status = e.to_string(),
                        }
                    }
                    let b = ui.small_button(format!("New {}", app.ui.reel.format.label()));
                    app.auto.add_response(&format!("reel.newFromItem.{id}"), &b, &name);
                    ui.add(egui::Label::new(RichText::new(format!("{name}  {secs:.1}s")).small()).truncate());
                    if b.clicked() {
                        start_reel(app, &ctx, &name, None, Some(id));
                    }
                });
            }
        });
        let mut r = rt();
        section(ui, "Workspace clips", true, |ui| {
            ui.label(RichText::new("Downloads stay here between projects.").small().color(t.text_dim))
                .on_hover_text("Preview freely. New Reel adds only the selected clip to your project.");
            ui.horizontal_wrapped(|ui| {
                let b = ui.add_enabled(ctx.channel.is_some() && !r.tasks.iter().any(|(label, _)| label == "Scan media"), egui::Button::new("Refresh"));
                app.auto.add_response("reel.media.scan", &b, "Refresh workspace folder");
                if b.clicked()
                    && let Some(ch) = ctx.channel
                {
                    let st = ctx.s.clone();
                    let legacy = filmcraft_studio::project_folder_path(app.session.path.as_deref(), "Downloads");
                    spawn(&mut r, "Scan media", move || {
                        let result = (|| -> filmcraft_studio::Result<(usize, usize)> {
                            let (mut added, unreadable) = st.scan_media(ch)?;
                            for folder in [st.app_settings().download_dir, legacy].into_iter().flatten() {
                                added += st.scan_download_folder(ch, &folder)?;
                            }
                            Ok((added, unreadable))
                        })();
                        Done::Message(match result {
                            Ok((added, unreadable)) => format!("Scan finished: {added} new videos; {unreadable} unreadable files skipped"),
                            Err(e) => format!("Scan failed: {e}"),
                        })
                    });
                }
                let open = ui.add_enabled(ctx.channel.is_some(), egui::Button::new("Folder"));
                app.auto.add_response("reel.media.openFolder", &open, "Open workspace folder");
                if open.clicked()
                    && let Some(ch) = ctx.channel
                {
                    match ctx.s.workspace_folder(ch) {
                        Ok(dir) => {
                            let clips = dir.join("assets").join("clips");
                            let target = if clips.is_dir() { clips } else { dir };
                            if let Err(e) = super::menu_dialogs::open_path(app, ui.ctx(), &target.to_string_lossy(), false) {
                                app.ui.status = e;
                            }
                        }
                        Err(e) => app.ui.status = e.to_string(),
                    }
                }
                let list = ui.selectable_value(&mut app.ui.reel.clips_grid, false, "☰ List");
                app.auto.add_response("reel.media.viewList", &list, "List view");
                let grid = ui.selectable_value(&mut app.ui.reel.clips_grid, true, "▦ Grid");
                app.auto.add_response("reel.media.viewGrid", &grid, "Grid view");
                let hide = ui.checkbox(&mut app.ui.reel.hide_done_clips, "Hide done");
                app.auto.add_response("reel.media.hideDone", &hide, "Hide clips marked done");
            });
            ui.add(egui::TextEdit::singleline(&mut app.ui.reel.media_filter).hint_text("Search clips").desired_width(f32::INFINITY));
            let filter = app.ui.reel.media_filter.to_lowercase();
            // Clips marked done (this app's own marks) can be hidden, or show greyed with a tick.
            let done: Vec<i64> = ctx.s.app_settings().done_clips;
            let hide_done = app.ui.reel.hide_done_clips;
            let clips: Vec<MediaRow> = r
                .media
                .iter()
                .filter(|m| m.kind == "VIDEO" && (filter.is_empty() || m.label.to_lowercase().contains(&filter)) && !(hide_done && done.contains(&m.id)))
                .take(60)
                .cloned()
                .collect();
            let mut toggled: Vec<i64> = Vec::new();
            if clips.is_empty() {
                ui.label(
                    RichText::new("No clips in this workspace yet. Download some, or drop files in its clips folder and refresh.").small().color(t.text_dim),
                );
            }
            if app.ui.reel.clips_grid {
                let gap = 8.0;
                let per_row = ((ui.available_width() + gap) / (150.0 + gap)).floor().max(1.0);
                let tile = ((ui.available_width() + gap) / per_row - gap).max(110.0);
                let mut removed = Vec::new();
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(gap, gap);
                    for m in &clips {
                        ui.vertical(|ui| {
                            ui.set_width(tile);
                            let (rect, click) = ui.allocate_exact_size(vec2(tile, tile * 9.0 / 16.0), egui::Sense::click());
                            delivery::preview_in(app, ui, rect, Some(&m.path), 1.0, true, &mut r);
                            let click = click.on_hover_text(format!("{} · {}", m.label, m.duration.map(|d| format!("{d:.1}s")).unwrap_or_default()));
                            if ui.is_rect_visible(rect) {
                                app.auto.add_response(&format!("reel.clip.tile.{}", m.id), &click, &m.label);
                            }
                            if click.double_clicked() {
                                r.pending_start = Some((m.label.clone(), m.path.clone()));
                            } else if click.clicked() {
                                preview_clip(app, &mut r, m);
                            }
                            ui.horizontal(|ui| {
                                let mut is_done = done.contains(&m.id);
                                let d = ui.checkbox(&mut is_done, "Done");
                                if ui.is_rect_visible(d.rect) {
                                    app.auto.add_response(&format!("reel.clip.done.{}", m.id), &d, "Mark clip done");
                                }
                                if d.changed() {
                                    toggled.push(m.id);
                                }
                                let name = if done.contains(&m.id) {
                                    RichText::new(format!("✓ {}", m.label)).small().color(t.text_dim)
                                } else {
                                    RichText::new(&m.label).small()
                                };
                                ui.add(egui::Label::new(name).truncate());
                            });
                            ui.horizontal(|ui| {
                                let new = ui.small_button(format!("New {}", app.ui.reel.format.label()));
                                if ui.is_rect_visible(new.rect) {
                                    app.auto.add_response(&format!("reel.new.{}", m.id), &new, &m.label);
                                }
                                if new.clicked() {
                                    r.pending_start = Some((m.label.clone(), m.path.clone()));
                                }
                                let remove = ui.small_button("Remove").on_hover_text("Remove from Create; keep the source file and existing Reels");
                                if ui.is_rect_visible(remove.rect) {
                                    app.auto.add_response(&format!("reel.remove.{}", m.id), &remove, "Remove from Create (keep file)");
                                }
                                if remove.clicked() {
                                    removed.push(m.id);
                                }
                            });
                        });
                    }
                });
                if let Some(channel) = ctx.channel {
                    for id in removed {
                        match ctx.s.remove_clip(channel, id) {
                            Ok(()) => {
                                r.media.retain(|row| row.id != id);
                                app.ui.status = "Removed from Create. Source file kept.".into();
                            }
                            Err(e) => app.ui.status = e.to_string(),
                        }
                    }
                }
            }
            for m in clips.iter().filter(|_| !app.ui.reel.clips_grid).cloned().collect::<Vec<_>>() {
                ui.horizontal(|ui| {
                    let b = ui.small_button(format!("New {}", app.ui.reel.format.label()));
                    // Rows scrolled out of view are not clickable, so agents don't see them.
                    let shown = ui.is_rect_visible(b.rect);
                    if shown {
                        app.auto.add_response(&format!("reel.new.{}", m.id), &b, &m.label);
                    }
                    let dims = match (m.width, m.height) {
                        (Some(w), Some(h)) => format!("{w}×{h}"),
                        _ => String::new(),
                    };
                    let preview = ui.small_button("Preview").on_hover_text(format!(
                        "Preview without adding to the project · {} · {}",
                        m.duration.map(|d| format!("{d:.1}s")).unwrap_or_default(),
                        dims
                    ));
                    if shown {
                        app.auto.add_response(&format!("reel.preview.{}", m.id), &preview, "Preview source");
                    }
                    if preview.clicked() {
                        preview_clip(app, &mut r, &m);
                    }
                    if b.clicked() {
                        r.pending_start = Some((m.label.clone(), m.path.clone()));
                    }
                    let remove = ui.small_button("Remove").on_hover_text("Remove from Create; keep the source file and existing Reels");
                    app.auto.add_response(&format!("reel.remove.{}", m.id), &remove, "Remove from Create (keep file)");
                    if remove.clicked()
                        && let Some(channel) = ctx.channel
                    {
                        match ctx.s.remove_clip(channel, m.id) {
                            Ok(()) => {
                                r.media.retain(|row| row.id != m.id);
                                app.ui.status = "Removed from Create. Source file kept.".into();
                            }
                            Err(e) => app.ui.status = e.to_string(),
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let mut is_done = done.contains(&m.id);
                    let d = ui.checkbox(&mut is_done, "Done");
                    if ui.is_rect_visible(d.rect) {
                        app.auto.add_response(&format!("reel.clip.done.{}", m.id), &d, "Mark clip done");
                    }
                    if d.changed() {
                        toggled.push(m.id);
                    }
                    let name = if done.contains(&m.id) {
                        RichText::new(format!("✓ {}", m.label)).small().color(t.text_dim)
                    } else {
                        RichText::new(&m.label).small()
                    };
                    ui.add(egui::Label::new(name).truncate());
                });
            }
            if !toggled.is_empty() {
                let mut a = ctx.s.app_settings();
                for id in toggled {
                    if let Some(i) = a.done_clips.iter().position(|d| *d == id) {
                        a.done_clips.remove(i);
                    } else {
                        a.done_clips.push(id);
                    }
                }
                if let Err(e) = ctx.s.save_app_settings(&a) {
                    app.ui.status = e.to_string();
                }
            }
            let restore = ui.small_button("Restore removed clips");
            app.auto.add_response("reel.media.restore", &restore, "Restore removed clips");
            if restore.clicked()
                && let Some(channel) = ctx.channel
            {
                match ctx.s.restore_clips(channel) {
                    Ok(count) => {
                        r.media = ctx.s.media(channel).unwrap_or_else(|_| r.media.clone());
                        app.ui.status = format!("Restored {count} clips");
                    }
                    Err(e) => app.ui.status = e.to_string(),
                }
            }
        });
        let pending = r.pending_start.take();
        drop(r);
        if let Some((name, path)) = pending {
            start_reel(app, &ctx, &name, Some(path), None);
        }
    });
}

// ------------------------------------------------------------------------------------------------
// Design: templates, card, AI, caption
// ------------------------------------------------------------------------------------------------

pub fn show_design(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    // The tabs stay at the top; only the tab's content scrolls.
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 8.0))));
    let ui = &mut child;
    ui.spacing_mut().item_spacing.y = 6.0;
    // Layout, card and caption work on the open Reel without the library; only Use AI needs it.
    let ctx = prelude(app, ui);
    let info = ctx.as_ref().map_or_else(|| reel_info(app), |c| c.info.clone());
    gaps::warning(app, ui);
    let guide = ui.checkbox(&mut app.ui.reel.instagram_guides, "Instagram safe-area guides").on_hover_text(
        "Preview only: reserves space for Instagram's top controls, right-side buttons and bottom captions. Expanded descriptions may cover more of the video.",
    );
    app.auto.add_response("reel.design.instagramGuides", &guide, "Instagram safe-area guides");
    let t = app.tokens;
    let previous = app.ui.reel.design_tab;
    let mut tab = previous;
    tab_bar(
        app,
        ui,
        &mut tab,
        "reel.design",
        &[
            (DesignTab::Templates, "templates", "Layout"),
            (DesignTab::Card, "card", "Card"),
            (DesignTab::Ai, "ai", "Use AI"),
            (DesignTab::Caption, "caption", "Caption & hashtags"),
        ],
    );
    app.ui.reel.design_tab = tab;
    // Leaving a text tab applies what was typed.
    if previous != tab
        && info.as_ref().is_some_and(|i| i.card != app.ui.reel.card)
        && let Err(e) = app.session.execute("reel.setCard", card_params(&app.ui.reel.card))
    {
        app.ui.status = e.to_string();
    }
    egui::ScrollArea::vertical().id_salt("sf-design").auto_shrink([false, false]).show(ui, |ui| {
        if info.is_none() {
            ui.label(RichText::new("Open or start a Reel (Create panel) to design it.").small().color(t.text_dim));
        }
        let is_reel = info.is_some();
        match tab {
            DesignTab::Templates => templates(app, ui, info.as_ref()),
            DesignTab::Card => card(app, ui, is_reel, CardPart::Card),
            DesignTab::Ai => match &ctx {
                Some(c) => write_card(app, ui, c.channel, &c.s),
                None => {
                    ui.label(RichText::new("Writing with AI needs the Shorts Factory library.").small().color(t.text_dim));
                }
            },
            DesignTab::Caption => card(app, ui, is_reel, CardPart::Caption),
        }
    });
}

// ------------------------------------------------------------------------------------------------
// Downloads
// ------------------------------------------------------------------------------------------------

pub fn show_downloads(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    frame(ui, rect, "sf-downloads", |ui| {
        let Some(ctx) = prelude(app, ui) else { return };
        let t = app.tokens;
        workspace_line(app, ui, &ctx);
        let settings = ctx.s.app_settings();
        let folder = settings.download_dir.clone().or_else(|| ctx.channel.and_then(|ch| ctx.s.workspace_folder(ch).ok()).map(|d| d.join("assets/clips")));
        let folder_label = folder.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "Choose a workspace".into());
        ui.label(RichText::new("Saved in Create → Workspace clips · choose a clip to start editing").small());
        ui.horizontal(|ui| {
            let b = ui.small_button("Change…");
            app.auto.add_response("reel.downloads.folder", &b, "Change download folder");
            if b.clicked() {
                let at = settings.download_dir.as_ref().map(|d| d.display().to_string());
                app.pick_ui(crate::native_dialogs::FileDialog::Folder { at }, move |app, paths| {
                    let Some(dir) = paths.first() else { return };
                    let mut r = rt();
                    if let Some(s) = studio(&mut r) {
                        let mut a = s.app_settings();
                        a.download_dir = Some(std::path::PathBuf::from(dir));
                        app.ui.status = match s.save_app_settings(&a) {
                            Ok(()) => format!("Downloads go to {dir}"),
                            Err(e) => e.to_string(),
                        };
                    }
                });
            }
            if settings.download_dir.is_some() {
                let reset = ui.small_button("Reset folder").on_hover_text("Use this workspace's clips folder");
                app.auto.add_response("reel.downloads.resetFolder", &reset, "Use workspace clips folder");
                if reset.clicked() {
                    let mut a = settings.clone();
                    a.download_dir = None;
                    if let Err(e) = ctx.s.save_app_settings(&a) {
                        app.ui.status = e.to_string();
                    }
                }
            }
            ui.add(egui::Label::new(RichText::new(&folder_label).small().color(t.text_dim)).truncate()).on_hover_text(&folder_label);
        });
        imports(app, ui, ctx.channel, &ctx.s);
    });
}

// ------------------------------------------------------------------------------------------------
// Export ▸ Library / Publish
// ------------------------------------------------------------------------------------------------

/// The tabs across the top of Export mode. Returns the area left for FilmCraft's own export
/// screen, or None when Library or Publish is showing.
pub fn export_tabs(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) -> Option<Rect> {
    if app.ui.reel.publish_tab == PublishTab::Cloud {
        app.ui.reel.publish_tab = PublishTab::Now;
    }
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 34.0));
    let rest = Rect::from_min_max(egui::pos2(rect.min.x, bar.max.y + 4.0), rect.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(10.0, 4.0))));
    let mut tab = app.ui.reel.export_tab;
    child.horizontal(|ui| {
        for (value, id, label) in
            [(ExportTab::File, "file", "Export file"), (ExportTab::Library, "library", "Library"), (ExportTab::Publish, "publish", "Publish")]
        {
            let b = ui.selectable_value(&mut tab, value, RichText::new(label).strong());
            app.auto.add_response(&format!("export.tab.{id}"), &b, label);
        }
    });
    app.ui.reel.export_tab = tab;
    match tab {
        ExportTab::File => Some(rest),
        ExportTab::Library => {
            delivery::library_page(app, ui, rest);
            None
        }
        ExportTab::Publish => {
            // Schedule (cloud publishing on a calendar) and the queue.
            delivery::publish_page(app, ui, rest);
            None
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Queue: render, send to the library, publish or schedule — one Reel after another
// ------------------------------------------------------------------------------------------------

/// Adds the open Reel to the render-and-publish queue.
fn enqueue(app: &mut FilmcraftApp, info: &ReelInfo, action: QueueAction) {
    if !gaps::queue_guard(app, info, &action) {
        return;
    }
    enqueue_range(app, info, action, None);
}
fn enqueue_range(app: &mut FilmcraftApp, info: &ReelInfo, action: QueueAction, range: Option<(i64, i64)>) {
    let Some(seq) = app.session.state.active_sequence else { return };
    let name = app.session.project.item(seq).map(|i| i.name.clone()).unwrap_or_else(|| "Reel".into());
    let item = QueueItem {
        range,
        id: crate::panels::reel::next_id(),
        sequence: seq.0,
        name,
        info: info.clone(),
        footage: footage(app, info),
        duration: range
            .map(|(a, b)| filmcraft_time::Tick(b.saturating_sub(a)).seconds())
            .unwrap_or_else(|| app.session.active_sequence().map_or(0.0, |q| q.duration().seconds())),
        project: app.session.project.clone(),
        action,
        status: "Waiting to render".into(),
        render: None,
        error: None,
    };
    let mut r = rt();
    log(&mut r, format!("Queued {}", item.name));
    r.queue.push(item);
}

fn next_id() -> u64 {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Starts the next waiting queue item's render when nothing else is rendering.
fn advance_queue(app: &mut FilmcraftApp, r: &mut Runtime) {
    if r.export.is_some() {
        return;
    }
    let Some(i) = r.queue.iter().position(|q| q.status == "Waiting to render") else { return };
    let Some(s) = studio(r) else { return };
    let Some(q) = r.queue.get_mut(i) else { return };
    // The Reel renders as it is now (edits made while it waited are kept); one that was deleted
    // or opened from another project fails.
    let seq = filmcraft_project::ItemId(q.sequence);
    if !std::sync::Arc::ptr_eq(&q.project, &app.session.project)
        && (q.range.is_some() || filmcraft_engine::edge_gaps::inspect(&app.session.project, seq).is_some())
    {
        q.status = "Failed".into();
        q.error = Some("The Reel changed after its empty-edge check. Queue it again to review the render range.".into());
        return;
    }
    let same_file = project_is_open(app, &q.project);
    match filmcraft_engine::reel::for_sequence(app.session.project.reel.as_ref(), seq) {
        Some(info) if same_file && app.session.project.item(seq).is_some() => {
            q.info = info;
            q.project = app.session.project.clone();
        }
        _ => {
            q.status = "Failed".into();
            q.error = Some("This Reel is no longer in the open project; queue it again".into());
            return;
        }
    }
    // Renders go to the project's own folder (beside the project file), not the app's.
    let dir = match filmcraft_studio::project_folder(app.session.path.as_deref(), "Renders") {
        Ok(d) => d,
        Err(e) => {
            q.status = "Failed".into();
            q.error = Some(e.to_string());
            return;
        }
    };
    let safe: String = q.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
    let path = dir.join(format!("{safe}-{}.mp4", filmcraft_studio::now_iso().replace([':', '.'], "-")));
    if let Err(e) = app.session.execute("file.saveCopy", json!({"path":path.with_extension("fcproj").to_string_lossy()})) {
        q.status = "Failed".into();
        q.error = Some(format!("Could not save the editable project: {e}"));
        return;
    }
    let preset = if q.info.format == "post" { "Match Source – Adaptive High Bitrate" } else { "Social Vertical 1080×1920" };
    let mut params = json!({"preset":preset,"path":path.to_string_lossy(),"range":"entire","sequence":q.sequence});
    if let Some((start, end)) = q.range {
        params["range"] = json!("custom");
        params["startTime"] = json!(start);
        params["endTime"] = json!(end);
    }
    match app.session.execute("export.quick", params) {
        Ok(v) => match v["job"].as_u64() {
            Some(job) => {
                q.status = "Rendering".into();
                let approve = !matches!(q.action, QueueAction::LibraryOnly) || s.app_settings().queue_auto_approve;
                r.export = Some(PendingExport {
                    job,
                    path,
                    approve,
                    info: q.info.clone(),
                    footage: q.footage.clone(),
                    duration: q.duration,
                    queue: Some(q.id),
                    sequence: q.sequence,
                    finished_at: None,
                    project: q.project.clone(),
                });
            }
            None => {
                q.status = "Failed".into();
                q.error = Some("the render did not start".into());
            }
        },
        Err(e) => {
            q.status = "Failed".into();
            q.error = Some(e.to_string());
        }
    }
}

/// A queued render that did not finish: the item says why, and the queue moves on.
fn fail_queued(r: &mut Runtime, queue: Option<u64>, why: String) {
    if let Some(id) = queue
        && let Some(q) = r.queue.iter_mut().find(|q| q.id == id)
    {
        q.status = "Failed".into();
        q.error = Some(why);
    }
}

/// A queue item reached the library: publish or schedule it.
fn queue_sent(r: &mut Runtime, id: u64, render: i64) {
    let Some(s) = studio(r) else { return };
    let Some(q) = r.queue.iter_mut().find(|q| q.id == id) else { return };
    q.render = Some(render);
    let Some(ch) = q.info.channel_id else { return };
    match q.action.clone() {
        QueueAction::LibraryOnly => q.status = "In the library".into(),
        QueueAction::Schedule(accounts, at) => {
            // Instagram waits for its time; YouTube is uploaded now with that publish time.
            q.status = "Scheduling".into();
            let name = q.name.clone();
            spawn(r, "Scheduling", move || {
                Done::Scheduled(name, filmcraft_studio::schedule_everywhere(&s, ch, render, &accounts, &at).map_err(|e| e.to_string()), at, id)
            });
        }
        QueueAction::PublishNow(accounts) => {
            q.status = "Publishing".into();
            let name = q.name.clone();
            for a in accounts {
                let st = s.clone();
                let name = name.clone();
                spawn(r, "Publishing", move || Done::Published(name, filmcraft_studio::publish_now(&st, ch, render, a).map_err(|e| e.to_string()), Some(id)));
            }
        }
        QueueAction::Cloud(account, at, now, youtube) => {
            // YouTube alongside the cloud post: at once, or uploaded now with YouTube's publish time.
            if !youtube.is_empty() {
                let (st, name) = (s.clone(), q.name.clone());
                if now {
                    for a in youtube {
                        let (st, name) = (st.clone(), name.clone());
                        spawn(r, "Publishing", move || Done::Published(name, filmcraft_studio::publish_now(&st, ch, render, a).map_err(|e| e.to_string()), None));
                    }
                } else {
                    // Booked in the cloud as well: it uploads with this PC off, within the quota.
                    let when = at.clone();
                    spawn(r, "Scheduling", move || {
                        let lines: Vec<String> = youtube
                            .iter()
                            .map(|a| match filmcraft_studio::cloud_youtube::schedule(&st, ch, render, *a, &when) {
                                Ok(_) => format!("YouTube: “{name}” booked in the cloud for {}", local_time(&when)),
                                Err(e) => format!("YouTube: booking “{name}” in the cloud failed: {e}"),
                            })
                            .collect();
                        Done::Message(lines.join("\n"))
                    });
                }
            }
            let Some(q) = r.queue.iter_mut().find(|q| q.id == id) else { return };
            q.status = "Publishing".into();
            spawn(r, "Cloud scheduling", move || {
                Done::CloudQueued(
                    id,
                    (|| {
                        let caption = s.caption_for(render)?;
                        let response = filmcraft_studio::cloud::schedule(&s, ch, render, &account, &caption, &at, now)?;
                        Ok(response.get("cloudConfirmed") == Some(&serde_json::Value::Bool(true)))
                    })()
                    .map_err(|e: filmcraft_studio::StudioError| e.to_string()),
                    now,
                )
            });
        }
    }
}

fn queue_view(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &Ctx) {
    let t = app.tokens;
    ui.label(RichText::new("Reels in the queue render one after another, go to the library approved, and are then published or scheduled on the accounts chosen in Reel Studio.").small().color(t.text_dim));
    if let Some(info) = ctx.info.clone() {
        ui.horizontal_wrapped(|ui| {
            let accounts = app.ui.reel.accounts.clone();
            if ui.button("Queue: render + library").clicked() {
                enqueue(app, &info, QueueAction::LibraryOnly);
            }
            if ui.add_enabled(!accounts.is_empty(), egui::Button::new("Queue: render + publish now")).clicked() {
                enqueue(app, &info, QueueAction::PublishNow(accounts.clone()));
            }
            ui.add(egui::TextEdit::singleline(&mut app.ui.reel.schedule_at).desired_width(130.0).hint_text("YYYY-MM-DD HH:MM"));
            if ui.add_enabled(!accounts.is_empty(), egui::Button::new("Queue: render + schedule")).clicked() {
                match filmcraft_studio::local_to_utc_iso(&app.ui.reel.schedule_at) {
                    Some(at) => enqueue(app, &info, QueueAction::Schedule(accounts.clone(), at)),
                    None => app.ui.status = "Type the time as YYYY-MM-DD HH:MM".into(),
                }
            }
        });
    } else {
        ui.label(RichText::new("Open a Reel to add it to the queue.").small().color(t.text_dim));
    }
    ui.separator();
    ui.label(RichText::new("Render queue").strong());
    let items: Vec<(u64, String, String, Option<String>)> = rt().queue.iter().map(|q| (q.id, q.name.clone(), q.status.clone(), q.error.clone())).collect();
    if items.is_empty() {
        ui.label(RichText::new("Nothing queued.").small().color(t.text_dim));
    }
    for (id, name, status, error) in items {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&name).strong());
            ui.label(RichText::new(&status).small());
            if let Some(e) = &error {
                ui.label(RichText::new(e).small().color(egui::Color32::from_rgb(0xff, 0x6b, 0x6b)));
            }
            if (status == "Waiting to render" || status == "Failed") && ui.small_button("Remove").clicked() {
                rt().queue.retain(|q| q.id != id);
            }
            if status == "Failed" && ui.small_button("Retry").clicked() {
                let mut r = rt();
                if let Some(q) = r.queue.iter_mut().find(|q| q.id == id) {
                    q.status = "Waiting to render".into();
                    q.error = None;
                }
            }
        });
    }
    ui.separator();
    if let Ok(q) = filmcraft_studio::youtube_queue::status(&ctx.s)
        && (q.waiting > 0 || q.uploaded_today > 0)
    {
        ui.label(RichText::new("YouTube queue").strong());
        let mut line = format!(
            "{} waiting · {} of {} uploads used in the last 24 hours",
            q.waiting,
            q.uploaded_today,
            filmcraft_studio::youtube_queue::DAILY_UPLOADS
        );
        if let Some(next) = &q.next {
            line.push_str(&format!(" · next {}", local_time(next)));
        }
        if let Some(until) = &q.paused_until {
            line.push_str(&format!(" · quota used up, continues after {}", local_time(until)));
        }
        ui.label(RichText::new(line).small());
        ui.label(RichText::new("Shorts upload up to a day and a half ahead, while the app is open; YouTube publishes each at its time.").small().color(t.text_dim));
        if q.waiting > 0 && filmcraft_studio::cloud::configured(&ctx.s) {
            let busy = rt().tasks.iter().any(|(label, _)| label == "Moving YouTube to the cloud");
            let b = ui.add_enabled(!busy, egui::Button::new("Move to the cloud"));
            app.auto.add_response("reel.queue.youtubeToCloud", &b, "Move the YouTube queue to the cloud");
            if b.on_hover_text("The cloud uploads them at their times with this PC off").clicked() {
                let s = ctx.s.clone();
                let mut runtime = rt();
                spawn(&mut runtime, "Moving YouTube to the cloud", move || match filmcraft_studio::cloud_youtube::move_queue(&s) {
                    Ok(lines) => Done::Message(lines.join("\n")),
                    Err(e) => Done::Message(format!("Moving YouTube to the cloud: {e}")),
                });
            }
        }
        ui.separator();
    }
    if let Ok(n) = filmcraft_studio::cloud_youtube::waiting(&ctx.s)
        && n > 0
    {
        ui.label(RichText::new(format!("{n} YouTube Short(s) waiting in the cloud: they upload with this PC off, within the daily quota.")).small());
        ui.separator();
    }
    ui.label(RichText::new("Upcoming in the cloud").strong());
    let mut upcoming: Vec<serde_json::Value> = rt()
        .cloud_posts
        .iter()
        .filter(|p| matches!(p.get("status").and_then(|v| v.as_str()), Some("scheduled" | "processing" | "publishing")))
        .cloned()
        .collect();
    upcoming.sort_by_key(|p| p.get("scheduled_for").and_then(|v| v.as_str()).unwrap_or("").to_string());
    if upcoming.is_empty() {
        ui.label(RichText::new("Nothing scheduled. Pick a day in the Schedule tab.").small().color(t.text_dim));
    }
    for p in upcoming.iter().take(40) {
        let at = p.get("scheduled_for").and_then(|v| v.as_str()).unwrap_or_default();
        let title = p.get("title").and_then(|v| v.as_str()).unwrap_or("Reel");
        let status = p.get("status").and_then(|v| v.as_str()).unwrap_or_default();
        ui.label(RichText::new(format!("{} · {title} · {status}", local_time(at))).small());
    }
    ui.label(RichText::new("Upcoming on this PC").strong());
    let now = filmcraft_studio::now_iso();
    match ctx.s.calendar(ctx.channel, &now, "9999-12-31T00:00:00.000Z") {
        Ok(entries) if entries.iter().all(|e| e.cloud) => {
            ui.label(RichText::new("No local publications scheduled.").small().color(t.text_dim));
        }
        Ok(entries) => {
            for e in entries.iter().filter(|e| !e.cloud).take(40) {
                ui.label(RichText::new(format!("{} · {} · {} · {}", local_time(&e.at), e.title, e.platform.to_lowercase(), e.status.to_lowercase())).small());
            }
        }
        Err(e) => {
            ui.label(RichText::new(e.to_string()).small());
        }
    }
}

/// A UTC ISO time as local `YYYY-MM-DD HH:MM`.
fn local_time(iso: &str) -> String {
    use chrono::TimeZone;
    filmcraft_studio::clock_ms(iso)
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| iso.to_string())
}

// ------------------------------------------------------------------------------------------------
// Start-up check and notifications
// ------------------------------------------------------------------------------------------------

/// Runs the connection check in the background; its report becomes the start-up popup.
fn start_health_check(s: &Studio) {
    let mut r = rt();
    if r.tasks.iter().any(|(l, _)| l == "Connection check") {
        return;
    }
    let st = s.clone();
    spawn(&mut r, "Connection check", move || Done::Health(filmcraft_studio::settings::health_check(&st)));
}

/// A short message in the corner: published, scheduled, failed.
fn notify(r: &mut Runtime, text: impl Into<String>, ok: bool) {
    let text = text.into();
    log(r, text.clone());
    r.notices.push(Notice { text, ok, shown: None });
}

/// Pop-ups drawn over the whole window: the start-up check, notifications, the reuse question
/// and the Plugin Settings window. Called every frame.
fn publication_updates_popup(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let (mut open, events) = {
        let r = rt();
        (r.updates_open, r.publication_updates.clone())
    };
    if !open {
        return;
    }
    let mut dismiss = false;
    // Updates to clear: None = none, Some(None) = all, Some(Some(ids)) = these.
    let mut clear: Option<Option<Vec<String>>> = None;
    egui::Window::new("Publishing updates").id(egui::Id::new("reel-publication-updates")).open(&mut open).default_width(420.0).show(ctx, |ui| {
        if events.is_empty() {
            ui.label("No publishing updates yet.");
        }
        egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
            for event in &events {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{} {}", if event.ok { "✓" } else { "!" }, event.title)).strong());
                        let x = ui.small_button("Clear").on_hover_text("Clear this update");
                        app.auto.add_response(&format!("reel.updates.clear.{}", event.id), &x, "Clear this update");
                        if x.clicked() {
                            clear = Some(Some(vec![event.id.clone()]));
                        }
                    });
                    ui.label(&event.message);
                    ui.label(RichText::new(local_time(&event.at)).small());
                    if let Some(link) = &event.link {
                        let response = ui.hyperlink_to("Open published video", link);
                        app.auto.add_response(&format!("reel.updates.link.{}", event.id), &response, "Open published video");
                    }
                });
            }
        });
        ui.horizontal(|ui| {
            let b = ui.button("Mark read and close");
            app.auto.add_response("reel.updates.dismiss", &b, "Mark updates read and close");
            dismiss = b.clicked();
            let all = ui.add_enabled(!events.is_empty(), egui::Button::new("Clear all"));
            app.auto.add_response("reel.updates.clearAll", &all, "Clear all updates");
            if all.clicked() {
                clear = Some(None);
            }
        });
    });
    if let Some(which) = clear {
        let mut r = rt();
        match &which {
            Some(ids) => r.publication_updates.retain(|e| !ids.contains(&e.id)),
            None => r.publication_updates.clear(),
        }
        if let Some(s) = studio(&mut r) {
            spawn(&mut r, "Clear updates", move || {
                Done::Message(match filmcraft_studio::notifications::clear(&s, which.as_deref()) {
                    Ok(()) => String::new(),
                    Err(e) => format!("Could not clear the updates: {e}"),
                })
            });
        }
        return;
    }
    let mut r = rt();
    r.updates_open = open && !dismiss;
    if !open || dismiss {
        let ids: Vec<String> = events.iter().filter(|e| !e.read).map(|e| e.id.clone()).collect();
        for event in &mut r.publication_updates {
            event.read = true;
        }
        if !ids.is_empty()
            && let Some(s) = studio(&mut r)
        {
            spawn(&mut r, "Mark updates read", move || {
                Done::Message(match filmcraft_studio::notifications::acknowledge(&s, &ids) {
                    Ok(()) => String::new(),
                    Err(e) => format!("Could not save notification acknowledgement: {e}"),
                })
            });
        }
    }
}

pub fn overlays(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    publication_updates_popup(app, ctx);
    gaps::overlay(app, ctx);
    clip_preview_window(app, ctx);
    // The start-up check, once per run.
    {
        let mut r = rt();
        if background() && !r.startup_checked {
            r.startup_checked = true;
            if let Some(s) = studio(&mut r)
                && s.app_settings().startup_check
            {
                drop(r);
                start_health_check(&s);
                rt().health_popup = Some(now);
            }
        }
    }
    let (health, popup_since) = {
        let r = rt();
        (r.health.clone(), r.health_popup)
    };
    if let (Some(h), Some(since)) = (health, popup_since) {
        let all_ok = h.lines.iter().all(|(_, ok)| *ok);
        let age = now - since;
        // A clean check fades after a few seconds; a problem stays until it is closed.
        let alpha = if all_ok { (1.0 - ((age - 6.0) / 2.0)).clamp(0.0, 1.0) as f32 } else { 1.0 };
        if alpha <= 0.0 {
            rt().health_popup = None;
        } else {
            let mut close = false;
            egui::Area::new(egui::Id::new("sf-health")).anchor(egui::Align2::RIGHT_TOP, [-16.0, 56.0]).order(egui::Order::Foreground).show(ctx, |ui| {
                ui.set_opacity(alpha);
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(380.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(if all_ok { "✓ Everything is connected" } else { "Connection check" }).strong());
                        if ui.small_button("Close").clicked() {
                            close = true;
                        }
                    });
                    for (line, ok) in &h.lines {
                        ui.label(RichText::new(format!("{} {line}", if *ok { "✓" } else { "✗" })).small().color(if *ok {
                            egui::Color32::from_rgb(0x4a, 0xde, 0x80)
                        } else {
                            egui::Color32::from_rgb(0xff, 0x6b, 0x6b)
                        }));
                    }
                    if !all_ok && ui.small_button("Plugin Settings…").clicked() {
                        app.ui.reel.plugin_open = true;
                        close = true;
                    }
                });
            });
            if close {
                rt().health_popup = None;
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    } else if popup_since.is_some() {
        // Still checking: a quiet line, so start-up is not blocked.
        egui::Area::new(egui::Id::new("sf-health")).anchor(egui::Align2::RIGHT_TOP, [-16.0, 56.0]).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Checking AI, accounts and cloud…");
                });
            });
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
    // Notifications, newest at the bottom, 6 seconds each.
    {
        let mut r = rt();
        for n in &mut r.notices {
            n.shown.get_or_insert(now);
        }
        r.notices.retain(|n| n.shown.is_some_and(|s| now - s < 6.0));
        let notices: Vec<(String, bool)> = r.notices.iter().map(|n| (n.text.clone(), n.ok)).collect();
        drop(r);
        if !notices.is_empty() {
            egui::Area::new(egui::Id::new("sf-notices")).anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -40.0]).order(egui::Order::Foreground).show(ctx, |ui| {
                for (text, ok) in notices {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(360.0);
                        ui.label(RichText::new(format!("{} {text}", if ok { "✓" } else { "✗" })).color(if ok {
                            egui::Color32::from_rgb(0x4a, 0xde, 0x80)
                        } else {
                            egui::Color32::from_rgb(0xff, 0x6b, 0x6b)
                        }));
                    });
                }
            });
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
    reuse_window(app, ctx);
    plugin_settings(app, ctx);
}

/// Menu and control-channel commands of Shorts Factory's UI.
pub fn route(app: &mut FilmcraftApp, _ctx: &egui::Context, id: &str, _params: &serde_json::Value) -> Option<Result<serde_json::Value, String>> {
    match id {
        "reel.pluginSettings" => {
            app.ui.reel.plugin_open = true;
            Some(Ok(serde_json::Value::Null))
        }
        "reel.connectionCheck" => {
            let mut r = rt();
            let s = studio(&mut r)?;
            drop(r);
            start_health_check(&s);
            rt().health_popup = Some(0.0);
            Some(Ok(serde_json::Value::Null))
        }
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------
// Plugin Settings window
// ------------------------------------------------------------------------------------------------

fn plugin_settings(app: &mut FilmcraftApp, ctx: &egui::Context) {
    if !app.ui.reel.plugin_open {
        return;
    }
    let mut open = true;
    egui::Window::new("Shorts Factory · Plugin Settings").id(egui::Id::new("sf-plugin-settings")).default_size([560.0, 620.0]).open(&mut open).show(
        ctx,
        |ui| {
            let Some(mut context) = prelude(app, ui) else { return };
            let channels = rt().channels.clone();
            let previous_channel = app.ui.reel.channel_id;
            let name = channels.iter().find(|c| Some(c.id) == previous_channel).map_or("Choose workspace", |c| c.name.as_str());
            ui.horizontal(|ui| {
                ui.label("Workspace");
                let picker = egui::ComboBox::from_id_salt("sf-settings-workspace").selected_text(name).show_ui(ui, |ui| {
                    for channel in &channels {
                        ui.selectable_value(&mut app.ui.reel.channel_id, Some(channel.id), &channel.name);
                    }
                });
                app.auto.add_response("reel.settings.workspace", &picker.response, "Settings workspace");
            });
            if app.ui.reel.channel_id != previous_channel {
                let Some(next) = prelude(app, ui) else { return };
                context = next;
            }
            let (s, brand, channel) = (context.s, context.brand, context.channel);
            let mut tab = app.ui.reel.plugin_tab;
            tab_bar(
                app,
                ui,
                &mut tab,
                "reel.settings",
                &[
                    (PluginTab::Brands, "brands", "Workspaces & brands"),
                    (PluginTab::Accounts, "accounts", "Accounts & keys"),
                    (PluginTab::Ai, "ai", "AI"),
                    (PluginTab::Cloud, "cloud", "Cloud"),
                    (PluginTab::Downloads, "downloads", "Downloads & tools"),
                ],
            );
            app.ui.reel.plugin_tab = tab;
            egui::ScrollArea::vertical().id_salt("sf-plugin-scroll").show(ui, |ui| match tab {
                PluginTab::Brands => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Logo: {}", brand.logo_path.as_deref().unwrap_or("none"))).small());
                        let b = ui.button("Choose logo…");
                        app.auto.add_response("reel.settings.logo", &b, "Choose logo");
                        if b.clicked()
                            && let Some(ch) = channel
                        {
                            app.pick_ui(
                                crate::native_dialogs::FileDialog::OpenFile {
                                    filter: "Images".into(),
                                    exts: vec!["png".into(), "jpg".into(), "jpeg".into(), "webp".into()],
                                },
                                move |app, paths| {
                                    let Some(file) = paths.first() else { return };
                                    let mut r = rt();
                                    let Some(s) = studio(&mut r) else { return };
                                    app.ui.status = match s.set_logo(ch, std::path::Path::new(file)) {
                                        Ok(_) => {
                                            load_channel(&mut r, ch);
                                            let brand = r.brand.clone();
                                            drop(r);
                                            if reel_info(app).is_some_and(|info| info.channel_id == Some(ch))
                                                && let Some(brand) = brand
                                            {
                                                match app.session.execute("reel.setBrand", json!({"brand": brand, "channelId": ch})) {
                                                    Ok(_) => "Logo saved in Brand Assets and applied to this Reel.".into(),
                                                    Err(e) => format!("Logo saved; could not update the open Reel: {e}"),
                                                }
                                            } else {
                                                "Logo saved in this workspace's Brand Assets.".into()
                                            }
                                        }
                                        Err(e) => e.to_string(),
                                    };
                                },
                            );
                        }
                    });
                    settings(app, ui, channel, &s, &brand);
                }
                PluginTab::Accounts => accounts_settings(app, ui, channel, &s),
                PluginTab::Ai => env_fields(
                    app,
                    ui,
                    &[
                        ("AI_PROVIDER", "Provider (hermes or ollama)", false),
                        ("OLLAMA_HOST", "Ollama address", false),
                        ("OLLAMA_MODEL", "Ollama model", false),
                        ("HERMES_BIN", "Hermes program", false),
                        ("HERMES_MODEL_POST", "Hermes model", false),
                    ],
                ),
                PluginTab::Cloud => {
                    env_fields(
                        app,
                        ui,
                        &[
                            ("CLOUD_SUPABASE_URL", "Supabase project URL", false),
                            ("CLOUD_SUPABASE_ANON_KEY", "Supabase anon key", true),
                            ("CLOUD_AUTH_EMAIL", "Sign-in email", false),
                            ("CLOUD_AUTH_PASSWORD", "Sign-in password", true),
                            ("META_GRAPH_VERSION", "Meta Graph API version", false),
                            ("META_APP_ID", "Meta app ID (to extend local tokens)", false),
                            ("META_APP_SECRET", "Meta app secret", true),
                        ],
                    );
                    ui.separator();
                    ui.label("Local and cloud tokens are stored separately. You can use the same valid Meta token for both, but replacing one does not update the other.");
                    cloud(app,ui,None,channel,&s,true);
                }
                PluginTab::Downloads => {
                    env_fields(
                        app,
                        ui,
                        &[
                            ("YTDLP_PATH", "yt-dlp program (empty: tools folder or PATH)", false),
                            ("FFMPEG_PATH", "FFmpeg program (for best video + audio)", false),
                        ],
                    );
                    // YouTube mostly sends picture and sound separately: FFmpeg joins them.
                    ui.horizontal_wrapped(|ui| {
                        match filmcraft_studio::ytdlp::ffmpeg(&s) {
                            Some(p) => ui.label(RichText::new(format!("FFmpeg: {}", p.display())).small().color(app.tokens.text_dim)),
                            None => ui.label(RichText::new("FFmpeg not found: most YouTube downloads need it to join picture and sound").small().color(app.tokens.danger)),
                        };
                        let pick = ui.small_button("Choose FFmpeg…");
                        app.auto.add_response("reel.settings.chooseFfmpeg", &pick, "Choose FFmpeg");
                        if pick.clicked() {
                            app.pick_ui(crate::native_dialogs::FileDialog::OpenFile { filter: "FFmpeg".into(), exts: vec!["exe".into()] }, |app, paths| {
                                let Some(path) = paths.first() else { return };
                                let mut r = rt();
                                let Some(s) = studio(&mut r) else { return };
                                app.ui.status = match filmcraft_studio::ytdlp::use_ffmpeg(&s, std::path::Path::new(path)) {
                                    Ok(p) => format!("FFmpeg ready: {}", p.display()),
                                    Err(e) => format!("Could not use that FFmpeg: {e}"),
                                };
                            });
                        }
                    });
                    let mut a = s.app_settings();
                    let before = a.clone();
                    ui.checkbox(&mut a.startup_check, "Check connections when the app starts");
                    ui.checkbox(&mut a.queue_auto_approve, "Queue: approve renders automatically");
                    if a != before
                        && let Err(e) = s.save_app_settings(&a)
                    {
                        app.ui.status = e.to_string();
                    }
                }
            });
        },
    );
    if !open {
        app.ui.reel.plugin_open = false;
    }
}

/// Text fields bound to `.env` keys; Save writes them to the library's `.env`.
fn env_fields(app: &mut FilmcraftApp, ui: &mut egui::Ui, fields: &[(&'static str, &str, bool)]) {
    let t = app.tokens;
    ui.label(RichText::new("Saved in the library's .env file, shared with the earlier app. Secrets stay on this PC.").small().color(t.text_dim));
    let current: Vec<String> = {
        let mut r = rt();
        let s = studio(&mut r);
        fields.iter().map(|(k, _, _)| s.as_ref().and_then(|s| s.var(k).map(str::to_string)).unwrap_or_default()).collect()
    };
    let drafts = &mut app.ui.reel.env_drafts;
    egui::Grid::new(("sf-env", fields.first().map(|f| f.0).unwrap_or(""))).num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
        for ((key, label, secret), value) in fields.iter().zip(&current) {
            ui.label(*label);
            let entry = drafts.entry((*key).to_string()).or_insert_with(|| value.clone());
            ui.add(egui::TextEdit::singleline(entry).password(*secret).desired_width(300.0));
            ui.end_row();
        }
    });
    let changed: Vec<(&'static str, String)> =
        fields.iter().zip(&current).filter_map(|((k, _, _), v)| drafts.get(*k).filter(|d| *d != v).map(|d| (*k, d.trim().to_string()))).collect();
    let b = ui.add_enabled(!changed.is_empty(), egui::Button::new("Save"));
    app.auto.add_response("reel.settings.save", &b, "Save settings");
    if b.clicked() {
        let mut r = rt();
        if let Some(Ok(s)) = r.studio.as_mut() {
            let pairs: Vec<(&str, &str)> = changed.iter().map(|(k, v)| (*k, v.as_str())).collect();
            app.ui.status = match s.write_env(&pairs) {
                Ok(()) => format!("Saved {} setting(s)", pairs.len()),
                Err(e) => e.to_string(),
            };
        }
    }
}

fn accounts_settings(app: &mut FilmcraftApp, ui: &mut egui::Ui, channel: Option<i64>, s: &Studio) {
    let t = app.tokens;
    ui.label(
        RichText::new("Each workspace's own publishing accounts. Keys are stored in the library's secrets folder and are never shown here.")
            .small()
            .color(t.text_dim),
    );
    // YouTube Shorts: the same Reels, published to the workspace's channel too.
    section(ui, "YouTube", true, |ui| {
        ui.label(
            RichText::new("Posts the same Reel to the workspace's YouTube channel as a Short: the headline is the title, the Instagram caption and hashtags the description (with #Shorts).")
                .small()
                .color(t.text_dim),
        );
        let guide = egui::CollapsingHeader::new("How to get the Google client ID and secret (one time)").default_open(true).show(ui, |ui| {
            let step = |ui: &mut egui::Ui, n: u8, text: &str| {
                ui.label(RichText::new(format!("{n}. {text}")).small());
            };
            let link = |app: &mut FilmcraftApp, ui: &mut egui::Ui, id: &str, label: &str, url: &str| {
                let b = ui.small_button(label);
                app.auto.add_response(&format!("reel.youtube.guide.{id}"), &b, label);
                if b.clicked() {
                    crate::links::open(app, ui.ctx(), url);
                }
            };
            step(ui, 1, "Open Google Cloud and create a project (any name, e.g. Shorts Factory), signed in with the Google account that owns the YouTube channel.");
            link(app, ui, "console", "Open Google Cloud console", "https://console.cloud.google.com/projectcreate");
            step(ui, 2, "Enable the YouTube Data API v3 for that project.");
            link(app, ui, "api", "Open YouTube Data API v3", "https://console.cloud.google.com/apis/library/youtube.googleapis.com");
            step(ui, 3, "Set up the consent screen (Google Auth Platform > Branding / Audience): app name, your email, audience External. Add your own Google account under Test users.");
            step(ui, 4, "Under Audience, press Publish app (In production). In Testing, Google ends the sign-in every 7 days. Your own sign-in will show \"Google hasn't verified this app\": choose Advanced > Continue.");
            link(app, ui, "consent", "Open the consent screen", "https://console.cloud.google.com/auth/overview");
            step(ui, 5, "Clients > Create client > Application type: Desktop app > Create. Copy the Client ID and Client secret.");
            link(app, ui, "clients", "Open Clients", "https://console.cloud.google.com/auth/clients");
            step(ui, 6, "Paste both below, press Save, then Connect YouTube to this workspace and allow access with the channel's account (pick the brand channel if asked).");
            ui.label(
                RichText::new("\"Access blocked … can only be used within its organization\" (Error 403: org_internal): the consent screen's audience is Internal. In Audience, press Make external (In production), then connect again. \"Error 400: redirect_uri_mismatch\": the client is a Web application; create a Desktop app client and paste its ID and secret. \"has not completed the Google verification process\" (Error 403: access_denied): the app is in Testing; in Audience press Publish app.")
                    .small()
                    .color(t.text_dim),
            );
            ui.label(
                RichText::new("Shorts upload as Public (Google no longer keeps uploads from unaudited projects private). Google's default quota allows about six uploads a day per project; the YouTube API audit form asks for more.")
                    .small()
                    .color(t.text_dim),
            );
            link(app, ui, "audit", "Open the YouTube API audit / quota form", "https://support.google.com/youtube/contact/yt_api_form");
        });
        app.auto.add_response("reel.youtube.guide", &guide.header_response, "How to get the Google client");
        env_fields(app, ui, &[("YOUTUBE_CLIENT_ID", "Google client ID", false), ("YOUTUBE_CLIENT_SECRET", "Google client secret", true)]);
        let connecting = rt().tasks.iter().any(|(l, _)| l == "Connect YouTube");
        let b = ui.add_enabled(channel.is_some() && !connecting, egui::Button::new(if connecting { "Waiting for Google…" } else { "Connect YouTube to this workspace" }));
        app.auto.add_response("reel.youtube.connect", &b, "Connect YouTube");
        if connecting {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Finish signing in in your browser (pick the channel, e.g. your JKR brand channel).").small().color(t.text_dim));
                let cancel = ui.small_button("Cancel");
                app.auto.add_response("reel.youtube.cancel", &cancel, "Cancel YouTube sign-in");
                if cancel.clicked()
                    && let Some(flag) = rt().youtube_cancel.take()
                {
                    flag.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            });
        }
        if b.clicked()
            && let Some(ch) = channel
        {
            match filmcraft_studio::youtube_connect::start(s, ch) {
                Ok(pending) => {
                    let url = pending.url.clone();
                    rt().youtube_cancel = Some(pending.cancel.clone());
                    crate::links::open(app, ui.ctx(), &url);
                    let st = s.clone();
                    spawn(&mut rt(), "Connect YouTube", move || {
                        Done::Notice(match filmcraft_studio::youtube_connect::finish(&st, pending) {
                            Ok(name) => (format!("YouTube connected: {name}. Tick it in Publish to post Shorts too."), true),
                            Err(e) if e.to_string() == "cancelled" => ("YouTube sign-in cancelled".into(), false),
                            Err(e) => (format!("YouTube was not connected: {e}"), false),
                        })
                    });
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
    });
    let accounts = rt().accounts.clone();
    if accounts.is_empty() {
        ui.label("No accounts for this workspace yet. Connect YouTube above; Instagram accounts come from the earlier app's Accounts page.");
    }
    for a in accounts {
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("{} · {}", a.platform, a.display_name)).strong());
            ui.label(RichText::new(format!("Status: {}", a.status.to_lowercase())).small());
            if a.platform == "INSTAGRAM" {
                let info = rt().token_info.get(&a.id).cloned();
                let (text, colour) = match &info {
                    Some(Ok(text)) if text.starts_with("valid") => (format!("Token {text}"), t.text_dim),
                    Some(Ok(text)) => (format!("Token {text}"), t.danger),
                    Some(Err(e)) => (format!("Token: {e}"), t.danger),
                    None => ("Token expiry: press Check expiry".into(), t.text_dim),
                };
                ui.label(RichText::new(text).small().color(colour));
                ui.horizontal(|ui| {
                    let check = ui.small_button("Check expiry");
                    app.auto.add_response(&format!("reel.accountTokenInfo.{}", a.id), &check, "Check token expiry");
                    if check.clicked() {
                        token_check(s, a.id, true);
                    }
                    let extend = ui.small_button("Extend token").on_hover_text(
                        "Makes the token last: about 60 days, or never expiring when the Instagram account belongs to a Facebook Page. Needs META_APP_ID and META_APP_SECRET (Cloud tab). Only works on a token that has not expired yet.",
                    );
                    app.auto.add_response(&format!("reel.accountExtend.{}", a.id), &extend, "Extend token");
                    if extend.clicked() {
                        let (st, id) = (s.clone(), a.id);
                        spawn(&mut rt(), "Extend token", move || {
                            Done::Token(id, st.extend_account_token(id).map(|i| format!("{} (extended)", i.describe())).map_err(|e| e.to_string()), true)
                        });
                    }
                });
            }
            ui.horizontal(|ui| {
                let test = ui.button("Test");
                app.auto.add_response(&format!("reel.accountTest.{}", a.id), &test, "Test account connection");
                if test.clicked() {
                    let st = s.clone();
                    let id = a.id;
                    let mut r = rt();
                    spawn(&mut r, "Account test", move || {
                        Done::Notice(match st.test_account(id, true) {
                            Ok(name) => (format!("{name}: the key works"), true),
                            Err(e) => (e.to_string(), false),
                        })
                    });
                }
                let draft = app.ui.reel.token_drafts.entry(a.id).or_default();
                let input = ui.add(egui::TextEdit::singleline(draft).password(true).hint_text("Paste a new access token").desired_width(240.0));
                app.auto.add_response(&format!("reel.accountToken.{}", a.id), &input, "New access token");
                let save = ui.add_enabled(!draft.trim().is_empty(), egui::Button::new("Save token"));
                app.auto.add_response(&format!("reel.accountSaveToken.{}", a.id), &save, "Save account token");
                if save.clicked() {
                    app.ui.status = match s.set_account_token(a.id, draft) {
                        Ok(()) => {
                            // Show the new token's expiry straight away.
                            if a.platform == "INSTAGRAM" {
                                token_check(s, a.id, true);
                            }
                            "Token saved; checking it with Meta…".into()
                        }
                        Err(e) => e.to_string(),
                    };
                    draft.clear();
                    if let Some(c) = channel {
                        load_channel(&mut rt(), c);
                    }
                }
            });
        });
    }
}

/// Reads a local account's token expiry from Meta on a worker.
fn token_check(s: &Studio, id: i64, tell: bool) {
    let st = s.clone();
    spawn(&mut rt(), "Token expiry", move || Done::Token(id, st.token_info(id).map(|i| i.describe()).map_err(|e| e.to_string()), tell));
}

fn settings(app: &mut FilmcraftApp, ui: &mut egui::Ui, channel: Option<i64>, s: &Studio, brand: &Brand) {
    ui.horizontal(|ui| {
        ui.label("AI provider");
        let current = filmcraft_studio::ai::provider(s);
        let mut selected = current.clone();
        let combo = egui::ComboBox::from_id_salt("reel-ai-provider").selected_text(&current).show_ui(ui, |ui| {
            ui.selectable_value(&mut selected, "hermes".into(), "Hermes");
            ui.selectable_value(&mut selected, "codex".into(), "Codex (this app)");
            ui.selectable_value(&mut selected, "ollama".into(), "Ollama");
        });
        app.auto.add_response("reel.ai.provider", &combo.response, "AI provider (all workspaces)");
        if selected != current {
            app.ui.status = match s.set_ai_provider(&selected) {
                Ok(()) => format!("AI provider set to {selected} for all workspaces"),
                Err(e) => e.to_string(),
            };
        }
    });
    section(ui, "Brand", true, |ui| {
        let edit = app.ui.reel.brand_edit.get_or_insert_with(|| brand.clone());
        let field_width = (ui.available_width() - 125.0).max(80.0);
        egui::Grid::new("reel-brand-fields").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
            for (id, label, value) in [
                ("handle", "Social handle", &mut edit.social_handle),
                ("website", "Website", &mut edit.website),
                ("contact", "Contact line", &mut edit.contact_line),
                ("font", "Font family", &mut edit.font),
                ("primary", "Accent colour", &mut edit.primary_color),
                ("secondary", "Secondary colour", &mut edit.secondary_color),
                ("background", "Background", &mut edit.background),
                ("card", "Card background", &mut edit.card_background),
                ("band", "Band colour", &mut edit.band_color),
                ("text", "Card text colour", &mut edit.card_text_color),
            ] {
                ui.label(label);
                ui.horizontal(|ui| {
                    let is_color = matches!(id, "primary" | "secondary" | "background" | "card" | "band" | "text");
                    if is_color {
                        let color = filmcraft_reel::color(value, [0.5, 0.5, 0.5, 1.0]);
                        let mut rgb = [color[0], color[1], color[2]].map(|v| (v * 255.0).round() as u8);
                        let picker = egui::color_picker::color_edit_button_srgb(ui, &mut rgb);
                        app.auto.add_response(&format!("reel.brand.{id}.picker"), &picker, label);
                        if picker.changed() {
                            *value = format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
                        }
                    }
                    let width = if is_color { field_width - 35.0 } else { field_width };
                    let response = ui.add(egui::TextEdit::singleline(value).desired_width(width.max(45.0)));
                    app.auto.add_response(&format!("reel.brand.{id}"), &response, label);
                });
                ui.end_row();
            }
        });
        ui.label("Headline size");
        let response = ui.add(egui::Slider::new(&mut edit.card_title_size, 18.0..=140.0));
        app.auto.add_response("reel.brand.titleSize", &response, "Headline size");
        let response = ui.checkbox(&mut edit.verified, "Show verified mark");
        app.auto.add_response("reel.brand.verified", &response, "Show verified mark");
        egui::ComboBox::from_id_salt("reel-brand-template").selected_text(&edit.default_layout).show_ui(ui, |ui| {
            for tpl in TEMPLATES {
                ui.selectable_value(&mut edit.default_layout, tpl.id.to_string(), tpl.name);
            }
        });
        let b = ui.add_enabled(channel.is_some(), egui::Button::new("Save workspace brand"));
        app.auto.add_response("reel.brand.save", &b, "Save workspace brand");
        if b.clicked()
            && let Some(ch) = channel
        {
            match s.save_brand(ch, edit) {
                Ok(()) => {
                    load_channel(&mut rt(), ch);
                    app.ui.status = "Workspace brand saved. Apply it to the current Reel when ready.".into();
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
        let b = ui.add_enabled(reel_info(app).is_some(), egui::Button::new("Apply saved brand to this Reel"));
        app.auto.add_response("reel.brand.apply", &b, "Apply saved brand to this Reel");
        if b.clicked()
            && let Some(ch) = channel
        {
            match s.brand(ch) {
                Ok(brand) => {
                    if let Err(e) = app.session.execute("reel.setBrand", json!({"brand": brand, "channelId": ch})) {
                        app.ui.status = e.to_string();
                    }
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
    });
    section(ui, "New workspace", false, |ui| {
        ui.label("Workspace name");
        let r = ui.add(egui::TextEdit::singleline(&mut app.ui.reel.workspace_name).desired_width(f32::INFINITY));
        app.auto.add_response("reel.workspace.name", &r, "Workspace name");
        ui.label("Folder slug (for example my-channel)");
        let r = ui.add(egui::TextEdit::singleline(&mut app.ui.reel.workspace_slug).desired_width(f32::INFINITY));
        app.auto.add_response("reel.workspace.slug", &r, "Workspace slug");
        let b = ui.button("Create workspace");
        app.auto.add_response("reel.workspace.create", &b, "Create workspace");
        if b.clicked() {
            match s.create_workspace(app.ui.reel.workspace_slug.trim(), &app.ui.reel.workspace_name) {
                Ok(ch) => {
                    let mut r = rt();
                    match s.channels() {
                        Ok(channels) => r.channels = channels,
                        Err(e) => log(&mut r, e.to_string()),
                    }
                    app.ui.reel.workspace_name.clear();
                    app.ui.reel.workspace_slug.clear();
                    log(&mut r, format!("Created {}. Select it in the workspace picker.", ch.name));
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
    });
}

fn templates(app: &mut FilmcraftApp, ui: &mut egui::Ui, info: Option<&ReelInfo>) {
    let t = app.tokens;
    if info.is_none() {
        ui.label(RichText::new("Start a Reel first (a workspace clip above, or Reel ▸ New Reel).").small().color(t.text_dim));
    }
    if let Some(selected) = info.and_then(|i| TEMPLATES.iter().find(|tpl| tpl.id == i.template)) {
        ui.label(RichText::new(format!("{} · {}", selected.name, selected.note)).small().color(t.text_dim));
    }
    for family in ["Card", "Design"] {
        ui.label(RichText::new(if family == "Card" { "Card layouts" } else { "Designs" }).small().color(t.text_dim));
        let columns = ((ui.available_width() / 110.0) as usize).clamp(1, 6);
        let entries: Vec<_> = TEMPLATES.iter().filter(|x| x.family == family).collect();
        for row in entries.chunks(columns) {
            ui.horizontal(|ui| {
                for tpl in row {
                    let on = info.is_some_and(|i| i.template == tpl.id);
                    let brand = info.map(|i| i.brand.clone()).unwrap_or_default();
                    let preview = filmcraft_reel::plan(tpl.id, &brand, &app.ui.reel.card, 8.0).ok();
                    let b = ui
                        .vertical(|ui| {
                            ui.set_width(104.0);
                            let (rect, _) = ui.allocate_exact_size(vec2(92.0, 142.0), egui::Sense::hover());
                            let scale = 132.0 / 1920.0;
                            let frame = egui::Rect::from_center_size(rect.center(), vec2(1080.0 * scale, 132.0));
                            ui.painter().rect_filled(frame, 3.0, egui::Color32::from_rgb(16, 19, 26));
                            if let Some(plan) = &preview {
                                let slot = egui::Rect::from_min_size(
                                    frame.min + vec2(plan.slot.x as f32, plan.slot.y as f32) * scale,
                                    vec2(plan.slot.w as f32, plan.slot.h as f32) * scale,
                                );
                                ui.painter().rect_filled(slot, 2.0, egui::Color32::from_rgb(49, 91, 113));
                                ui.painter().text(slot.center(), egui::Align2::CENTER_CENTER, "VIDEO", egui::FontId::proportional(8.0), egui::Color32::WHITE);
                                for control in &plan.controls {
                                    let Some(layer) = plan.over.iter().find(|l| l.layer.as_ref().is_some_and(|l| l.uid == control.layer)) else { continue };
                                    let Some(filmcraft_project::ParamValue::Vec2(p)) = layer.params.get("position").map(|p| &p.value) else { continue };
                                    let (label, size) = match control.id.as_str() {
                                        "title" => ("HEADLINE", 6.0),
                                        "caption" | "point1" => ("Supporting text", 4.5),
                                        "handle" | "channel" => ("Brand", 4.0),
                                        _ => continue,
                                    };
                                    let at = frame.min + vec2(p.x as f32, p.y as f32) * scale;
                                    let painter = ui.painter().with_clip_rect(frame);
                                    painter.rect_filled(
                                        egui::Rect::from_min_size(at - vec2(1.0, 1.0), vec2(58.0, size + 3.0)),
                                        1.0,
                                        egui::Color32::from_black_alpha(200),
                                    );
                                    painter.text(at, egui::Align2::LEFT_TOP, label, egui::FontId::proportional(size), egui::Color32::WHITE);
                                }
                            }
                            ui.add_enabled(info.is_some(), egui::Button::new(RichText::new(tpl.name).small()).selected(on))
                        })
                        .inner
                        .on_hover_text(tpl.note);
                    if ui.is_rect_visible(b.rect) {
                        app.auto.add_response(&format!("reel.template.{}", tpl.id), &b, tpl.name);
                    }
                    if b.clicked()
                        && let Err(e) = app.session.execute("reel.apply", json!({"template": tpl.id}))
                    {
                        app.ui.status = e.to_string();
                    }
                }
            });
        }
    }
}

fn card(app: &mut FilmcraftApp, ui: &mut egui::Ui, is_reel: bool, part: CardPart) {
    let t = app.tokens;
    /// One card field; true when the user finished editing it (focus left).
    fn line(ui: &mut egui::Ui, app: &mut FilmcraftApp, id: &str, label: &str, multi: bool, pick: fn(&mut Card) -> &mut String) -> bool {
        ui.label(RichText::new(label).small().color(app.tokens.text_dim));
        let field = pick(&mut app.ui.reel.card);
        let r = if multi {
            ui.add(egui::TextEdit::multiline(field).desired_rows(2).desired_width(f32::INFINITY))
        } else {
            ui.add(egui::TextEdit::singleline(field).desired_width(f32::INFINITY))
        };
        app.auto.add_response(&format!("reel.card.{id}"), &r, label);
        // Typing applies a moment after it stops (see `card_typed_at`); leaving the field applies now.
        if r.changed() {
            rt().card_typed_at = Some(ui.input(|i| i.time));
        }
        r.lost_focus()
    }
    let mut changed = false;
    if part == CardPart::Card {
        let fonts_panel = egui::CollapsingHeader::new("Fonts & text styling").default_open(false).show(ui, |ui| {
            if !filmcraft_text::fonts::system_scanned() {
                filmcraft_text::fonts::scan_system();
            }
            let families = filmcraft_text::fonts::families();
            let info = reel_info(app).unwrap_or_default();
            let controls = filmcraft_reel::plan(&info.template, &info.brand, &app.ui.reel.card, 15.0).map(|p| p.controls).unwrap_or_default();
            ui.label(
                RichText::new("Standard: Inter Bold headlines · Inter Regular supporting text. Channel styling stays separate.").small().color(t.text_dim),
            );
            let reset = ui.button("Use standard text fonts");
            app.auto.add_response("reel.card.fonts.standard", &reset, "Use standard text fonts");
            if reset.clicked() {
                app.ui.reel.card.fonts.retain(|field, _| !matches!(field.as_str(), "title" | "caption" | "point1" | "point2" | "point3" | "point4"));
                app.ui.reel.card.text_styles.retain(|field, _| !matches!(field.as_str(), "title" | "caption" | "point1" | "point2" | "point3" | "point4"));
                changed = true;
            }
            let timing = egui::ComboBox::from_id_salt("reel-text-timing")
                .selected_text(match app.ui.reel.card.text_timing {
                    filmcraft_reel::TextTiming::Always => "Text throughout the video",
                    filmcraft_reel::TextTiming::IntroOutro => "Opening and closing only",
                })
                .show_ui(ui, |ui| {
                    for (value, label) in [
                        (filmcraft_reel::TextTiming::Always, "Text throughout the video"),
                        (filmcraft_reel::TextTiming::IntroOutro, "Opening and closing only"),
                    ] {
                        let response = ui.selectable_value(&mut app.ui.reel.card.text_timing, value, label);
                        changed |= response.changed();
                        app.auto.add_response(&format!("reel.card.timing.{value:?}"), &response, label);
                    }
                });
            app.auto.add_response("reel.card.timing", &timing.response, "Text visibility");
            for control in controls.iter().filter(|c| c.param == "text") {
                let (field, label) = (control.id.as_str(), control.name.as_str());
                let font = app.ui.reel.card.fonts.entry(field.into()).or_default();
                ui.label(RichText::new(label).small());
                let combo = egui::ComboBox::from_id_salt(("card-font", field))
                    .selected_text(if font.is_empty() { "Template default" } else { font.as_str() })
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        let default_font = ui.selectable_value(font, String::new(), "Template default");
                        app.auto.add_response(&format!("reel.card.font.{field}.default"), &default_font, "Template default");
                        changed |= default_font.changed();
                        for (family, _) in &families {
                            let b = ui.selectable_value(font, family.clone(), family);
                            app.auto.add_response(&format!("reel.card.font.{field}.{family}"), &b, family);
                            changed |= b.changed();
                        }
                    });
                app.auto.add_response(&format!("reel.card.font.{field}"), &combo.response, label);
                let styles = egui::CollapsingHeader::new(format!("{label} styling")).id_salt(("text-style", field)).show(ui, |ui| {
                    let style = app.ui.reel.card.text_styles.entry(field.into()).or_default();
                    ui.horizontal_wrapped(|ui| {
                        for (name, size) in [("Small", 32.0), ("Medium", 48.0), ("Large", 64.0), ("XL", 80.0)] {
                            let b = ui.selectable_label(style.size == Some(size), name);
                            app.auto.add_response(&format!("reel.card.style.{field}.preset.{name}"), &b, name);
                            if b.clicked() {
                                style.size = Some(size);
                                changed = true;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        let mut size = style.size.unwrap_or(if field == "title" { 56.0 } else { 32.0 });
                        ui.label("Size");
                        let r = ui.add(egui::DragValue::new(&mut size).range(16.0..=160.0).suffix(" px"));
                        app.auto.add_response(&format!("reel.card.style.{field}.size"), &r, "Text size");
                        if r.changed() {
                            style.size = Some(size);
                            changed = true;
                        }
                        let r = ui.small_button("Auto");
                        app.auto.add_response(&format!("reel.card.style.{field}.auto"), &r, "Template text size");
                        if r.clicked() {
                            style.size = None;
                            changed = true;
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        for (id, label, value, default) in
                            [("caps", "ALL CAPS", &mut style.uppercase, field == "title"), ("bold", "Bold", &mut style.bold, field == "title")]
                        {
                            let mut checked = value.unwrap_or(default);
                            let r = ui.checkbox(&mut checked, label);
                            app.auto.add_response(&format!("reel.card.style.{field}.{id}"), &r, label);
                            if r.changed() {
                                *value = Some(checked);
                                changed = true;
                            }
                        }
                        let r = ui.checkbox(&mut style.italic, "Italic");
                        app.auto.add_response(&format!("reel.card.style.{field}.italic"), &r, "Italic");
                        changed |= r.changed();
                    });
                    ui.horizontal(|ui| {
                        ui.label("Character limit");
                        let r = ui.add(egui::DragValue::new(&mut style.max_chars).range(0..=2000));
                        app.auto.add_response(&format!("reel.card.style.{field}.limit"), &r, "Character limit; zero is unlimited");
                        changed |= r.changed();
                    });
                    ui.label(RichText::new("0 = unlimited. Excess text is shortened in the video; your original text is kept.").small());
                    ui.horizontal(|ui| {
                        ui.label("Tracking");
                        let r = ui.add(egui::DragValue::new(&mut style.tracking).range(-100.0..=500.0));
                        app.auto.add_response(&format!("reel.card.style.{field}.tracking"), &r, "Letter spacing");
                        changed |= r.changed();
                        ui.label("Line gap");
                        let r = ui.add(egui::DragValue::new(&mut style.line_spacing).range(-10.0..=80.0));
                        app.auto.add_response(&format!("reel.card.style.{field}.leading"), &r, "Line spacing");
                        changed |= r.changed();
                    });
                    ui.horizontal_wrapped(|ui| {
                        for (align, label) in [(0, "Left"), (1, "Center"), (2, "Right")] {
                            let r = ui.selectable_label(style.align == Some(align), label);
                            app.auto.add_response(&format!("reel.card.style.{field}.align.{align}"), &r, label);
                            if r.clicked() {
                                style.align = Some(align);
                                changed = true;
                            }
                        }
                    });
                });
                app.auto.add_response(&format!("reel.card.style.{field}"), &styles.header_response, label);
            }
            let add = ui.button("Add font…");
            app.auto.add_response("reel.card.addFont", &add, "Add font to brand assets");
            if add.clicked()
                && let Some(channel) = app.ui.reel.channel_id
            {
                app.pick_ui(
                    crate::native_dialogs::FileDialog::OpenFile { filter: "Font".into(), exts: vec!["ttf".into(), "otf".into(), "ttc".into()] },
                    move |app, paths| {
                        let Some(path) = paths.first() else { return };
                        let result = (|| -> Result<(), String> {
                            let source = std::path::Path::new(path);
                            if std::fs::metadata(source).map_err(|e| e.to_string())?.len() > 32 * 1024 * 1024 {
                                return Err("Choose a font smaller than 32 MB".into());
                            }
                            let data = std::fs::read(source).map_err(|e| e.to_string())?;
                            let faces = filmcraft_text::fonts::add_font_data(data);
                            if faces.is_empty() {
                                return Err("This file is not a readable font".into());
                            }
                            let mut r = rt();
                            let s = studio(&mut r).ok_or("Library is unavailable")?;
                            let dir = s.workspace_folder(channel).map_err(|e| e.to_string())?.join("assets/brand/fonts");
                            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                            let name = source.file_name().ok_or("The font has no file name")?;
                            let target = dir.join(name);
                            if target.exists() && std::fs::read(&target).map_err(|e| e.to_string())? != std::fs::read(source).map_err(|e| e.to_string())? {
                                return Err("A different font with this name is already in Brand Assets. Rename the file first.".into());
                            }
                            if !target.exists() {
                                std::fs::copy(source, target).map_err(|e| e.to_string())?;
                            }
                            Ok(())
                        })();
                        app.ui.status = match result {
                            Ok(()) => "Font added to Brand Assets. Choose it above.".into(),
                            Err(e) => e,
                        };
                    },
                );
            }
        });
        app.auto.add_response("reel.card.fonts", &fonts_panel.header_response, "Fonts");
        changed |= line(ui, app, "title", "Headline", true, |c| &mut c.title);
        changed |= line(ui, app, "caption", "Supporting line", true, |c| &mut c.caption);
        changed |= line(ui, app, "attribution", "Credit (Original: @creator)", false, |c| &mut c.attribution);
        section(ui, "Additional text", true, |ui| {
            changed |= line(ui, app, "badge", "Badge", false, |c| &mut c.badge);
            changed |= line(ui, app, "cta", "Closing call to action", false, |c| &mut c.cta);
            ui.label(RichText::new("Points (one per line)").small().color(t.text_dim));
            let mut points = app.ui.reel.card.bullets.join("\n");
            let pr = ui.add(egui::TextEdit::multiline(&mut points).desired_rows(2).desired_width(f32::INFINITY));
            if pr.changed() {
                app.ui.reel.card.bullets = points.lines().map(str::to_string).filter(|l| !l.trim().is_empty()).take(4).collect();
            }
            changed |= pr.lost_focus();
            app.auto.add_response("reel.card.points", &pr, "Points");
        });
    }
    if part == CardPart::Caption {
        section(ui, "Post caption and hashtags", true, |ui| {
            changed |= line(ui, app, "instagramCaption", "Instagram caption", true, |c| &mut c.instagram_caption);
            ui.label(RichText::new("Hashtags").small().color(t.text_dim));
            let mut tags = app.ui.reel.card.hashtags.join(" ");
            let tr = ui.add(egui::TextEdit::singleline(&mut tags).desired_width(f32::INFINITY));
            if tr.changed() {
                app.ui.reel.card.hashtags = tags.split_whitespace().map(str::to_string).collect();
            }
            changed |= tr.lost_focus();
            app.auto.add_response("reel.card.hashtags", &tr, "Hashtags");
        });
    }
    // Half a second after the last keystroke, what was typed (or cleared) goes onto the Reel.
    let now = ui.input(|i| i.time);
    let typed = rt().card_typed_at;
    if let Some(at) = typed {
        if now - at >= 0.5 {
            changed = true;
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(0.5 - (now - at)));
        }
    }
    if changed {
        rt().card_typed_at = None;
    }
    if changed && is_reel {
        let p = card_params(&app.ui.reel.card);
        if let Err(e) = app.session.execute("reel.setCard", p) {
            app.ui.status = e.to_string();
        }
    }
}

fn write_card(app: &mut FilmcraftApp, ui: &mut egui::Ui, channel: Option<i64>, s: &Studio) {
    let r = rt();
    let articles: Vec<Article> = r.articles.clone();
    drop(r);
    let chosen = app.ui.reel.article.and_then(|id| articles.iter().find(|a| a.id == id)).map(|a| a.title.clone()).unwrap_or_else(|| "From my notes".into());
    egui::ComboBox::from_id_salt("reel-article").selected_text(chosen).width(ui.available_width()).show_ui(ui, |ui| {
        ui.selectable_value(&mut app.ui.reel.article, None, "From my notes");
        for a in articles.iter().take(40) {
            ui.selectable_value(&mut app.ui.reel.article, Some(a.id), format!("{} — {}", a.title, a.site));
        }
    });
    if app.ui.reel.article.is_none() {
        ui.add(
            egui::TextEdit::multiline(&mut app.ui.reel.notes)
                .hint_text("What the clip shows, or the original post text")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
    }
    let busy = rt().tasks.iter().any(|(l, _)| l == "Write card");
    let b = ui.add_enabled(channel.is_some() && !busy, egui::Button::new(if busy { "Writing…" } else { "Write the card" }));
    app.auto.add_response("reel.write", &b, "Write the card");
    if b.clicked()
        && let Some(ch) = channel
    {
        let s = s.clone();
        let (article, notes) = (app.ui.reel.article, app.ui.reel.notes.clone());
        let target = CardTarget::capture(app);
        let mut r = rt();
        spawn(&mut r, "Write card", move || {
            Done::Card(
                (|| {
                    let (text, source) = match article {
                        Some(id) => {
                            let a = s.article(ch, id)?;
                            (format!("{}\n\n{}", a.title, a.text), if a.site.is_empty() { a.url } else { a.site })
                        }
                        None => (notes, "Uploaded video".to_string()),
                    };
                    filmcraft_studio::ai::write_card(&s, ch, &text, &source)
                })()
                .map(Box::new)
                .map_err(|e| e.to_string()),
                Box::new(target),
            )
        });
    }
}

fn render_actions(app: &mut FilmcraftApp, ui: &mut egui::Ui, p: &LibraryPost, channel: Option<i64>, s: &Studio) {
    let t = app.tokens;
    let Some(r) = p.render.clone() else { return };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("Render {} · {}", r.id, r.status.to_lowercase())).small());
        if !r.approved && ["RENDERED", "APPROVED"].contains(&r.status.as_str()) {
            let b = ui.small_button("Approve");
            app.auto.add_response(&format!("reel.approve.{}", r.id), &b, "Approve");
            if b.clicked() {
                let mut rr = rt();
                match s.approve(r.id) {
                    Ok(()) => log(&mut rr, format!("Approved render {}", r.id)),
                    Err(e) => log(&mut rr, format!("Approve failed: {e}")),
                }
                if let Some(c) = channel {
                    load_channel(&mut rr, c);
                }
            }
        }
        if r.approved && !app.ui.reel.accounts.is_empty() {
            let publishing = rt().tasks.iter().any(|(label, _)| label == "Publishing");
            let b = ui.add_enabled(!publishing, egui::Button::new(if publishing { "Publishing…" } else { "Publish now" }));
            app.auto.add_response(&format!("reel.publish.{}", r.id), &b, "Publish now");
            if b.clicked()
                && let Some(ch) = channel
            {
                let mut rr = rt();
                for a in app.ui.reel.accounts.clone() {
                    let st = s.clone();
                    let name = p.title.clone();
                    spawn(&mut rr, "Publishing", move || {
                        Done::Published(name, filmcraft_studio::publish_now(&st, ch, r.id, a).map_err(|e| e.to_string()), None)
                    });
                }
            }
            if ui.small_button("Schedule").clicked()
                && let Some(ch) = channel
            {
                let mut rr = rt();
                match filmcraft_studio::local_to_utc_iso(&app.ui.reel.schedule_at) {
                    // Instagram waits for its time; YouTube is uploaded now with that publish time.
                    Some(at) => {
                        let (st, accounts, name, render) = (s.clone(), app.ui.reel.accounts.clone(), p.title.clone(), r.id);
                        spawn(&mut rr, "Scheduling", move || {
                            Done::Scheduled(name, filmcraft_studio::schedule_everywhere(&st, ch, render, &accounts, &at).map_err(|e| e.to_string()), at, 0)
                        });
                    }
                    None => log(&mut rr, "Type the time as YYYY-MM-DD HH:MM"),
                }
            }
        }
        if let Some(path) = r.path.clone()
            && ui.small_button("Show file").clicked()
        {
            let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
        }
    });
    for (platform, status, link) in &p.published {
        ui.label(
            RichText::new(format!("{platform}: {}{}", status.to_lowercase(), link.as_deref().map(|l| format!(" · {l}")).unwrap_or_default()))
                .small()
                .color(t.text_dim),
        );
    }
}

fn library(app: &mut FilmcraftApp, ui: &mut egui::Ui, channel: Option<i64>, s: &Studio) {
    let t = app.tokens;
    ui.horizontal(|ui| {
        ui.heading("Library");
        let response = ui.checkbox(&mut app.ui.reel.show_archived, "Archived");
        app.auto.add_response("reel.library.archived", &response, "Show archived posts");
        let list = ui.selectable_value(&mut app.ui.reel.library_grid, false, "☰ List");
        app.auto.add_response("reel.library.viewList", &list, "List view");
        let grid = ui.selectable_value(&mut app.ui.reel.library_grid, true, "▦ Grid");
        app.auto.add_response("reel.library.viewGrid", &grid, "Grid view");
    });
    let last_archive = rt().archived.filter(|(ch, _)| Some(*ch) == channel);
    if let Some((ch, story)) = last_archive {
        ui.horizontal(|ui| {
            ui.label("Post archived.");
            let b = ui.small_button("Undo");
            app.auto.add_response("reel.library.undoArchive", &b, "Undo archive");
            if b.clicked() {
                match s.restore_post(ch, story) {
                    Ok(()) => {
                        let mut r = rt();
                        r.archived = None;
                        load_channel(&mut r, ch);
                    }
                    Err(e) => app.ui.status = e.to_string(),
                }
            }
        });
    }
    let query = ui.add(egui::TextEdit::singleline(&mut app.ui.reel.library_filter).hint_text("Search title, status or platform").desired_width(f32::INFINITY));
    app.auto.add_response("reel.library.search", &query, "Search library");
    let b = ui.button("Refresh library");
    app.auto.add_response("reel.library.refresh", &b, "Refresh library");
    if b.clicked()
        && let Some(ch) = channel
    {
        let mut runtime = rt();
        runtime.previews.clear();
        load_channel(&mut runtime, ch);
    }
    let filter = app.ui.reel.library_filter.to_lowercase();
    let r = rt();
    let posts: Vec<LibraryPost> = (if app.ui.reel.show_archived { &r.archived_posts } else { &r.library })
        .iter()
        .filter(|p| filter.is_empty() || format!("{} {} {} {:?}", p.title, p.status, p.kind, p.published).to_lowercase().contains(&filter))
        .cloned()
        .collect();
    drop(r);
    ui.label(RichText::new("Showing the latest 40 posts in this view.").small().color(t.text_dim));
    if posts.is_empty() {
        ui.label(RichText::new(if filter.is_empty() { "No posts in this workspace yet." } else { "No posts match your search." }).small().color(t.text_dim));
    }
    if app.ui.reel.library_grid {
        delivery::library_grid(app, ui, &posts, channel);
    } else {
        for p in posts {
            delivery::library_row(app, ui, &p, channel, s);
        }
    }
}

fn library_actions(app: &mut FilmcraftApp, ui: &mut egui::Ui, post: &LibraryPost, channel: i64, s: &Studio) {
    if app.ui.reel.show_archived {
        let b = ui.small_button("Restore");
        app.auto.add_response(&format!("reel.restore.{}", post.story_id), &b, "Restore post");
        if b.clicked() {
            match s.restore_post(channel, post.story_id) {
                Ok(()) => {
                    let mut r = rt();
                    if r.archived == Some((channel, post.story_id)) {
                        r.archived = None;
                    }
                    load_channel(&mut r, channel);
                }
                Err(e) => app.ui.status = e.to_string(),
            }
        }
        return;
    }
    ui.horizontal(|ui| {
        let project = post.render.as_ref().and_then(|r| r.path.as_ref()).map(|p| p.with_extension("fcproj")).filter(|p| p.is_file());
        let hint = if project.is_none() {
            "This older render has no saved desktop project"
        } else if app.session.is_dirty() {
            "Save your current project before opening another"
        } else {
            "Open the editable desktop project"
        };
        let b = ui.add_enabled(project.is_some() && !app.session.is_dirty(), egui::Button::new("Reopen")).on_hover_text(hint);
        app.auto.add_response(&format!("reel.reopen.{}", post.story_id), &b, "Reopen project");
        if b.clicked()
            && let Some(path) = project
        {
            match crate::menus::invoke(app, ui.ctx(), "file.open", json!({"path": path.to_string_lossy()})) {
                Ok(_) => {
                    if let Some(script) = post.script_id
                        && let Err(e) =
                            app.session.execute("reel.linkPost", json!({"post": PostLink {channel_id: channel, story_id: post.story_id, script_id: script}}))
                    {
                        app.ui.status = e.to_string();
                    }
                    app.ui.reel.loaded_for = None;
                    app.ui.reel.page = ReelPage::Design;
                }
                Err(e) => app.ui.status = e,
            }
        }
        let menu = ui.menu_button("More", |ui| {
            let rename = ui.button("Rename…");
            app.auto.add_response(&format!("reel.rename.{}", post.story_id), &rename, "Rename post");
            if rename.clicked() {
                app.ui.reel.rename_story = Some(post.story_id);
                app.ui.reel.rename_title = post.title.clone();
                ui.close();
            }
            let archive = ui.button("Archive").on_hover_text("Hide this post; keep its files and publication history. Restore it from Archived.");
            app.auto.add_response(&format!("reel.archive.{}", post.story_id), &archive, "Archive post");
            if archive.clicked() {
                match s.archive_post(channel, post.story_id) {
                    Ok(_) => {
                        let mut r = rt();
                        r.archived = Some((channel, post.story_id));
                        load_channel(&mut r, channel);
                    }
                    Err(e) => app.ui.status = e.to_string(),
                }
                ui.close();
            }
        });
        app.auto.add_response(&format!("reel.more.{}", post.story_id), &menu.response, "Post actions");
    });
    if app.ui.reel.rename_story == Some(post.story_id) {
        let response = ui.add(egui::TextEdit::singleline(&mut app.ui.reel.rename_title).desired_width(f32::INFINITY));
        app.auto.add_response("reel.library.name", &response, "Post name");
        ui.horizontal(|ui| {
            let save = ui.small_button("Save name");
            app.auto.add_response("reel.library.saveName", &save, "Save name");
            if save.clicked() {
                match s.rename_post(channel, post.story_id, &app.ui.reel.rename_title) {
                    Ok(()) => {
                        app.ui.reel.rename_story = None;
                        load_channel(&mut rt(), channel);
                    }
                    Err(e) => app.ui.status = e.to_string(),
                }
            }
            if ui.small_button("Cancel").clicked() {
                app.ui.reel.rename_story = None;
            }
        });
    }
}

fn imports(app: &mut FilmcraftApp, ui: &mut egui::Ui, channel: Option<i64>, s: &Studio) {
    let t = app.tokens;
    match filmcraft_studio::ytdlp::binary(s) {
        Some(p) => {
            ui.label(
                RichText::new(format!(
                    "yt-dlp ready · {}",
                    if filmcraft_studio::ytdlp::ffmpeg(s).is_some() { "video + audio merging available" } else { "single-file MP4" }
                ))
                .small()
                .color(t.text_dim),
            )
            .on_hover_text(p.display().to_string());
            let check = ui.small_button("Check downloader");
            app.auto.add_response("reel.downloads.check", &check, "Check downloader");
            if check.clicked() {
                let st = s.clone();
                spawn(&mut rt(), "Check downloader", move || {
                    Done::Message(match filmcraft_studio::ytdlp::version(&st) {
                        Ok(version) => format!("yt-dlp {version} is ready"),
                        Err(e) => e.to_string(),
                    })
                });
            }
        }
        None => {
            ui.label(RichText::new("yt-dlp is not installed.").small());
            let installing = rt().tasks.iter().any(|(label, _)| label == "Installing yt-dlp");
            let install = ui
                .add_enabled(!installing, egui::Button::new(if installing { "Installing…" } else { "Install yt-dlp" }))
                .on_hover_text("Downloads the latest official yt-dlp.exe from github.com/yt-dlp/yt-dlp into the library's tools folder");
            app.auto.add_response("reel.downloads.install", &install, "Install yt-dlp");
            if install.clicked() {
                let st = s.clone();
                let mut r = rt();
                spawn(&mut r, "Installing yt-dlp", move || {
                    Done::Message(match filmcraft_studio::ytdlp::install(&st) {
                        Ok(p) => match filmcraft_studio::ytdlp::version(&st) {
                            Ok(version) => format!("yt-dlp {version} ready at {}", p.display()),
                            Err(e) => format!("yt-dlp was downloaded but could not start: {e}"),
                        },
                        Err(e) => format!("yt-dlp install failed: {e}"),
                    })
                });
            }
        }
    }
    let links_input = ui
        .add(egui::TextEdit::multiline(&mut app.ui.reel.links).desired_rows(2).hint_text("Video links · one per line (up to 10)").desired_width(f32::INFINITY));
    app.auto.add_response("reel.downloads.links", &links_input, "Video links");
    let draft = ui.checkbox(&mut app.ui.reel.draft_downloads, "Write a card with AI after downloading");
    app.auto.add_response("reel.downloads.draft", &draft, "Draft a card after download");
    if app.ui.reel.draft_downloads {
        let notes = ui.add(egui::TextEdit::singleline(&mut app.ui.reel.link_notes).hint_text("Notes for the AI (optional)").desired_width(f32::INFINITY));
        app.auto.add_response("reel.downloads.notes", &notes, "Notes for AI");
    }
    let options = egui::CollapsingHeader::new("Download options").default_open(false).show(ui, |ui| {
        let o = &mut app.ui.reel.download;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Quality").small());
            let shown = match o.quality.as_str() {
                "1080" => "Up to 1080p",
                "720" => "Up to 720p",
                "480" => "Up to 480p",
                _ => "Best",
            };
            egui::ComboBox::from_id_salt("dl-quality").selected_text(shown).show_ui(ui, |ui| {
                for (v, l) in [("best", "Best"), ("1080", "Up to 1080p"), ("720", "Up to 720p"), ("480", "Up to 480p")] {
                    ui.selectable_value(&mut o.quality, v.to_string(), l);
                }
            });
            ui.label(RichText::new("Cookies").small());
            let shown = if o.cookies_from.is_empty() { "None".to_string() } else { o.cookies_from.clone() };
            egui::ComboBox::from_id_salt("dl-cookies").selected_text(shown).show_ui(ui, |ui| {
                for v in ["", "chrome", "edge", "firefox", "brave"] {
                    ui.selectable_value(&mut o.cookies_from, v.to_string(), if v.is_empty() { "None" } else { v });
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Section").small());
            ui.add(egui::TextEdit::singleline(&mut o.section_start).hint_text("start").desired_width(70.0));
            ui.add(egui::TextEdit::singleline(&mut o.section_end).hint_text("end").desired_width(70.0));
            ui.label(RichText::new("Rate limit").small());
            ui.add(egui::TextEdit::singleline(&mut o.rate_limit).hint_text("e.g. 5M").desired_width(60.0));
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("Format").small());
            ui.add(egui::TextEdit::singleline(&mut o.format).hint_text("yt-dlp -f expression (optional)").desired_width(f32::INFINITY));
        });
    });
    app.auto.add_response("reel.downloads.options", &options.header_response, "Download options");
    let running = rt().imports_running;
    let links: Vec<String> = app.ui.reel.links.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
    let b = ui.add_enabled(
        channel.is_some() && filmcraft_studio::ytdlp::binary(s).is_some() && !running && !links.is_empty() && links.len() <= 10,
        egui::Button::new(if running { "Downloading…" } else { "Download" }),
    );
    app.auto.add_response("reel.import", &b, "Download");
    if links.len() > 10 {
        ui.label(RichText::new("At most ten links per batch.").small().color(t.text_dim));
    }
    let start = |jobs: Vec<filmcraft_studio::ytdlp::Import>, ch: i64| {
        let mut r = rt();
        let list = r.imports.clone();
        {
            let mut l = list.lock().unwrap_or_else(PoisonError::into_inner);
            for j in &jobs {
                l.retain(|x| x.url != j.url);
                l.insert(0, j.clone());
            }
        }
        r.imports_running = true;
        let st = s.clone();
        spawn(&mut r, "Imports", move || {
            for job in jobs {
                let url = job.url.clone();
                let list2 = list.clone();
                let update = move |j: &filmcraft_studio::ytdlp::Import| {
                    let mut l = list2.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(x) = l.iter_mut().find(|x| x.url == url) {
                        *x = j.clone();
                    }
                };
                filmcraft_studio::ytdlp::run(&st, ch, job, &update);
            }
            Done::Message("__imports_done".into())
        });
    };
    if b.clicked()
        && let Some(ch) = channel
    {
        let notes = app.ui.reel.link_notes.clone();
        let options = app.ui.reel.download.clone();
        start(
            links
                .iter()
                .map(|u| filmcraft_studio::ytdlp::Import {
                    channel_id: Some(ch),
                    url: u.clone(),
                    notes: notes.clone(),
                    options: options.clone(),
                    no_draft: !app.ui.reel.draft_downloads,
                    save_to: None,
                    stage: "Waiting".into(),
                    ..Default::default()
                })
                .collect(),
            ch,
        );
        app.ui.reel.links.clear();
    }
    let list: Vec<filmcraft_studio::ytdlp::Import> = rt().imports.lock().unwrap_or_else(PoisonError::into_inner).clone();
    for (i, job) in list.iter().enumerate() {
        if job.channel_id != channel {
            continue;
        }
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(if job.title.is_empty() { job.url.as_str() } else { job.title.as_str() }).strong());
            ui.label(RichText::new(format!("{}{}", job.stage, if job.progress > 0.0 && job.progress < 100.0 { format!(" · {:.0}%", job.progress) } else { String::new() })).small().color(t.text_dim));
            if let Some(e) = &job.error {
                ui.label(RichText::new(e).small().color(egui::Color32::from_rgb(0xff, 0x6b, 0x6b)));
            }
            if let Some(file) = &job.file { ui.add(egui::Label::new(RichText::new(format!("Saved: {}", file.display())).small()).wrap()); }
            ui.horizontal_wrapped(|ui| {
                if let Some(file) = &job.file {
                    let preview = ui.small_button("Preview");
                    app.auto.add_response(&format!("reel.downloads.preview.{i}"), &preview, "Preview download");
                    if preview.clicked() {
                        rt().clip_preview = Some((file.clone(), job.title.clone(), 0.0));
                    }
                    let show = ui.small_button("Show in Create");
                    app.auto.add_response(&format!("reel.downloads.create.{i}"), &show, "Show download in Create");
                    if show.clicked() && let Some(ch) = channel {
                            match s.register_download(ch, file, &job.title, &job.url) {
                                Ok(_) => { load_channel(&mut rt(), ch); app.ui.reel.media_filter.clear(); app.ui.reel.hide_done_clips = false; app.show_panel(crate::PanelKind::Create); },
                                Err(e) => app.ui.status = e.to_string(),
                            }
                    }
                    let folder = ui.small_button("Open folder");
                    app.auto.add_response(&format!("reel.downloads.openFolder.{i}"), &folder, "Open downloaded file folder");
                    if folder.clicked() && let Some(parent) = file.parent() && let Err(e) = super::menu_dialogs::open_path(app, ui.ctx(), &parent.to_string_lossy(), false) { app.ui.status = e; }

                }
                if job.error.is_some() && !running {
                    if ui.small_button("Retry").clicked()
                        && let Some(ch) = channel
                    {
                        start(vec![job.clone()], ch);
                    }
                    if job.asset_id.is_none() && ui.small_button("Replace link").clicked() {
                        app.ui.reel.links = job.url.clone();
                        rt().imports.lock().unwrap_or_else(PoisonError::into_inner).remove(i);
                    }
                }
                if let (Some(file), Some(ch)) = (job.file.clone(), channel)
                    && ui.small_button("Open as Reel").clicked()
                {
                    let brand = rt().brand.clone().unwrap_or_default();
                    let card = job.card.clone().unwrap_or_default();
                    let p = json!({"name": job.title, "template": brand.default_layout, "brand": brand, "channelId": ch, "card": card, "paths": [file.to_string_lossy()]});
                    match app.session.execute("reel.new", p) {
                        Ok(_) => {
                            if let Some(post) = job.post {
                                let _ = app.session.execute("reel.linkPost", json!({"post": post}));
                            }
                            app.ui.reel.loaded_for = None;
                        }
                        Err(e) => app.ui.status = e.to_string(),
                    }
                }
            });
        });
    }
}

fn cloud(app: &mut FilmcraftApp, ui: &mut egui::Ui, info: Option<&ReelInfo>, channel: Option<i64>, s: &Studio, accounts_only: bool) {
    let t = app.tokens;
    if !filmcraft_studio::cloud::configured(s) {
        ui.label(RichText::new("Cloud publishing is not configured. It uses the CLOUD_SUPABASE_URL, CLOUD_SUPABASE_ANON_KEY, CLOUD_AUTH_EMAIL and CLOUD_AUTH_PASSWORD settings in the library's .env file.").small().color(t.text_dim));
        return;
    }
    let refresh = |s: &Studio| {
        let mut r = rt();
        r.cloud_loaded = true;
        let (a, b) = (s.clone(), s.clone());
        spawn(&mut r, "Cloud accounts", move || {
            Done::CloudAccounts(
                filmcraft_studio::cloud::call(&a, "accounts", json!({})).map(|v| v.as_array().cloned().unwrap_or_default()).map_err(|e| e.to_string()),
            )
        });
        spawn(&mut r, "Cloud schedules", move || {
            Done::CloudPosts(
                filmcraft_studio::cloud::call(&b, "listSchedules", json!({"offset": 0}))
                    .map(|v| v.get("items").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|i| i.get("post").cloned()).collect()).unwrap_or_default())
                    .map_err(|e| e.to_string()),
            )
        });
    };
    if !rt().cloud_loaded {
        refresh(s);
    }
    let (accounts, posts, error) = {
        let r = rt();
        (r.cloud_accounts.clone(), r.cloud_posts.clone(), r.cloud_error.clone())
    };
    if !error.is_empty() {
        ui.label(RichText::new(&error).small().color(egui::Color32::from_rgb(0xff, 0x6b, 0x6b)));
    }
    if ui.small_button("↻ Refresh cloud").clicked() {
        refresh(s);
    }
    ui.label(RichText::new("Instagram accounts in the cloud").small().color(t.text_dim));
    for a in &accounts {
        let id = a.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let name = a.get("account_name").and_then(|v| v.as_str()).unwrap_or("account").to_string();
        let expires = a.get("token_expires_at").and_then(|v| v.as_str());
        let (_, level) = filmcraft_studio::cloud::token_health(expires);
        let left = filmcraft_studio::cloud::expiry_text(expires);
        let ig_user = a.get("instagram_user_id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let colour = match level {
            0 => t.text_dim,
            1 | 2 => egui::Color32::from_rgb(0xf5, 0xb0, 0x41),
            _ => egui::Color32::from_rgb(0xff, 0x6b, 0x6b),
        };
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(&mut app.ui.reel.cloud_account, Some(id.clone()), format!("@{name}"));
            ui.label(RichText::new(left).small().color(colour));
            for (label, action, tip) in [
                ("Check", "instagramStatus", "Refresh this account's health"),
                ("Extend token", "extendInstagramToken", "Extend the Instagram user token through the backend"),
            ] {
                if ui.small_button(label).on_hover_text(tip).clicked() {
                    let (st, id) = (s.clone(), id.clone());
                    let mut r = rt();
                    r.cloud_loaded = false;
                    spawn(&mut r, "Account", move || {
                        Done::Notice(match filmcraft_studio::cloud::call(&st, action, json!({"id": id})) {
                            Ok(h) => (
                                format!("Cloud token valid {}", filmcraft_studio::cloud::expiry_text(h.get("expires").and_then(|v| v.as_str()))),
                                h.get("status").and_then(|v| v.as_str()) != Some("expired"),
                            ),
                            Err(e) => (
                                format!("Cloud account {label} failed: {e}. An expired token cannot be extended: paste a fresh one and press Replace token."),
                                false,
                            ),
                        })
                    });
                }
            }
        });
        let local = rt().accounts.iter().find(|local| local.platform == "INSTAGRAM" && local.external_id == ig_user).map(|local| local.id);
        if let Some(local) = local {
            let b = ui
                .button("Use this workspace's local token")
                .on_hover_text("Validates and copies the local credential to this same Instagram profile in the cloud. Local storage is unchanged.");
            app.auto.add_response(&format!("reel.cloudUseLocalToken.{id}"), &b, "Use the matching local Instagram token in cloud");
            if b.clicked() {
                let (st, cloud) = (s.clone(), id.clone());
                spawn(&mut rt(), "Cloud credential", move || {
                    Done::Notice(match filmcraft_studio::cloud::use_local_token(&st, local, &cloud) {
                        Ok(message) => (message, true),
                        Err(e) => (e.to_string(), false),
                    })
                });
            }
        }
        // A fresh token (Graph API Explorer) replaces the stored one and is extended at once.
        ui.horizontal(|ui| {
            let key = format!("cloud-token:{id}");
            let draft = app.ui.reel.env_drafts.entry(key).or_default();
            let input = ui.add(egui::TextEdit::singleline(draft).password(true).hint_text("Paste a fresh access token").desired_width(220.0));
            app.auto.add_response(&format!("reel.cloudToken.{id}"), &input, "Fresh cloud access token");
            let replace = ui
                .add_enabled(!draft.trim().is_empty(), egui::Button::new("Replace token"))
                .on_hover_text("Connect this account with a fresh token, then extend it to about 60 days");
            app.auto.add_response(&format!("reel.cloudReplace.{id}"), &replace, "Replace cloud token");
            if replace.clicked() {
                let token = std::mem::take(draft);
                let (st, id, ig, name) = (s.clone(), id.clone(), ig_user.clone(), name.clone());
                let mut r = rt();
                r.cloud_loaded = false;
                spawn(&mut r, "Replace token", move || {
                    Done::Notice(match filmcraft_studio::cloud::replace_token(&st, &id, &ig, &token) {
                        Ok(text) => (format!("Cloud @{name}: {text}"), !text.contains("failed")),
                        Err(e) => (format!("Cloud @{name}: replacing the token failed: {e}"), false),
                    })
                });
            }
        });
    }
    // Schedule the current Reel's approved render.
    if accounts_only {
        return;
    }
    let render = info.and_then(|i| i.post).and_then(|p| rt().library.iter().find(|x| x.story_id == p.story_id).and_then(|x| x.render.clone()));
    match (render, channel, app.ui.reel.cloud_account.clone()) {
        (Some(r), Some(ch), Some(account)) if r.approved => {
            ui.label(RichText::new(format!("Render {} at {} (local time)", r.id, app.ui.reel.schedule_at)).small());
            ui.horizontal(|ui| {
                for (label, now) in [("Schedule in cloud", false), ("Publish now via cloud", true)] {
                    if ui.small_button(label).clicked() {
                        let at = if now { Some(filmcraft_studio::now_iso()) } else { filmcraft_studio::local_to_utc_iso(&app.ui.reel.schedule_at) };
                        let Some(at) = at else {
                            app.ui.status = "Type the time as YYYY-MM-DD HH:MM".into();
                            continue;
                        };
                        let caption = s.caption_for(r.id).unwrap_or_default();
                        let (st, account, rid) = (s.clone(), account.clone(), r.id);
                        let mut rr = rt();
                        rr.cloud_loaded = false;
                        spawn(&mut rr, "Cloud scheduling", move || {
                            Done::Message(match filmcraft_studio::cloud::schedule(&st, ch, rid, &account, &caption, &at, now) {
                                Ok(v) if v.get("cloudConfirmed") == Some(&serde_json::Value::Bool(true)) => "Scheduled in cloud — safe to close the app".into(),
                                Ok(_) => "The cloud accepted the upload but has not confirmed the schedule yet; refresh to check".into(),
                                Err(e) => format!("Cloud scheduling failed (retry keeps the same request): {e}"),
                            })
                        });
                    }
                }
            });
        }
        _ => {
            ui.label(RichText::new("Send the Reel to the library, approve it and pick a cloud account to schedule it here.").small().color(t.text_dim));
        }
    }
    ui.label(RichText::new("Cloud schedules").small().color(t.text_dim));
    for post in posts.iter().take(30) {
        let id = post.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let status = post.get("status").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let when = post.get("scheduled_for").and_then(|v| v.as_str()).unwrap_or_default();
        let title = post.get("title").and_then(|v| v.as_str()).unwrap_or("Reel");
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{title} · {status} · {}", when.get(..16).unwrap_or(when))).small());
            if let Some(link) = post.get("instagram_permalink").and_then(|v| v.as_str()) {
                ui.hyperlink_to(RichText::new("open").small(), link);
            }
            let mut act = |label: &str, action: &'static str| {
                if ui.small_button(label).clicked() {
                    let (st, id) = (s.clone(), id.clone());
                    let mut r = rt();
                    r.cloud_loaded = false;
                    spawn(&mut r, "Cloud", move || {
                        Done::Message(match filmcraft_studio::cloud::call(&st, action, json!({"id": id})) {
                            Ok(_) => format!("Cloud: {action} done"),
                            Err(e) => format!("Cloud {action} failed: {e}"),
                        })
                    });
                }
            };
            match status.as_str() {
                "scheduled" => {
                    act("Publish now", "publishNow");
                    act("Cancel", "cancelPost");
                }
                "failed" => act("Retry", "retryPost"),
                "uploading" => act("Delete", "deleteDraft"),
                _ => {}
            }
        });
    }
}

/// The most urgent cloud token warning, for the banner at the top of the panel.
fn token_warning() -> Option<(String, u8)> {
    let r = rt();
    r.cloud_accounts
        .iter()
        .map(|a| {
            let name = a.get("account_name").and_then(|v| v.as_str()).unwrap_or("account").to_string();
            let (left, level) = filmcraft_studio::cloud::token_health(a.get("token_expires_at").and_then(|v| v.as_str()));
            (format!("@{name}: {left}"), level)
        })
        .filter(|(_, l)| *l > 0)
        .max_by_key(|(_, l)| *l)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_guard() -> std::sync::MutexGuard<'static, ()> {
        static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
        GUARD.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publish ▸ Schedule: cloud posts on their day, the day's actions, and scheduling the open
    /// Reel on a picked day — with no local publishing or Publish now, and no network (the cloud
    /// data is already loaded).
    fn schedule_frame(app: &mut FilmcraftApp, ctx: &egui::Context) {
        app.auto.begin_frame();
        let mut output =
            ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1500.0, 2400.0))), ..Default::default() }, |ui| {
                delivery::publish_page(app, ui, ui.available_rect_before_wrap());
            });
        output.textures_delta.clear();
    }

    #[test]
    fn cloud_schedule_lives_on_the_calendar() {
        use chrono::{Duration, Local};
        let _guard = test_guard();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.execute("reel.new", json!({"channelId": 1, "card": {"title": "Trailer"}})).unwrap();
        app.ui.reel.channel_id = Some(1);
        app.ui.reel.export_tab = ExportTab::Publish;
        app.ui.reel.publish_tab = PublishTab::Calendar;
        let tomorrow = Local::now().date_naive() + Duration::days(1);
        let day = tomorrow.format("%Y-%m-%d").to_string();
        let post_at = schedule::at_local(tomorrow, "09:15").unwrap();
        let expires = filmcraft_studio::iso_ms(filmcraft_studio::clock_ms(&filmcraft_studio::now_iso()).unwrap() + 30 * 86_400_000);
        let mut env = std::collections::HashMap::new();
        for k in ["CLOUD_SUPABASE_URL", "CLOUD_SUPABASE_ANON_KEY", "CLOUD_AUTH_EMAIL", "CLOUD_AUTH_PASSWORD"] {
            env.insert(k.to_string(), "test".to_string());
        }
        let studio = Studio { root: Default::default(), database: ":memory:".into(), storage: Default::default(), secrets: Default::default(), env };
        *rt() = Runtime {
            studio: Some(Ok(studio)),
            channels: vec![Channel { id: 1, slug: "qa".into(), name: "QA".into() }],
            loaded_channel: Some(1),
            brand: Some(Brand::default()),
            cloud_loaded: true,
            cloud_accounts: vec![json!({"id": "acc1", "account_name": "jkr_studio_", "token_expires_at": expires})],
            cloud_posts: vec![
                json!({"id": "p1", "title": "Earlier trailer", "status": "scheduled", "scheduled_for": post_at, "instagram_account_id": "acc1"}),
                json!({"id": "p2", "title": "Posted trailer", "status": "published", "scheduled_for": post_at, "instagram_account_id": "acc1"}),
            ],
            ..Default::default()
        };
        app.ui.reel.calendar_day = Some(day.clone());
        app.ui.reel.schedule_time = "18:00".into();
        let ctx = egui::Context::default();
        for _ in 0..2 {
            schedule_frame(&mut app, &ctx);
        }
        let find = |app: &FilmcraftApp, id: &str| app.auto.find(id).cloned();
        assert!(find(&app, &format!("reel.schedule.day.{day}")).is_some(), "the month shows the day");
        assert!(
            find(&app, "reel.schedule.cancel.p1").is_some() && find(&app, "reel.schedule.reschedule.p1").is_some(),
            "the day's post can be cancelled or rescheduled"
        );
        assert_eq!(app.ui.reel.cloud_account.as_deref(), Some("acc1"), "the only account is chosen");
        assert_eq!(find(&app, "reel.schedule.submit").and_then(|e| e.enabled), Some(true), "tomorrow 18:00 can be scheduled");
        // A finished post can be cleared from the calendar (hidden here only); a scheduled one cannot.
        assert!(find(&app, "reel.schedule.clear.p1").is_none());
        assert!(find(&app, "reel.schedule.clear.p2").is_some());
        app.ui.reel.cleared_posts.push("p2".into());
        schedule_frame(&mut app, &ctx);
        schedule_frame(&mut app, &ctx);
        assert!(find(&app, "reel.schedule.cancel.p1").is_some() && find(&app, "reel.schedule.clear.p2").is_none(), "the cleared post is hidden");
        app.ui.reel.show_cleared = true;
        schedule_frame(&mut app, &ctx);
        schedule_frame(&mut app, &ctx);
        assert!(find(&app, "reel.schedule.unclear.p2").is_some(), "Show cleared brings it back");
        app.ui.reel.show_cleared = false;
        // The Publish tab keeps local publishing and Publish now.
        app.ui.reel.publish_tab = PublishTab::Now;
        schedule_frame(&mut app, &ctx);
        schedule_frame(&mut app, &ctx);
        assert!(
            find(&app, "reel.publish.delivery.local").is_some() && find(&app, "reel.publish.renderNow").is_some(),
            "local publishing and Publish now are there"
        );
        app.ui.reel.publish_tab = PublishTab::Calendar;
        schedule_frame(&mut app, &ctx);
        schedule_frame(&mut app, &ctx);
        // A day in the past cannot be booked.
        app.ui.reel.calendar_day = Some((Local::now().date_naive() - Duration::days(1)).format("%Y-%m-%d").to_string());
        schedule_frame(&mut app, &ctx);
        schedule_frame(&mut app, &ctx);
        assert_eq!(find(&app, "reel.schedule.submit").and_then(|e| e.enabled), Some(false));
        // Nor a time after the account's token expires.
        app.ui.reel.calendar_day = Some((Local::now().date_naive() + Duration::days(40)).format("%Y-%m-%d").to_string());
        app.ui.reel.calendar_month = None;
        schedule_frame(&mut app, &ctx);
        schedule_frame(&mut app, &ctx);
        assert_eq!(find(&app, "reel.schedule.submit").and_then(|e| e.enabled), Some(false), "the token expires first");
        *rt() = Runtime::default();
    }

    #[test]
    fn a_library_result_cannot_link_another_project_with_the_same_sequence_id() {
        let _guard = test_guard();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.execute("reel.new", json!({})).unwrap();
        let sequence = app.session.state.active_sequence.unwrap().0;
        let source = app.session.project.clone();
        app.session.execute("reel.setCard", json!({"title": "An edit while rendering"})).unwrap();
        assert!(project_is_open(&app, &source), "ordinary edits retain the originating project");
        app.session.undo();
        assert!(project_is_open(&app, &source), "undo retains the originating project");
        app.session = filmcraft_engine::Session::default();
        app.session.execute("reel.new", json!({})).unwrap();
        assert_eq!(app.session.state.active_sequence.unwrap().0, sequence);
        assert!(!project_is_open(&app, &source));
        let (tx, rx) = channel();
        assert!(tx.send(Done::Sent(Ok((PostLink { channel_id: 1, story_id: 91, script_id: 92 }, 93)), sequence, source, None)).is_ok());
        *rt() = Runtime { studio: Some(Err("Isolated test library".into())), tasks: vec![("Send to library".into(), rx)], ..Default::default() };
        poll(&mut app, 0.0);
        assert!(reel_info(&app).unwrap().post.is_none(), "the unrelated Reel must remain unlinked");
        *rt() = Runtime::default();
    }

    #[test]
    fn ai_review_cannot_make_reels_using_another_workspaces_brand() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let before = app.session.project.clone();
        let ctx = Ctx {
            s: Studio {
                root: Default::default(),
                database: ":memory:".into(),
                storage: Default::default(),
                secrets: Default::default(),
                env: Default::default(),
            },
            info: None,
            channel: Some(2),
            brand: Brand::default(),
        };
        let run = ai::Run { channel: 1, ..Default::default() };
        assert_eq!(ai::make_reels(&mut app, &ctx, &run), 0);
        assert!(std::sync::Arc::ptr_eq(&before, &app.session.project));
        assert!(app.ui.status.contains("workspace"));
    }

    #[test]
    fn opening_account_settings_loads_the_workspace_without_the_reel_studio_panel() {
        let _guard = test_guard();
        let root = std::env::temp_dir().join(format!("shorts-settings-first-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let database = root.join("accounts.db");
        let _ = std::fs::remove_file(&database);
        let studio = Studio { database, storage: root.clone(), secrets: root.clone(), root: root.clone(), env: Default::default() };
        studio.connect().unwrap().execute_batch(
            "CREATE TABLE channels (id INTEGER, slug TEXT, name TEXT, enabled INTEGER);
             CREATE TABLE channel_settings (channel_id INTEGER, json TEXT);
             CREATE TABLE media_assets (id INTEGER, label TEXT, asset_type TEXT, file_path TEXT, duration REAL, width INTEGER, height INTEGER, created_at TEXT, channel_id INTEGER, tags TEXT DEFAULT '[]', source TEXT);
             CREATE TABLE articles (id INTEGER, title TEXT, site_name TEXT, url TEXT, text TEXT, channel_id INTEGER);
             CREATE TABLE social_accounts (id INTEGER, platform TEXT, display_name TEXT, external_account_id TEXT, enabled INTEGER, status TEXT, credentials_reference TEXT, channel_id INTEGER);
             INSERT INTO channels VALUES (1, 'qa', 'QA settings workspace', 1);
             INSERT INTO social_accounts VALUES (7, 'INSTAGRAM', 'QA account', '1784', 1, 'UNVERIFIED', 'qa.json', 1);"
        ).unwrap();
        *rt() = Runtime { studio: Some(Ok(studio)), ..Default::default() };
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.ui.reel.plugin_open = true;
        app.ui.reel.plugin_tab = PluginTab::Accounts;
        let ctx = egui::Context::default();
        for _ in 0..2 {
            app.auto.begin_frame();
            let mut output = ctx
                .run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(1000.0, 800.0))), ..Default::default() }, |_ui| {
                    plugin_settings(&mut app, &ctx)
                });
            output.textures_delta.clear();
        }
        assert_eq!(app.ui.reel.channel_id, Some(1));
        assert!(app.auto.find("reel.settings.workspace").is_some());
        assert!(app.auto.find("reel.accountTest.7").is_some());
        assert!(app.auto.find("reel.accountToken.7").is_some());
        assert!(app.auto.find("reel.accountSaveToken.7").is_some());
        *rt() = Runtime::default();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ai_results_only_match_the_original_unedited_reel() {
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.execute("reel.new", json!({})).unwrap();
        app.ui.reel.channel_id = Some(1);
        let target = CardTarget::capture(&app);
        assert!(target.matches(&app));
        app.ui.reel.card.title = "Manual edit".into();
        assert!(!target.matches(&app));
        app.ui.reel.card = target.card.clone();
        app.ui.reel.channel_id = Some(2);
        assert!(!target.matches(&app));
        app.ui.reel.channel_id = Some(1);
        app.session.state.active_sequence = None;
        assert!(!target.matches(&app));
        app.session.state.active_sequence = target.sequence;
        assert!(target.matches(&app));
        app.session.execute("reel.setCard", json!({"title": "Changed in the editor"})).unwrap();
        assert!(!target.matches(&app));
    }

    #[test]
    fn design_tabs_stay_visible_while_the_tab_scrolls() {
        let _guard = test_guard();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.ui.reel.channel_id = Some(1);
        app.ui.reel.design_tab = DesignTab::Templates;
        app.session.execute("reel.new", json!({"template": "quoteCard"})).unwrap();
        let studio =
            Studio { root: Default::default(), database: ":memory:".into(), storage: Default::default(), secrets: Default::default(), env: Default::default() };
        *rt() = Runtime {
            studio: Some(Ok(studio)),
            channels: vec![Channel { id: 1, slug: "qa".into(), name: "QA".into() }],
            loaded_channel: Some(1),
            brand: Some(Brand::default()),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut top = None;
        for frame in 0..4 {
            app.auto.begin_frame();
            let mut events = vec![];
            if frame >= 2 {
                events.push(egui::Event::PointerMoved(egui::pos2(150.0, 250.0)));
                events.push(egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, -600.0),
                    modifiers: Default::default(),
                    phase: egui::TouchPhase::Move,
                });
            }
            let mut output = ctx.run_ui(
                egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(420.0, 380.0))), events, ..Default::default() },
                |ui| {
                    show_design(&mut app, ui, ui.available_rect_before_wrap());
                },
            );
            output.textures_delta.clear();
            let tab = app.auto.elements.iter().find(|e| e.id == "reel.design.templates").unwrap();
            if frame == 1 {
                top = Some(tab.rect);
            }
            if frame >= 2 {
                assert_eq!(Some(tab.rect), top);
            }
            assert!(tab.rect[1] >= 0.0 && tab.rect[1] + tab.rect[3] < 200.0);
        }
        *rt() = Runtime::default();
    }

    #[test]
    fn offline_library_keeps_template_controls_enabled_for_an_open_reel() {
        let _guard = test_guard();
        let mut app = FilmcraftApp::new(filmcraft_engine::Session::default());
        app.session.execute("reel.new", json!({"template": "quoteCard"})).unwrap();
        // Never read or schedule anything in the user's actual library.
        *rt() = Runtime { studio: Some(Err("Offline test library".into())), ..Default::default() };
        let ctx = egui::Context::default();
        for _ in 0..2 {
            app.auto.begin_frame();
            let mut output =
                ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(330.0, 5000.0))), ..Default::default() }, |ui| {
                    show_design(&mut app, ui, ui.available_rect_before_wrap());
                });
            output.textures_delta.clear();
        }
        for tpl in TEMPLATES {
            let element = app.auto.find(&format!("reel.template.{}", tpl.id)).unwrap();
            assert_eq!(element.enabled, Some(true), "{}", tpl.id);
            assert!(element.rect[0] + element.rect[2] <= 330.0, "{} leaves the docked panel", tpl.id);
        }
        *rt() = Runtime::default();
    }
}
