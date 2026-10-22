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
    /// Use the virality analyzer (Whisper, a vision model and FFmpeg) to find the moments.
    pub analyzer: bool,
    /// Animate each new Reel with Flick (Remotion scenes over the footage).
    pub flick: bool,
    /// Stop at Flick's plan for review before building.
    pub flick_review: bool,
}

impl Default for AiDraft {
    fn default() -> Self {
        AiDraft { links: String::new(), wants: Wants::default(), template: String::new(), format: Format::Reel, queue: false, analyzer: true, flick: false, flick_review: true }
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
    /// Asked for one long 16:9 video rather than Reels.
    pub long: bool,
    /// Found by the virality analyzer.
    pub analyzed: bool,
    /// Make the Reels as soon as the parts are found (the Create panel's AI Reels).
    pub auto_make: bool,
    /// Klap's shorts for comparison: each with its downloaded file, or why it failed.
    pub klap: Option<Result<Vec<(filmcraft_studio::klap::Clip, Option<std::path::PathBuf>)>, String>>,
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
        let dim = app.tokens.text_dim;
        let w = &mut app.ui.reel.ai.wants;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Make").small());
            for (m, l) in [(Mode::Auto, "AI decides"), (Mode::Single, "One Reel"), (Mode::Multiple, "Several Reels"), (Mode::Long, "One long video (16:9)")] {
                if ui.selectable_value(&mut w.mode, m, l).changed() {
                    // A long video's length is minutes; a Reel's seconds.
                    w.max_seconds = if m == Mode::Long { w.max_seconds.max(600.0) } else { w.max_seconds.min(180.0) };
                }
            }
            if !matches!(w.mode, Mode::Single | Mode::Long) {
                ui.label(RichText::new("up to").small());
                ui.add(egui::DragValue::new(&mut w.max_reels).range(1..=20));
            }
        });
        if w.mode == Mode::Long {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("At most").small());
                let mut minutes = (w.max_seconds / 60.0).round();
                if ui.add(egui::DragValue::new(&mut minutes).range(1.0..=240.0).suffix(" min")).changed() {
                    w.max_seconds = minutes * 60.0;
                }
                ui.label(
                    RichText::new("The AI edits the whole video tighter, opening with its best moment; it goes to the library as a long video with thumbnails.")
                        .small()
                        .color(dim),
                );
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Each").small());
                ui.add(egui::DragValue::new(&mut w.min_seconds).range(3.0..=600.0).suffix(" s"));
                ui.label(RichText::new("to").small());
                ui.add(egui::DragValue::new(&mut w.max_seconds).range(3.0..=600.0).suffix(" s"));
                if w.min_seconds > w.max_seconds {
                    w.min_seconds = w.max_seconds;
                }
            });
        }
        ui.add(egui::TextEdit::singleline(&mut w.notes).hint_text("Direction for the AI (optional): what to look for").desired_width(f32::INFINITY));
        ui.checkbox(&mut w.hook, "Hook first, filler cut")
            .on_hover_text("The AI opens each Reel with its strongest moment and cuts dead air, filler and repeats (needs the video's captions and FFmpeg)");
        let analyzer = &mut app.ui.reel.ai.analyzer;
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(analyzer, "Virality analyzer")
                .on_hover_text("Whisper (when there are no captions), a vision model and FFmpeg find the viral moments, score them 0-100, suggest hooks and crop to 9:16 on the subject");
            let (vp, vm) = filmcraft_studio::virality::vision_provider(&ctx.s);
            ui.label(RichText::new(format!("vision: {}", if vp == "off" { "off".to_string() } else { format!("{vp} {vm}") })).small().color(t.text_dim));
        });
        let flick_ready = { let st = filmcraft_studio::flick::status(&ctx.s); st.skill && st.remotion };
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled(flick_ready, egui::Checkbox::new(&mut app.ui.reel.ai.flick, "Add Flick motion scenes"))
                .on_hover_text("After each Reel is made, Flick plans and builds animated Remotion scenes for its words, opening on the hook, and puts them on the Reel");
            if app.ui.reel.ai.flick {
                ui.checkbox(&mut app.ui.reel.ai.flick_review, "Review Flick's plan first");
            }
            if !flick_ready {
                ui.label(RichText::new("(set up Flick in Reel Studio ▸ Design ▸ Motion)").small().color(t.text_dim));
            }
        });
        if app.ui.reel.ai.analyzer {
            match filmcraft_studio::virality::whisper_ready() {
                Ok(()) => {}
                Err(e) => {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("Whisper: {e}; videos with captions still work.")).small().color(t.text_dim));
                        let busy = rt().tasks.iter().any(|(l, _)| l == "Whisper model");
                        if ui.add_enabled(!busy && e.contains("model"), egui::Button::new(if busy { "Downloading…" } else { "Download Whisper (about 290 MB)" })).clicked() {
                            spawn(&mut rt(), "Whisper model", move || match filmcraft_studio::virality::download_whisper() {
                                Ok(m) => Done::Notice((format!("Whisper model {m} is ready"), true)),
                                Err(e) => Done::Notice((format!("Whisper download failed: {e}"), false)),
                            });
                        }
                    });
                }
            }
        }
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
            find(&ctx.s, ch, links, Vec::new(), wants, app.ui.reel.ai.analyzer, false, options, None, &runs);
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

        // 4. Predictions vs results.
        results_view(app, ui, &ctx);

        // Approval.
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
/// Downloads each link (workspace clips are already here), then finds the parts on a worker;
/// with `auto` the Reels are made as soon as the parts are found.
#[allow(clippy::too_many_arguments)]
pub(super) fn find(
    s: &Studio,
    channel: i64,
    links: Vec<String>,
    files: Vec<(std::path::PathBuf, String)>,
    wants: Wants,
    analyzer: bool,
    auto: bool,
    options: filmcraft_studio::ytdlp::Options,
    save_to: Option<std::path::PathBuf>,
    runs: &Runs,
) {
    let ids: Vec<u64> = {
        let mut l = lock(runs);
        let mut ids = Vec::new();
        let flags = |import: Import| Run {
            id: super::next_id(),
            channel,
            import,
            stage: "Waiting".into(),
            long: wants.mode == Mode::Long,
            analyzed: analyzer && wants.mode != Mode::Long,
            auto_make: auto,
            ..Default::default()
        };
        for url in &links {
            let run = flags(Import { url: url.clone(), options: options.clone(), no_draft: true, save_to: save_to.clone(), ..Default::default() });
            ids.push(run.id);
            l.insert(0, run);
        }
        for (file, title) in &files {
            let run = flags(Import { file: Some(file.clone()), title: title.clone(), stage: "Done".into(), ..Default::default() });
            ids.push(run.id);
            l.insert(0, run);
        }
        ids
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
            // A workspace clip is here already; a link is downloaded first.
            let done = if job.url.is_empty() && job.file.as_ref().is_some_and(|f| f.is_file()) {
                let mut j = job.clone();
                j.duration = j.file.as_ref().and_then(|f| filmcraft_studio::thumbnails::duration(&st, f).ok());
                j
            } else {
                filmcraft_studio::ytdlp::run(&st, channel, job, &update)
            };
            if let Some(e) = done.error.clone() {
                set(&|x| {
                    x.error = Some(e.clone());
                    x.stage = "Failed".into();
                });
                continue;
            }
            set(&|x| x.stage = "The AI is choosing the parts".into());
            let meta = done.metadata.clone().unwrap_or_default();
            let (segments, fallback) = match done.file.clone().filter(|_| analyzer && wants.mode != Mode::Long) {
                Some(file) => {
                    let say = |m: &str| {
                        let m = m.to_string();
                        set(&|x| x.stage = m.clone());
                    };
                    match filmcraft_studio::virality::analyze(&st, channel, &file, Some(&meta), &wants, &say) {
                        Ok(a) => {
                            let mut notes = a.notes.clone();
                            notes.insert(0, format!("Transcript: {} · vision: {} · scored by {}", a.transcript, if a.vision.is_empty() { "none" } else { &a.vision }, a.ranked_by));
                            (a.segments, Some(notes.join(" · ")))
                        }
                        Err(e) => {
                            let (s, f) = filmcraft_studio::autopilot::plan(&st, channel, &meta, done.duration.unwrap_or(0.0), &wants);
                            (s, Some(format!("the analyzer failed ({e}); {}", f.unwrap_or_else(|| "the AI chose instead".into()))))
                        }
                    }
                }
                None => filmcraft_studio::autopilot::plan(&st, channel, &meta, done.duration.unwrap_or(0.0), &wants),
            };
            // The edit goes on the timeline as cuts (made with the Reel), nothing is rendered here.
            set(&|x| {
                x.segments = segments.iter().cloned().map(|s| (s, true)).collect();
                x.fallback = fallback.clone();
                x.stage = if x.auto_make { format!("{} part(s) found: making the Reels", segments.len()) } else { format!("{} part(s) to review", segments.len()) };
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
            if run.analyzed {
                ui.label(RichText::new(f).small().color(t.text_dim));
                ui.label(
                    RichText::new("A virality score is a prediction, not a guarantee of views: check it against real results below (4 · Predictions vs results).")
                        .small()
                        .italics()
                        .color(t.text_dim),
                );
            } else {
                ui.label(RichText::new(format!("The AI did not choose ({f}); these are the chapters or even parts.")).small().color(t.text_dim));
            }
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
                }
                ui.label(RichText::new(format!("{:.0} s", seg.seconds())).small().color(t.text_dim));
            });
            if let (Some(hook), true) = (seg.pieces.first().copied(), seg.pieces.len() > 1) {
                let cut = (seg.end - seg.start - seg.seconds() + (hook.end - hook.start)).max(0.0);
                let mut keep = true;
                ui.horizontal_wrapped(|ui| {
                    if ui.checkbox(&mut keep, "").changed() && !keep {
                        seg.pieces.clear();
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
            if let Some(score) = seg.score {
                let colour = if score >= 75.0 { t.accent } else if score >= 55.0 { t.text } else { t.text_dim };
                let parts = seg.scores.as_ref().map(|p| format!("hook {:.0} · emotion {:.0} · clarity {:.0} · visual {:.0} · share {:.0}", p.hook, p.emotion, p.clarity, p.visual, p.share)).unwrap_or_default();
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Virality {score:.0}")).strong().color(colour));
                    ui.label(RichText::new(parts).small().color(t.text_dim));
                    if seg.focus_x.is_some() {
                        ui.label(RichText::new("· framed on the subject").small().color(t.text_dim));
                    }
                });
            }
            ui.add(egui::TextEdit::multiline(&mut seg.card.title).desired_rows(1).desired_width(f32::INFINITY).id_salt(("ai-title", run.id, i)));
            for (k, h) in seg.hooks.clone().iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Use").on_hover_text("Make this hook the headline").clicked() {
                        seg.card.title = h.clone();
                    }
                    ui.label(RichText::new(format!("Hook {}: {h}", k + 1)).small());
                });
            }
            if !seg.why.is_empty() {
                ui.label(RichText::new(&seg.why).small().italics().color(t.text_dim));
            }
        }
        let ticked = run.segments.iter().filter(|(s, on)| *on && s.end > s.start).count();
        if run.long && run.import.file.is_some() && !run.segments.is_empty() {
            long_actions(app, ui, &mut run);
        } else if run.import.file.is_some() && !run.segments.is_empty() {
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
            klap_compare(app, ui, runs, &run);
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

/// Every frame: a run asked to make its Reels as soon as its parts are found makes them (one
/// run per frame, in the workspace that is open).
pub(super) fn auto_make(app: &mut FilmcraftApp) {
    let runs = rt().ai_runs.clone();
    let ready = lock(&runs).iter().find(|r| r.auto_make && r.made == 0 && r.error.is_none() && !r.segments.is_empty()).cloned();
    let Some(run) = ready else { return };
    let ctx = {
        let mut r = rt();
        let Some(s) = super::studio(&mut r) else { return };
        let Some(brand) = r.brand.clone() else { return };
        super::Ctx { s, info: None, channel: app.ui.reel.channel_id.or(r.loaded_channel), brand }
    };
    if ctx.channel != Some(run.channel) {
        if let Some(x) = lock(&runs).iter_mut().find(|x| x.id == run.id) {
            x.stage = "Open this video's workspace to make its Reels".into();
        }
        return;
    }
    let n = make_reels(app, &ctx, &run);
    if let Some(x) = lock(&runs).iter_mut().find(|x| x.id == run.id) {
        x.made = n.max(1);
        x.auto_make = false;
        x.stage = format!("{n} Reel(s) made: open them from the Project panel");
    }
}

/// Settings of the Create panel's AI Reels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Maker {
    pub per_video: usize,
    pub min_seconds: f64,
    pub max_seconds: f64,
    /// Workspace clips picked for AI Reels.
    pub picked: Vec<i64>,
    /// Footage already in the project picked for AI Reels (item ids).
    pub picked_items: Vec<u64>,
    pub links: String,
}

