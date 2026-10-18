//! Export-style publishing and a visual library. Preview sessions never edit the open project.
use super::*;
use egui::{Align2, Color32, Sense, pos2};
use filmcraft_project::{ItemId, Project};
use filmcraft_time::{FrameRate, Tick};
use std::path::Path;
use std::sync::Arc;

pub(super) struct Preview {
    project: Arc<Project>,
    item: ItemId,
    width: u32,
    height: u32,
    rate: FrameRate,
    duration: f64,
}

fn load_preview(path: &Path) -> Result<Box<Preview>, String> {
    let mut session = filmcraft_engine::Session::default();
    // Separate ids also isolate the shared decoder and frame caches from the open project.
    Arc::make_mut(&mut session.project).next_id = (1u64 << 62) | (next_id().min((1u64 << 60) - 1) << 2);
    let result = session.execute("file.import", json!({"paths": [path.to_string_lossy()]})).map_err(|e| e.to_string())?;
    let item = result.get("items").and_then(|v| v.as_array()).and_then(|v| v.first()).and_then(|v| v.as_u64()).map(ItemId).ok_or("Could not load preview")?;
    let media = session.project.item(item).and_then(|i| i.as_media()).ok_or("No media in preview")?;
    let video = media.info.video.as_ref().ok_or("No video in preview")?;
    let preview = Preview {
        project: session.project.clone(),
        item,
        width: video.width,
        height: video.height,
        rate: video.frame_rate,
        duration: media.info.duration.seconds(),
    };
    Ok(Box::new(preview))
}

fn preview(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect, path: Option<&Path>, seconds: f64, thumbnail: bool) -> Option<f64> {
    let mut r = rt();
    preview_in(app, ui, rect, path, seconds, thumbnail, &mut r)
}

/// [`preview`] for a caller that already holds the panel state.
pub(super) fn preview_in(
    app: &mut FilmcraftApp,
    ui: &mut egui::Ui,
    rect: Rect,
    path: Option<&Path>,
    seconds: f64,
    thumbnail: bool,
    r: &mut Runtime,
) -> Option<f64> {
    ui.painter().rect_filled(rect, 4.0, Color32::BLACK);
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let Some(path) = path else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "No render yet", egui::FontId::proportional(12.0), app.tokens.text_dim);
        return None;
    };
    let state = {
        if !r.previews.contains_key(path) && r.tasks.iter().filter(|(name, _)| name == "Library preview").count() < 3 {
            if r.previews.len() >= 80
                && let Some(old) = r.previews.keys().next().cloned()
            {
                r.previews.remove(&old);
            }
            let file = path.to_path_buf();
            r.previews.insert(file.clone(), Ok(None));
            spawn(r, "Library preview", move || Done::Preview(file.clone(), load_preview(&file)));
        }
        r.previews.get(path).cloned()
    };
    match state {
        Some(Ok(Some(p))) => {
            let seconds = if seconds.is_finite() { seconds.max(0.0).min(p.duration.max(0.0)) } else { 0.0 };
            let frame = p.rate.frame_at(Tick::from_seconds_f64(seconds));
            let size = if thumbnail { 120 } else { 560 };
            let key = crate::frames::FrameKey { target: crate::frames::Target::Item(p.item), frame, size, revision: p.item.0, draft: false };
            app.frames.request(key, p.rate.tick_of(frame), size as f32 / p.width.max(1) as f32, &p.project, if thumbnail { 50 } else { 0 });
            if let Some(image) = app.frames.get(&key) {
                let name = format!("library-preview-{}-{size}", p.item.0);
                let texture = app.texture_for(ui.ctx(), &name, key, &image);
                let picture = super::super::monitor::fit(rect, p.width as f32, p.height as f32);
                ui.painter().image(texture, picture, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
            }
            Some(p.duration)
        }
        Some(Err(error)) => {
            let response = ui.interact(rect, ui.id().with(path), Sense::hover()).on_hover_text(error);
            ui.painter().text(response.rect.center(), Align2::CENTER_CENTER, "Preview unavailable", egui::FontId::proportional(12.0), app.tokens.text_dim);
            None
        }
        _ => {
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Loading…", egui::FontId::proportional(12.0), app.tokens.text_dim);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            None
        }
    }
}

