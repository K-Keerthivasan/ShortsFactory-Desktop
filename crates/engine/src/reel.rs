//! Reel Studio (`reel.*`): vertical Reels made from a channel template, edited like any sequence.
//!
//! A template ([`filmcraft_reel`]) is built into ordinary clips on five named video tracks, bottom
//! to top: **Backdrop** (a blurred copy of the footage, when the design uses one), **Card** (the
//! graphic drawn under the footage), **Footage** (the main clip, fitted into the template's slot
//! with Motion and Crop), **Overlay** (the words, scrims and frames, as a graphic clip whose card
//! fields are template properties) and **Logo** (the channel logo). Nothing is special after that:
//! every clip trims, moves, takes effects and keyframes like any other, and `reel.apply` with
//! another template rebuilds only the clips it made.
//!
//! `Project::reel` keeps the [`ReelInfo`]: template, channel, brand, card, the main clip and the
//! library post the Reel publishes as.

use filmcraft_geom::Vec2;
use filmcraft_project::gtemplate::TemplateLink;
use filmcraft_project::{ClipId, EffectInstance, GraphicMeta, ItemId, MediaRef, Param, ParamValue, Project, Track, TrackId, TrackItem, TrackKind, find_effect};
use filmcraft_reel::{Backdrop, Brand, Card, Fit, REEL_FPS, REEL_HEIGHT, REEL_WIDTH, ReelInfo, ReelPlan, TEMPLATES};
use filmcraft_time::{Tick, TimeRange};
use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, f64_p, has_seq, item_p, str_p};
use crate::{EngineError, Result, Session};

const TRACKS: [&str; 5] = ["Backdrop", "Card", "Footage", "Overlay", "Logo"];
const FOOTAGE: usize = 2;
/// Names of the clips a template makes; re-applying a template replaces exactly these.
const MADE: [&str; 4] = ["Reel Backdrop", "Reel Card", "Reel Overlay", "Reel Logo"];

type Run = fn(&mut Session, &Value) -> Result<Value>;
type Enabled = fn(&Session) -> std::result::Result<(), String>;
fn spec(id: &'static str, label: &'static str, menu: &'static [&'static str], params: &'static str, enabled: Enabled, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu, shortcut: None, params, enabled, run, journal: true }
}
fn query(id: &'static str, label: &'static str, params: &'static str, run: Run) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal: false }
}
const M: &[&str] = &["Reel"];