impl Default for Maker {
    fn default() -> Self {
        Maker { per_video: 2, min_seconds: 60.0, max_seconds: 90.0, picked: Vec::new(), picked_items: Vec::new(), links: String::new() }
    }
}

/// Create ▸ AI Reels: pick workspace clips (or paste links); the AI finds the best part of
/// each and makes edited Reels (cuts on the timeline, inside the template) to open and refine.
pub(super) fn maker(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &super::Ctx, clips: &[(i64, std::path::PathBuf, String)]) {
    let t = app.tokens;
    let runs = rt().ai_runs.clone();
    ui.label(
        RichText::new("Tick Pick on footage in this project or on workspace clips below (or paste links): the AI finds the best part of each, edits it on the timeline (hook first, dull shots cut, framed for 9:16) and makes Reels with your template, ready to open and refine.")
            .small()
            .color(t.text_dim),
    );
    let m = &mut app.ui.reel.maker;
    ui.add(egui::TextEdit::multiline(&mut m.links).hint_text("Links (optional), one per line").desired_rows(1).desired_width(f32::INFINITY));
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Reels per video").small());
        ui.selectable_value(&mut m.per_video, 1, "1");
        ui.selectable_value(&mut m.per_video, 2, "2");
        ui.label(RichText::new("· length").small());
        ui.add(egui::DragValue::new(&mut m.min_seconds).range(15.0..=180.0).suffix(" s"));
        ui.label("–");
        ui.add(egui::DragValue::new(&mut m.max_seconds).range(15.0..=180.0).suffix(" s"));
        if m.min_seconds > m.max_seconds {
            m.min_seconds = m.max_seconds;
        }
    });
    let flick_ready = {
        let st = filmcraft_studio::flick::status(&ctx.s);
        st.skill && st.remotion
    };
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled(flick_ready, egui::Checkbox::new(&mut app.ui.reel.ai.flick, "Add Flick motion scenes"));
        if app.ui.reel.ai.flick {
            ui.checkbox(&mut app.ui.reel.ai.flick_review, "Review Flick's plan first");
        }
        ui.checkbox(&mut app.ui.reel.ai.queue, "Render into the library too");
    });
    let mut picked: Vec<(std::path::PathBuf, String)> =
        clips.iter().filter(|(id, _, _)| app.ui.reel.maker.picked.contains(id)).map(|(_, p, l)| (p.clone(), l.clone())).collect();
    // Footage already in the project, by its file.
    for id in app.ui.reel.maker.picked_items.clone() {
        let Some(item) = app.session.project.item(filmcraft_project::ItemId(id)) else { continue };
        let Some(filmcraft_project::MediaRef::File { path }) = item.as_media().map(|m| &m.media) else { continue };
        let p = std::path::PathBuf::from(path);
        if !picked.iter().any(|(x, _)| x == &p) {
            picked.push((p, item.name.clone()));
        }
    }
    if !picked.is_empty() {
        ui.label(RichText::new(format!("Picked: {}", picked.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join(" · "))).small());
    }
    let links: Vec<String> = app.ui.reel.maker.links.lines().map(str::trim).filter(|l| l.starts_with("http")).map(str::to_string).collect();
    let n = picked.len() + links.len();
    let busy = lock(&runs).iter().any(|r| r.channel == ctx.channel.unwrap_or_default() && r.error.is_none() && (r.auto_make || (r.segments.is_empty() && r.stage != "Done")));
    ui.horizontal_wrapped(|ui| {
        let b = ui.add_enabled(ctx.channel.is_some() && n > 0 && n <= 20, egui::Button::new(format!("Make AI Reels ({n} video{})", if n == 1 { "" } else { "s" })));
        app.auto.add_response("reel.create.aiReels", &b, "Make AI Reels");
        if n == 0 {
            ui.label(RichText::new("Tick Pick next to the footage or clips to use").small().color(t.text_dim));
        }
        if (!app.ui.reel.maker.picked.is_empty() || !app.ui.reel.maker.picked_items.is_empty()) && ui.small_button("Clear picks").clicked() {
            app.ui.reel.maker.picked.clear();
            app.ui.reel.maker.picked_items.clear();
        }
        if b.clicked()
            && let Some(ch) = ctx.channel
        {
            let m = app.ui.reel.maker.clone();
            let wants = Wants {
                mode: if m.per_video > 1 { Mode::Multiple } else { Mode::Single },
                max_reels: m.per_video.clamp(1, 2),
                min_seconds: m.min_seconds,
                max_seconds: m.max_seconds,
                notes: String::new(),
                hook: true,
            };
            let options = app.ui.reel.download.clone();
            find(&ctx.s, ch, links, picked, wants, true, true, options, None, &runs);
            app.ui.reel.maker.links.clear();
            app.ui.reel.maker.picked.clear();
            app.ui.reel.maker.picked_items.clear();
        }
    });
    let _ = busy;
    // Progress of this workspace's AI Reels.
    let list: Vec<Run> = lock(&runs).iter().filter(|r| Some(r.channel) == ctx.channel).take(12).cloned().collect();
    for run in list {
        let title = if run.import.title.is_empty() { run.import.url.clone() } else { run.import.title.clone() };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(title).small().strong());
            match &run.error {
                Some(e) => ui.label(RichText::new(e).small().color(t.danger)),
                None => ui.label(RichText::new(&run.stage).small().color(t.text_dim)),
            };
            if let Some(best) = run.segments.iter().filter_map(|(s, _)| s.score).fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b)))) {
                ui.label(RichText::new(format!("best {best:.0}")).small());
            }
        });
    }
}

