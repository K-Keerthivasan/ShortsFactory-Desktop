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
    /// yt-dlp download options.
    pub download: filmcraft_studio::ytdlp::Options,
    /// The cloud Instagram account (Supabase id) schedules go to.
    pub cloud_account: Option<String>,
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
}

struct QueueItem {
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
    CloudAccounts(Result<Vec<serde_json::Value>, String>),
    CloudPosts(Result<Vec<serde_json::Value>, String>),
    Card(Result<Box<Card>, String>, Box<CardTarget>),
    /// The library hand-off finished, for this sequence (a Reel).
    Sent(Result<(PostLink, i64), String>, u64),
    Message(String),
    Health(filmcraft_studio::settings::HealthReport),
    /// A publication finished: the Reel's name, the link or the error, the queue item.
    Published(String, Result<Option<String>, String>, Option<u64>),
    Notice((String, bool)),
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
}

/// Library data and background work. Not UI state: it is rebuilt from the library on demand.
#[derive(Default)]
struct Runtime {
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
    /// The queue item whose render is on its way to the library.
    sent_queue: Option<u64>,
    /// The AI panel's links, from download to the parts chosen.
    ai_runs: ai::Runs,
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
    let Some(s) = studio(r) else { return };
    let result = (|| -> filmcraft_studio::Result<()> {
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
            Done::Card(Ok(card), target) => {
                if !target.matches(app) {
                    log(&mut r, "AI writing finished, but the Reel or its text changed. Run Write again on the current Reel.");
                    continue;
                }
                let keep = app.ui.reel.card.clone();
                app.ui.reel.card = Card { badge: keep.badge, bullets: keep.bullets, cta: keep.cta, ..*card };
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
            Done::Notice((text, ok)) => notify(&mut r, text, ok),
            Done::Sent(Ok((link, render)), sequence) => {
                // The post links to the Reel that was rendered, wherever the editor is now.
                if app.session.project.item(filmcraft_project::ItemId(sequence)).is_some()
                    && let Err(e) = app.session.execute("reel.linkPost", json!({"post": link, "sequence": sequence}))
                {
                    log(&mut r, format!("Could not link the post: {e}"));
                }
                log(&mut r, format!("Sent to the library as post {} (render {render})", link.story_id));
                if let Some(id) = r.sent_queue.take() {
                    queue_sent(&mut r, id, render);
                }
                if let Some(c) = r.loaded_channel {
                    load_channel(&mut r, c);
                }
            }
            Done::Sent(Err(e), _) => log(&mut r, format!("{label} failed: {e}")),
            Done::CloudAccounts(Ok(a)) => {
                r.cloud_accounts = a;
                r.cloud_error.clear();
            }
            Done::CloudPosts(Ok(p)) => {
                r.cloud_posts = p;
                r.cloud_error.clear();
            }
            Done::CloudAccounts(Err(e)) | Done::CloudPosts(Err(e)) => r.cloud_error = e,
            Done::Message(m) if m == "__imports_done" => r.imports_running = false,
            Done::Message(m) => {
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
                r.sent_queue = pending.queue;
                let thumb = pending.path.with_extension("png");
                if app.session.state.active_sequence.map(|q| q.0) == Some(pending.sequence) {
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
                            )
                        });
                    }
                    _ => log(&mut r, "Export finished, but the project is not a Reel of a workspace"),
                }
            }
            Some((true, Some(Err(e)))) => {
                r.export = None;
                log(&mut r, format!("Export failed: {e}"));
            }
            Some((true, None)) => {
                r.export = None;
                log(&mut r, "The export stopped");
            }
            Some((false, _)) => {}
        }
    }
    // Scheduled publications that are due (when the earlier app's worker is not running).
    if !r.due_running && now - r.last_due_check > 60.0 {
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
    advance_queue(app, &mut r);
    r.log.retain(|l| !l.is_empty());
}