pub fn commands() -> Vec<CommandSpec> {
    vec![
        query("reel.templates", "Reel Templates", r#"{}"#, |_, _| {
            Ok(json!({"templates": TEMPLATES.iter().map(|t| json!({"id": t.id, "name": t.name, "note": t.note, "family": t.family})).collect::<Vec<_>>()}))
        }),
        query("reel.info", "Reel Info", r#"{}"#, |s, _| Ok(json!({"reel": current(s).map(|r| r.to_value()).unwrap_or(Value::Null)}))),
        spec(
            "reel.new",
            "New Reel…",
            M,
            r#"{"name":str?,"format":"reel"|"post"?,"template":str?,"brand":Brand?,"channelId":n?,"card":Card?,"item":itemId? (project footage to build around),"paths":[str]? (a file; reused when already in the project),"seconds":f64? (length without footage)}"#,
            always,
            new_reel,
        ),
        spec(
            "reel.apply",
            "Apply Reel Template",
            M,
            r#"{"template":str,"brand":Brand?,"card":Card?,"clip":clipId? (the main footage; default: the Reel's main clip, else the selected or lowest video clip at the playhead),"item":itemId? (place this media as the footage),"seconds":f64?}"#,
            has_seq,
            apply,
        ),
        spec(
            "reel.setCard",
            "Edit Reel Card",
            M,
            r#"{"title":str?,"caption":str?,"attribution":str?,"badge":str?,"bullets":[str]?,"cta":str?,"instagramCaption":str?,"hashtags":[str]?}"#,
            has_reel,
            set_card,
        ),
        spec("reel.setBrand", "Set Reel Brand", M, r#"{"brand":Brand,"channelId":n?,"rebuild":bool?=true}"#, has_reel, set_brand),
        spec("reel.duplicate", "New Version of This Reel", M, r#"{"name":str?}"#, has_reel, duplicate),
        query("reel.usesOf", "Reels Made From", r#"{"item":itemId?,"path":str?}"#, uses_of),
        spec("reel.linkPost", "Link Reel to Post", &[], r#"{"post":{"channelId":n,"storyId":n,"scriptId":n}|null}"#, has_reel, link_post),
    ]
}

fn has_reel(s: &Session) -> std::result::Result<(), String> {
    if current(s).is_some() { Ok(()) } else { Err("this sequence is not a Reel".into()) }
}

fn info(s: &Session) -> ReelInfo {
    current(s).unwrap_or_default()
}

/// The Reel the active sequence is, if it is one. A project can hold several Reels: each
/// sequence keeps its own template, card and post (`Project::reel` = `{"sequences": {id: …}}`).
pub fn current(s: &Session) -> Option<ReelInfo> {
    for_sequence(s.project.reel.as_ref(), s.state.active_sequence?)
}

/// The Reel stored for sequence `seq`.
pub fn for_sequence(v: Option<&Value>, seq: ItemId) -> Option<ReelInfo> {
    ReelInfo::from_value(v?.get("sequences")?.get(seq.0.to_string()))
}

fn store(pr: &mut Project, seq: ItemId, info: Value) {
    let mut root = pr.reel.take().filter(|v| v.get("sequences").is_some_and(Value::is_object)).unwrap_or_else(|| json!({"sequences": {}}));
    root["sequences"][seq.0.to_string()] = info;
    pr.reel = Some(root);
}

fn parse<T: serde::de::DeserializeOwned + Default>(p: &Value, key: &str, cmd: &str) -> Result<Option<T>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|e| bad(cmd, format!("`{key}`: {e}"))),
    }
}

fn new_reel(s: &mut Session, p: &Value) -> Result<Value> {
    let brand: Brand = parse(p, "brand", "reel.new")?.unwrap_or_default();
    let template = str_p(p, "template").map(str::to_string).unwrap_or_else(|| brand.default_layout.clone());
    let name = str_p(p, "name").unwrap_or("Reel").to_string();
    let format = if str_p(p, "format") == Some("post") { "post".to_string() } else { "reel".to_string() };
    let paths: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
    s.edit_group("New Reel", |s| {
        let (fw, fh) = filmcraft_reel::frame_for(&format);
        let seq = s.execute("file.newSequence", json!({"name": name, "width": fw, "height": fh, "fps": REEL_FPS, "video": TRACKS.len(), "audio": 2}))?;
        // A project item as it is; a file already in the project is reused, never imported twice.
        let mut item = item_p(p, "item").filter(|i| s.project.item(*i).is_some()).map(|i| i.0);
        if item.is_none()
            && let Some(path) = paths.first()
        {
            item = match existing_item(s, path) {
                Some(i) => Some(i.0),
                None => s.execute("file.import", json!({"paths": [path]}))?.get("items").and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_u64),
            };
        }
        let mut args = json!({"template": template, "brand": brand, "card": p.get("card").cloned().unwrap_or(Value::Null), "seconds": f64_p(p, "seconds").unwrap_or(15.0), "format": format});
        if let Some(i) = item {
            args["item"] = json!(i);
        }
        if let Some(c) = p.get("channelId") {
            args["channelId"] = c.clone();
        }
        let r = apply(s, &args)?;
        Ok(json!({"sequence": seq.get("sequence").cloned().unwrap_or(Value::Null), "reel": r}))
    })
}

/// The clip the template wraps: asked for, the Reel's own, the selection, or the lowest video
/// clip under the playhead.
fn main_clip(s: &Session, p: &Value, reel: &ReelInfo) -> Option<ClipId> {
    let seq = s.active_sequence()?;
    let exists = |c: ClipId| seq.video_tracks.iter().any(|t| t.items.iter().any(|i| i.id == c && !MADE.contains(&i.name.as_str())));
    if let Some(c) = crate::commands::clip_p(p, "clip") {
        return exists(c).then_some(c);
    }
    if let Some(c) = reel.main_clip.map(ClipId).filter(|c| exists(*c)) {
        return Some(c);
    }
    if let Some(c) = s.state.selection.iter().copied().find(|c| exists(*c)) {
        return Some(c);
    }
    let t = s.playhead();
    seq.video_tracks.iter().filter_map(|tr| tr.item_at(t)).find(|i| !MADE.contains(&i.name.as_str()) && i.graphic.is_none()).map(|i| i.id)
}