/// Klap, the optional paid provider: its shorts and scores for the same link, to compare; each
/// can be downloaded and opened as a Reel.
fn klap_compare(app: &mut FilmcraftApp, ui: &mut egui::Ui, runs: &Runs, run: &Run) {
    let Some(s) = super::studio(&mut rt()) else { return };
    if !filmcraft_studio::klap::configured(&s) || !run.import.url.starts_with("https://") {
        return;
    }
    let t = app.tokens;
    let busy = rt().tasks.iter().any(|(l, _)| l == "Klap");
    match &run.klap {
        None => {
            let b = ui.add_enabled(!busy, egui::Button::new(if busy { "Klap is working…" } else { "Compare with Klap (paid)" }));
            app.auto.add_response(&format!("ai.klap.{}", run.id), &b, "Compare with Klap");
            if b.on_hover_text("Klap makes its own shorts with its virality scores from the same link (uses your Klap credits)").clicked() {
                let (url, id, runs, w) = (run.import.url.clone(), run.id, runs.clone(), app.ui.reel.ai.wants.clone());
                spawn(&mut rt(), "Klap", move || {
                    let r = filmcraft_studio::klap::shorts(&s, &url, w.max_reels, w.max_seconds, &|_| {})
                        .map(|c| c.into_iter().map(|c| (c, None)).collect())
                        .map_err(|e| e.to_string());
                    if let Some(x) = lock(&runs).iter_mut().find(|x| x.id == id) {
                        x.klap = Some(r);
                    }
                    Done::Notice(("Klap's shorts are ready to compare".into(), true))
                });
            }
        }
        Some(Err(e)) => {
            ui.label(RichText::new(format!("Klap: {e}")).small().color(t.danger));
        }
        Some(Ok(clips)) => {
            ui.label(RichText::new("Klap's shorts (its own scores, for comparison)").small().strong());
            for (n, (c, file)) in clips.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("Klap {:.0}", c.score)).strong());
                    ui.label(RichText::new(&c.name).small());
                    match file {
                        Some(f) => {
                            if ui.small_button("Open as a Reel").clicked() {
                                let p = json!({"name": format!("Klap – {}", c.name), "template": "logoOnly", "channelId": run.channel, "paths": [f.to_string_lossy()]});
                                if let Err(e) = app.session.execute("reel.new", p) {
                                    app.ui.status = e.to_string();
                                }
                            }
                        }
                        None => {
                            if ui.add_enabled(!busy, egui::Button::new("Download").small()).clicked() {
                                let (st, clip, id, runs) = (s.clone(), c.clone(), run.id, runs.clone());
                                let dir = run.import.file.as_ref().and_then(|f| f.parent().map(|p| p.join("klap"))).unwrap_or_else(std::env::temp_dir);
                                spawn(&mut rt(), "Klap", move || {
                                    let r = filmcraft_studio::klap::download(&st, &clip, &dir, &|_| {});
                                    let msg = match &r {
                                        Ok(f) => (format!("Klap's clip is saved: {}", f.display()), true),
                                        Err(e) => (format!("Klap download failed: {e}"), false),
                                    };
                                    if let (Ok(f), Some(x)) = (r, lock(&runs).iter_mut().find(|x| x.id == id))
                                        && let Some(Ok(list)) = x.klap.as_mut()
                                        && let Some(slot) = list.get_mut(n)
                                    {
                                        slot.1 = Some(f);
                                    }
                                    Done::Notice(msg)
                                });
                            }
                        }
                    }
                });
                if !c.explanation.is_empty() {
                    ui.label(RichText::new(&c.explanation).small().italics().color(t.text_dim));
                }
            }
        }
    }
}

