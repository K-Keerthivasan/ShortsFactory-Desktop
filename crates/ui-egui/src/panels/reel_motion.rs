//! Reel Studio ▸ Design ▸ Motion: Flick's animated Remotion scenes for a Reel, run in the app
//! (`filmcraft_studio::flick`): transcript → plan to approve → build → onto the Reel.
//!
//! The AI panel's analyzer queues Flick for every Reel it makes ([`queue_plan`]), with the
//! moment's transcript in Reel time and its hook as the direction; one Flick job runs at a time.
//! Each Reel (sequence) keeps its own state here; finished scenes go onto the Reel the first
//! time it is open ([`tick`]).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use egui::RichText;
use filmcraft_studio::Studio;
use filmcraft_studio::flick::{self, Run, Scene};
use serde_json::json;

use super::{Done, reel_info, rt, spawn};
use crate::FilmcraftApp;

/// One Reel's Flick work.
#[derive(Default)]
struct State {
    run: Option<Run>,
    stage: String,
    error: Option<String>,
    transcript: String,
    direction: String,
    feedback: HashMap<String, String>,
    studio_url: Option<String>,
    /// Waiting in the queue or running.
    working: bool,
    /// The built scenes are on the Reel.
    placed: bool,
}

static STATES: Mutex<Option<HashMap<u64, State>>> = Mutex::new(None);

fn with<R>(seq: u64, f: impl FnOnce(&mut State) -> R) -> R {
    let mut g = STATES.lock().unwrap_or_else(PoisonError::into_inner);
    f(g.get_or_insert_with(HashMap::new).entry(seq).or_default())
}

/// A job for the Flick worker.
enum Job {
    /// Start a run and plan it; build at once unless the plan is to be reviewed.
    Plan {
        seq: u64,
        studio: Studio,
        channel: i64,
        parent: PathBuf,
        lines: Vec<(f64, f64, String)>,
        size: (u32, u32),
        direction: String,
        brand: Vec<PathBuf>,
        review: bool,
        footage_area: Option<[f64; 4]>,
    },
    Build { seq: u64, studio: Studio, run: Run },
    Revise { seq: u64, studio: Studio, run: Run, id: String, feedback: String },
    Setup { studio: Studio },
}

static QUEUE: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());
/// A worker is running the queue (set and cleared with the queue locked).
static WORKER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const TASK: &str = "Flick";

fn running() -> bool {
    rt().tasks.iter().any(|(l, _)| l == TASK)
}

/// Queues a job; one worker runs the queue in order.
fn push(job: Job) {
    if let Job::Plan { seq, .. } | Job::Build { seq, .. } | Job::Revise { seq, .. } = &job {
        with(*seq, |s| {
            s.working = true;
            s.error = None;
            s.stage = "Waiting for the Flick queue".into();
        });
    }
    {
        let mut q = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
        q.push_back(job);
        if WORKER.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
    }
    spawn(&mut rt(), TASK, || {
        let mut done = 0;
        loop {
            let job = {
                let mut q = QUEUE.lock().unwrap_or_else(PoisonError::into_inner);
                match q.pop_front() {
                    Some(j) => j,
                    None => {
                        WORKER.store(false, std::sync::atomic::Ordering::SeqCst);
                        break;
                    }
                }
            };
            done += 1;
            // A job that panics must not stop the queue.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(job)));
        }
        Done::Notice((format!("Flick: {done} job(s) finished"), true))
    });
}