fn set(e: &mut EffectInstance, id: &str, v: ParamValue) {
    e.params.insert(id.to_string(), Param::new(v));
}
fn motion(ti: &mut TrackItem) -> Option<&mut EffectInstance> {
    ti.effects.iter_mut().find(|e| e.effect == "motion")
}
fn effect(id: &str) -> Option<EffectInstance> {
    find_effect(id).map(|d| d.instance())
}

/// Fits the footage into the slot: Motion scales and centres it, Crop trims what overflows.
fn fit_footage(ti: &mut TrackItem, src: (f64, f64), plan: &ReelPlan) {
    let (sw, sh) = src;
    if !(sw > 0.0 && sh > 0.0) {
        return;
    }
    let slot = plan.slot;
    let scale = match plan.fit {
        Fit::Cover => (slot.w / sw).max(slot.h / sh),
        Fit::Contain => (slot.w / sw).min(slot.h / sh),
    };
    ti.scale_to_frame = false;
    ti.effects.retain(|e| e.effect != "crop");
    if let Some(m) = motion(ti) {
        set(m, "scale", ParamValue::Float(scale * 100.0));
        set(m, "uniform_scale", ParamValue::Bool(true));
        let (cx, cy) = slot.center();
        set(m, "position", ParamValue::Vec2(Vec2::new(cx, cy)));
        set(m, "anchor", ParamValue::Vec2(Vec2::new(sw / 2.0, sh / 2.0)));
    }
    if plan.fit == Fit::Cover {
        let cut_x = ((sw - slot.w / scale) / 2.0 / sw * 100.0).max(0.0);
        let cut_y = ((sh - slot.h / scale) / 2.0 / sh * 100.0).max(0.0);
        if (cut_x > 0.01 || cut_y > 0.01)
            && let Some(mut crop) = effect("crop")
        {
            set(&mut crop, "left", ParamValue::Float(cut_x));
            set(&mut crop, "right", ParamValue::Float(cut_x));
            set(&mut crop, "top", ParamValue::Float(cut_y));
            set(&mut crop, "bottom", ParamValue::Float(cut_y));
            ti.effects.push(crop);
        }
    }
}

