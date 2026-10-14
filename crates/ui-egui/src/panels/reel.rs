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
    /// Video links for yt-dlp, one per line.
    pub links: String,
    pub link_notes: String,
    /// yt-dlp download options.
    pub download: filmcraft_studio::ytdlp::Options,
    /// The cloud Instagram account (Supabase id) schedules go to.
    pub cloud_account: Option<String>,
    /// The card was loaded from this project's Reel info (so a project switch reloads it).
    pub loaded_for: Option<String>,
}

enum Done {
    CloudAccounts(Result<Vec<serde_json::Value>, String>),
    CloudPosts(Result<Vec<serde_json::Value>, String>),
    Card(Result<Card, String>),
    Sent(Result<(PostLink, i64), String>),
    Message(String),
}

struct PendingExport {
    job: u64,
    path: std::path::PathBuf,
    approve: bool,
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
            Done::Card(Ok(card)) => {
                let keep = app.ui.reel.card.clone();
                app.ui.reel.card = Card { badge: keep.badge, bullets: keep.bullets, cta: keep.cta, ..card };
                log(&mut r, "The AI wrote the card");
                if reel_info(app).is_some() {
                    let c = app.ui.reel.card.clone();
                    if let Err(e) = app.session.execute("reel.setCard", card_params(&c)) {
                        log(&mut r, format!("Card not applied: {e}"));
                    }
                }
            }
            Done::Card(Err(e)) => log(&mut r, format!("AI writing failed: {e}")),
            Done::Sent(Ok((link, render))) => {
                if let Err(e) = app.session.execute("reel.linkPost", json!({"post": link})) {
                    log(&mut r, format!("Could not link the post: {e}"));
                }
                log(&mut r, format!("Sent to the library as post {} (render {render})", link.story_id));
                if let Some(c) = r.loaded_channel {
                    load_channel(&mut r, c);
                }
            }
            Done::Sent(Err(e)) => log(&mut r, format!("{label} failed: {e}")),
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
                log(&mut r, m);
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
                let _ = app.session.execute("file.exportFrame", json!({"path": thumb.to_string_lossy(), "format": "png", "time": filmcraft_reel::secs(1.0).0}));
                match (reel_info(app), studio(&mut r)) {
                    (Some(info), Some(s)) => {
                        let foot = footage(app, &info);
                        let duration = app.session.active_sequence().map_or(0.0, |q| q.duration().seconds());
                        log(&mut r, "Export finished; sending to the library…");
                        spawn(&mut r, "Send to library", move || {
                            let f = foot.as_ref().map(|(p, a, b)| (p.as_path(), *a, *b));
                            Done::Sent(
                                s.send_to_library(&info, &pending.path, thumb.is_file().then_some(thumb.as_path()), f, duration, pending.approve)
                                    .map_err(|e| e.to_string()),
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
    r.log.retain(|l| !l.is_empty());
}

fn card_params(c: &Card) -> serde_json::Value {
    json!({"title": c.title, "caption": c.caption, "attribution": c.attribution, "badge": c.badge, "bullets": c.bullets, "cta": c.cta, "instagramCaption": c.instagram_caption, "hashtags": c.hashtags})
}

fn section(ui: &mut egui::Ui, title: &str, open: bool, body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(RichText::new(title).strong()).default_open(open).show(ui, body);
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let t = app.tokens;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(10.0, 8.0))));
    let ui = &mut child;
    egui::ScrollArea::vertical().id_salt("reel-studio").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let info = reel_info(app);
        // Load the card from the Reel when a different project is opened.
        let key = info.as_ref().map(|i| format!("{}:{:?}", app.session.project.name, i.main_clip));
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
            ui.label(RichText::new("Templates still work: Reel ▸ New Reel, then pick one below.").small().color(t.text_dim));
            drop(r);
            templates(app, ui, None);
            return;
        };
        if r.channels.is_empty() {
            r.channels = s.channels().unwrap_or_default();
        }
        // ---- Workspace ----
        let current = app.ui.reel.channel_id.or_else(|| r.channels.first().map(|c| c.id));
        let name = r.channels.iter().find(|c| Some(c.id) == current).map(|c| c.name.clone()).unwrap_or_else(|| "Choose a workspace".into());
        let mut picked = current;
        let combo = egui::ComboBox::from_id_salt("reel-channel").selected_text(&name).width(ui.available_width() - 70.0).show_ui(ui, |ui| {
            for c in &r.channels {
                ui.selectable_value(&mut picked, Some(c.id), &c.name);
            }
        });
        app.auto.add("reel.channel", combo.response.rect, &name);
        if picked != app.ui.reel.channel_id {
            app.ui.reel.channel_id = picked;
        }
        if let Some(c) = picked
            && r.loaded_channel != Some(c)
        {
            load_channel(&mut r, c);
            // The open Reel takes the workspace's brand.
            if let (Some(b), Some(_)) = (r.brand.clone(), info.as_ref()) {
                let _ = app.session.execute("reel.setBrand", json!({"brand": b, "channelId": c}));
            }
        }
        let brand = r.brand.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            let sw = |c: &str| egui::Color32::from_rgb(
                u8::from_str_radix(c.get(1..3).unwrap_or("80"), 16).unwrap_or(128),
                u8::from_str_radix(c.get(3..5).unwrap_or("80"), 16).unwrap_or(128),
                u8::from_str_radix(c.get(5..7).unwrap_or("80"), 16).unwrap_or(128),
            );
            for c in [&brand.primary_color, &brand.card_background, &brand.band_color] {
                let (rr, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().rect_filled(rr, 3.0, sw(c));
            }
            ui.label(RichText::new(format!("{}  {}", brand.handle_line(), if brand.logo_path.is_some() { "· logo" } else { "· no logo set" })).small().color(t.text_dim));
            if ui.small_button("↻").on_hover_text("Reload the workspace from the library").clicked()
                && let Some(c) = picked
            {
                r.channels = s.channels().unwrap_or_default();
                load_channel(&mut r, c);
            }
        });
        drop(r);
        if let Some((msg, level)) = token_warning() {
            let colour = if level >= 3 { egui::Color32::from_rgb(0xff, 0x6b, 0x6b) } else { egui::Color32::from_rgb(0xf5, 0xb0, 0x41) };
            ui.label(RichText::new(format!("⚠ Instagram token {msg}. Extend it in the cloud account manager.")).small().color(colour));
        }
        let mut r = rt();
        // ---- New Reel ----
        section(ui, "New Reel from a workspace clip", info.is_none(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut app.ui.reel.media_filter).hint_text("Search clips").desired_width(f32::INFINITY));
            let filter = app.ui.reel.media_filter.to_lowercase();
            let clips: Vec<MediaRow> = r.media.iter().filter(|m| m.kind == "VIDEO" && (filter.is_empty() || m.label.to_lowercase().contains(&filter))).take(40).cloned().collect();
            if clips.is_empty() {
                ui.label(RichText::new("No clips in this workspace yet. Drop files in its clips folder, or import with File ▸ Import and use Reel ▸ New Reel.").small().color(t.text_dim));
            }
            for m in clips {
                ui.horizontal(|ui| {
                    let b = ui.small_button("New Reel");
                    app.auto.add(&format!("reel.new.{}", m.id), b.rect, &m.label);
                    let dims = match (m.width, m.height) {
                        (Some(w), Some(h)) => format!("{w}×{h}"),
                        _ => String::new(),
                    };
                    ui.label(RichText::new(format!("{}  {}  {}", m.label, m.duration.map(|d| format!("{d:.1}s")).unwrap_or_default(), dims)).small());
                    if b.clicked() {
                        let p = json!({"name": m.label, "template": brand.default_layout, "brand": brand, "channelId": picked, "card": app.ui.reel.card, "paths": [m.path.to_string_lossy()]});
                        match app.session.execute("reel.new", p) {
                            Ok(_) => log(&mut r, format!("New Reel from {}", m.label)),
                            Err(e) => log(&mut r, format!("Could not start the Reel: {e}")),
                        }
                    }
                });
            }
        });
        drop(r);
        // ---- Templates ----
        section(ui, "Templates", true, |ui| templates(app, ui, info.as_ref()));
        // ---- Card ----
        section(ui, "Card", true, |ui| card(app, ui, info.is_some(), picked, &s));
        // ---- Deliver ----
        section(ui, "Export and publish", true, |ui| deliver(app, ui, info.as_ref(), picked, &s));
        // ---- Library ----
        section(ui, "Download from links (yt-dlp)", false, |ui| imports(app, ui, picked, &s));
        section(ui, "Cloud schedule (Supabase)", false, |ui| cloud(app, ui, info.as_ref(), picked, &s));
        section(ui, "Library", false, |ui| library(app, ui, picked, &s));
        // ---- Activity ----
        let r = rt();
        if !r.tasks.is_empty() || r.export.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(r.tasks.iter().map(|(l, _)| l.as_str()).chain(r.export.as_ref().map(|_| "Exporting")).collect::<Vec<_>>().join(" · ")).small());
            });
        }
        for l in r.log.iter().take(8) {
            ui.label(RichText::new(l).small().color(t.text_dim));
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
        ui.horizontal_wrapped(|ui| {
            for tpl in TEMPLATES.iter().filter(|x| x.family == family) {
                let on = info.is_some_and(|i| i.template == tpl.id);
                let b = ui.add_enabled(info.is_some(), egui::Button::new(RichText::new(tpl.name).small()).selected(on)).on_hover_text(tpl.note);
                app.auto.add(&format!("reel.template.{}", tpl.id), b.rect, tpl.name);
                if b.clicked()
                    && let Err(e) = app.session.execute("reel.apply", json!({"template": tpl.id}))
                {
                    app.ui.status = e.to_string();
                }
            }
        });
    }
}

