//! The AI panel: video links in, Reels out, nothing published unapproved.
//!
//! 1. **Find** – each link is downloaded with yt-dlp, then the workspace's AI (Hermes, Codex,
//!    Ollama) reads its chapters and captions and picks the parts worth a Reel, one or several,
//!    with a card for each (`filmcraft_studio::autopilot`).
//! 2. **Review** – the parts it chose, with their times and headlines, to tick, adjust or drop.
//! 3. **Make** – one Reel (or Post) per part from the same downloaded file (`reel.new` with a
//!    source range), each queued to render into the library.
//! 4. **Approve** – the renders waiting for approval, to approve and publish or schedule.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Rect, RichText};
use filmcraft_studio::Studio;
use filmcraft_studio::autopilot::{Mode, Segment, Wants};
use filmcraft_studio::ytdlp::Import;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{Done, Format, QueueAction, enqueue, frame, load_channel, log, prelude, render_actions, rt, spawn, workspace_line};
use crate::FilmcraftApp;

/// The AI panel's settings (kept with the workspace layout).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiDraft {
    /// Video links, one per line.
    pub links: String,
    pub wants: Wants,
    /// The template every Reel starts with; empty uses the workspace's default.
    pub template: String,
    pub format: Format,
    /// Queue each new Reel to render into the library for approval.
    pub queue: bool,
}

impl Default for AiDraft {
    fn default() -> Self {
        AiDraft { links: String::new(), wants: Wants::default(), template: String::new(), format: Format::Reel, queue: true }
    }
}

/// One link going through the workflow.
#[derive(Clone, Debug, Default)]
pub(super) struct Run {
    pub id: u64,
    pub channel: i64,
    pub import: Import,
    pub stage: String,
    /// The parts and whether each is ticked.
    pub segments: Vec<(Segment, bool)>,
    /// Why the AI's choice was not used (the chapters or an even split were).
    pub fallback: Option<String>,
    pub error: Option<String>,
    pub made: usize,
}

pub(super) type Runs = Arc<Mutex<Vec<Run>>>;