fn apply(s: &mut Session, p: &Value) -> Result<Value> {
    let mut reel = info(s);
    let template = str_p(p, "template").map(str::to_string).filter(|t| !t.is_empty()).unwrap_or_else(|| reel.template.clone());
    let template = if template.is_empty() { reel.brand.default_layout.clone() } else { template };
    if let Some(b) = parse::<Brand>(p, "brand", "reel.apply")? {
        reel.brand = b;
    }
    if let Some(c) = parse::<Card>(p, "card", "reel.apply")? {
        reel.card = c;
    }
    if let Some(c) = p.get("channelId").and_then(Value::as_i64) {
        reel.channel_id = Some(c);
    }
    let seq_id = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let (w, h, rate) = s.active_sequence().map(|q| (q.settings.width, q.settings.height, q.settings.frame_rate)).ok_or(EngineError::NoSequence)?;
    if (w, h) != (REEL_WIDTH, REEL_HEIGHT) && (w, h) != (REEL_WIDTH, filmcraft_reel::POST_HEIGHT) {
        return Err(bad(
            "reel.apply",
            format!(
                "Reels are {REEL_WIDTH}×{REEL_HEIGHT} and Posts {REEL_WIDTH}×{}; this sequence is {w}×{h}. Start one with Reel ▸ New Reel",
                filmcraft_reel::POST_HEIGHT
            ),
        ));
    }
    reel.format = if h == filmcraft_reel::POST_HEIGHT { "post".into() } else { "reel".into() };
    // Footage placed from the project panel goes onto the Footage track at the playhead (or 0).
    let placed = item_p(p, "item");
    let logo_item = match reel.brand.logo_path.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(path) => existing_item(s, path)
            .or_else(|| s.execute("file.import", json!({"paths": [path]})).ok().and_then(|r| r.get("items")?.as_array()?.first()?.as_u64()).map(ItemId)),
        None => None,
    };
    let before = s.history.undo.len();
    let fallback = rate.snap_nearest(Tick::from_seconds_f64(f64_p(p, "seconds").unwrap_or(15.0).clamp(1.0, 3600.0)));
    // Footage from the project goes in with its sound: the picture on the Footage track, the
    // linked audio on A1, placed together by the timeline's own placement.
    let main = match placed {
        Some(item) => {
            s.edit("Apply Reel Template", |pr, _| ensure_tracks(pr, seq_id))?;
            let pi = s.project.item(item).ok_or_else(|| bad("reel.apply", "no such item"))?;
            let dur = Some(pi.duration()).filter(|d| *d > Tick::ZERO && *d < Tick::from_seconds_f64(86_400.0)).unwrap_or(fallback);
            let (vt, at) = s
                .active_sequence()
                .map(|q| (q.video_tracks.get(FOOTAGE).map(|t| t.id), q.audio_tracks.first().map(|t| t.id)))
                .ok_or(EngineError::NoSequence)?;
            let (_, has_audio) = s.project.source_streams(item);
            let clips = crate::commands::place_item(
                s,
                item,
                TimeRange::new(Tick::ZERO, rate.snap(dur).max(rate.frame_duration())),
                Tick::ZERO,
                vt,
                at.filter(|_| has_audio),
                false,
                "Apply Reel Template",
                None,
            )?;
            let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
            clips.into_iter().find(|c| q.video_tracks.iter().any(|t| t.items.iter().any(|i| i.id == *c)))
        }
        None => main_clip(s, p, &reel),
    };
    let start_at = if placed.is_some() { Tick::ZERO } else { s.playhead() };
    let reel_card = reel.card.clone();
    let reel_brand = reel.brand.clone();
    let template_id = template.clone();
    let result = s.edit("Apply Reel Template", |pr, st| {
        let pr: &mut Project = pr;
        let main_id = main;
        ensure_tracks(pr, seq_id)?;
        let q = pr.sequence(seq_id).ok_or(EngineError::NoSequence)?;
        let found = main_id.and_then(|c| q.find_item(c).map(|(_, i)| i.clone()));
        let (start, dur) = found.as_ref().map_or((rate.snap(start_at), fallback), |i| (i.start, i.duration));
        let plan = filmcraft_reel::plan_for(&template_id, &reel_brand, &reel_card, dur.seconds(), h).map_err(|e| bad("reel.apply", e))?;
        // Clear what an earlier template made in this span.
        let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let span = TimeRange::new(start, dur);
        for tr in &mut q.video_tracks {
            tr.items.retain(|i| !(MADE.contains(&i.name.as_str()) && i.range().overlaps(&span)));
        }
        // Footage: onto the Footage track if it is elsewhere, then fitted to the slot.
        let mut footage = found;
        if let Some(mut ti) = footage.take() {
            let size = pr.conformed_source_size(ti.item, (1, 1)).or_else(|| pr.source_size(ti.item).map(|(a, b)| (f64::from(a), f64::from(b))));
            fit_footage(&mut ti, size.unwrap_or((f64::from(w), f64::from(h))), &plan);
            let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
            for tr in &mut q.video_tracks {
                tr.items.retain(|i| i.id != ti.id);
            }
            let tr = q.video_tracks.get_mut(FOOTAGE).ok_or(EngineError::NoSequence)?;
            if tr.items.iter().any(|i| i.range().overlaps(&ti.range())) {
                return Err(bad("reel.apply", "the Footage track is not free for this clip; move what is there first"));
            }
            tr.items.push(ti.clone());
            tr.sort();
            footage = Some(ti);
        }
        // Backdrop: the same footage, filling the frame, blurred and dimmed, without sound.
        let mut made: Vec<(usize, TrackItem)> = Vec::new();
        if let (Backdrop::BlurredClip { blur, brightness, saturation }, Some(main)) = (plan.backdrop, footage.as_ref()) {
            let mut bg = main.clone();
            bg.id = ClipId(pr.alloc_id());
            bg.link = None;
            bg.group = None;
            bg.name = MADE[0].into();
            bg.effects.retain(|e| e.effect == "motion" || e.effect == "opacity");
            let size = pr.conformed_source_size(bg.item, (1, 1)).unwrap_or((f64::from(w), f64::from(h)));
            let full = ReelPlan { slot: filmcraft_reel::Rect::new(0.0, 0.0, f64::from(w), f64::from(h)), fit: Fit::Cover, ..plan.clone() };
            fit_footage(&mut bg, size, &full);
            if let Some(m) = motion(&mut bg)
                && let Some(Param { value: ParamValue::Float(sc), .. }) = m.params.get("scale").cloned()
            {
                set(m, "scale", ParamValue::Float(sc * 1.16));
            }
            bg.effects.retain(|e| e.effect != "crop");
            if let Some(mut e) = effect("gaussian_blur") {
                set(&mut e, "blurriness", ParamValue::Float(blur));
                set(&mut e, "repeat_edge", ParamValue::Bool(true));
                bg.effects.push(e);
            }
            if let Some(mut e) = effect("proc_amp") {
                set(&mut e, "brightness", ParamValue::Float(brightness));
                set(&mut e, "saturation", ParamValue::Float(saturation));
                bg.effects.push(e);
            }
            made.push((0, bg));
        }
        // Card and Overlay: graphic clips holding the template's layers.
        for (track, name, layers, controls) in [(1, MADE[1], plan.under.clone(), Vec::new()), (3, MADE[2], plan.over.clone(), plan.controls.clone())] {
            if layers.is_empty() {
                continue;
            }
            let src = crate::graphics::graphic_source(pr, w, h, rate);
            let mut ti = pr
                .make_track_item(src, TrackKind::Video, start, TimeRange::new(Tick::ZERO, dur), rate)
                .ok_or_else(|| bad("reel.apply", "could not make a graphic clip"))?;
            ti.name = name.into();
            ti.effects.extend(layers);
            for e in &mut ti.effects {
                filmcraft_project::resolve_auto_points(e, (w, h), (w, h));
            }
            if !controls.is_empty() {
                let link = TemplateLink { id: format!("reel:{}", plan.template), name: template_name(plan.template), controls };
                ti.graphic = Some(Box::new(GraphicMeta { template: Some(link), ..Default::default() }));
            }
            made.push((track, ti));
        }
        // Logo: the channel's own image, placed and sized to the template's spot.
        if let (Some(spot), Some(item)) = (plan.logo, logo_item)
            && let Some(mut ti) = pr.make_track_item(item, TrackKind::Video, start, TimeRange::new(Tick::ZERO, dur), rate)
        {
            ti.name = MADE[3].into();
            ti.link = None;
            let (lw, lh) = pr.source_size(item).map_or((spot.size, spot.size), |(a, b)| (f64::from(a), f64::from(b)));
            let longest = lw.max(lh).max(1.0);
            if let Some(m) = motion(&mut ti) {
                set(m, "scale", ParamValue::Float(spot.size / longest * 100.0));
                set(m, "position", ParamValue::Vec2(Vec2::new(spot.center.0, spot.center.1)));
                set(m, "anchor", ParamValue::Vec2(Vec2::new(lw / 2.0, lh / 2.0)));
            }
            if let Some(o) = ti.effects.iter_mut().find(|e| e.effect == "opacity") {
                set(o, "opacity", ParamValue::Float(spot.opacity));
            }
            made.push((4, ti));
        }
        let q = pr.sequence_mut(seq_id).ok_or(EngineError::NoSequence)?;
        let mut ids = Vec::new();
        for (track, ti) in made {
            let tr = q.video_tracks.get_mut(track).ok_or(EngineError::NoSequence)?;
            tr.items.retain(|i| !i.range().overlaps(&ti.range()));
            ids.push(json!({"track": TRACKS.get(track).copied().unwrap_or(""), "clip": ti.id.0}));
            tr.items.push(ti);
            tr.sort();
        }
        q.check().map_err(EngineError::Other)?;
        st.selection = footage.as_ref().map(|f| vec![f.id]).unwrap_or_default();
        Ok((footage.map(|f| f.id.0), ids))
    })?;
    reel.template = template;
    reel.main_clip = result.0;
    let reel_value = reel.to_value();
    s.edit("Apply Reel Template", |pr, _| {
        store(pr, seq_id, reel_value);
        Ok(())
    })?;
    // One undo step for the whole application: placement, clips and the Reel info go back together.
    if s.history.undo.len() > before + 1 {
        s.history.undo.truncate(before + 1);
    }
    Ok(json!({"template": reel.template, "mainClip": result.0, "clips": result.1}))
}

