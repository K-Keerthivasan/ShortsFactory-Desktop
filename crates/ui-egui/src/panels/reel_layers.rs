//! Reel Studio ▸ Design ▸ Layers: every layer the layout made, editable after it is applied.
//!
//! The template builds the Reel from ordinary editor parts: graphic clips (Card, Overlay) whose
//! layers are text and shapes, the logo, and the footage clips. This lists them all and edits
//! each through the editor's own commands (`graphics.set`, `graphics.setText`,
//! `effects.setParam`), so every change is a normal, undoable edit and also shows in the
//! Graphics and Effect Controls panels. Choosing another layout rebuilds the layers.

use egui::RichText;
use filmcraft_project::{ParamValue, TrackItem, graphic};
use serde_json::json;

use crate::FilmcraftApp;

fn vec2(e: &filmcraft_project::EffectInstance, k: &str) -> Option<(f64, f64)> {
    match e.params.get(k).map(|p| &p.value) {
        Some(ParamValue::Vec2(v)) => Some((v.x, v.y)),
        _ => None,
    }
}

fn float(e: &filmcraft_project::EffectInstance, k: &str) -> Option<f64> {
    match e.params.get(k).map(|p| &p.value) {
        Some(ParamValue::Float(v)) => Some(*v),
        _ => None,
    }
}

fn colour(e: &filmcraft_project::EffectInstance, k: &str) -> Option<[f32; 4]> {
    match e.params.get(k).map(|p| &p.value) {
        Some(ParamValue::Color(c)) => Some(*c),
        _ => None,
    }
}

fn text(e: &filmcraft_project::EffectInstance, k: &str) -> String {
    match e.params.get(k).map(|p| &p.value) {
        Some(ParamValue::Text(s)) => s.clone(),
        _ => String::new(),
    }
}

fn hex(c: [f32; 4]) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
}

/// The open Reel's parts, top track first: (track name, a clip, every clip it stands for).
/// A graphic clip is its own part (its layers are listed); the media clips of a track (the cuts
/// of the footage, their backdrops) are one part, edited together.
fn parts(app: &FilmcraftApp) -> Vec<(String, TrackItem, Vec<u64>)> {
    let Some(q) = app.session.active_sequence() else { return Vec::new() };
    let mut out = Vec::new();
    for tr in q.video_tracks.iter().rev() {
        let media: Vec<&TrackItem> = tr.items.iter().filter(|i| graphic::layer_indices(&i.effects).is_empty()).collect();
        if let Some(first) = media.first() {
            out.push((tr.name.clone(), (*first).clone(), media.iter().map(|i| i.id.0).collect()));
        }
        for it in tr.items.iter().filter(|i| !graphic::layer_indices(&i.effects).is_empty()) {
            out.push((tr.name.clone(), it.clone(), vec![it.id.0]));
        }
    }
    out
}