/// A render's thumbnails: the 16:9 and 9:16 ones side by side, made at the moment shown in the
/// preview or the best moment, with an optional headline. Publishing uploads them.
fn thumbnail_section(app: &mut FilmcraftApp, ui: &mut egui::Ui, render: i64, studio: &filmcraft_studio::Studio) {
    use filmcraft_studio::thumbnails::{self, Size};
    let t = app.tokens;
    let covers = thumbnails::covers(studio, render);
    egui::CollapsingHeader::new("Thumbnail").id_salt(("thumbnail", render)).default_open(true).show(ui, |ui| {
        match &covers {
            Some(c) => {
                let h = 120.0;
                ui.horizontal(|ui| {
                    for (size, w) in [(Size::Landscape, h * 16.0 / 9.0), (Size::Vertical, h * 9.0 / 16.0), (Size::Portrait, h * 0.8), (Size::Square, h)] {
                        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
                        preview(app, ui, rect, c.file(size), 0.0, true);
                        if resp.on_hover_text(size.label()).clicked()
                            && let Some(f) = c.file(size)
                            && let Err(e) = super::super::menu_dialogs::open_path(app, ui.ctx(), &f.to_string_lossy(), false)
                        {
                            app.ui.status = e;
                        }
                    }
                });
                ui.label(
                    RichText::new(format!("From {:.1} s{} · YouTube gets the image, Instagram this moment as its cover", c.at, if c.headline.is_empty() { String::new() } else { format!(" · “{}”", c.headline) }))
                        .small()
                        .color(t.text_dim),
                );
            }
            None => {
                ui.label(RichText::new("No thumbnails yet: they are made when the video reaches the library, or here.").small().color(t.text_dim));
            }
        }
        let busy = rt().tasks.iter().any(|(l, _)| l == "Thumbnails");
        let draft_id = egui::Id::new(("thumb-headline", render));
        let mut headline: String = ui.data_mut(|d| d.get_temp(draft_id)).unwrap_or_else(|| covers.as_ref().map(|c| c.headline.clone()).unwrap_or_default());
        ui.add(egui::TextEdit::singleline(&mut headline).hint_text("Headline on the thumbnail (optional; Reels already show theirs)").desired_width(f32::INFINITY));
        ui.data_mut(|d| d.insert_temp(draft_id, headline.clone()));
        let at = app.ui.reel.library_preview_time;
        let mut run: Option<Option<f64>> = None;
        ui.horizontal_wrapped(|ui| {
            let here = ui.add_enabled(!busy, egui::Button::new(format!("Use this moment ({at:.1} s)")));
            app.auto.add_response("reel.library.thumbnail.here", &here, "Use the preview's moment as the thumbnail");
            if here.on_hover_text("Scrub the preview above to the frame you want").clicked() {
                run = Some(Some(at));
            }
            let best = ui.add_enabled(!busy, egui::Button::new(if busy { "Making…" } else { "Best moment" }));
            app.auto.add_response("reel.library.thumbnail.best", &best, "Pick the best moment for the thumbnail");
            if best.on_hover_text("The app picks the sharpest, most colourful, well-exposed frame").clicked() {
                run = Some(None);
            }
        });
        if let Some(at) = run {
            let st = studio.clone();
            let old: Vec<std::path::PathBuf> = covers.as_ref().map(|c| c.files.iter().map(|(_, f)| f.clone()).collect()).unwrap_or_default();
            let mut r = rt();
            for f in &old {
                r.previews.remove(f);
            }
            spawn(&mut r, "Thumbnails", move || match thumbnails::make_for_render(&st, render, at, &headline) {
                Ok(c) => Done::Thumbnails(c.files.iter().map(|(_, f)| f.clone()).collect(), Ok(format!("Thumbnails made from {:.1} s", c.at))),
                Err(e) => Done::Thumbnails(old, Err(e.to_string())),
            });
        }
    });
}

fn columns(rect: Rect) -> (Rect, Rect) {
    let width = (rect.width() * 0.38).clamp(240.0, 460.0).min((rect.width() - 140.0).max(120.0));
    (Rect::from_min_max(rect.min, pos2(rect.min.x + width, rect.max.y)), Rect::from_min_max(pos2(rect.min.x + width + 8.0, rect.min.y), rect.max))
}