fn template_name(id: &str) -> String {
    filmcraft_reel::find(id).map_or_else(|| id.to_string(), |t| t.name.to_string())
}

/// An already imported media item for `path`, so re-applying never imports the logo twice.
fn existing_item(s: &Session, path: &str) -> Option<ItemId> {
    let norm = |p: &str| p.replace('\\', "/").to_ascii_lowercase();
    let want = norm(path);
    s.project.items.iter().find(|(_, i)| i.as_media().is_some_and(|m| matches!(&m.media, MediaRef::File { path } if norm(path) == want))).map(|(id, _)| *id)
}

/// Gives the sequence the five Reel tracks, named, adding any that are missing.
fn ensure_tracks(pr: &mut Project, seq: ItemId) -> Result<()> {
    let missing = pr.sequence(seq).map_or(0, |q| TRACKS.len().saturating_sub(q.video_tracks.len()));
    let ids: Vec<u64> = (0..missing).map(|_| pr.alloc_id()).collect();
    let q = pr.sequence_mut(seq).ok_or(EngineError::NoSequence)?;
    for id in ids {
        let n = q.video_tracks.len() + 1;
        q.video_tracks.push(Track::new(TrackId(id), TrackKind::Video, format!("Video {n}")));
    }
    for (tr, name) in q.video_tracks.iter_mut().zip(TRACKS) {
        if tr.name.starts_with("Video ") {
            tr.name = name.into();
        }
    }
    Ok(())
}

