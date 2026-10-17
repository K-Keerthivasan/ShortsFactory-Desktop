//! Export ▸ Publish ▸ Schedule: cloud publishing on a calendar.
//!
//! The month shows every cloud post (Supabase) on its day. Clicking a day picks it: its posts are
//! listed with their actions (cancel, retry, reschedule, open on Instagram), and the open Reel can
//! be scheduled on it — rendered, approved, uploaded to B2 and confirmed with Supabase through the
//! queue. Publishing runs in the cloud, so it happens whether or not this app is open.

use chrono::{Datelike, Local, NaiveDate, NaiveTime, TimeZone};

use super::*;

/// "YYYY-MM-DD" of a cloud post's time, in local time.
fn day_of(iso: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(local_time(iso).get(..10)?, "%Y-%m-%d").ok()
}

/// The UTC ISO time for a local day and "HH:MM", if it is a real local time.
pub(super) fn at_local(day: NaiveDate, hhmm: &str) -> Option<String> {
    let time = NaiveTime::parse_from_str(hhmm.trim(), "%H:%M").ok()?;
    let local = Local.from_local_datetime(&day.and_time(time)).earliest()?;
    Some(filmcraft_studio::iso_ms(local.timestamp_millis()))
}

fn status_colour(t: &crate::theme::Tokens, status: &str) -> egui::Color32 {
    match status {
        "published" => egui::Color32::from_rgb(0x4a, 0xde, 0x80),
        "failed" | "needs_reconciliation" | "expired" => t.danger,
        "cancelled" | "deleted" => t.text_dim,
        _ => t.text,
    }
}

fn load_cloud(s: &Studio, force: bool) {
    let mut r = rt();
    if r.cloud_loaded && !force {
        return;
    }
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
}

/// A cloud action on one post (cancel, retry, reschedule…), then a refresh.
fn post_action(s: &Studio, label: &'static str, action: &'static str, body: serde_json::Value) {
    let st = s.clone();
    let mut r = rt();
    spawn(&mut r, "Cloud schedule", move || {
        Done::Notice(match filmcraft_studio::cloud::call(&st, action, body) {
            Ok(_) => (format!("Cloud: {label} done"), true),
            Err(e) => (format!("Cloud: {label} failed: {e}"), false),
        })
    });
    r.cloud_loaded = false;
}

pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &Ctx) {
    let t = app.tokens;
    if !filmcraft_studio::cloud::configured(&ctx.s) {
        ui.label("Cloud publishing is not set up. Add the Supabase URL, anon key, email and password in File ▸ Project Settings ▸ Plugin Settings ▸ Cloud.");
        return;
    }
    load_cloud(&ctx.s, false);
    let (accounts, mut posts, error) = {
        let r = rt();
        (r.cloud_accounts.clone(), r.cloud_posts.clone(), r.cloud_error.clone())
    };
    // YouTube Shorts scheduled from this PC: uploaded already, published by YouTube at their time.
    let now = filmcraft_studio::clock_ms(&filmcraft_studio::now_iso()).unwrap_or(0);
    let window = |days: i64| filmcraft_studio::iso_ms(now + days * 86_400_000);
    for e in ctx.s.calendar(ctx.channel, &window(-120), &window(400)).unwrap_or_default().into_iter().filter(|e| e.platform == "YOUTUBE") {
        let due = filmcraft_studio::clock_ms(&e.at).is_some_and(|t| t > now);
        let status = match e.status.as_str() {
            "PUBLISHED" if due => "on YouTube",
            "PUBLISHED" => "published",
            "PUBLISH_FAILED" | "FAILED" => "failed",
            "NEEDS_RECONCILIATION" => "needs_reconciliation",
            _ => "uploading",
        };
        let video = e.link.as_deref().and_then(|l| l.rsplit('/').next()).unwrap_or_default().to_string();
        posts.push(json!({
            "id": format!("youtube-{}", e.render), "title": e.title, "status": status, "scheduled_for": e.at, "youtube": true,
            "studio": if video.is_empty() { serde_json::Value::Null } else { json!(format!("https://studio.youtube.com/video/{video}/edit")) },
            "link": e.link,
        }));
    }
    if !error.is_empty() {
        ui.label(RichText::new(format!("Cloud: {error}")).small().color(t.danger));
    }
    let name_of = |id: &str| {
        accounts
            .iter()
            .find(|a| a.get("id").and_then(|v| v.as_str()) == Some(id))
            .and_then(|a| a.get("account_name").and_then(|v| v.as_str()))
            .unwrap_or("Instagram")
            .to_string()
    };

    // Which account new posts go to, with its token's expiry.
    ui.label(RichText::new("Instagram account").strong());
    if accounts.is_empty() {
        let loading = rt().tasks.iter().any(|(label, _)| label == "Cloud accounts");
        ui.label(
            RichText::new(if loading { "Loading cloud accounts…" } else { "No cloud accounts. Connect one in the cloud dashboard." }).small().color(t.text_dim),
        );
    }
    if app.ui.reel.cloud_account.is_none() {
        app.ui.reel.cloud_account = accounts.first().and_then(|a| a.get("id").and_then(|v| v.as_str())).map(str::to_string);
    }
    for a in &accounts {
        let id = a.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let name = a.get("account_name").and_then(|v| v.as_str()).unwrap_or("Instagram").to_string();
        let expires = a.get("token_expires_at").and_then(|v| v.as_str());
        let (_, level) = filmcraft_studio::cloud::token_health(expires);
        ui.horizontal_wrapped(|ui| {
            let b = ui.radio_value(&mut app.ui.reel.cloud_account, Some(id.clone()), format!("@{name}"));
            app.auto.add_response(&format!("reel.schedule.account.{id}"), &b, &name);
            ui.label(RichText::new(format!("token valid {}", filmcraft_studio::cloud::expiry_text(expires))).small().color(if level >= 3 {
                t.danger
            } else {
                t.text_dim
            }));
        });
    }
    ui.separator();

    // The month.
    let today = Local::now().date_naive();
    let (mut y, mut m) = app.ui.reel.calendar_month.unwrap_or((today.year(), today.month()));
    ui.horizontal(|ui| {
        let prev = ui.small_button("◀");
        app.auto.add_response("reel.schedule.prevMonth", &prev, "Previous month");
        if prev.clicked() {
            (y, m) = if m == 1 { (y - 1, 12) } else { (y, m - 1) };
        }
        let month = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
        ui.label(RichText::new(format!("{} {y}", month.get(m as usize - 1).copied().unwrap_or(""))).strong());
        let next = ui.small_button("▶");
        app.auto.add_response("reel.schedule.nextMonth", &next, "Next month");
        if next.clicked() {
            (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
        }
        if ui.small_button("Today").clicked() {
            (y, m) = (today.year(), today.month());
            app.ui.reel.calendar_day = Some(today.format("%Y-%m-%d").to_string());
        }
        let refresh = ui.small_button("↻ Refresh");
        app.auto.add_response("reel.schedule.refresh", &refresh, "Refresh cloud posts");
        if refresh.clicked() {
            load_cloud(&ctx.s, true);
        }
    });
    app.ui.reel.calendar_month = Some((y, m));
    let Some(first) = NaiveDate::from_ymd_opt(y, m, 1) else { return };
    let Some(end) = (if m == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1) } else { NaiveDate::from_ymd_opt(y, m + 1, 1) }) else { return };
    let selected = app.ui.reel.calendar_day.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).unwrap_or(today);
    let cleared = app.ui.reel.cleared_posts.clone();
    let show_cleared = app.ui.reel.show_cleared;
    let live = |p: &serde_json::Value| {
        !matches!(p.get("status").and_then(|v| v.as_str()), Some("deleted"))
            && (show_cleared || !cleared.iter().any(|c| Some(c.as_str()) == p.get("id").and_then(|v| v.as_str())))
    };
    let on_day = |d: NaiveDate| -> Vec<&serde_json::Value> {
        let mut v: Vec<&serde_json::Value> =
            posts.iter().filter(|p| live(p) && p.get("scheduled_for").and_then(|v| v.as_str()).and_then(day_of) == Some(d)).collect();
        v.sort_by_key(|p| p.get("scheduled_for").and_then(|v| v.as_str()).unwrap_or("").to_string());
        v
    };
    // Seven cells and their gaps fit the column: each group frame adds its margin and stroke.
    let frame_extra = egui::Frame::group(ui.style()).total_margin().sum().x;
    let width = ((ui.available_width() - 6.0 * 4.0) / 7.0 - frame_extra).max(24.0);
    let mut picked = None;
    egui::Grid::new("sf-schedule-month").spacing([4.0, 4.0]).show(ui, |ui| {
        for d in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
            ui.label(RichText::new(d).small().color(t.text_dim));
        }
        ui.end_row();
        let lead = first.weekday().num_days_from_monday();
        for _ in 0..lead {
            ui.label("");
        }
        let (mut col, mut day) = (lead, first);
        while day < end {
            let items = on_day(day);
            let fill = if day == selected { t.accent.gamma_multiply(0.35) } else { egui::Color32::TRANSPARENT };
            let cell = egui::Frame::group(ui.style()).fill(fill).show(ui, |ui| {
                // A fixed-size cell: the day, then its posts in brief (titles are in the day list).
                ui.set_width(width);
                ui.set_max_width(width);
                ui.set_min_height(50.0);
                ui.vertical(|ui| {
                    let label = RichText::new(day.day().to_string()).small();
                    ui.label(if day == today {
                        label.strong().color(t.accent)
                    } else if day < today {
                        label.color(t.text_dim)
                    } else {
                        label
                    });
                    let status = |p: &&serde_json::Value| p.get("status").and_then(|v| v.as_str()).unwrap_or_default().to_string();
                    // The worst state shows: failed, then scheduled, then published.
                    let worst = items.iter().map(status).max_by_key(|s| match s.as_str() {
                        "failed" | "needs_reconciliation" | "expired" => 3,
                        "published" | "cancelled" => 1,
                        _ => 2,
                    });
                    let brief = match items.as_slice() {
                        [] => String::new(),
                        [one] => local_time(one.get("scheduled_for").and_then(|v| v.as_str()).unwrap_or_default()).get(11..).unwrap_or("").to_string(),
                        many => many.len().to_string(),
                    };
                    if let Some(w) = worst {
                        ui.add(egui::Label::new(RichText::new(format!("● {brief}")).small().color(status_colour(&t, &w))).truncate());
                    }
                });
            });
            let click = ui.interact(cell.response.rect, ui.id().with(("sf-day", day.ordinal(), y)), egui::Sense::click());
            app.auto.add_response(&format!("reel.schedule.day.{}", day.format("%Y-%m-%d")), &click, &day.format("%d %B").to_string());
            if click.clicked() {
                picked = Some(day);
            }
            col += 1;
            if col == 7 {
                ui.end_row();
                col = 0;
            }
            day = day.succ_opt().unwrap_or(end);
        }
    });
    let selected = picked.unwrap_or(selected);
    app.ui.reel.calendar_day = Some(selected.format("%Y-%m-%d").to_string());
    ui.separator();

    // The chosen day: its posts and their actions.
    let day_posts = on_day(selected);
    let finished = |p: &&serde_json::Value| matches!(p.get("status").and_then(|v| v.as_str()), Some("published" | "cancelled" | "failed"));
    let mut clear: Vec<String> = Vec::new();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(selected.format("%A %d %B %Y").to_string()).strong());
        let done: Vec<String> = day_posts.iter().filter(|p| finished(p)).filter_map(|p| p.get("id").and_then(|v| v.as_str()).map(str::to_string)).collect();
        let b = ui.add_enabled(done.iter().any(|id| !cleared.contains(id)), egui::Button::new("Clear finished").small());
        app.auto.add_response("reel.schedule.clearDay", &b, "Clear this day's finished posts");
        if b.on_hover_text("Hides this day's published, cancelled and failed posts here (nothing is deleted)").clicked() {
            clear.extend(done);
        }
        if !cleared.is_empty() {
            let c = ui.checkbox(&mut app.ui.reel.show_cleared, format!("Show cleared ({})", cleared.len()));
            app.auto.add_response("reel.schedule.showCleared", &c, "Show cleared posts");
        }
    });
    if day_posts.is_empty() {
        ui.label(RichText::new("Nothing scheduled this day.").small().color(t.text_dim));
    }
    for p in day_posts {
        let id = p.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let at = p.get("scheduled_for").and_then(|v| v.as_str()).unwrap_or_default();
        let status = p.get("status").and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let title = p.get("title").and_then(|v| v.as_str()).unwrap_or("Reel");
        let youtube = p.get("youtube").and_then(|v| v.as_bool()).unwrap_or(false);
        let account = if youtube { "YouTube".to_string() } else { name_of(p.get("instagram_account_id").and_then(|v| v.as_str()).unwrap_or_default()) };
        ui.group(|ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(local_time(at).get(11..).unwrap_or("").to_string()).strong());
                ui.label(title);
                ui.label(RichText::new(format!("@{account} · {status}")).small().color(status_colour(&t, &status)));
            });
            if let Some(e) = p.get("last_error").and_then(|v| v.as_str()).filter(|e| !e.is_empty()) {
                ui.label(RichText::new(e).small().color(t.danger));
            }
            ui.horizontal_wrapped(|ui| {
                if matches!(status.as_str(), "published" | "cancelled" | "failed") {
                    if cleared.contains(&id) {
                        let b = ui.small_button("Unclear");
                        app.auto.add_response(&format!("reel.schedule.unclear.{id}"), &b, "Show this post again");
                        if b.clicked() {
                            app.ui.reel.cleared_posts.retain(|c| *c != id);
                        }
                    } else {
                        let b = ui.small_button("Clear").on_hover_text("Hide it here (nothing is deleted)");
                        app.auto.add_response(&format!("reel.schedule.clear.{id}"), &b, "Clear post");
                        if b.clicked() {
                            clear.push(id.clone());
                        }
                    }
                }
                if let Some(link) = p.get("instagram_permalink").and_then(|v| v.as_str()) {
                    let r = ui.hyperlink_to("Open on Instagram", link);
                    app.auto.add_response(&format!("reel.schedule.open.{id}"), &r, "Open on Instagram");
                }
                // Changing or cancelling a YouTube schedule happens in YouTube Studio.
                if let Some(link) = p.get("studio").and_then(|v| v.as_str()) {
                    let r = ui.hyperlink_to(if status == "on YouTube" { "Change in YouTube Studio" } else { "Open in YouTube Studio" }, link);
                    app.auto.add_response(&format!("reel.schedule.studio.{id}"), &r, "Open in YouTube Studio");
                }
                if status == "scheduled" {
                    let key = format!("reschedule:{id}");
                    let draft = app.ui.reel.env_drafts.entry(key).or_insert_with(|| local_time(at));
                    ui.add(egui::TextEdit::singleline(draft).desired_width(130.0).hint_text("YYYY-MM-DD HH:MM"));
                    let move_to = filmcraft_studio::local_to_utc_iso(draft);
                    let b = ui.add_enabled(move_to.is_some(), egui::Button::new("Reschedule").small());
                    app.auto.add_response(&format!("reel.schedule.reschedule.{id}"), &b, "Reschedule");
                    if b.clicked()
                        && let Some(to) = move_to
                    {
                        post_action(
                            &ctx.s,
                            "Reschedule",
                            "reschedulePost",
                            json!({"id": id, "scheduledFor": to, "timezone": filmcraft_studio::cloud::local_timezone()}),
                        );
                    }
                    let c = ui.small_button("Cancel");
                    app.auto.add_response(&format!("reel.schedule.cancel.{id}"), &c, "Cancel post");
                    if c.clicked() {
                        post_action(&ctx.s, "Cancel", "cancelPost", json!({"id": id}));
                    }
                }
                if status == "failed" {
                    let b = ui.small_button("Retry");
                    app.auto.add_response(&format!("reel.schedule.retry.{id}"), &b, "Retry post");
                    if b.clicked() {
                        post_action(&ctx.s, "Retry", "retryPost", json!({"id": id}));
                    }
                }
            });
        });
    }
    for id in clear {
        if !app.ui.reel.cleared_posts.contains(&id) {
            app.ui.reel.cleared_posts.push(id);
        }
    }
    // Ids of posts the cloud no longer lists need not be kept.
    if !posts.is_empty() {
        app.ui.reel.cleared_posts.retain(|c| posts.iter().any(|p| p.get("id").and_then(|v| v.as_str()) == Some(c.as_str())));
    }
    ui.separator();

    // Schedule the open Reel on the chosen day.
    let Some(info) = ctx.info.clone() else {
        ui.label(RichText::new("Open a Reel to schedule it here.").small().color(t.text_dim));
        return;
    };
    ui.label(RichText::new("Schedule the open Reel").strong());
    if app.ui.reel.schedule_time.trim().is_empty() {
        app.ui.reel.schedule_time = "18:00".into();
    }
    let at = at_local(selected, &app.ui.reel.schedule_time);
    ui.horizontal_wrapped(|ui| {
        ui.label(selected.format("%a %d %b").to_string());
        ui.label("at");
        let time = ui.add(egui::TextEdit::singleline(&mut app.ui.reel.schedule_time).desired_width(54.0).hint_text("HH:MM"));
        app.auto.add_response("reel.schedule.time", &time, "Time (HH:MM, local)");
    });
    section(ui, "Post caption", false, |ui| {
        ui.label(&info.card.instagram_caption);
        ui.label(RichText::new(info.card.hashtags.join(" ")).small());
    });
    let account = app.ui.reel.cloud_account.clone();
    let expires = account
        .as_deref()
        .and_then(|id| accounts.iter().find(|a| a.get("id").and_then(|v| v.as_str()) == Some(id)))
        .and_then(|a| a.get("token_expires_at").and_then(|v| v.as_str()));
    let at_ms = at.as_deref().and_then(filmcraft_studio::clock_ms);
    let problem = if at.is_none() {
        Some("Type the time as HH:MM".to_string())
    } else if at_ms.is_some_and(|ms| ms <= filmcraft_studio::clock_ms(&filmcraft_studio::now_iso()).unwrap_or(0) + 60_000) {
        Some("Pick a time in the future".to_string())
    } else if account.is_none() {
        Some("Choose an Instagram account".to_string())
    } else if expires.and_then(filmcraft_studio::clock_ms).zip(at_ms).is_some_and(|(e, a)| e <= a) {
        Some("This account's token expires before that time: extend it in Plugin Settings ▸ Accounts & keys first".to_string())
    } else if info.channel_id != ctx.channel || ctx.channel.is_none() {
        Some("This Reel belongs to another workspace".to_string())
    } else {
        None
    };
    if let Some(p) = &problem {
        ui.label(RichText::new(p).small().color(t.danger));
    }
    ui.horizontal_wrapped(|ui| {
        let b = ui.add_enabled(problem.is_none(), egui::Button::new("Schedule in cloud").fill(t.accent));
        app.auto.add_response("reel.schedule.submit", &b, "Schedule in cloud");
        if b.clicked()
            && let (Some(at), Some(account)) = (at.clone(), account.clone())
        {
            enqueue(app, &info, QueueAction::Cloud(account, at, false, Vec::new()));
            rt().cloud_loaded = false;
        }
        let lib = ui.button("Render to library only");
        app.auto.add_response("reel.publish.libraryOnly", &lib, "Render to library without publishing");
        if lib.clicked() {
            enqueue(app, &info, QueueAction::LibraryOnly);
        }
    });
    ui.label(
        RichText::new("Renders the Reel, approves it, uploads it and books it in the cloud; follow it in the Queue tab. It then appears on this calendar.")
            .small()
            .color(t.text_dim),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_day_and_time_become_utc() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 3).unwrap();
        let iso = at_local(day, "18:30").unwrap();
        assert_eq!(local_time(&iso), "2026-11-03 18:30");
        assert_eq!(day_of(&iso), Some(day));
        assert!(at_local(day, "25:00").is_none());
        assert!(at_local(day, "six").is_none());
    }
}