/// Published posts with a virality prediction, beside their real views, likes and comments.
fn results_view(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &super::Ctx) {
    let t = app.tokens;
    let Some(ch) = ctx.channel else { return };
    ui.separator();
    ui.label(RichText::new("4 · Predictions vs results").strong());
    ui.label(
        RichText::new("Virality scores are predictions. This compares them with how the published posts actually did, so you can see whether they hold for JKR.")
            .small()
            .color(t.text_dim),
    );
    let id = egui::Id::new(("virality-results", ch));
    let busy = rt().tasks.iter().any(|(l, _)| l == "Virality results");
    let b = ui.add_enabled(!busy, egui::Button::new(if busy { "Checking…" } else { "Check results" }));
    app.auto.add_response("ai.viralityResults", &b, "Check virality predictions against results");
    if b.clicked() {
        let s = ctx.s.clone();
        let ectx = ui.ctx().clone();
        spawn(&mut rt(), "Virality results", move || {
            let r = filmcraft_studio::virality::results(&s, ch).map_err(|e| e.to_string());
            ectx.data_mut(|d| d.insert_temp(id, std::sync::Arc::new(r)));
            Done::Message(String::new())
        });
    }
    let shown: Option<std::sync::Arc<Result<(Vec<filmcraft_studio::virality::PostResult>, Option<f64>), String>>> = ui.data(|d| d.get_temp(id));
    match shown.as_deref() {
        Some(Ok((rows, rho))) if rows.is_empty() => {
            let _ = rho;
            ui.label(RichText::new("No published post carries a score yet: Reels made with the analyzer do, once published.").small().color(t.text_dim));
        }
        Some(Ok((rows, rho))) => {
            ui.label(RichText::new(match rho {
                Some(r) => format!("Rank correlation of score and views: {r:+.2} over {} posts (near +1: the score predicts; near 0: it does not yet)", rows.iter().filter(|r| r.views.is_some()).count()),
                None => "Fewer than three posts with views: too few to judge yet.".to_string(),
            }).small());
            let mut rows = rows.clone();
            rows.sort_by(|a, b| b.score.total_cmp(&a.score));
            for r in rows {
                let n = |v: Option<f64>| v.map_or("–".to_string(), |v| format!("{v:.0}"));
                ui.label(RichText::new(format!("{:.0} · {} · {} views · {} likes · {} comments · {}", r.score, r.platform.to_lowercase(), n(r.views), n(r.likes), n(r.comments), r.title)).small());
            }
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).small().color(t.danger));
        }
        None => {}
    }
}