pub(super) fn library_page(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let (left, right) = columns(rect);
    for area in [left, right] {
        ui.painter().rect_filled(area, app.tokens.radius, app.tokens.panel_bg);
    }
    let mut selected = None;
    frame(ui, left, "sf-library-list", |ui| {
        let Some(ctx) = prelude(app, ui) else { return };
        workspace_line(app, ui, &ctx);
        library(app, ui, ctx.channel, &ctx.s);
        let r = rt();
        let posts = if app.ui.reel.show_archived { &r.archived_posts } else { &r.library };
        let query = app.ui.reel.library_filter.to_lowercase();
        let filtered: Vec<_> = posts
            .iter()
            .filter(|p| query.is_empty() || format!("{} {} {} {:?}", p.title, p.status, p.kind, p.published).to_lowercase().contains(&query))
            .collect();
        let post =
            filtered.iter().find(|p| app.ui.reel.library_selected == ctx.channel.map(|ch| (ch, p.story_id))).copied().or_else(|| filtered.first().copied());
        if let Some(post) = post {
            let key = ctx.channel.map(|ch| (ch, post.story_id));
            if app.ui.reel.library_selected != key {
                app.ui.reel.library_preview_time = 0.0;
            }
            app.ui.reel.library_selected = key;
            selected = Some((post.clone(), ctx.channel, ctx.s));
        } else {
            app.ui.reel.library_selected = None;
        }
    });
    frame(ui, right, "sf-library-detail", |ui| {
        let Some((post, channel, studio)) = selected else {
            ui.label("Choose a rendered post to preview it here.");
            return;
        };
        ui.heading(&post.title);
        let height = (right.height() * 0.62).max(140.0);
        let (picture, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let path = post.render.as_ref().and_then(|r| r.path.as_deref()).filter(|p| p.is_file());
        let duration = preview(app, ui, picture, path, app.ui.reel.library_preview_time, false);
        app.auto.add("reel.library.preview", picture, "Rendered video preview");
        if let Some(duration) = duration.filter(|d| d.is_finite() && *d > 0.0) {
            let slider = ui.add(egui::Slider::new(&mut app.ui.reel.library_preview_time, 0.0..=duration).suffix(" s").text("Preview"));
            app.auto.add_response("reel.library.scrub", &slider, "Scrub rendered video");
        }
        ui.horizontal_wrapped(|ui| {
            let play = ui.add_enabled(path.is_some(), egui::Button::new("Play video"));
            app.auto.add_response("reel.library.play", &play, "Play saved video");
            if play.clicked()
                && let Some(path) = path
                && let Err(e) = super::super::menu_dialogs::open_path(app, ui.ctx(), &path.to_string_lossy(), false)
            {
                app.ui.status = e;
            }
            ui.label(format!("{} · {}", post.kind.to_lowercase(), post.status.to_lowercase()));
        });
        if let Some(render) = post.render.as_ref().filter(|_| path.is_some()) {
            thumbnail_section(app, ui, render.id, &studio);
        }
        if let Some(channel) = channel {
            library_actions(app, ui, &post, channel, &studio);
            if !app.ui.reel.show_archived {
                render_actions(app, ui, &post, Some(channel), &studio);
            }
        }
    });
}

/// The library as tiles: each post's thumbnail (9:16), title and status; a click previews it.
pub(super) fn library_grid(app: &mut FilmcraftApp, ui: &mut egui::Ui, posts: &[LibraryPost], channel: Option<i64>) {
    let t = app.tokens;
    let gap = 8.0;
    let per_row = ((ui.available_width() + gap) / (110.0 + gap)).floor().max(1.0);
    let tile = ((ui.available_width() + gap) / per_row - gap).max(80.0);
    let thumb = vec2(tile, tile * 16.0 / 9.0);
    ui.spacing_mut().item_spacing = vec2(gap, gap);
    ui.horizontal_wrapped(|ui| {
        for post in posts {
            let key = channel.map(|ch| (ch, post.story_id));
            let selected = app.ui.reel.library_selected == key;
            let (rect, response) = ui.allocate_exact_size(vec2(tile, thumb.y + 40.0), Sense::click());
            let picture = Rect::from_min_size(rect.min, thumb);
            let path = post.render.as_ref().and_then(|r| r.thumbnail.as_deref().filter(|p| p.is_file()).or(r.path.as_deref()));
            ui.painter().rect_filled(picture, t.radius, Color32::from_black_alpha(90));
            preview(app, ui, picture, path, 0.0, true);
            if selected {
                ui.painter().rect_stroke(rect.expand(2.0), t.radius, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Outside);
            } else if response.hovered() {
                ui.painter().rect_stroke(rect.expand(1.0), t.radius, egui::Stroke::new(1.0, t.text_dim), egui::StrokeKind::Outside);
            }
            let title = ui.painter().layout(post.title.clone(), egui::FontId::proportional(12.0), t.text, tile);
            let title_rect = Rect::from_min_size(pos2(rect.min.x, picture.max.y + 4.0), vec2(tile, 16.0));
            ui.painter().with_clip_rect(title_rect).galley(title_rect.min, title, t.text);
            let status =
                if post.published.iter().any(|(_, s, _)| s.eq_ignore_ascii_case("published")) { "published".to_string() } else { post.status.to_lowercase() };
            ui.painter().text(pos2(rect.min.x, picture.max.y + 22.0), Align2::LEFT_TOP, status, egui::FontId::proportional(11.0), t.text_dim);
            let response = response.on_hover_text(&post.title);
            app.auto.add_response(&format!("reel.library.tile.{}", post.story_id), &response, &post.title);
            if response.clicked() {
                app.ui.reel.library_selected = key;
                app.ui.reel.library_preview_time = 0.0;
            }
        }
    });
}

pub(super) fn library_row(app: &mut FilmcraftApp, ui: &mut egui::Ui, post: &LibraryPost, channel: Option<i64>, studio: &Studio) {
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (rect, response) = ui.allocate_exact_size(vec2(76.0, 116.0), Sense::click());
            let path = post.render.as_ref().and_then(|r| r.thumbnail.as_deref().filter(|p| p.is_file()).or(r.path.as_deref()));
            preview(app, ui, rect, path, 0.0, true);
            app.auto.add_response(&format!("reel.library.select.{}", post.story_id), &response, &post.title);
            if response.clicked() {
                app.ui.reel.library_selected = channel.map(|ch| (ch, post.story_id));
                app.ui.reel.library_preview_time = 0.0;
            }
            ui.vertical(|ui| {
                let selected = app.ui.reel.library_selected == channel.map(|ch| (ch, post.story_id));
                let title = ui.add(egui::Button::new(RichText::new(&post.title).strong()).selected(selected).wrap());
                app.auto.add_response(&format!("reel.library.title.{}", post.story_id), &title, &post.title);
                if title.clicked() {
                    app.ui.reel.library_selected = channel.map(|ch| (ch, post.story_id));
                    app.ui.reel.library_preview_time = 0.0;
                }
                ui.label(
                    RichText::new(format!("{} · {}", post.status.to_lowercase(), post.created_at.get(..10).unwrap_or(""))).small().color(app.tokens.text_dim),
                );
                if let Some(channel) = channel {
                    if app.ui.reel.show_archived {
                        library_actions(app, ui, post, channel, studio);
                    } else {
                        let remove = ui.small_button("Remove").on_hover_text("Remove from Library; keep files and publication history. Restore from Archived.");
                        app.auto.add_response(&format!("reel.removePost.{}", post.story_id), &remove, "Remove from library");
                        if remove.clicked() {
                            match studio.archive_post(channel, post.story_id) {
                                Ok(_) => {
                                    let mut r = rt();
                                    r.archived = Some((channel, post.story_id));
                                    load_channel(&mut r, channel);
                                }
                                Err(e) => notify(&mut rt(), format!("Could not remove “{}”: {e}", post.title), false),
                            }
                        }
                    }
                }
            });
        });
    });
}