fn lock(runs: &Runs) -> std::sync::MutexGuard<'_, Vec<Run>> {
    runs.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, rect: Rect) {
    frame(ui, rect, "sf-ai", |ui| {
        let Some(ctx) = prelude(app, ui) else { return };
        let t = app.tokens;
        workspace_line(app, ui, &ctx);
        let provider = filmcraft_studio::ai::provider(&ctx.s);
        ui.label(RichText::new(format!("AI: {provider} · change it in File ▸ Project Settings ▸ Plugin Settings ▸ AI")).small().color(t.text_dim));
        ui.separator();

        // 1. Links and what to make.
        ui.label(RichText::new("1 · Video links").strong());
        ui.add(
            egui::TextEdit::multiline(&mut app.ui.reel.ai.links)
                .hint_text("YouTube or other video links, one per line")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        let w = &mut app.ui.reel.ai.wants;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Make").small());
            for (m, l) in [(Mode::Auto, "AI decides"), (Mode::Single, "One Reel"), (Mode::Multiple, "Several Reels")] {
                ui.selectable_value(&mut w.mode, m, l);
            }
            if w.mode != Mode::Single {
                ui.label(RichText::new("up to").small());
                ui.add(egui::DragValue::new(&mut w.max_reels).range(1..=20));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Each").small());
            ui.add(egui::DragValue::new(&mut w.min_seconds).range(3.0..=600.0).suffix(" s"));
            ui.label(RichText::new("to").small());
            ui.add(egui::DragValue::new(&mut w.max_seconds).range(3.0..=600.0).suffix(" s"));
            if w.min_seconds > w.max_seconds {
                w.min_seconds = w.max_seconds;
            }
        });
        ui.add(egui::TextEdit::singleline(&mut w.notes).hint_text("Direction for the AI (optional): what to look for").desired_width(f32::INFINITY));
        ui.checkbox(&mut w.hook, "Hook first, filler cut")
            .on_hover_text("The AI opens each Reel with its strongest moment and cuts dead air, filler and repeats (needs the video's captions and FFmpeg)");
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("As").small());
            for f in [Format::Reel, Format::Post] {
                ui.selectable_value(&mut app.ui.reel.ai.format, f, f.label());
            }
            let shown = if app.ui.reel.ai.template.is_empty() {
                "Workspace default".to_string()
            } else {
                filmcraft_reel::find(&app.ui.reel.ai.template).map_or_else(|| app.ui.reel.ai.template.clone(), |x| x.name.to_string())
            };
            egui::ComboBox::from_id_salt("ai-template").selected_text(shown).show_ui(ui, |ui| {
                ui.selectable_value(&mut app.ui.reel.ai.template, String::new(), "Workspace default");
                for x in filmcraft_reel::TEMPLATES {
                    ui.selectable_value(&mut app.ui.reel.ai.template, x.id.to_string(), x.name);
                }
            });
        });
        let links: Vec<String> = app.ui.reel.ai.links.lines().map(str::trim).filter(|l| l.starts_with("http")).map(str::to_string).collect();
        let runs = rt().ai_runs.clone();
        let busy = lock(&runs).iter().any(|r| r.error.is_none() && r.segments.is_empty() && r.stage != "Done");
        let b = ui.add_enabled(
            ctx.channel.is_some() && !busy && !links.is_empty() && links.len() <= 10,
            egui::Button::new(if busy { "Working…" } else { "Find Reels" }),
        );
        app.auto.add_response("ai.find", &b, "Find Reels");
        if links.len() > 10 {
            ui.label(RichText::new("At most ten links at a time.").small().color(t.text_dim));
        }
        if b.clicked()
            && let Some(ch) = ctx.channel
        {
            let wants = app.ui.reel.ai.wants.clone();
            let options = app.ui.reel.download.clone();
            find(&ctx.s, ch, links, wants, options, None, &runs);
            app.ui.reel.ai.links.clear();
        }

        // 2 and 3. What the AI chose, to review and make.
        let list: Vec<Run> = lock(&runs).iter().filter(|run| ctx.channel == Some(run.channel)).cloned().collect();
        if !list.is_empty() {
            ui.separator();
            ui.label(RichText::new("2 · Review the parts, then make the Reels").strong());
        }
        for run in list {
            review(app, ui, &ctx, &runs, run);
        }

        // 4. Approval.
        ui.separator();
        ui.label(RichText::new("3 · Approve and publish").strong());
        let n = app.ui.reel.accounts.len();
        ui.label(
            RichText::new(if n == 0 {
                "No account chosen to publish to (choose accounts in Reel Studio)".to_string()
            } else {
                format!("Publishes to {n} account(s) chosen in Reel Studio")
            })
            .small()
            .color(t.text_dim),
        );
        let waiting: Vec<_> = rt()
            .library
            .iter()
            .filter(|p| p.published.is_empty() && p.render.as_ref().is_some_and(|r| r.status == "RENDERED" || (r.approved && r.status == "APPROVED")))
            .take(20)
            .cloned()
            .collect();
        if waiting.is_empty() {
            ui.label(RichText::new("Nothing waiting. New Reels appear here once they have rendered.").small().color(t.text_dim));
        }
        for p in waiting {
            ui.group(|ui| {
                ui.label(RichText::new(&p.title).strong());
                render_actions(app, ui, &p, ctx.channel, &ctx.s);
            });
        }
        if ui.small_button("Refresh").clicked()
            && let Some(c) = ctx.channel
        {
            load_channel(&mut rt(), c);
        }
    });
}