/// A long video's actions: make it into the library, or open the edit in a 16:9 sequence.
fn long_actions(app: &mut FilmcraftApp, ui: &mut egui::Ui, run: &mut Run) {
    let Some(file) = run.import.file.clone() else { return };
    let Some((seg, _)) = run.segments.iter().find(|(s, on)| *on && s.end > s.start).cloned() else { return };
    let busy = rt().tasks.iter().any(|(l, _)| l == "Long video");
    ui.horizontal_wrapped(|ui| {
        let make = ui.add_enabled(!busy, egui::Button::new(if busy { "Making the long video…" } else { "Make the long video" }));
        app.auto.add_response(&format!("ai.makeLong.{}", run.id), &make, "Make the long video");
        if make.on_hover_text("Edits it, adds it to the library for approval, and makes its thumbnails").clicked() {
            let Some(s) = super::studio(&mut rt()) else { return };
            let (channel, title, seg) = (run.channel, seg.card.title.clone(), seg.clone());
            run.made += 1;
            spawn(&mut rt(), "Long video", move || match filmcraft_studio::autopilot::make_long(&s, channel, &file, &seg) {
                Ok(render) => Done::Notice((format!("Long video “{title}” is in the library (render {render}) with its thumbnails"), true)),
                Err(e) => Done::Notice((format!("The long video failed: {e}"), false)),
            });
        }
        let open = ui.add_enabled(seg.file.as_ref().is_some_and(|f| f.is_file()), egui::Button::new("Open the edit in the editor"));
        app.auto.add_response(&format!("ai.openLong.{}", run.id), &open, "Open the long edit in a 16:9 sequence");
        if open.on_hover_text("A new 16:9 sequence with the edited video, to polish by hand").clicked()
            && let Some(edit) = seg.file.clone()
        {
            let name: String = seg.card.title.chars().take(60).collect();
            let result = (|| -> Result<(), String> {
                app.session
                    .execute("file.newSequence", json!({"name": name, "width": 1920, "height": 1080, "fps": 30, "video": 2, "audio": 2}))
                    .map_err(|e| e.to_string())?;
                let imported = app.session.execute("file.import", json!({"paths": [edit.to_string_lossy()]})).map_err(|e| e.to_string())?;
                let item = imported.get("items").and_then(|v| v.as_array()).and_then(|a| a.first()).and_then(|v| v.as_u64()).ok_or("the edit could not be imported")?;
                app.session.execute("timeline.place", json!({"item": item})).map_err(|e| e.to_string())?;
                Ok(())
            })();
            app.ui.status = match result {
                Ok(()) => "The long edit is open in a new 16:9 sequence".into(),
                Err(e) => format!("Could not open the edit: {e}"),
            };
        }
    });
    if run.made > 0 {
        ui.label(RichText::new("Approve it in 3 · Approve and publish; YouTube gets it as a normal video with its 16:9 thumbnail.").small().color(app.tokens.text_dim));
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
        // The prediction travels with the post, to compare with its results later.
        let mut seg = seg.clone();
        seg.card.virality = seg.score;
        let seg = &seg;
        // The edit is the timeline: one clip per cut (hook first), framed on the subject, inside
        // the template; every cut can be trimmed, moved or removed in the editor.
        let cuts: Vec<[f64; 2]> =
            if seg.pieces.is_empty() { vec![[seg.start, seg.end]] } else { seg.pieces.iter().map(|p| [p.start, p.end]).collect() };
        let mut params = json!({
            "name": name, "template": template, "brand": ctx.brand, "channelId": run.channel, "card": seg.card, "format": format.id(),
            "paths": [file.to_string_lossy()], "cuts": cuts,
        });
        if let Some(f) = seg.focus_x {
            params["focusX"] = json!(f);
        }
        match app.session.execute("reel.new", params) {
            Ok(v) => {
                made += 1;
                // Flick: animated scenes for the Reel's words, opening on its hook.
                let lines = seg.reel_lines();
                if app.ui.reel.ai.flick
                    && !lines.is_empty()
                    && let Some(seq) = v.get("sequence").and_then(|x| x.as_u64())
                    && let Some(s) = super::studio(&mut rt())
                {
                    let size = app.session.active_sequence().map_or((1080, 1920), |q| (q.settings.width, q.settings.height));
                    match filmcraft_studio::project_folder(app.session.path.as_deref(), "Motion") {
                        Ok(parent) => super::motion::queue_plan(seq, &s, run.channel, parent, lines, size, seg.motion_direction(), super::motion::brand_files(), app.ui.reel.ai.flick_review, super::motion::footage_area(app)),
                        Err(e) => log(&mut rt(), format!("Flick skipped for “{name}”: {e}")),
                    }
                }
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