fn card_params(c: &Card) -> serde_json::Value {
    json!({"title": c.title, "caption": c.caption, "attribution": c.attribution, "badge": c.badge, "bullets": c.bullets, "cta": c.cta, "instagramCaption": c.instagram_caption, "hashtags": c.hashtags})
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

fn activity(app: &FilmcraftApp, ui: &mut egui::Ui) {
    let t = app.tokens;
    let r = rt();
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
        section(ui, "Project footage", true, |ui| {
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
            let b =
                ui.add_enabled(ctx.channel.is_some() && !r.tasks.iter().any(|(label, _)| label == "Scan media"), egui::Button::new("Refresh workspace folder"));
            app.auto.add_response("reel.media.scan", &b, "Refresh workspace folder");
            if b.clicked()
                && let Some(ch) = ctx.channel
            {
                let st = ctx.s.clone();
                spawn(&mut r, "Scan media", move || {
                    Done::Message(match st.scan_media(ch) {
                        Ok((added, unreadable)) => format!("Scan finished: {added} new videos; {unreadable} unreadable files skipped"),
                        Err(e) => format!("Scan failed: {e}"),
                    })
                });
            }
            ui.add(egui::TextEdit::singleline(&mut app.ui.reel.media_filter).hint_text("Search clips").desired_width(f32::INFINITY));
            let filter = app.ui.reel.media_filter.to_lowercase();
            let clips: Vec<MediaRow> =
                r.media.iter().filter(|m| m.kind == "VIDEO" && (filter.is_empty() || m.label.to_lowercase().contains(&filter))).take(60).cloned().collect();
            if clips.is_empty() {
                ui.label(
                    RichText::new("No clips in this workspace yet. Download some, or drop files in its clips folder and refresh.").small().color(t.text_dim),
                );
            }
            for m in clips {
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
                    let preview = ui
                        .add_sized(vec2(ui.available_width().max(1.0), 22.0), egui::Button::new(&m.label).frame(false).wrap_mode(egui::TextWrapMode::Truncate))
                        .on_hover_text(format!("Preview in the Source monitor · {} · {}", m.duration.map(|d| format!("{d:.1}s")).unwrap_or_default(), dims));
                    if shown {
                        app.auto.add_response(&format!("reel.preview.{}", m.id), &preview, "Preview source");
                    }
                    if preview.clicked() {
                        let existing = app.session.project.items.values().find_map(|item| match &item.as_media()?.media {
                            filmcraft_project::MediaRef::File { path } if std::path::Path::new(path) == m.path => Some(item.id.0),
                            _ => None,
                        });
                        let result = (|| -> Result<(), String> {
                            let item = match existing {
                                Some(id) => id,
                                None => app
                                    .session
                                    .execute("file.import", json!({"paths": [m.path.to_string_lossy()]}))
                                    .map_err(|e| e.to_string())?
                                    .get("items")
                                    .and_then(|v| v.as_array())
                                    .and_then(|a| a.first())
                                    .and_then(|v| v.as_u64())
                                    .ok_or("The source was not imported")?,
                            };
                            app.session.execute("source.open", json!({"item": item})).map_err(|e| e.to_string())?;
                            app.show_panel(crate::PanelKind::Source);
                            Ok(())
                        })();
                        if let Err(e) = result {
                            log(&mut r, format!("Could not preview the clip: {e}"));
                        }
                    }
                    if b.clicked() {
                        r.pending_start = Some((m.label.clone(), m.path.clone()));
                    }
                });
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
        let folder =
            settings.download_dir.clone().map(|d| d.display().to_string()).unwrap_or_else(|| "the workspace's clips folder (added to its library)".into());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("Saved to: {folder}")).small().color(t.text_dim));
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
            if settings.download_dir.is_some() && ui.small_button("Use workspace folder").clicked() {
                let mut a = settings.clone();
                a.download_dir = None;
                if let Err(e) = ctx.s.save_app_settings(&a) {
                    app.ui.status = e.to_string();
                }
            }
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
            ui.painter().rect_filled(rest, app.tokens.radius, app.tokens.panel_bg);
            frame(ui, rest, "sf-library", |ui| {
                let Some(ctx) = prelude(app, ui) else { return };
                workspace_line(app, ui, &ctx);
                library(app, ui, ctx.channel, &ctx.s);
            });
            None
        }
        ExportTab::Publish => {
            ui.painter().rect_filled(rest, app.tokens.radius, app.tokens.panel_bg);
            frame(ui, rest, "sf-publish", |ui| {
                let Some(ctx) = prelude(app, ui) else { return };
                workspace_line(app, ui, &ctx);
                let mut tab = app.ui.reel.publish_tab;
                tab_bar(
                    app,
                    ui,
                    &mut tab,
                    "reel.publish",
                    &[
                        (PublishTab::Now, "now", "Publish"),
                        (PublishTab::Queue, "queue", "Queue"),
                        (PublishTab::Calendar, "calendar", "Calendar"),
                        (PublishTab::Cloud, "cloud", "Cloud"),
                    ],
                );
                app.ui.reel.publish_tab = tab;
                match tab {
                    PublishTab::Now => deliver(app, ui, ctx.info.as_ref(), ctx.channel, &ctx.s),
                    PublishTab::Queue => queue_view(app, ui, &ctx),
                    PublishTab::Calendar => calendar(app, ui, &ctx),
                    PublishTab::Cloud => cloud(app, ui, ctx.info.as_ref(), ctx.channel, &ctx.s),
                }
                ui.separator();
                activity(app, ui);
            });
            None
        }
    }
}