/// Writes card fields into the Overlay's layers, keeping everything else the user changed.
fn set_card(s: &mut Session, p: &Value) -> Result<Value> {
    let mut reel = info(s);
    let mut card = reel.card.clone();
    let text = |k: &str| str_p(p, k).map(str::to_string);
    if let Some(v) = text("title") {
        card.title = v;
    }
    if let Some(v) = text("caption") {
        card.caption = v;
    }
    if let Some(v) = text("attribution") {
        card.attribution = v;
    }
    if let Some(v) = text("badge") {
        card.badge = v;
    }
    if let Some(v) = text("cta") {
        card.cta = v;
    }
    if let Some(v) = text("instagramCaption") {
        card.instagram_caption = v;
    }
    let list = |k: &str| p.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>());
    if let Some(v) = list("bullets") {
        card.bullets = v.into_iter().take(4).collect();
    }
    if let Some(v) = list("hashtags") {
        card.hashtags = v;
    }
    let values: Vec<(&str, String)> = vec![
        ("title", card.title.clone()),
        ("caption", card.caption.clone()),
        ("badge", card.badge.clone()),
        ("point1", card.bullets.first().cloned().unwrap_or_default()),
        ("point2", card.bullets.get(1).cloned().unwrap_or_default()),
        ("point3", card.bullets.get(2).cloned().unwrap_or_default()),
        ("point4", card.bullets.get(3).cloned().unwrap_or_default()),
    ];
    let cta = if card.cta.trim().is_empty() { reel.brand.cta.clone().unwrap_or_default() } else { card.cta.clone() };
    reel.card = card;
    let value = reel.to_value();
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    s.edit("Edit Reel Card", |pr, _| {
        // Only this sequence's Reel: other sequences are other Reels with their own cards.
        if let Some(q) = pr.sequence_mut(seq) {
            for ti in q.video_tracks.iter_mut().flat_map(|t| t.items.iter_mut()).filter(|i| i.name == MADE[2]) {
                let Some(link) = ti.graphic.as_ref().and_then(|g| g.template.clone()) else { continue };
                for (id, v) in &values {
                    let Some(c) = link.controls.iter().find(|c| c.id == *id) else { continue };
                    let Some(e) = ti.effects.iter_mut().find(|e| e.layer.as_ref().is_some_and(|l| l.uid == c.layer)) else { continue };
                    let Some(param) = e.params.get_mut(&c.param) else { continue };
                    param.value = ParamValue::Text(v.clone());
                    // The design headline holds the title, then switches to the call to action.
                    if let Some(k) = param.keyframes.first_mut() {
                        k.value = ParamValue::Text(v.clone());
                    }
                    if *id == "title"
                        && param.keyframes.len() > 1
                        && let Some(k) = param.keyframes.last_mut()
                        && !cta.trim().is_empty()
                    {
                        k.value = ParamValue::Text(cta.clone());
                    }
                }
            }
        }
        store(pr, seq, value);
        Ok(())
    })?;
    Ok(json!({"reel": current(s).map(|r| r.to_value())}))
}