fn card(app: &mut FilmcraftApp, ui: &mut egui::Ui, is_reel: bool, channel: Option<i64>, s: &Studio) {
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
        app.auto.add(&format!("reel.card.{id}"), r.rect, label);
        r.lost_focus()
    }
    let mut changed = line(ui, app, "title", "Headline", true, |c| &mut c.title);
    changed |= line(ui, app, "caption", "Supporting line", false, |c| &mut c.caption);
    changed |= line(ui, app, "attribution", "Credit (Original: @creator)", false, |c| &mut c.attribution);
    changed |= line(ui, app, "badge", "Badge", false, |c| &mut c.badge);
    changed |= line(ui, app, "cta", "Closing call to action", false, |c| &mut c.cta);
    ui.label(RichText::new("Points (one per line)").small().color(t.text_dim));
    let mut points = app.ui.reel.card.bullets.join("\n");
    let pr = ui.add(egui::TextEdit::multiline(&mut points).desired_rows(2).desired_width(f32::INFINITY));
    if pr.changed() {
        app.ui.reel.card.bullets = points.lines().map(str::to_string).filter(|l| !l.trim().is_empty()).take(4).collect();
    }
    changed |= pr.lost_focus();
    changed |= line(ui, app, "instagramCaption", "Instagram caption", true, |c| &mut c.instagram_caption);
    ui.label(RichText::new("Hashtags").small().color(t.text_dim));
    let mut tags = app.ui.reel.card.hashtags.join(" ");
    let tr = ui.add(egui::TextEdit::singleline(&mut tags).desired_width(f32::INFINITY));
    if tr.changed() {
        app.ui.reel.card.hashtags = tags.split_whitespace().map(str::to_string).collect();
    }
    changed |= tr.lost_focus();
    if changed && is_reel {
        let p = card_params(&app.ui.reel.card);
        if let Err(e) = app.session.execute("reel.setCard", p) {
            app.ui.status = e.to_string();
        }
    }
    ui.separator();
    ui.label(RichText::new("Write with AI").strong());
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
    app.auto.add("reel.write", b.rect, "Write the card");
    if b.clicked()
        && let Some(ch) = channel
    {
        let s = s.clone();
        let (article, notes) = (app.ui.reel.article, app.ui.reel.notes.clone());
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
                .map_err(|e| e.to_string()),
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
    app.auto.add("reel.export", b.rect, "Export and send to the library");
    if info.channel_id.is_none() {
        ui.label(RichText::new("Choose the workspace above first.").small().color(t.text_dim));
    }
    if b.clicked() {
        let dir = s.root.join("Editor").join("exports");
        let _ = std::fs::create_dir_all(&dir);
        let safe: String = app.session.project.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
        let path = dir.join(format!("{safe}-{}.mp4", filmcraft_studio::now_iso().replace([':', '.'], "-")));
        match app.session.execute("export.quick", json!({"preset": "Social Vertical 1080×1920", "path": path.to_string_lossy(), "range": "entire"})) {
            Ok(r) => match r["job"].as_u64() {
                Some(job) => rt().export = Some(PendingExport { job, path, approve: app.ui.reel.approve_on_send }),
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
            app.auto.add(&format!("reel.approve.{}", r.id), b.rect, "Approve");
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
            app.auto.add(&format!("reel.publish.{}", r.id), b.rect, "Publish now");
            if b.clicked()
                && let Some(ch) = channel
            {
                let mut rr = rt();
                for a in app.ui.reel.accounts.clone() {
                    let st = s.clone();
                    spawn(&mut rr, "Publishing", move || {
                        Done::Message(match filmcraft_studio::publish_now(&st, ch, r.id, a) {
                            Ok(Some(link)) => format!("Published: {link}"),
                            Ok(None) => "Published".into(),
                            Err(e) => format!("Publishing failed: {e}"),
                        })
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
    let posts: Vec<LibraryPost> = rt().library.clone();
    if posts.is_empty() {
        ui.label(RichText::new("No posts in this workspace yet.").small().color(t.text_dim));
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
            render_actions(app, ui, &p, channel, s);
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
    app.auto.add("reel.import", b.rect, "Download and draft");
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