pub(super) fn show(app: &mut FilmcraftApp, ui: &mut egui::Ui) {
    let t = app.tokens;
    if super::reel_info(app).is_none() {
        ui.label(RichText::new("Open or start a Reel to edit its layers.").small().color(t.text_dim));
        return;
    }
    ui.label(
        RichText::new("Every part the layout made, editable: text, plates, rules, the logo and the footage. Changes are ordinary edits (undo works) and also show in the Graphics and Effect Controls panels. Choosing another layout rebuilds them.")
            .small()
            .color(t.text_dim),
    );
    let mut actions: Vec<(&'static str, serde_json::Value)> = Vec::new();
    for (track, it, clips) in parts(app) {
        let layers = graphic::layer_indices(&it.effects);
        let clip = it.id.0;
        let title = match (layers.is_empty(), clips.len()) {
            (true, 1) => format!("{track} · {}", it.name),
            (true, n) => format!("{track} · {n} cuts (edited together)"),
            (false, _) => format!("{} · {track} · {} layer(s)", it.name, layers.len()),
        };
        egui::CollapsingHeader::new(RichText::new(title).strong()).id_salt(("reel-layers", clip)).default_open(it.name == "Reel Overlay").show(ui, |ui| {
            if layers.is_empty() {
                // A media clip (the footage, the logo, a backdrop): its Motion.
                let Some(m) = it.effects.iter().find(|e| e.effect == "motion") else {
                    ui.label(RichText::new("No editable position").small().color(t.text_dim));
                    return;
                };
                let (mut x, mut y) = vec2(m, "position").unwrap_or((540.0, 960.0));
                let mut scale = float(m, "scale").unwrap_or(100.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("X");
                    let a = ui.add(egui::DragValue::new(&mut x).speed(2.0));
                    ui.label("Y");
                    let b = ui.add(egui::DragValue::new(&mut y).speed(2.0));
                    // Every cut of the track moves and scales together.
                    if a.changed() || b.changed() {
                        for c in &clips {
                            actions.push(("effects.setParam", json!({"clip": c, "effect": "motion", "param": "position", "value": [x, y], "merge": format!("pos-{clip}")})));
                        }
                    }
                    ui.label("Scale");
                    if ui.add(egui::DragValue::new(&mut scale).speed(0.5).range(1.0..=1000.0).suffix(" %")).changed() {
                        for c in &clips {
                            actions.push(("effects.setParam", json!({"clip": c, "effect": "motion", "param": "scale", "value": scale, "merge": format!("scale-{clip}")})));
                        }
                    }
                });
                return;
            }
            for (li, &ei) in layers.iter().enumerate().rev() {
                let e = &it.effects[ei];
                let is_text = e.effect == graphic::TEXT_LAYER;
                let name = graphic::layer_display_name(e, li);
                ui.group(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        let mut on = e.enabled;
                        if ui.checkbox(&mut on, RichText::new(&name).strong()).on_hover_text("Show or hide this layer").changed() {
                            actions.push(("graphics.set", json!({"clip": clip, "layer": li, "props": {"enabled": on}})));
                        }
                        ui.label(RichText::new(if is_text { "text" } else { "shape" }).small().color(t.text_dim));
                        if ui.small_button("Select").on_hover_text("Select it in the Program monitor and the Graphics panel").clicked() {
                            actions.push(("graphics.selectLayer", json!({"clip": clip, "layers": [li]})));
                        }
                    });
                    let set = |actions: &mut Vec<(&'static str, serde_json::Value)>, k: &str, v: serde_json::Value| {
                        let mut props = serde_json::Map::new();
                        props.insert(k.into(), v);
                        actions.push(("graphics.set", json!({"clip": clip, "layer": li, "props": props, "merge": true})));
                    };
                    ui.horizontal_wrapped(|ui| {
                        if let Some((mut x, mut y)) = vec2(e, "position") {
                            ui.label("X");
                            let a = ui.add(egui::DragValue::new(&mut x).speed(2.0));
                            ui.label("Y");
                            let b = ui.add(egui::DragValue::new(&mut y).speed(2.0));
                            if a.changed() || b.changed() {
                                set(&mut actions, "position", json!([x, y]));
                            }
                        }
                        if is_text {
                            if let Some(mut s) = float(e, "size") {
                                ui.label("Size");
                                if ui.add(egui::DragValue::new(&mut s).speed(0.5).range(4.0..=400.0)).changed() {
                                    set(&mut actions, "size", json!(s));
                                }
                            }
                            if let Some(mut w) = float(e, "box_width").filter(|w| *w > 0.0) {
                                ui.label("Width");
                                if ui.add(egui::DragValue::new(&mut w).speed(2.0).range(20.0..=1080.0)).changed() {
                                    set(&mut actions, "box_width", json!(w));
                                }
                            }
                        } else if let Some((mut w, mut h)) = vec2(e, "size") {
                            ui.label("W");
                            let a = ui.add(egui::DragValue::new(&mut w).speed(2.0).range(0.0..=4000.0));
                            ui.label("H");
                            let b = ui.add(egui::DragValue::new(&mut h).speed(2.0).range(0.0..=4000.0));
                            if a.changed() || b.changed() {
                                set(&mut actions, "size", json!([w, h]));
                            }
                        }
                        if let Some(mut o) = float(e, "opacity") {
                            ui.label("Opacity");
                            if ui.add(egui::DragValue::new(&mut o).speed(0.5).range(0.0..=100.0).suffix(" %")).changed() {
                                set(&mut actions, "opacity", json!(o));
                            }
                        }
                        if let Some(c) = colour(e, "fill_color") {
                            let mut rgba = egui::Rgba::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
                            if egui::color_picker::color_edit_button_rgba(ui, &mut rgba, egui::color_picker::Alpha::Opaque).changed() {
                                set(&mut actions, "fill_color", json!(hex([rgba.r(), rgba.g(), rgba.b(), 1.0])));
                            }
                        }
                    });
                    if is_text {
                        let id = egui::Id::new(("reel-layer-text", clip, li));
                        let mut s: String = ui.data_mut(|d| d.get_temp(id)).unwrap_or_else(|| text(e, "text"));
                        let r = ui.add(egui::TextEdit::multiline(&mut s).desired_rows(1).desired_width(f32::INFINITY));
                        if r.changed() {
                            ui.data_mut(|d| d.insert_temp(id, s.clone()));
                        }
                        if r.lost_focus() {
                            ui.data_mut(|d| d.remove::<String>(id));
                            if s != text(e, "text") {
                                actions.push(("graphics.setText", json!({"clip": clip, "layer": li, "text": s})));
                            }
                        }
                    }
                });
            }
        });
    }
    for (cmd, p) in actions {
        if let Err(e) = app.session.execute(cmd, p) {
            app.ui.status = format!("{cmd}: {e}");
        }
    }
}
