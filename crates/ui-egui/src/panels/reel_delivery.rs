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
    ui.painter().rect_filled(rect, 4.0, Color32::BLACK);
    if !ui.is_rect_visible(rect) {
        return None;
    }
    let Some(path) = path else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "No render yet", egui::FontId::proportional(12.0), app.tokens.text_dim);
        return None;
    };
    let state = {
        let mut r = rt();
        if !r.previews.contains_key(path) && r.tasks.iter().filter(|(name, _)| name == "Library preview").count() < 3 {
            if r.previews.len() >= 80
                && let Some(old) = r.previews.keys().next().cloned()
            {
                r.previews.remove(&old);
            }
            let file = path.to_path_buf();
            r.previews.insert(file.clone(), Ok(None));
            spawn(&mut r, "Library preview", move || Done::Preview(file.clone(), load_preview(&file)));
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
        if let Some(channel) = channel {
            library_actions(app, ui, &post, channel, &studio);
            if !app.ui.reel.show_archived {
                render_actions(app, ui, &post, Some(channel), &studio);
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
                                Err(e) => app.ui.status = e.to_string(),
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
        // Cloud publishing on a calendar, and the render queue behind it.
        let mut tab = match app.ui.reel.publish_tab {
            PublishTab::Queue => PublishTab::Queue,
            _ => PublishTab::Now,
        };
        tab_bar(app, ui, &mut tab, "reel.publish", &[(PublishTab::Now, "schedule", "Schedule"), (PublishTab::Queue, "queue", "Queue")]);
        app.ui.reel.publish_tab = tab;
        if tab == PublishTab::Queue {
            queue_view(app, ui, &ctx);
        } else {
            has_reel = ctx.info.is_some();
            super::schedule::show(app, ui, &ctx);
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