fn run_job(job: Job) {
    match job {
        Job::Setup { studio } => {
            let r = flick::setup(&studio, &|_| {});
            with(0, |s| {
                s.error = r.err().map(|e| e.to_string());
                s.stage = if s.error.is_none() { "Flick is set up".into() } else { String::new() };
            });
        }
        Job::Plan { seq, studio, channel, parent, lines, size, direction, brand, review, footage_area } => {
            let say = |m: &str| with(seq, |s| s.stage = m.to_string());
            let r = (|| -> filmcraft_studio::Result<Run> {
                say("Preparing the Flick workspace");
                let mut run = flick::start(&studio, &parent, lines, size, &direction, &brand)?;
                run.footage_area = footage_area;
                say("The AI is planning the scenes (Flick's rules)");
                flick::plan(&studio, channel, &mut run)?;
                with(seq, |s| s.run = Some(run.clone()));
                if !review {
                    flick::build(&studio, &mut run, &say)?;
                }
                Ok(run)
            })();
            finish(seq, r, review);
        }
        Job::Build { seq, studio, mut run } => {
            let say = |m: &str| with(seq, |s| s.stage = m.to_string());
            let r = flick::build(&studio, &mut run, &say).map(|()| run);
            finish(seq, r, false);
        }
        Job::Revise { seq, studio, mut run, id, feedback } => {
            let say = |m: &str| with(seq, |s| s.stage = m.to_string());
            let r = flick::revise(&studio, &mut run, &id, &feedback, &say).map(|()| run);
            with(seq, |s| {
                s.feedback.remove(&id);
            });
            finish(seq, r, false);
        }
    }
}

fn finish(seq: u64, r: filmcraft_studio::Result<Run>, planned_only: bool) {
    with(seq, |s| {
        s.working = false;
        match r {
            Ok(run) => {
                let built = run.scenes.iter().filter(|x| x.file.is_some()).count();
                s.stage = if planned_only {
                    format!("{} scene(s) planned: review and approve them", run.scenes.len())
                } else {
                    format!("{built} scene(s) rendered: they go onto the Reel when it is open")
                };
                s.placed = false;
                s.run = Some(run);
            }
            Err(e) => {
                s.stage.clear();
                s.error = Some(e.to_string());
            }
        }
    });
}

/// Queues Flick for a Reel the AI panel just made: the moment's words (Reel time) and its
/// hook as the direction; the plan waits for review when `review`.
#[allow(clippy::too_many_arguments)]
pub(super) fn queue_plan(
    seq: u64,
    studio: &Studio,
    channel: i64,
    parent: PathBuf,
    lines: Vec<(f64, f64, String)>,
    size: (u32, u32),
    direction: String,
    brand: Vec<PathBuf>,
    review: bool,
    footage_area: Option<[f64; 4]>,
) {
    with(seq, |s| {
        s.transcript = lines.iter().map(|(a, _, t)| format!("[{a:.1}] {t}")).collect::<Vec<_>>().join("\n");
        s.direction = direction.clone();
    });
    push(Job::Plan { seq, studio: studio.clone(), channel, parent, lines, size, direction, brand, review, footage_area });
}

/// Every frame: built scenes go onto the open Reel once.
pub(super) fn tick(app: &mut FilmcraftApp) {
    let Some(seq) = app.session.state.active_sequence.map(|q| q.0) else { return };
    let ready = with(seq, |s| s.run.as_ref().filter(|r| !s.placed && !s.working && r.scenes.iter().any(|x| x.file.is_some())).map(|r| r.scenes.clone()));
    if let Some(scenes) = ready {
        with(seq, |s| s.placed = true);
        app.ui.status = match place(app, &scenes) {
            Ok(n) => format!("Flick: {n} animated scene(s) added to this Reel (top track)"),
            Err(e) => format!("Flick: could not add the scenes: {e}"),
        };
    }
}

/// The Reel's footage: its file, where it starts in the source, and how long it lasts.
fn footage(app: &FilmcraftApp) -> Option<(PathBuf, f64, f64, f64)> {
    let info = reel_info(app)?;
    let seq = app.session.active_sequence()?;
    let (_, ti) = seq.find_item(filmcraft_project::ClipId(info.main_clip?))?;
    let media = app.session.project.item(ti.item)?.as_media()?;
    let filmcraft_project::MediaRef::File { path } = &media.media else { return None };
    Some((PathBuf::from(path), ti.source_in.seconds(), ti.start.seconds(), ti.duration.seconds()))
}

fn reel_seconds(app: &FilmcraftApp) -> f64 {
    app.session.active_sequence().map_or(0.0, |q| q.duration().seconds())
}

