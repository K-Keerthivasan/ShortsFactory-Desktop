//! Reel Studio ▸ Design ▸ Captions: captions from what is said (local Whisper, word-timed) or
//! from a written script, in a look the channel keeps.
//!
//! Listening runs on a worker (a separate Whisper process), so the app never waits on it; the
//! captions are then placed on the open sequence's "Social captions" track in one undoable edit.
//! Every caption stays editable (Captions panel, Program monitor) and exports burn them in. The
//! AI editors (Claude, Codex, Hermes) do the same through `studio.captions`, correcting names and
//! words first, or make animated captions with Remotion from the exported words.

use std::sync::Mutex;

use egui::RichText;
use filmcraft_studio::captions::{self as cap, Look, Timed};
use serde::{Deserialize, Serialize};

use crate::FilmcraftApp;

/// What the tab is editing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Draft {
    pub look: Look,
    /// Text to show instead of what is heard (a script, or a written video with no speech).
    pub script: String,
    /// The workspace whose look was loaded.
    pub channel: Option<i64>,
}

struct Job {
    sequence: u64,
    look: Look,
    script: String,
    status: String,
    done: Option<Result<Vec<(u64, Vec<Timed>)>, String>>,
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);

fn job<R>(f: impl FnOnce(&mut Option<Job>) -> R) -> R {
    f(&mut JOB.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn working() -> bool {
    job(|j| j.as_ref().is_some_and(|j| j.done.is_none()))
}

/// Places finished captions (called every frame from the panel's poll).
pub(super) fn tick(app: &mut FilmcraftApp) {
    let Some(j) = job(|j| if j.as_ref().is_some_and(|j| j.done.is_some()) { j.take() } else { None }) else { return };
    let seq = app.session.state.active_sequence.map(|s| s.0);
    app.ui.status = match j.done {
        Some(Err(e)) => format!("Captions: {e}"),
        _ if seq != Some(j.sequence) => "Captions: open the sequence they were made for, then make them again".into(),
        Some(Ok(heard)) => {
            let script = Some(j.script.as_str()).filter(|s| !s.trim().is_empty());
            match cap::finish(&mut app.session, &heard, &j.look, script) {
                Ok(v) => format!("{} captions made on the \"{}\" track: edit any of them in the Captions panel or the Program monitor", v["captions"], cap::TRACK),
                Err(e) => format!("Captions: {e}"),
            }
        }
        None => String::new(),
    };
}

/// Starts making captions: listens to what has no transcript yet (worker), then places them.
fn start(app: &mut FilmcraftApp) {
    let Some(seq) = app.session.state.active_sequence else {
        app.ui.status = "Open a sequence first".into();
        return;
    };
    let d = &app.ui.reel.captions;
    if let Err(e) = d.look.check() {
        app.ui.status = e.to_string();
        return;
    }
    let heard = cap::heard(&app.session);
    let missing: Vec<(u64, std::path::PathBuf)> = heard.iter().filter(|(_, _, has)| !has).map(|(i, p, _)| (*i, p.clone())).collect();
    // A written video with no sound to listen to: only the script.
    let script_only = heard.is_empty() && !d.script.trim().is_empty();
    let (look, script) = (d.look.clone(), d.script.clone());
    job(|j| *j = Some(Job { sequence: seq.0, look, script, status: "Starting…".into(), done: None }));
    if missing.is_empty() || script_only {
        job(|j| {
            if let Some(j) = j {
                j.done = Some(Ok(Vec::new()));
            }
        });
        return;
    }
    std::thread::spawn(move || {
        let r = cap::transcribe(&missing, &|m| {
            let m = m.to_string();
            job(|j| {
                if let Some(j) = j {
                    j.status = m;
                }
            });
        })
        .map_err(|e| e.to_string());
        job(|j| {
            if let Some(j) = j {
                j.done = Some(r);
            }
        });
    });
}

fn colour_field(ui: &mut egui::Ui, label: &str, value: &mut String, alpha: bool) -> bool {
    let h = value.trim().trim_start_matches('#').to_string();
    let byte = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok()).unwrap_or(255);
    let mut c = egui::Color32::from_rgba_unmultiplied(byte(0), byte(2), byte(4), if h.len() == 8 { byte(6) } else { 255 });
    ui.label(label);
    let changed = if alpha {
        egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::OnlyBlend).changed()
    } else {
        let mut rgb = [c.r(), c.g(), c.b()];
        let r = egui::color_picker::color_edit_button_srgb(ui, &mut rgb).changed();
        c = egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        r
    };
    if changed {
        *value = if alpha { format!("#{:02X}{:02X}{:02X}{:02X}", c.r(), c.g(), c.b(), c.a()) } else { format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b()) };
    }
    changed
}

pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui, ctx: &super::Ctx) {
    let t = app.tokens;
    // The workspace's own look when it changes.
    if app.ui.reel.captions.channel != ctx.channel {
        app.ui.reel.captions.channel = ctx.channel;
        if let Some(p) = super::rt().profile.clone() {
            app.ui.reel.captions.look = p.captions;
        }
    }
    ui.label(
        RichText::new("Captions from what is said in the video (local Whisper), or from your own text for written content. They go on a \"Social captions\" track: every caption stays editable and exports burn them in.")
            .small()
            .color(t.text_dim),
    );
    let has_track = app.session.active_sequence().is_some_and(|q| q.caption_tracks.iter().any(|tr| tr.name == cap::TRACK));
    let d = &mut app.ui.reel.captions;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Style").small());
        for name in cap::PRESETS {
            let on = d.look.preset == *name;
            if ui.selectable_label(on, *name).clicked()
                && let Some(l) = cap::preset(name)
            {
                d.look = l;
            }
        }
    });
    let l = &mut d.look;
    egui::Grid::new("reel-captions-look").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
        ui.label("Font");
        ui.add(egui::TextEdit::singleline(&mut l.font).desired_width(160.0));
        ui.end_row();
        ui.label("Size");
        ui.add(egui::DragValue::new(&mut l.size).range(8.0..=300.0).speed(0.5));
        ui.end_row();
        ui.label("Words per caption");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut l.words).range(1..=30));
            ui.selectable_value(&mut l.lines, 1, "1 line");
            ui.selectable_value(&mut l.lines, 2, "2 lines");
        });
        ui.end_row();
        ui.label("Position");
        ui.horizontal(|ui| {
            for (id, label) in [("upper", "Upper"), ("middle", "Middle"), ("lower", "Lower")] {
                ui.selectable_value(&mut l.position, id.to_string(), label);
            }
        });
        ui.end_row();
    });
    ui.horizontal_wrapped(|ui| {
        colour_field(ui, "Text", &mut l.color, false);
        ui.checkbox(&mut l.uppercase, "CAPITALS");
        ui.label("Outline");
        ui.add(egui::DragValue::new(&mut l.outline).range(0.0..=40.0).speed(0.2));
        colour_field(ui, "", &mut l.outline_color, false);
        ui.checkbox(&mut l.background, "Box");
        if l.background {
            colour_field(ui, "", &mut l.background_color, true);
        }
    });
    ui.label(RichText::new("Text to show (optional): a script, corrected names, or the words of a written video with no speech. Empty: what is said.").small());
    ui.add(egui::TextEdit::multiline(&mut d.script).desired_rows(3).desired_width(f32::INFINITY).hint_text("Leave empty to caption what is said"));
    let busy = working();
    ui.horizontal_wrapped(|ui| {
        let b = ui.add_enabled(!busy && app.session.active_sequence().is_some(), egui::Button::new(if has_track { "Make captions again" } else { "Make captions" }));
        app.auto.add_response("reel.captions.make", &b, "Make captions");
        if b.clicked() {
            start(app);
        }
        let b = ui.add_enabled(has_track, egui::Button::new("Apply this style")).on_hover_text("Restyle the captions without changing their words");
        app.auto.add_response("reel.captions.style", &b, "Apply caption style");
        if b.clicked() {
            app.ui.status = match cap::restyle(&mut app.session, &app.ui.reel.captions.look) {
                Ok(()) => "Caption style applied".into(),
                Err(e) => e.to_string(),
            };
        }
        let b = ui.add_enabled(ctx.channel.is_some(), egui::Button::new("Save as the channel's style"));
        app.auto.add_response("reel.captions.save", &b, "Save caption style for the channel");
        if b.clicked()
            && let Some(ch) = ctx.channel
        {
            let mut p = super::rt().profile.clone().unwrap_or_default();
            p.captions = app.ui.reel.captions.look.clone();
            app.ui.status = match ctx.s.save_profile(ch, &p) {
                Ok(()) => {
                    super::load_channel(&mut super::rt(), ch);
                    "Caption style saved for this channel".into()
                }
                Err(e) => e.to_string(),
            };
        }
    });
    if busy {
        let status = job(|j| j.as_ref().map(|j| j.status.clone()).unwrap_or_default());
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(status).small());
        });
    }
    ui.separator();
    ui.label(RichText::new("With an AI editor").strong());
    ui.label(
        RichText::new("Claude, Codex or Hermes listen, fix names and spelling against the script, split the captions on the beat of speech and keep them clear of faces and the layout's words. Or they make animated word-by-word captions with Remotion.")
            .small()
            .color(t.text_dim),
    );
    ui.horizontal_wrapped(|ui| {
        let on = filmcraft_studio::ai_editor::status(&ctx.s).enabled;
        for (agent, label) in [("claude", "Captions with Claude"), ("codex", "Captions with Codex"), ("hermes", "Captions with Hermes")] {
            let b = ui.add_enabled(on, egui::Button::new(label)).on_disabled_hover_text("Set up AI editors in Plugin Settings ▸ AI, then restart the app");
            app.auto.add_response(&format!("reel.captions.agent.{agent}"), &b, label);
            if b.clicked() {
                let look = serde_json::to_string(&app.ui.reel.captions.look).unwrap_or_default();
                let script = app.ui.reel.captions.script.trim().to_string();
                let task = format!(
                    "Task: captions for the open sequence. Use `studio.captions` (action \"heard\", then \"make\"). Read the transcript (`transcript.inspect`) and fix misheard words and names{} by passing your corrected `words` (sequence seconds) to `studio.captions` make. Keep each caption to a few words, split on the rhythm of speech, never over faces or the layout's headline; check frames with `render_frame`. Start with this look: {look}. For animated word-by-word captions, export the words with `studio.captions` action \"remotion\" and build them with Remotion (Flick or the Remotion MCP) as a transparent overlay on its own track. Tell me what you changed.",
                    if script.is_empty() { String::new() } else { format!(" (the script says: \"{script}\")") }
                );
                super::edit_with_agent(app, ctx, agent, &task);
            }
        }
    });
    let b = ui.add_enabled(app.session.active_sequence().is_some(), egui::Button::new("Export words for Remotion"));
    app.auto.add_response("reel.captions.remotion", &b, "Export caption words for Remotion");
    if b.clicked() {
        let dir = filmcraft_studio::project_folder(app.session.path.as_deref(), "Captions");
        let script = Some(app.ui.reel.captions.script.as_str()).filter(|s| !s.trim().is_empty());
        app.ui.status = match dir {
            Ok(d) => {
                let file = d.join("captions.remotion.json");
                match cap::export_remotion(&app.session, script, &file) {
                    Ok(n) => format!("{n} words written to {} (Remotion's caption format)", file.display()),
                    Err(e) => e.to_string(),
                }
            }
            Err(e) => e.to_string(),
        };
    }
}