// ------------------------------------------------------------------------------------------------
// Queue: render, send to the library, publish or schedule — one Reel after another
// ------------------------------------------------------------------------------------------------

/// Adds the open Reel to the render-and-publish queue.
fn enqueue(app: &mut FilmcraftApp, info: &ReelInfo, action: QueueAction) {
    let Some(seq) = app.session.state.active_sequence else { return };
    let name = app.session.project.item(seq).map(|i| i.name.clone()).unwrap_or_else(|| "Reel".into());
    let item = QueueItem {
        id: crate::panels::reel::next_id(),
        sequence: seq.0,
        name,
        info: info.clone(),
        footage: footage(app, info),
        duration: app.session.active_sequence().map_or(0.0, |q| q.duration().seconds()),
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
    // The same item: an id from another project names something else (`created` tells them apart).
    let same_file = app.session.project.item(seq).map(|i| i.created) == q.project.item(seq).map(|i| i.created);
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
    let dir = s.root.join("Editor").join("exports");
    let _ = std::fs::create_dir_all(&dir);
    let safe: String = q.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
    let path = dir.join(format!("{safe}-{}.mp4", filmcraft_studio::now_iso().replace([':', '.'], "-")));
    let preset = if q.info.format == "post" { "Match Source – Adaptive High Bitrate" } else { "Social Vertical 1080×1920" };
    match app.session.execute("export.quick", json!({"preset": preset, "path": path.to_string_lossy(), "range": "entire", "sequence": q.sequence})) {
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

/// A queue item reached the library: publish or schedule it.
fn queue_sent(r: &mut Runtime, id: u64, render: i64) {
    let Some(s) = studio(r) else { return };
    let Some(q) = r.queue.iter_mut().find(|q| q.id == id) else { return };
    q.render = Some(render);
    let Some(ch) = q.info.channel_id else { return };
    match q.action.clone() {
        QueueAction::LibraryOnly => q.status = "In the library".into(),
        QueueAction::Schedule(accounts, at) => match s.schedule(ch, render, &accounts, &at) {
            Ok(n) => {
                q.status = format!("Scheduled on {n} account(s)");
                notify(r, format!("Scheduled: {}", r.queue.iter().find(|q| q.id == id).map(|q| q.name.clone()).unwrap_or_default()), true);
            }
            Err(e) => {
                q.status = "Failed".into();
                q.error = Some(e.to_string());
            }
        },
        QueueAction::PublishNow(accounts) => {
            q.status = "Publishing".into();
            let name = q.name.clone();
            for a in accounts {
                let st = s.clone();
                let name = name.clone();
                spawn(r, "Publishing", move || Done::Published(name, filmcraft_studio::publish_now(&st, ch, render, a).map_err(|e| e.to_string()), Some(id)));
            }
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
    ui.label(RichText::new("Publish queue").strong());
    let now = filmcraft_studio::now_iso();
    match ctx.s.calendar(ctx.channel, &now, "9999-12-31T00:00:00.000Z") {
        Ok(entries) if entries.is_empty() => {
            ui.label(RichText::new("No upcoming publications.").small().color(t.text_dim));
        }
        Ok(entries) => {
            for e in entries.iter().take(40) {
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
// Calendar
// ------------------------------------------------------------------------------------------------

fn calendar(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &Ctx) {
    use chrono::{Datelike, Local, NaiveDate, TimeZone};
    let t = app.tokens;
    let today = Local::now().date_naive();
    let (mut y, mut m) = app.ui.reel.calendar_month.unwrap_or((today.year(), today.month()));
    ui.horizontal(|ui| {
        if ui.small_button("◀").clicked() {
            (y, m) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
        }
        ui.label(
            RichText::new(format!(
                "{} {y}",
                ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"]
                    .get(m as usize - 1)
                    .copied()
                    .unwrap_or("")
            ))
            .strong(),
        );
        if ui.small_button("▶").clicked() {
            (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        }
        if ui.small_button("Today").clicked() {
            (y, m) = (today.year(), today.month());
        }
        ui.checkbox(&mut app.ui.reel.calendar_all, "All workspaces");
    });
    app.ui.reel.calendar_month = Some((y, m));
    let Some(first) = NaiveDate::from_ymd_opt(y, m, 1) else { return };
    let next = if m == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1) } else { NaiveDate::from_ymd_opt(y, m + 1, 1) };
    let Some(next) = next else { return };
    let to_iso = |d: NaiveDate| {
        d.and_hms_opt(0, 0, 0)
            .and_then(|n| Local.from_local_datetime(&n).earliest())
            .map(|l| filmcraft_studio::iso_ms(l.timestamp_millis()))
            .unwrap_or_default()
    };
    let channel = if app.ui.reel.calendar_all { None } else { ctx.channel };
    let mut entries = ctx.s.calendar(channel, &to_iso(first), &to_iso(next)).unwrap_or_default();
    // Cloud schedules (all accounts of the Supabase user) join the local ones.
    for p in rt().cloud_posts.iter() {
        let at = p.get("scheduled_for").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        if at.is_empty() {
            continue;
        }
        entries.push(filmcraft_studio::settings::CalendarEntry {
            at,
            channel: 0,
            title: p.get("title").and_then(|v| v.as_str()).unwrap_or("Reel").to_string(),
            platform: "INSTAGRAM".into(),
            status: p.get("status").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            link: p.get("instagram_permalink").and_then(|v| v.as_str()).map(str::to_string),
            render: 0,
            cloud: true,
        });
    }
    let by_day = |d: NaiveDate| -> Vec<&filmcraft_studio::settings::CalendarEntry> {
        entries.iter().filter(|e| local_time(&e.at).get(..10) == Some(d.format("%Y-%m-%d").to_string().as_str())).collect()
    };
    let width = ((ui.available_width() - 6.0 * 4.0) / 7.0).max(60.0);
    egui::Grid::new("sf-calendar").spacing([4.0, 4.0]).show(ui, |ui| {
        for d in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
            ui.label(RichText::new(d).small().color(t.text_dim));
        }
        ui.end_row();
        let lead = first.weekday().num_days_from_monday();
        for _ in 0..lead {
            ui.label("");
        }
        let mut col = lead;
        let mut day = first;
        while day < next {
            let items = by_day(day);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(width);
                ui.set_min_height(64.0);
                let label = RichText::new(day.day().to_string()).small();
                ui.label(if day == today { label.strong().color(t.accent) } else { label });
                for e in items.iter().take(3) {
                    let colour = match e.status.as_str() {
                        "PUBLISHED" | "published" => egui::Color32::from_rgb(0x4a, 0xde, 0x80),
                        "PUBLISH_FAILED" | "failed" | "NEEDS_RECONCILIATION" => egui::Color32::from_rgb(0xff, 0x6b, 0x6b),
                        _ => t.text,
                    };
                    let text = format!("{} {}{}", local_time(&e.at).get(11..).unwrap_or(""), if e.cloud { "☁ " } else { "" }, e.title);
                    ui.add(egui::Label::new(RichText::new(text).small().color(colour)).truncate()).on_hover_text(format!(
                        "{} · {} · {}",
                        e.title,
                        e.platform.to_lowercase(),
                        e.status.to_lowercase()
                    ));
                }
                if items.len() > 3 {
                    ui.label(RichText::new(format!("+{} more", items.len() - 3)).small().color(t.text_dim));
                }
            });
            col += 1;
            if col == 7 {
                ui.end_row();
                col = 0;
            }
            day = day.succ_opt().unwrap_or(next);
        }
    });
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
pub fn overlays(app: &mut FilmcraftApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    // The start-up check, once per run.
    {
        let mut r = rt();
        if !r.startup_checked {
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
                        if ui.small_button("✕").clicked() {
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
            let mut r = rt();
            let Some(s) = studio(&mut r) else {
                ui.label("The Shorts Factory library was not found.");
                return;
            };
            let brand = r.brand.clone().unwrap_or_default();
            let channel = app.ui.reel.channel_id;
            drop(r);
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
                                            "Logo saved. Re-apply a template to use it on the open Reel.".into()
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
                        ],
                    );
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
    let accounts = rt().accounts.clone();
    if accounts.is_empty() {
        ui.label("No accounts for this workspace yet. Connect them with the earlier app's Accounts page; adding new ones here is planned.");
    }
    for a in accounts {
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(format!("{} · {}", a.platform, a.display_name)).strong());
            ui.label(RichText::new(format!("Status: {}", a.status.to_lowercase())).small());
            ui.horizontal(|ui| {
                if ui.button("Test").clicked() {
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
                ui.add(egui::TextEdit::singleline(draft).password(true).hint_text("Paste a new access token").desired_width(240.0));
                if ui.add_enabled(!draft.trim().is_empty(), egui::Button::new("Save token")).clicked() {
                    app.ui.status = match s.set_account_token(a.id, draft) {
                        Ok(()) => "Token saved; press Test to check it".into(),
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
                            let (rect, _) = ui.allocate_exact_size(vec2(92.0, 110.0), egui::Sense::hover());
                            let frame = egui::Rect::from_center_size(rect.center(), vec2(56.25, 100.0));
                            ui.painter().rect_filled(frame, 3.0, egui::Color32::from_rgb(16, 19, 26));
                            if let Some(plan) = &preview {
                                let slot = egui::Rect::from_min_size(
                                    frame.min + vec2(plan.slot.x as f32, plan.slot.y as f32) * (100.0 / 1920.0),
                                    vec2(plan.slot.w as f32, plan.slot.h as f32) * (100.0 / 1920.0),
                                );
                                ui.painter().rect_filled(slot, 2.0, egui::Color32::from_rgb(49, 91, 113));
                                ui.painter().text(slot.center(), egui::Align2::CENTER_CENTER, "VIDEO", egui::FontId::proportional(8.0), egui::Color32::WHITE);
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
        r.lost_focus()
    }
    let mut changed = false;
    if part == CardPart::Card {
        changed |= line(ui, app, "title", "Headline", true, |c| &mut c.title);
        changed |= line(ui, app, "caption", "Supporting line", false, |c| &mut c.caption);
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

fn deliver(app: &mut FilmcraftApp, ui: &mut egui::Ui, info: Option<&ReelInfo>, channel: Option<i64>, s: &Studio) {
    let t = app.tokens;
    let Some(info) = info else {
        ui.label(RichText::new("Start a Reel to export and publish it.").small().color(t.text_dim));
        return;
    };
    if let Some(p) = info.post {
        ui.label(RichText::new(format!("Library post {} · version {}", p.story_id, p.script_id)).small().color(t.text_dim));
    }
    ui.checkbox(&mut app.ui.reel.approve_on_send, "Approve for publishing when sent");
    let exporting = rt().export.is_some();
    let b = ui.add_enabled(!exporting && info.channel_id.is_some(), egui::Button::new(if exporting { "Exporting…" } else { "Export and send to the library" }));
    app.auto.add_response("reel.export", &b, "Export and send to the library");
    if info.channel_id.is_none() {
        ui.label(RichText::new("Choose the workspace above first.").small().color(t.text_dim));
    }
    if b.clicked() {
        let dir = s.root.join("Editor").join("exports");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            app.ui.status = format!("Could not create the export folder: {e}");
            return;
        }
        let safe: String = app.session.project.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
        let path = dir.join(format!("{safe}-{}.mp4", filmcraft_studio::now_iso().replace([':', '.'], "-")));
        if let Err(e) = app.session.execute("file.saveCopy", json!({"path": path.with_extension("fcproj").to_string_lossy()})) {
            app.ui.status = format!("Could not save the editable project: {e}");
            return;
        }
        let preset = if info.format == "post" { "Match Source – Adaptive High Bitrate" } else { "Social Vertical 1080×1920" };
        match app.session.execute("export.quick", json!({"preset": preset, "path": path.to_string_lossy(), "range": "entire"})) {
            Ok(r) => match r["job"].as_u64() {
                Some(job) => {
                    rt().export = Some(PendingExport {
                        job,
                        path,
                        approve: app.ui.reel.approve_on_send,
                        info: info.clone(),
                        footage: footage(app, info),
                        duration: app.session.active_sequence().map_or(0.0, |q| q.duration().seconds()),
                        queue: None,
                        sequence: app.session.state.active_sequence.map_or(0, |q| q.0),
                    })
                }
                None => app.ui.status = "The export did not start".into(),
            },
            Err(e) => app.ui.status = e.to_string(),
        }
    }
    // Accounts and schedule time for the publish buttons in the library list.
    let accounts: Vec<Account> = rt().accounts.clone();
    ui.label(RichText::new("Publish on").small().color(t.text_dim));
    for a in &accounts {
        let mut on = app.ui.reel.accounts.contains(&a.id);
        let label = format!("{} · {} ({})", a.platform, a.display_name, a.status.to_lowercase());
        if ui.add_enabled(a.enabled, egui::Checkbox::new(&mut on, label)).changed() {
            app.ui.reel.accounts.retain(|x| *x != a.id);
            if on {
                app.ui.reel.accounts.push(a.id);
            }
        }
    }
    if accounts.is_empty() {
        ui.label(RichText::new("No publishing accounts are connected for this workspace.").small().color(t.text_dim));
    }
    if app.ui.reel.schedule_at.is_empty() {
        app.ui.reel.schedule_at = filmcraft_studio::local_in(60);
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Schedule at").small().color(t.text_dim));
        ui.add(egui::TextEdit::singleline(&mut app.ui.reel.schedule_at).desired_width(140.0).hint_text("YYYY-MM-DD HH:MM"));
    });
    // The Reel's own post, with its newest render.
    let post = info.post.and_then(|p| rt().library.iter().find(|x| x.story_id == p.story_id).cloned());
    if let Some(p) = post {
        render_actions(app, ui, &p, channel, s);
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
            let b = ui.small_button("Publish now");
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
                    Some(at) => match s.schedule(ch, r.id, &app.ui.reel.accounts, &at) {
                        Ok(n) => log(&mut rr, format!("Scheduled on {n} account(s) for {}", app.ui.reel.schedule_at)),
                        Err(e) => log(&mut rr, format!("Scheduling failed: {e}")),
                    },
                    None => log(&mut rr, "Type the time as YYYY-MM-DD HH:MM"),
                }
                load_channel(&mut rr, ch);
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
        load_channel(&mut rt(), ch);
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
    for p in posts {
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(&p.title).strong());
            ui.label(
                RichText::new(format!("{} · {} · {}", p.kind.to_lowercase(), p.status.to_lowercase(), p.created_at.get(..10).unwrap_or("")))
                    .small()
                    .color(t.text_dim),
            );
            if let Some(ch) = channel {
                library_actions(app, ui, &p, ch, s);
            }
            if !app.ui.reel.show_archived {
                render_actions(app, ui, &p, channel, s);
            }
        });
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
    ui.label(
        RichText::new("Up to ten links, one per line. Each is downloaded with yt-dlp, checked, added to this workspace and drafted by the local AI.")
            .small()
            .color(t.text_dim),
    );
    match filmcraft_studio::ytdlp::binary(s) {
        Some(p) => {
            ui.label(
                RichText::new(format!(
                    "yt-dlp: {}{}",
                    p.display(),
                    if filmcraft_studio::ytdlp::ffmpeg(s).is_some() { " · FFmpeg found (best video + audio)" } else { " · no FFmpeg: single-file MP4 only" }
                ))
                .small()
                .color(t.text_dim),
            );
        }
        None => {
            ui.label(RichText::new("yt-dlp is not installed.").small());
            if ui
                .small_button("Install yt-dlp")
                .on_hover_text("Downloads the latest official yt-dlp.exe from github.com/yt-dlp/yt-dlp into the library's tools folder")
                .clicked()
            {
                let st = s.clone();
                let mut r = rt();
                spawn(&mut r, "Installing yt-dlp", move || {
                    Done::Message(match filmcraft_studio::ytdlp::install(&st) {
                        Ok(p) => format!("yt-dlp installed at {}", p.display()),
                        Err(e) => format!("yt-dlp install failed: {e}"),
                    })
                });
            }
        }
    }
    ui.add(egui::TextEdit::multiline(&mut app.ui.reel.links).desired_rows(3).hint_text("https://…").desired_width(f32::INFINITY));
    ui.add(egui::TextEdit::singleline(&mut app.ui.reel.link_notes).hint_text("Notes for the AI (optional)").desired_width(f32::INFINITY));
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
    let running = rt().imports_running;
    let links: Vec<String> = app.ui.reel.links.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
    let b = ui.add_enabled(
        channel.is_some() && !running && !links.is_empty() && links.len() <= 10,
        egui::Button::new(if running { "Importing…" } else { "Download and draft" }),
    );
    app.auto.add_response("reel.import", &b, "Download and draft");
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
                    url: u.clone(),
                    notes: notes.clone(),
                    options: options.clone(),
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
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(if job.title.is_empty() { job.url.as_str() } else { job.title.as_str() }).strong());
            ui.label(RichText::new(format!("{}{}", job.stage, if job.progress > 0.0 && job.progress < 100.0 { format!(" · {:.0}%", job.progress) } else { String::new() })).small().color(t.text_dim));
            if let Some(e) = &job.error {
                ui.label(RichText::new(e).small().color(egui::Color32::from_rgb(0xff, 0x6b, 0x6b)));
            }
            ui.horizontal(|ui| {
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

fn cloud(app: &mut FilmcraftApp, ui: &mut egui::Ui, info: Option<&ReelInfo>, channel: Option<i64>, s: &Studio) {
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
        let (left, level) = filmcraft_studio::cloud::token_health(a.get("token_expires_at").and_then(|v| v.as_str()));
        let colour = match level {
            0 => t.text_dim,
            1 | 2 => egui::Color32::from_rgb(0xf5, 0xb0, 0x41),
            _ => egui::Color32::from_rgb(0xff, 0x6b, 0x6b),
        };
        ui.horizontal(|ui| {
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
                        Done::Message(match filmcraft_studio::cloud::call(&st, action, json!({"id": id})) {
                            Ok(_) => format!("Cloud account: {label} done"),
                            Err(e) => format!("Cloud account {label} failed: {e}"),
                        })
                    });
                }
            }
        });
    }
    // Schedule the current Reel's approved render.
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