/// Where the open Reel's template shows the footage (x, y, w, h), so overlays avoid its card.
pub(super) fn footage_area(app: &FilmcraftApp) -> Option<[f64; 4]> {
    let info = reel_info(app)?;
    let q = app.session.active_sequence()?;
    let plan = filmcraft_reel::plan_for(&info.template, &info.brand, &info.card, q.duration().seconds().max(1.0), q.settings.height).ok()?;
    Some([plan.slot.x, plan.slot.y, plan.slot.w, plan.slot.h])
}

/// The workspace logo, for Flick's brand assets.
pub(super) fn brand_files() -> Vec<PathBuf> {
    rt().brand.as_ref().and_then(|b| b.logo_path.clone()).map(PathBuf::from).into_iter().collect()
}

pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, s: &Studio) {
    let t = app.tokens;
    ui.label(
        RichText::new("Flick (Creatorberry) turns the Reel's words into original animated scenes in Remotion, one per line, placed over the Reel at their times.")
            .small()
            .color(t.text_dim),
    );
    let st = flick::status(s);
    if !(st.skill && st.remotion) {
        ui.label(
            RichText::new(match &st.node {
                Some(v) => format!("Node.js {v} · Flick {} · Remotion {}", if st.skill { "ready" } else { "not fetched" }, if st.remotion { "installed" } else { "not installed" }),
                None => "Node.js 20+ is needed (winget install OpenJS.NodeJS.LTS).".to_string(),
            })
            .small(),
        );
        let b = ui.add_enabled(!running() && st.node.is_some(), egui::Button::new("Set up Flick (once, a few hundred MB)"));
        app.auto.add_response("reel.motion.setup", &b, "Set up Flick");
        if b.on_hover_text("Fetches Flick's files (pinned version) and installs Remotion and its headless browser in the app's tools folder").clicked() {
            with(0, |x| x.stage = "Setting up Flick (this takes a few minutes)".into());
            push(Job::Setup { studio: s.clone() });
        }
        status_line(ui, &t, 0);
        return;
    }
    ui.label(RichText::new(format!("Builder: {} (FLICK_AGENT in Plugin Settings ▸ AI)", st.agent)).small().color(t.text_dim));
    let (Some(info), Some(seq)) = (reel_info(app), app.session.state.active_sequence.map(|q| q.0)) else {
        ui.label(RichText::new("Open or start a Reel first.").small().color(t.text_dim));
        return;
    };
    ui.separator();
    let working = with(seq, |x| x.working);
    let has_run = with(seq, |x| x.run.is_some());

    // 1-2. The words and the direction.
    if !has_run {
        ui.label(RichText::new("1 · The Reel's words").strong());
        let mut transcript = with(seq, |x| x.transcript.clone());
        let mut direction = with(seq, |x| x.direction.clone());
        ui.horizontal_wrapped(|ui| {
            let ready = filmcraft_studio::virality::whisper_ready();
            let b = ui.add_enabled(!working && ready.is_ok() && footage(app).is_some(), egui::Button::new("Transcribe the footage (Whisper)"));
            app.auto.add_response("reel.motion.transcribe", &b, "Transcribe the Reel's footage");
            if let Err(e) = &ready {
                ui.label(RichText::new(format!("Whisper: {e}; or paste the words below")).small().color(t.text_dim));
            }
            if b.clicked()
                && let Some((file, from, at, len)) = footage(app)
            {
                let st = s.clone();
                with(seq, |x| x.stage = "Transcribing the footage".into());
                spawn(&mut rt(), "Flick transcript", move || match transcribe(&st, &file, from, len) {
                    Ok(lines) => {
                        with(seq, |x| {
                            x.transcript = lines.iter().map(|(a, _, l)| format!("[{:.1}] {l}", a + at)).collect::<Vec<_>>().join("\n");
                            x.stage = format!("{} line(s) transcribed", lines.len());
                        });
                        Done::Message(String::new())
                    }
                    Err(e) => {
                        with(seq, |x| x.error = Some(e.to_string()));
                        Done::Message(String::new())
                    }
                });
            }
        });
        ui.add(
            egui::TextEdit::multiline(&mut transcript)
                .hint_text("The words, one line each; [12.5] at the start of a line sets its time in seconds (without times they spread over the Reel)")
                .desired_rows(5)
                .desired_width(f32::INFINITY),
        );
        ui.label(RichText::new("2 · Your direction").strong());
        ui.add(
            egui::TextEdit::multiline(&mut direction)
                .hint_text("Think like a director: what should happen on screen, what moves, what to emphasise, the feeling, what to avoid")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        with(seq, |x| {
            x.transcript = transcript.clone();
            x.direction = direction.clone();
        });
        let b = ui.add_enabled(!working && !transcript.trim().is_empty() && info.channel_id.is_some(), egui::Button::new("Plan the scenes"));
        app.auto.add_response("reel.motion.plan", &b, "Plan the Flick scenes");
        if b.clicked()
            && let Some(ch) = info.channel_id
        {
            let size = app.session.active_sequence().map_or((1080, 1920), |q| (q.settings.width, q.settings.height));
            match filmcraft_studio::project_folder(app.session.path.as_deref(), "Motion") {
                Ok(parent) => queue_plan(seq, s, ch, parent, parse_lines(&transcript, reel_seconds(app)), size, direction, brand_files(), true, footage_area(app)),
                Err(e) => app.ui.status = e.to_string(),
            }
        }
        status_line(ui, &t, seq);
        return;
    }

    // 3. The plan to edit and approve, then the scenes.
    let Some(mut run) = with(seq, |x| x.run.clone()) else { return };
    let built = run.scenes.iter().any(|sc| sc.file.is_some());
    ui.label(RichText::new(if built { "3 · The scenes" } else { "3 · Review the plan, then build" }).strong());
    ui.label(RichText::new(format!("Workspace: {}", run.dir.display())).small().color(t.text_dim));
    let mut remove = None;
    let mut changed = false;
    for (i, sc) in run.scenes.iter_mut().enumerate() {
        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{}.", i + 1)).strong());
                changed |= ui.add_enabled(!built, egui::TextEdit::singleline(&mut sc.name).desired_width(200.0)).changed();
                changed |= ui.add_enabled(!built, egui::DragValue::new(&mut sc.start).speed(0.1).suffix(" s").range(0.0..=3600.0)).changed();
                ui.label("–");
                changed |= ui.add_enabled(!built, egui::DragValue::new(&mut sc.end).speed(0.1).suffix(" s").range(0.0..=3600.0)).changed();
                if !built && !working && ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
            });
            ui.label(RichText::new(&sc.transcript).small().italics());
            if built {
                scene_actions(app, ui, s, seq, sc, working);
            } else {
                changed |= field(ui, "On screen", &mut sc.on_screen, 2);
                changed |= field(ui, "Text on screen", &mut sc.text, 1);
                changed |= field(ui, "Sequence", &mut sc.sequence, 1);
                changed |= field(ui, "Sound effect", &mut sc.sound, 1);
                changed |= field(ui, "Transition", &mut sc.transition, 1);
            }
        });
    }
    if let Some(i) = remove {
        run.scenes.remove(i);
        changed = true;
    }
    if changed && !working {
        for sc in &mut run.scenes {
            sc.end = sc.end.max(sc.start + 0.5);
            if !built {
                sc.id = flick::kebab(&sc.name);
            }
        }
        let _ = run.save();
        with(seq, |x| x.run = Some(run.clone()));
    }
    ui.horizontal_wrapped(|ui| {
        if !built {
            let b = ui.add_enabled(!working && !run.scenes.is_empty(), egui::Button::new(format!("Approve and build {} scene(s)", run.scenes.len())));
            app.auto.add_response("reel.motion.build", &b, "Approve and build the Flick scenes");
            if b.on_hover_text("The coding agent writes each scene in Remotion, then the app renders them (can take a while)").clicked() {
                push(Job::Build { seq, studio: s.clone(), run: run.clone() });
            }
        } else {
            let b = ui.add_enabled(!working, egui::Button::new("Add the scenes to the Reel again"));
            app.auto.add_response("reel.motion.place", &b, "Add the Flick scenes to the Reel");
            if b.on_hover_text("Puts the rendered scenes on a new top track at their times (after a revision, delete the old track first)").clicked() {
                app.ui.status = match place(app, &run.scenes) {
                    Ok(n) => format!("{n} Flick scene(s) added to the Reel"),
                    Err(e) => format!("Could not add the scenes: {e}"),
                };
            }
            if ui.add_enabled(!working, egui::Button::new("Preview in Remotion Studio")).clicked() {
                let r = run.clone();
                with(seq, |x| x.stage = "Starting Remotion Studio".into());
                spawn(&mut rt(), "Remotion Studio", move || {
                    match flick::open_studio(&r) {
                        Ok(url) => with(seq, |x| {
                            x.stage = format!("Remotion Studio is open at {url}");
                            x.studio_url = Some(url);
                        }),
                        Err(e) => with(seq, |x| x.error = Some(e.to_string())),
                    }
                    Done::Message(String::new())
                });
            }
            if let Some(url) = with(seq, |x| x.studio_url.clone()) {
                ui.hyperlink_to("Open Studio", url);
            }
        }
        if ui.add_enabled(!working, egui::Button::new("Start over")).clicked() {
            with(seq, |x| *x = State::default());
        }
    });
    status_line(ui, &t, seq);
}