/// Downloads each link and asks the AI for its parts, on a worker.
fn find(
    s: &Studio,
    channel: i64,
    links: Vec<String>,
    wants: Wants,
    options: filmcraft_studio::ytdlp::Options,
    save_to: Option<std::path::PathBuf>,
    runs: &Runs,
) {
    let ids: Vec<u64> = {
        let mut l = lock(runs);
        links
            .iter()
            .map(|url| {
                let id = super::next_id();
                let import = Import { url: url.clone(), options: options.clone(), no_draft: true, save_to: save_to.clone(), ..Default::default() };
                l.insert(0, Run { id, channel, import, stage: "Waiting".into(), ..Default::default() });
                id
            })
            .collect()
    };
    let st = s.clone();
    let runs = runs.clone();
    let mut r = rt();
    log(&mut r, format!("AI: finding Reels in {} video(s)", ids.len()));
    spawn(&mut r, "AI", move || {
        for id in ids {
            let set = |f: &dyn Fn(&mut Run)| {
                if let Some(x) = lock(&runs).iter_mut().find(|x| x.id == id) {
                    f(x);
                }
            };
            let Some(job) = lock(&runs).iter().find(|x| x.id == id).map(|x| x.import.clone()) else { continue };
            let update = |j: &Import| {
                set(&|x| {
                    x.import = j.clone();
                    x.stage = if j.stage == "Downloading" { format!("Downloading {:.0}%", j.progress) } else { j.stage.clone() };
                })
            };
            let done = filmcraft_studio::ytdlp::run(&st, channel, job, &update);
            if let Some(e) = done.error.clone() {
                set(&|x| {
                    x.error = Some(e.clone());
                    x.stage = "Failed".into();
                });
                continue;
            }
            set(&|x| x.stage = "The AI is choosing the parts".into());
            let meta = done.metadata.clone().unwrap_or_default();
            let (mut segments, fallback) = filmcraft_studio::autopilot::plan(&st, channel, &meta, done.duration.unwrap_or(0.0), &wants);
            // The hook edits, made into clips beside the download.
            let mut failed = None;
            if let Some(file) = done.file.clone() {
                for (i, seg) in segments.iter_mut().enumerate() {
                    if seg.pieces.len() < 2 {
                        continue;
                    }
                    set(&|x| x.stage = format!("Editing part {} (hook first)", i + 1));
                    match filmcraft_studio::autopilot::assemble(&st, &file, seg) {
                        Ok(f) => seg.file = Some(f),
                        Err(e) => {
                            seg.pieces.clear();
                            failed = Some(e.to_string());
                        }
                    }
                }
            }
            let fallback = fallback.or(failed.map(|e| format!("the hook edit was skipped: {e}")));
            set(&|x| {
                x.segments = segments.iter().cloned().map(|s| (s, true)).collect();
                x.fallback = fallback.clone();
                x.stage = format!("{} part(s) to review", segments.len());
            });
        }
        Done::Notice(("AI: the parts are ready to review".into(), true))
    });
}

fn clock(t: f64) -> String {
    let t = t.max(0.0) as u64;
    format!("{}:{:02}", t / 60, t % 60)
}