fn set_brand(s: &mut Session, p: &Value) -> Result<Value> {
    let brand: Brand = parse(p, "brand", "reel.setBrand")?.ok_or_else(|| bad("reel.setBrand", "need `brand`"))?;
    let mut reel = info(s);
    reel.brand = brand;
    if let Some(c) = p.get("channelId").and_then(Value::as_i64) {
        reel.channel_id = Some(c);
    }
    let value = reel.to_value();
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    s.edit("Set Reel Brand", |pr, _| {
        store(pr, seq, value);
        Ok(())
    })?;
    if p.get("rebuild").and_then(Value::as_bool).unwrap_or(true) && s.active_sequence().is_some() {
        return apply(s, &json!({"template": reel.template}));
    }
    Ok(json!({"reel": current(s).map(|r| r.to_value())}))
}

fn link_post(s: &mut Session, p: &Value) -> Result<Value> {
    let mut reel = info(s);
    reel.post = parse(p, "post", "reel.linkPost")?;
    let value = reel.to_value();
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    s.edit("Link Reel to Post", |pr, _| {
        store(pr, seq, value);
        Ok(())
    })?;
    Ok(json!({"reel": current(s).map(|r| r.to_value())}))
}

/// The Reels (sequences) already built around a project item, or around the file at `path`.
fn uses_of(s: &mut Session, p: &Value) -> Result<Value> {
    let item = item_p(p, "item").or_else(|| str_p(p, "path").and_then(|path| existing_item(s, path)));
    let Some(item) = item else { return Ok(json!({"item": Value::Null, "reels": []})) };
    let mut out = Vec::new();
    for (id, pi) in &s.project.items {
        let Some(q) = pi.as_sequence() else { continue };
        let Some(info) = for_sequence(s.project.reel.as_ref(), *id) else { continue };
        let uses = info.main_clip.is_some_and(|c| q.find_item(ClipId(c)).is_some_and(|(_, it)| it.item == item));
        if uses {
            out.push(json!({"sequence": id.0, "name": pi.name, "template": info.template, "post": info.post}));
        }
    }
    Ok(json!({"item": item.0, "reels": out}))
}

/// Copies the active Reel into a new sequence: same footage (no import), same cut, card and
/// template, published as a post of its own.
fn duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let mut reel = info(s);
    reel.post = None;
    let base = s.project.item(seq).map(|i| i.name.clone()).unwrap_or_else(|| "Reel".into());
    let name = str_p(p, "name").map(str::to_string).unwrap_or_else(|| format!("{base} (version 2)"));
    let value = reel.to_value();
    let new = s.edit("New Version of This Reel", |pr, st| {
        let it = pr.item(seq).cloned().ok_or(EngineError::NoSequence)?;
        let id = pr.add_item(&name, it.label, it.kind, None);
        store(pr, id, value);
        st.project_selection = vec![id];
        Ok(id)
    })?;
    s.execute("sequence.open", json!({"item": new.0}))?;
    Ok(json!({"sequence": new.0, "name": name}))
}