fn field(ui: &mut egui::Ui, label: &str, value: &mut String, rows: usize) -> bool {
    ui.label(RichText::new(label).small());
    ui.add(egui::TextEdit::multiline(value).desired_rows(rows).desired_width(f32::INFINITY)).changed()
}

/// A rendered scene: play it, revise it from feedback, or save it as reusable.
fn scene_actions(app: &mut FilmcraftApp, ui: &mut egui::Ui, s: &Studio, seq: u64, sc: &Scene, working: bool) {
    let t = app.tokens;
    match &sc.file {
        Some(f) => {
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("Play").clicked()
                    && let Err(e) = super::super::menu_dialogs::open_path(app, ui.ctx(), &f.to_string_lossy(), false)
                {
                    app.ui.status = e;
                }
                ui.label(RichText::new(f.display().to_string()).small().color(t.text_dim));
            });
        }
        None => {
            ui.label(RichText::new("Not rendered").small().color(t.danger));
        }
    }
    let mut feedback = with(seq, |x| x.feedback.get(&sc.id).cloned().unwrap_or_default());
    ui.add(egui::TextEdit::singleline(&mut feedback).hint_text("What should change in this scene?").desired_width(f32::INFINITY));
    with(seq, |x| x.feedback.insert(sc.id.clone(), feedback.clone()));
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(!working && !feedback.trim().is_empty(), egui::Button::new("Revise this scene")).clicked()
            && let Some(run) = with(seq, |x| x.run.clone())
        {
            push(Job::Revise { seq, studio: s.clone(), run, id: sc.id.clone(), feedback: feedback.clone() });
        }
        if ui.add_enabled(!working && sc.file.is_some(), egui::Button::new("Save as reusable")).on_hover_text("Adds its component to Flick's animation library for later runs").clicked()
            && let Some(r) = with(seq, |x| x.run.clone())
        {
            app.ui.status = match flick::save_reusable(s, &r, &sc.id, &sc.on_screen, &sc.transcript, "scenes that do not show this pattern") {
                Ok(p) => format!("Saved to Flick's library: {}", p.display()),
                Err(e) => e.to_string(),
            };
        }
    });
}