fn review(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &super::Ctx, runs: &Runs, mut run: Run) {
    let t = app.tokens;
    let mut remove = false;
    let mut make = false;
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            let title = if run.import.title.is_empty() { run.import.url.clone() } else { run.import.title.clone() };
            ui.label(RichText::new(title).strong());
            ui.label(RichText::new(&run.stage).small().color(t.text_dim));
            if ui.small_button("Remove").on_hover_text("Remove from the list (the download stays)").clicked() {
                remove = true;
            }
        });
        if let Some(e) = &run.error {
            ui.label(RichText::new(e).small().color(t.danger));
            return;
        }
        if let Some(f) = &run.fallback {
            ui.label(RichText::new(format!("The AI did not choose ({f}); these are the chapters or even parts.")).small().color(t.text_dim));
        }
        let duration = run.import.duration.unwrap_or(f64::INFINITY);
        for (i, (seg, on)) in run.segments.iter_mut().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(on, "");
                let a = ui.add(egui::DragValue::new(&mut seg.start).range(0.0..=duration).speed(0.5).custom_formatter(|v, _| clock(v)));
                ui.label("–");
                let b = ui.add(egui::DragValue::new(&mut seg.end).range(0.0..=duration).speed(0.5).custom_formatter(|v, _| clock(v)));
                // New times: the plain range, without the AI's edit.
                if (a.changed() || b.changed()) && !seg.pieces.is_empty() {
                    seg.pieces.clear();
                    seg.file = None;
                }
                ui.label(RichText::new(format!("{:.0} s", seg.seconds())).small().color(t.text_dim));
            });
            if let (Some(hook), true) = (seg.pieces.first().copied(), seg.file.is_some()) {
                let cut = (seg.end - seg.start - seg.seconds() + (hook.end - hook.start)).max(0.0);
                let mut keep = seg.file.is_some();
                ui.horizontal_wrapped(|ui| {
                    if ui.checkbox(&mut keep, "").changed() && !keep {
                        seg.pieces.clear();
                        seg.file = None;
                    }
                    ui.label(
                        RichText::new(format!(
                            "Hook {}–{} first, then {} range(s); {:.0} s cut",
                            clock(hook.start),
                            clock(hook.end),
                            seg.pieces.len() - 1,
                            cut
                        ))
                        .small()
                        .color(t.text_dim),
                    );
                });
            }
            ui.add(egui::TextEdit::multiline(&mut seg.card.title).desired_rows(1).desired_width(f32::INFINITY).id_salt(("ai-title", run.id, i)));
            if !seg.why.is_empty() {
                ui.label(RichText::new(&seg.why).small().italics().color(t.text_dim));
            }
        }
        let ticked = run.segments.iter().filter(|(s, on)| *on && s.end > s.start).count();
        if run.import.file.is_some() && !run.segments.is_empty() {
            ui.horizontal_wrapped(|ui| {
                let label = format!("Make {ticked} {}{}", app.ui.reel.ai.format.label(), if ticked == 1 { "" } else { "s" });
                let b = ui.add_enabled(ticked > 0, egui::Button::new(label));
                app.auto.add_response(&format!("ai.make.{}", run.id), &b, "Make Reels");
                make = b.clicked();
                ui.checkbox(&mut app.ui.reel.ai.queue, "Render into the library for approval");
            });
            if run.made > 0 {
                ui.label(RichText::new(format!("{} made — open them from the Project panel", run.made)).small().color(t.text_dim));
            }
        }
    });
    if make {
        run.made += make_reels(app, ctx, &run);
    }
    let mut l = lock(runs);
    if remove {
        l.retain(|x| x.id != run.id);
    } else if let Some(x) = l.iter_mut().find(|x| x.id == run.id) {
        // Keep the worker's progress; take the edits made here.
        if x.segments.len() == run.segments.len() {
            x.segments = run.segments;
        }
        x.made = run.made;
    }
}

/// One Reel per ticked part, from the one downloaded file; queued for the library if asked.
pub(super) fn make_reels(app: &mut FilmcraftApp, ctx: &super::Ctx, run: &Run) -> usize {
    if ctx.channel != Some(run.channel) {
        app.ui.status = "Switch to this video's workspace before making its Reels".into();
        return 0;
    }
    let Some(file) = run.import.file.clone() else { return 0 };
    let template = if app.ui.reel.ai.template.is_empty() { ctx.brand.default_layout.clone() } else { app.ui.reel.ai.template.clone() };
    let format = app.ui.reel.ai.format;
    let queue = app.ui.reel.ai.queue;
    let base: String = if run.import.title.is_empty() { "AI Reel".into() } else { run.import.title.chars().take(50).collect() };
    let mut made = 0;
    let parts: Vec<&Segment> = run.segments.iter().filter(|(s, on)| *on && s.end > s.start).map(|(s, _)| s).collect();
    let many = parts.len() > 1;
    for (i, seg) in parts.into_iter().enumerate() {
        let name = if many { format!("{base} – part {}", i + 1) } else { base.clone() };
        // The hook edit is its own clip; otherwise the range of the download.
        let params = match seg.file.as_ref().filter(|f| f.is_file()) {
            Some(edit) => json!({
                "name": name, "template": template, "brand": ctx.brand, "channelId": run.channel, "card": seg.card, "format": format.id(),
                "paths": [edit.to_string_lossy()],
            }),
            None => json!({
                "name": name, "template": template, "brand": ctx.brand, "channelId": run.channel, "card": seg.card, "format": format.id(),
                "paths": [file.to_string_lossy()], "sourceIn": seg.start, "sourceOut": seg.end,
            }),
        };
        match app.session.execute("reel.new", params) {
            Ok(_) => {
                made += 1;
                if queue && let Some(info) = filmcraft_engine::reel::current(&app.session) {
                    enqueue(app, &info, QueueAction::LibraryOnly);
                }
            }
            Err(e) => log(&mut rt(), format!("Could not make “{name}”: {e}")),
        }
    }
    app.ui.reel.loaded_for = None;
    log(&mut rt(), format!("AI: made {made} {}(s) from {base}", format.label()));
    made
}