pub(super) fn publish_page(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    let (left, right) = columns(rect);
    for area in [left, right] {
        ui.painter().rect_filled(area, app.tokens.radius, app.tokens.panel_bg);
    }
    let mut has_reel = false;
    frame(ui, left, "sf-publish-settings", |ui| {
        let Some(ctx) = prelude(app, ui) else { return };
        workspace_line(app, ui, &ctx);
        let mut tab = app.ui.reel.publish_tab;
        tab_bar(
            app,
            ui,
            &mut tab,
            "reel.publish",
            &[(PublishTab::Now, "now", "Publish"), (PublishTab::Calendar, "schedule", "Schedule"), (PublishTab::Queue, "queue", "Queue")],
        );
        app.ui.reel.publish_tab = tab;
        match tab {
            PublishTab::Queue => {
                queue_view(app, ui, &ctx);
                activity(app, ui);
                return;
            }
            // Cloud publishing on a calendar.
            PublishTab::Calendar => {
                has_reel = ctx.info.is_some();
                super::schedule::show(app, ui, &ctx);
                activity(app, ui);
                return;
            }
            PublishTab::Cloud => {
                app.ui.reel.publish_tab = PublishTab::Now;
            }
            PublishTab::Now => {}
        }
        let Some(info) = ctx.info else {
            ui.label("Open a Reel to publish it.");
            return;
        };
        has_reel = true;
        ui.heading("Publish Reel");
        let accounts = rt().accounts.clone();
        let instagram: Vec<_> = accounts.iter().filter(|a| a.platform != "YOUTUBE").cloned().collect();
        let youtube: Vec<_> = accounts.iter().filter(|a| a.platform == "YOUTUBE").cloned().collect();
        let tick = |app: &mut FilmcraftApp, ui: &mut egui::Ui, account: &filmcraft_studio::Account, label: String| {
            let mut selected = app.ui.reel.accounts.contains(&account.id);
            let check = ui.add_enabled(account.enabled, egui::Checkbox::new(&mut selected, label));
            app.auto.add_response(&format!("reel.publish.account.{}", account.id), &check, &account.display_name);
            if check.changed() {
                app.ui.reel.accounts.retain(|id| *id != account.id);
                if selected {
                    app.ui.reel.accounts.push(account.id);
                }
            }
        };

        // Instagram: from this PC, or through the cloud.
        ui.label(RichText::new("Instagram").strong());
        ui.horizontal(|ui| {
            let local = ui.selectable_value(&mut app.ui.reel.use_cloud, false, "Local publishing");
            app.auto.add_response("reel.publish.delivery.local", &local, "Local publishing");
            let cloud = ui.add_enabled(filmcraft_studio::cloud::configured(&ctx.s), egui::Button::new("Cloud publishing").selected(app.ui.reel.use_cloud));
            app.auto.add_response("reel.publish.delivery.cloud", &cloud, "Cloud publishing");
            if cloud.clicked() {
                app.ui.reel.use_cloud = true;
            }
        });
        if app.ui.reel.use_cloud {
            let mut runtime = rt();
            if !runtime.cloud_loaded {
                runtime.cloud_loaded = true;
                let s = ctx.s.clone();
                spawn(&mut runtime, "Cloud accounts", move || {
                    Done::CloudAccounts(
                        filmcraft_studio::cloud::call(&s, "accounts", serde_json::json!({}))
                            .map(|v| v.as_array().cloned().unwrap_or_default())
                            .map_err(|e| e.to_string()),
                    )
                });
            }
            let cloud_accounts = runtime.cloud_accounts.clone();
            drop(runtime);
            for account in cloud_accounts {
                let id = account.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                let name = account.get("account_name").and_then(|v| v.as_str()).unwrap_or("Instagram");
                let b = ui.radio_value(&mut app.ui.reel.cloud_account, Some(id.to_string()), name);
                app.auto.add_response(&format!("reel.publish.cloudAccount.{id}"), &b, name);
            }
            ui.label(RichText::new("Cloud publishing continues when the app is closed; the Schedule tab shows it on a calendar.").small().color(app.tokens.text_dim));
        } else {
            if instagram.is_empty() {
                ui.label(RichText::new("No Instagram account in this workspace.").small().color(app.tokens.text_dim));
            }
            for account in &instagram {
                tick(app, ui, account, format!("{} · {}", account.display_name, account.status.to_lowercase()));
            }
        }
        ui.separator();

        // YouTube Shorts: the same Reel, posted (or scheduled by YouTube) on the workspace's channel.
        ui.label(RichText::new("YouTube Shorts").strong());
        if youtube.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("No YouTube channel connected to this workspace.").small().color(app.tokens.text_dim));
                let connect = ui.small_button("Connect YouTube…");
                app.auto.add_response("reel.publish.connectYoutube", &connect, "Connect YouTube");
                if connect.clicked() {
                    app.ui.reel.plugin_open = true;
                    app.ui.reel.plugin_tab = PluginTab::Accounts;
                }
            });
        }
        for account in &youtube {
            ui.horizontal_wrapped(|ui| {
                tick(app, ui, account, format!("{} · {}", account.display_name, account.status.to_lowercase()));
                // Visibility, kept with the channel's key.
                let current = ctx.s.youtube_privacy(account.id).unwrap_or_else(|_| "public".into());
                let mut chosen = current.clone();
                egui::ComboBox::from_id_salt(("yt-privacy", account.id)).selected_text(match chosen.as_str() {
                    "unlisted" => "Unlisted",
                    "private" => "Private",
                    _ => "Public",
                })
                .show_ui(ui, |ui| {
                    for (v, l) in [("public", "Public"), ("unlisted", "Unlisted"), ("private", "Private")] {
                        ui.selectable_value(&mut chosen, v.to_string(), l);
                    }
                });
                if chosen != current
                    && let Err(e) = ctx.s.set_youtube_privacy(account.id, &chosen)
                {
                    app.ui.status = e.to_string();
                }
            });
        }
        let youtube_ticked: Vec<i64> = youtube.iter().filter(|a| app.ui.reel.accounts.contains(&a.id)).map(|a| a.id).collect();
        if !youtube_ticked.is_empty() {
            ui.label(RichText::new("Title").small().color(app.tokens.text_dim));
            let mut title = app.ui.reel.card.youtube_title.clone();
            let field = ui.add(egui::TextEdit::singleline(&mut title).hint_text(info.card.title.chars().take(100).collect::<String>()).char_limit(100).desired_width(f32::INFINITY));
            app.auto.add_response("reel.publish.youtubeTitle", &field, "YouTube title");
            if field.changed() {
                app.ui.reel.card.youtube_title = title;
            }
            if field.lost_focus()
                && app.ui.reel.card.youtube_title != info.card.youtube_title
                && let Err(e) = app.session.execute("reel.setCard", serde_json::json!({"youtubeTitle": app.ui.reel.card.youtube_title}))
            {
                app.ui.status = e.to_string();
            }
            let shown = if app.ui.reel.card.youtube_title.trim().is_empty() { info.card.title.as_str() } else { app.ui.reel.card.youtube_title.as_str() };
            ui.label(RichText::new(format!("{} / 100 · empty uses the headline", shown.chars().count().min(100))).small().color(app.tokens.text_dim));
            section(ui, "Description (the Instagram caption)", false, |ui| {
                let caption = if info.card.instagram_caption.trim().is_empty() { info.card.title.clone() } else { info.card.instagram_caption.clone() };
                ui.label(RichText::new(format!("{caption}\n\n{}\n\n#Shorts", info.card.hashtags.join(" "))).small());
            });
            ui.label(
                RichText::new("Schedule uploads it now and YouTube publishes it at the time (this PC can be off then); Publish now posts it at once.")
                    .small()
                    .color(app.tokens.text_dim),
            );
            let seconds = app.session.active_sequence().map_or(0.0, |q| q.duration().seconds());
            if seconds > 180.0 {
                ui.label(
                    RichText::new(format!("This Reel is {seconds:.0} s: YouTube posts it as a normal video, not a Short (Shorts are up to 3 minutes)."))
                        .small()
                        .color(app.tokens.danger),
                );
            }
        }
        ui.separator();
        section(ui, "Post caption", true, |ui| {
            ui.label(&info.card.instagram_caption);
            ui.label(info.card.hashtags.join(" "));
        });
        if app.ui.reel.schedule_at.is_empty() {
            app.ui.reel.schedule_at = filmcraft_studio::local_in(60);
        }
        ui.label("Schedule time (local)");
        let time = ui.add(egui::TextEdit::singleline(&mut app.ui.reel.schedule_at).hint_text("YYYY-MM-DD HH:MM"));
        app.auto.add_response("reel.publish.time", &time, "Schedule time");
        ui.separator();
        let busy = {
            let runtime = rt();
            runtime.export.is_some()
                || runtime.tasks.iter().any(|(label, _)| label == "Publishing")
                || runtime.queue.iter().any(|item| matches!(item.status.as_str(), "Waiting to render" | "Rendering" | "Publishing" | "Cloud processing"))
        };
        // In cloud mode only YouTube accounts go alongside the cloud post (Instagram ticks are local ones).
        let local_accounts: Vec<i64> = if app.ui.reel.use_cloud { youtube_ticked.clone() } else { app.ui.reel.accounts.clone() };
        let cloud = app.ui.reel.use_cloud.then(|| app.ui.reel.cloud_account.clone()).flatten();
        let ready = !busy && (cloud.is_some() || !local_accounts.is_empty()) && info.channel_id == ctx.channel && ctx.channel.is_some();
        ui.horizontal_wrapped(|ui| {
            let queue = ui.add_enabled(ready, egui::Button::new("Schedule"));
            app.auto.add_response("reel.publish.scheduleReel", &queue, "Render and schedule Reel");
            if queue.clicked() {
                match filmcraft_studio::local_to_utc_iso(&app.ui.reel.schedule_at) {
                    Some(at) => enqueue(
                        app,
                        &info,
                        match &cloud {
                            Some(account) => QueueAction::Cloud(account.clone(), at, false, youtube_ticked.clone()),
                            None => QueueAction::Schedule(local_accounts.clone(), at),
                        },
                    ),
                    None => app.ui.status = "Use YYYY-MM-DD HH:MM for the schedule time".into(),
                }
            }
            let publish = ui.add_enabled(ready, egui::Button::new("Publish now").fill(app.tokens.accent));
            app.auto.add_response("reel.publish.renderNow", &publish, "Render and publish Reel");
            if publish.clicked() {
                enqueue(
                    app,
                    &info,
                    match &cloud {
                        Some(account) => QueueAction::Cloud(account.clone(), filmcraft_studio::now_iso(), true, youtube_ticked.clone()),
                        None => QueueAction::PublishNow(local_accounts.clone()),
                    },
                );
            }
        });
        ui.label(RichText::new("Renders the current Reel, approves it, then publishes to Instagram and YouTube as chosen above.").small().color(app.tokens.text_dim));
        let library = ui.button("Render to library only");
        app.auto.add_response("reel.publish.libraryOnly", &library, "Render to library without publishing");
        if library.clicked() {
            enqueue(app, &info, QueueAction::LibraryOnly);
        }
        activity(app, ui);
    });
    if has_reel {
        let monitor = Rect::from_min_max(right.min + vec2(12.0, 12.0), right.max - vec2(12.0, 100.0));
        super::super::monitor::show(app, ui, monitor, super::super::monitor::Which::Program);
        app.auto.add("reel.publish.preview", monitor, "Reel preview");
        let summary = Rect::from_min_max(pos2(right.min.x + 12.0, right.max.y - 88.0), right.max - vec2(12.0, 10.0));
        frame(ui, summary, "sf-publish-summary", |ui| {
            ui.strong("Output summary");
            if let Some(sequence) = app.session.active_sequence() {
                ui.label(format!("H.264 · {}×{} · {:.1} seconds", sequence.settings.width, sequence.settings.height, sequence.duration().seconds()));
            }
            ui.label("Preview the complete composition before publishing.");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_previews_have_separate_projects_and_cache_ids() {
        let file = std::env::temp_dir().join(format!("library-preview-{}.png", std::process::id()));
        image::RgbaImage::from_pixel(8, 12, image::Rgba([30, 80, 150, 255])).save(&file).unwrap();
        let app = FilmcraftApp::new(filmcraft_engine::Session::default());
        let before = app.session.project.clone();
        let a = load_preview(&file).unwrap();
        let b = load_preview(&file).unwrap();
        assert_ne!(a.item, b.item);
        assert!(a.item.0 >= 1u64 << 62);
        assert_eq!((a.width, a.height), (8, 12));
        assert!(Arc::ptr_eq(&before, &app.session.project));
        assert!(app.session.project.items.is_empty());
        assert!(a.project.item(a.item).is_some());
        std::fs::remove_file(file).unwrap();
    }

    #[test]
    fn missing_preview_is_an_error_and_columns_fit_small_windows() {
        assert!(load_preview(Path::new("missing-library-preview.mp4")).is_err());
        for width in [360.0, 800.0, 1400.0] {
            let outer = Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 700.0));
            let (left, right) = columns(outer);
            assert!(left.width() > 0.0 && right.width() > 0.0);
            assert!(right.right() <= outer.right());
            assert!(left.right() < right.left());
        }
    }
}