fn status_line(ui: &mut egui::Ui, t: &crate::theme::Tokens, seq: u64) {
    let (stage, error, working) = with(seq, |x| (x.stage.clone(), x.error.clone(), x.working));
    if working || (seq == 0 && running()) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(stage).small());
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    } else if let Some(e) = error {
        ui.label(RichText::new(e).small().color(t.danger));
    } else if !stage.is_empty() {
        ui.label(RichText::new(stage).small().color(t.text_dim));
    }
}

/// Whisper on the Reel's part of the footage: (start, end, text) from the clip's start.
fn transcribe(s: &Studio, file: &std::path::Path, from: f64, len: f64) -> filmcraft_studio::Result<Vec<(f64, f64, String)>> {
    let ff = filmcraft_studio::ytdlp::ffmpeg(s).ok_or_else(|| filmcraft_studio::StudioError::Other("FFmpeg is needed".into()))?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let tmp = std::env::temp_dir().join(format!("flick-audio-{}-{stamp}.m4a", std::process::id()));
    let mut c = std::process::Command::new(ff);
    c.args(["-y", "-hide_banner", "-loglevel", "error", "-ss", &format!("{from:.3}"), "-t", &format!("{len:.3}"), "-i"]).arg(file).args(["-vn", "-c:a", "aac"]).arg(&tmp);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    if !c.status().is_ok_and(|st| st.success()) {
        return Err(filmcraft_studio::StudioError::Other("FFmpeg could not take the footage's sound".into()));
    }
    let lines = filmcraft_studio::virality::whisper(&tmp);
    let _ = std::fs::remove_file(&tmp);
    let lines = lines?;
    Ok(lines.iter().enumerate().map(|(i, (a, l))| (*a, lines.get(i + 1).map_or(len, |n| n.0), l.clone())).collect())
}

/// "[12.5] words" lines (or plain lines spread over the Reel) as (start, end, text).
pub fn parse_lines(text: &str, total: f64) -> Vec<(f64, f64, String)> {
    let raw: Vec<(Option<f64>, String)> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| match l.strip_prefix('[').and_then(|r| r.split_once(']')) {
            Some((t, rest)) => (t.trim().parse().ok(), rest.trim().to_string()),
            None => (None, l.to_string()),
        })
        .collect();
    let total = if total > 0.0 { total } else { raw.len() as f64 * 3.0 };
    let n = raw.len().max(1) as f64;
    let starts: Vec<f64> = raw.iter().enumerate().map(|(i, (t, _))| t.unwrap_or(i as f64 * total / n)).collect();
    raw.into_iter().enumerate().map(|(i, (_, l))| (starts[i], starts.get(i + 1).copied().unwrap_or(total).max(starts[i] + 0.5), l)).collect()
}

/// Puts the rendered scenes on a new top video track (their sound on a new audio track) at
/// their times. Returns how many were placed.
fn place(app: &mut FilmcraftApp, scenes: &[Scene]) -> Result<usize, String> {
    let (v, a) = app.session.active_sequence().map(|q| (q.video_tracks.len(), q.audio_tracks.len())).ok_or("no sequence is open")?;
    app.session
        .execute("sequence.addTracks", json!({"video": 1, "audio": 1, "videoAfter": format!("V{v}"), "audioAfter": format!("A{a}")}))
        .map_err(|e| e.to_string())?;
    let (vt, at) = (format!("V{}", v + 1), format!("A{}", a + 1));
    let mut n = 0;
    for sc in scenes {
        let Some(f) = sc.file.as_ref().filter(|f| f.is_file()) else { continue };
        let imported = app.session.execute("file.import", json!({"paths": [f.to_string_lossy()]})).map_err(|e| e.to_string())?;
        let item = imported.get("items").and_then(|x| x.as_array()).and_then(|x| x.first()).and_then(|x| x.as_u64()).ok_or("a scene could not be imported")?;
        app.session.execute("timeline.place", json!({"item": item, "seconds": sc.start, "track": vt, "audioTrack": at})).map_err(|e| e.to_string())?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    #[test]
    fn typed_lines_keep_their_times_and_plain_ones_spread() {
        let l = super::parse_lines("[1.5] first\n[4] second", 10.0);
        assert_eq!(l, vec![(1.5, 4.0, "first".to_string()), (4.0, 10.0, "second".to_string())]);
        let l = super::parse_lines("a\nb", 10.0);
        assert_eq!(l, vec![(0.0, 5.0, "a".to_string()), (5.0, 10.0, "b".to_string())]);
    }
}
