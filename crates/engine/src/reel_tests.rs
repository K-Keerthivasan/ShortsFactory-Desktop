use serde_json::{Value, json};

use crate::Session;
use filmcraft_project::{ClipId, ParamValue, TrackItem};
use filmcraft_reel::TEMPLATES;

#[test]
fn applying_a_workspace_logo_puts_it_in_brand_assets_without_duplicates() {
    let (mut s, _) = reel("quoteCard");
    let dir = std::env::temp_dir().join(format!("reel-logo-bin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let logo = dir.join("logo.png");
    s.execute("file.exportFrame", json!({"path": logo.to_string_lossy(), "format": "png", "time": 0})).unwrap();
    let mut brand = crate::reel::current(&s).unwrap().brand;
    brand.logo_path = Some(logo.to_string_lossy().into_owned());
    for _ in 0..2 {
        s.execute("reel.setBrand", json!({"brand": brand})).unwrap();
    }
    let bins: Vec<_> = s
        .project
        .root
        .children
        .iter()
        .filter_map(|e| match e {
            filmcraft_project::BinEntry::Bin(b) if b.name == "Brand Assets" => Some(b),
            _ => None,
        })
        .collect();
    assert_eq!(bins.len(), 1);
    assert_eq!(bins[0].children.len(), 1);
}

#[test]
fn changing_card_fonts_keeps_clips_and_is_undoable() {
    let (mut s, _) = reel("quoteCard");
    let original = track_items(&s, 3)[0].clone();
    s.execute("reel.setCard", json!({"fonts": {"title": "Georgia"}})).unwrap();
    let overlay = track_items(&s, 3)[0].clone();
    assert_eq!(overlay.id, original.id, "changing fonts does not rebuild the timeline");
    let control = overlay.graphic.as_ref().unwrap().template.as_ref().unwrap().controls.iter().find(|c| c.id == "title").unwrap();
    let title = overlay.effects.iter().find(|e| e.layer.as_ref().is_some_and(|l| l.uid == control.layer)).unwrap();
    assert_eq!(title.params["font"].value, ParamValue::Text("Georgia".into()));
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(track_items(&s, 3)[0], original);
    assert!(s.execute("reel.setCard", json!({"fonts": {"title": "x".repeat(257)}})).is_err());
}

#[test]
fn text_styles_reflow_the_existing_graphic_and_reject_invalid_limits() {
    let (mut s, _) = reel("quoteCard");
    let footage = track_items(&s, 2);
    let overlay = track_items(&s, 3)[0].id;
    s.execute("reel.setCard",json!({"textStyles":{"title":{"size":80,"uppercase":true,"bold":true,"italic":true,"maxChars":14}},"textTiming":"always","attribution":"Original: creator"})).unwrap();
    assert_eq!(track_items(&s, 2), footage);
    assert_eq!(track_items(&s, 3)[0].id, overlay);
    let effects = &track_items(&s, 3)[0].effects;
    let title = effects.iter().find(|e| e.params.get("name").is_some_and(|p| p.value == ParamValue::Text("Title".into()))).unwrap();
    assert_eq!(title.params["font_style"].value, ParamValue::Text("Bold Italic".into()));
    let ParamValue::Text(text) = &title.params["text"].value else { panic!("no title") };
    assert!(text.chars().count() <= 14);
    assert!(effects.iter().any(|e| e.params.get("text").is_some_and(|p| p.value == ParamValue::Text("Original: creator".into()))));
    for styles in [json!({"title":{"size":-1}}), json!({"title":{"maxChars":2001}}), json!({"title":{"align":99}})] {
        assert!(s.execute("reel.setCard", json!({"textStyles":styles})).is_err());
    }
}

/// A fresh Reel built around procedural demo footage (no media files needed).
fn reel(template: &str) -> (Session, Value) {
    let mut s = Session::default();
    s.execute("reel.new", json!({"name": "Test Reel", "template": template, "seconds": 8})).unwrap();
    let item = s.execute("file.importDemoFootage", json!({"scene": "Aurora"})).unwrap();
    let id = item["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).or_else(|| item["item"].as_u64()).unwrap();
    let card = json!({"title": "Ten seconds that changed the match", "caption": "Round two", "badge": "07", "bullets": ["Fast", "Clean"], "cta": "Follow for part two"});
    let brand = json!({"channelName": "JKR", "socialHandle": "jkr", "primaryColor": "#22c55e"});
    let r = s.execute("reel.apply", json!({"template": template, "item": id, "card": card, "brand": brand})).unwrap();
    (s, r)
}

fn track_items(s: &Session, track: usize) -> Vec<TrackItem> {
    s.active_sequence().unwrap().video_tracks.get(track).map(|t| t.items.clone()).unwrap_or_default()
}

#[test]
fn every_template_builds_a_valid_reel() {
    for t in TEMPLATES {
        let (s, r) = reel(t.id);
        let q = s.active_sequence().unwrap();
        assert_eq!((q.settings.width, q.settings.height), (1080, 1920), "{}", t.id);
        assert!(q.check().is_ok(), "{}: {:?}", t.id, q.check());
        let names: Vec<_> = q.video_tracks.iter().take(5).map(|t| t.name.clone()).collect();
        assert_eq!(names, ["Backdrop", "Card", "Footage", "Overlay", "Logo"]);
        assert_eq!(track_items(&s, 2).len(), 1, "{}: the footage sits on the Footage track", t.id);
        let main = ClipId(r["mainClip"].as_u64().unwrap());
        assert_eq!(track_items(&s, 2)[0].id, main);
        let info = crate::reel::current(&s).unwrap();
        assert_eq!(info.template, t.id);
        assert_eq!(info.main_clip, Some(main.0));
    }
}

#[test]
fn applying_is_one_undo_step_and_reapplying_replaces_only_what_it_made() {
    let (mut s, _) = reel("quoteCard");
    let overlay = track_items(&s, 3);
    assert_eq!(overlay.len(), 1);
    assert_eq!(overlay[0].name, "Reel Overlay");
    assert!(overlay[0].graphic.as_ref().and_then(|g| g.template.as_ref()).is_some_and(|t| t.controls.iter().any(|c| c.id == "title")));
    // Another template: one Overlay, the same footage clip, now with a blurred backdrop.
    let before = s.history.undo.len();
    s.execute("reel.apply", json!({"template": "jkr-cinematic-v2"})).unwrap();
    assert_eq!(s.history.undo.len(), before + 1);
    assert_eq!(track_items(&s, 3).len(), 1);
    assert_eq!(track_items(&s, 0).len(), 1, "cinematic draws a blurred backdrop");
    assert!(track_items(&s, 0)[0].effects.iter().any(|e| e.effect == "gaussian_blur"));
    assert!(track_items(&s, 1).is_empty(), "the quote card's Card layer is gone");
    s.execute("edit.undo", json!({})).unwrap();
    assert!(track_items(&s, 0).is_empty());
    assert_eq!(crate::reel::current(&s).unwrap().template, "quoteCard");
}

#[test]
fn the_footage_is_fitted_and_cropped_to_the_slot() {
    let (s, _) = reel("quoteCard");
    let main = &track_items(&s, 2)[0];
    let motion = main.effects.iter().find(|e| e.effect == "motion").unwrap();
    let ParamValue::Vec2(pos) = &motion.params["position"].value else { panic!("no position") };
    // Quote card slot: 1080 × 1270 from y = 650.
    assert!((pos.x - 540.0).abs() < 1e-6 && (pos.y - (650.0 + 635.0)).abs() < 1e-6, "{pos:?}");
    assert!(main.effects.iter().any(|e| e.effect == "crop"), "a 16:9 demo clip overflows the slot and is cropped");
}

#[test]
fn editing_the_card_rewrites_the_overlay_text() {
    let (mut s, _) = reel("jkr-clean-v2");
    s.execute("reel.setCard", json!({"title": "A new headline", "cta": "Subscribe"})).unwrap();
    let overlay = &track_items(&s, 3)[0];
    let title = overlay.effects.iter().find(|e| e.params.get("name").is_some_and(|n| n.value == ParamValue::Text("Title".into()))).unwrap();
    assert_eq!(title.params["text"].value, ParamValue::Text("A new headline".into()));
    let info = crate::reel::current(&s).unwrap();
    assert_eq!(info.card.title, "A new headline");
}

#[test]
fn adding_a_supporting_line_to_an_empty_card_creates_the_layer_in_place() {
    for template in ["quoteCard", "fullBleed", "headline", "framed", "jkr-clean-v2", "jkr-cinematic-v2"] {
        let (mut s, _) = reel(template);
        s.execute("reel.apply", json!({"card":{"title":"A headline without supporting text","caption":""}})).unwrap();
        let footage = track_items(&s, 2).to_vec();
        let original = track_items(&s, 3)[0].clone();
        assert!(!original.graphic.as_ref().unwrap().template.as_ref().unwrap().controls.iter().any(|c| c.id == "caption"));
        s.execute("reel.setCard", json!({"caption":"A supporting line that must appear"})).unwrap();
        let overlay = &track_items(&s, 3)[0];
        assert_eq!(overlay.id, original.id);
        assert_eq!(overlay.range(), original.range());
        assert_eq!(track_items(&s, 2), footage);
        let controls = &overlay.graphic.as_ref().unwrap().template.as_ref().unwrap().controls;
        let caption = controls.iter().find(|c| c.id == "caption").unwrap();
        let layer = overlay.effects.iter().find(|e| e.layer.as_ref().is_some_and(|l| l.uid == caption.layer)).unwrap();
        assert_eq!(layer.params["text"].value, ParamValue::Text("A supporting line that must appear".into()), "{template}");
        assert_eq!(layer.params["font"].value, ParamValue::Text("Inter".into()));
        s.execute("reel.setCard", json!({"caption":"","fonts":{"caption":"Georgia"}})).unwrap();
        s.execute("reel.setCard", json!({"caption":"Visible again"})).unwrap();
        let overlay = &track_items(&s, 3)[0];
        let caption = overlay.graphic.as_ref().unwrap().template.as_ref().unwrap().controls.iter().find(|c| c.id == "caption").unwrap();
        let layer = overlay.effects.iter().find(|e| e.layer.as_ref().is_some_and(|l| l.uid == caption.layer)).unwrap();
        assert_eq!(layer.params["text"].value, ParamValue::Text("Visible again".into()));
        assert_eq!(layer.params["font"].value, ParamValue::Text("Georgia".into()));
    }
}

#[test]
fn hostile_parameters_are_errors_not_crashes() {
    let mut s = Session::default();
    assert!(s.execute("reel.apply", json!({"template": "quoteCard"})).is_err(), "no sequence");
    s.execute("file.newSequence", json!({"name": "Wide", "width": 1920, "height": 1080})).unwrap();
    assert!(s.execute("reel.apply", json!({"template": "quoteCard"})).is_err(), "not a 9:16 sequence");
    assert!(s.execute("reel.setCard", json!({"title": "x"})).is_err(), "not a Reel");
    let mut s = Session::default();
    s.execute("reel.new", json!({"seconds": -5, "template": "nope"})).unwrap_err();
    s.execute("reel.new", json!({"seconds": 1.0e300, "brand": {"primaryColor": "not a colour"}})).unwrap();
    assert!(s.execute("reel.apply", json!({"template": "quoteCard", "card": "not an object"})).is_err());
    assert!(s.execute("reel.apply", json!({"template": "quoteCard", "item": 999_999})).is_err());
    assert!(s.execute("reel.linkPost", json!({"post": {"channelId": "x"}})).is_err());
    s.execute("reel.linkPost", json!({"post": {"channelId": 1, "storyId": 2, "scriptId": 3}})).unwrap();
    assert_eq!(crate::reel::current(&s).unwrap().post.map(|p| p.script_id), Some(3));
}

#[test]
fn the_reel_survives_save_and_open() {
    let (s, _) = reel("framed");
    let json = serde_json::to_string(&*s.project).unwrap();
    let back: filmcraft_project::Project = serde_json::from_str(&json).unwrap();
    assert_eq!(crate::reel::for_sequence(back.reel.as_ref(), s.state.active_sequence.unwrap()).unwrap().template, "framed");
}

/// Renders one frame of every template to `target/reel-frames/` for looking at
/// (`cargo test -p filmcraft-engine reel_frames -- --ignored`).
#[test]
#[ignore = "writes images for visual review"]
fn reel_frames() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/reel-frames");
    std::fs::create_dir_all(&dir).unwrap();
    for t in TEMPLATES {
        let (mut s, _) = reel(t.id);
        let path = dir.join(format!("{}.png", t.id));
        let r = s.execute("file.exportFrame", json!({"path": path.to_string_lossy(), "format": "png", "time": filmcraft_reel::secs(1.2).0}));
        assert!(r.is_ok(), "{}: {r:?}", t.id);
    }
}

/// [`reel_frames`] with a long all-caps headline and a supporting line over bright footage, the
/// case where words used to overlap the footage or run off the frame
/// (`cargo test -p filmcraft-engine reel_frames_long -- --ignored`).
#[test]
#[ignore = "writes images for visual review"]
fn reel_frames_long() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/reel-frames-long");
    std::fs::create_dir_all(&dir).unwrap();
    for t in TEMPLATES {
        let mut s = Session::default();
        s.execute("reel.new", json!({"name": "Long", "template": t.id, "seconds": 8})).unwrap();
        let item = s.execute("file.importDemoFootage", json!({"scene": "OceanSunset"})).unwrap();
        let id = item["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).or_else(|| item["item"].as_u64()).unwrap();
        let card = json!({"title": "Watch the cameras, spot creepy changes, survive your shift.", "caption": "A new horror game where every room hides a change you must catch.", "bullets": ["Spot the change", "Report it fast"]});
        let brand = json!({"channelName": "JKR", "socialHandle": "jkr_studio_", "primaryColor": "#ff2e3f"});
        s.execute("reel.apply", json!({"template": t.id, "item": id, "card": card, "brand": brand})).unwrap();
        let path = dir.join(format!("{}.png", t.id));
        let r = s.execute("file.exportFrame", json!({"path": path.to_string_lossy(), "format": "png", "time": filmcraft_reel::secs(1.2).0}));
        assert!(r.is_ok(), "{}: {r:?}", t.id);
    }
}

#[test]
fn a_reel_exports_to_a_vertical_mp4() {
    let mut s = Session::default();
    s.execute("reel.new", json!({"name": "Export", "template": "fullBleed", "seconds": 1, "card": {"title": "Export test"}})).unwrap();
    let dir = std::env::temp_dir().join(format!("reel-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("reel.mp4");
    let r = s.execute("export.quick", json!({"preset": "Social Vertical 1080×1920", "path": path.to_string_lossy(), "wait": true})).unwrap();
    assert!(path.is_file(), "{r}");
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.len() > 1000 && bytes.windows(4).any(|w| w == b"ftyp"), "an MP4 file");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn each_sequence_is_its_own_reel() {
    let (mut s, _) = reel("framed");
    let first = s.state.active_sequence.unwrap();
    s.execute("reel.new", json!({"name": "Second", "template": "jkr-meme-v2", "seconds": 4})).unwrap();
    let second = s.state.active_sequence.unwrap();
    assert_ne!(first, second);
    assert_eq!(crate::reel::current(&s).unwrap().template, "jkr-meme-v2");
    assert_eq!(crate::reel::for_sequence(s.project.reel.as_ref(), first).unwrap().template, "framed", "the first Reel is untouched");
}

#[test]
fn footage_brings_its_sound_and_undoes_in_one_step() {
    let mut s = Session::default();
    s.execute("reel.new", json!({"name": "Sound", "template": "quoteCard", "seconds": 4})).unwrap();
    let r = s.execute("file.importDemoFootage", json!({"scene": "OceanSunset"})).unwrap();
    let item = r["items"].as_array().and_then(|a| a.first()).and_then(Value::as_u64).or_else(|| r["item"].as_u64()).unwrap();
    let (_, has_audio) = s.project.source_streams(filmcraft_project::ItemId(item));
    let before = s.history.undo.len();
    s.execute("reel.apply", json!({"template": "quoteCard", "item": item})).unwrap();
    assert_eq!(s.history.undo.len(), before + 1, "one undo step");
    let q = s.active_sequence().unwrap();
    let video = &q.video_tracks[2].items[0];
    if has_audio {
        let audio = q.audio_tracks[0].items.first().expect("the footage's sound is on A1");
        assert!(video.link.is_some() && video.link == audio.link, "picture and sound are linked");
    }
    s.execute("edit.undo", json!({})).unwrap();
    let q = s.active_sequence().unwrap();
    assert!(q.video_tracks[2].items.is_empty() && q.audio_tracks.iter().all(|t| t.items.is_empty()), "footage and sound are gone together");
}

#[test]
fn a_second_reel_reuses_the_source_and_versions_are_their_own_reels() {
    let (mut s, r) = reel("framed");
    let first = s.state.active_sequence.unwrap();
    let main = s.active_sequence().unwrap().find_item(ClipId(r["mainClip"].as_u64().unwrap())).unwrap().1.item;
    let items = s.project.items.len();
    s.execute("reel.new", json!({"name": "Again", "item": main.0})).unwrap();
    // Only the new sequence was added: the footage was not imported a second time.
    assert_eq!(s.project.items.len(), items + 1);
    let uses = s.execute("reel.usesOf", json!({"item": main.0})).unwrap();
    assert_eq!(uses["reels"].as_array().unwrap().len(), 2);
    s.execute("sequence.open", json!({"item": first.0})).unwrap();
    s.execute("reel.linkPost", json!({"post": {"channelId": 1, "storyId": 5, "scriptId": 6}})).unwrap();
    let v = s.execute("reel.duplicate", json!({})).unwrap();
    assert_ne!(v["sequence"].as_u64(), Some(first.0));
    let copy = crate::reel::current(&s).unwrap();
    assert_eq!(copy.template, "framed");
    assert!(copy.post.is_none(), "a new version publishes as its own post");
    assert_eq!(crate::reel::for_sequence(s.project.reel.as_ref(), first).unwrap().post.map(|p| p.story_id), Some(5));
}

#[test]
fn a_post_is_a_4_by_5_sequence_with_the_template_fitted() {
    let mut s = Session::default();
    s.execute("reel.new", json!({"name": "Post", "format": "post", "template": "quoteCard", "seconds": 4, "card": {"title": "A post"}})).unwrap();
    let q = s.active_sequence().unwrap();
    assert_eq!((q.settings.width, q.settings.height), (1080, 1350));
    assert_eq!(crate::reel::current(&s).unwrap().format, "post");
    s.execute("reel.apply", json!({"template": "jkr-cinematic-v2"})).unwrap();
}

#[test]
fn a_reel_can_use_one_segment_of_the_source() {
    let (mut s, r) = reel("framed");
    let main = s.active_sequence().unwrap().find_item(ClipId(r["mainClip"].as_u64().unwrap())).unwrap().1.item;
    let v = s.execute("reel.new", json!({"name": "Part 2", "item": main.0, "sourceIn": 1.0, "sourceOut": 3.0})).unwrap();
    let clip = s.active_sequence().unwrap().find_item(ClipId(v["reel"]["mainClip"].as_u64().unwrap())).unwrap().1.clone();
    assert!((clip.source_in.seconds() - 1.0).abs() < 0.05, "starts at 1 s of the source, not {}", clip.source_in.seconds());
    assert!((clip.duration.seconds() - 2.0).abs() < 0.05, "two seconds long, not {}", clip.duration.seconds());
}

#[test]
fn clearing_the_supporting_line_removes_it_from_the_picture() {
    for template in TEMPLATES.iter().map(|t| t.id) {
        let (mut s, _) = reel(template);
        s.execute("reel.setCard", json!({"caption": "A supporting line"})).unwrap();
        s.execute("reel.setCard", json!({"caption": ""})).unwrap();
        let overlay = &track_items(&s, 3)[0];
        let shown: Vec<String> = overlay
            .effects
            .iter()
            .filter_map(|e| match e.params.get("text").map(|p| &p.value) {
                Some(ParamValue::Text(t)) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(!shown.iter().any(|t| t.contains("A supporting line")), "{template}: still drawn: {shown:?}");
        assert_eq!(crate::reel::current(&s).unwrap().card.caption, "", "{template}");
    }
}

#[test]
fn exact_tick_source_ranges_are_preserved_and_hostile_ticks_fail() {
    let (mut s, r) = reel("quoteCard");
    let main = s.active_sequence().unwrap().find_item(ClipId(r["mainClip"].as_u64().unwrap())).unwrap().1.item;
    let start = 254_016_000_000_i64;
    let end = start * 3;
    s.execute("reel.new", json!({"item":main.0,"sourceInTicks":start,"sourceOutTicks":end})).unwrap();
    let info = crate::reel::current(&s).unwrap();
    let q = s.active_sequence().unwrap();
    let (_, clip) = q.find_item(ClipId(info.main_clip.unwrap())).unwrap();
    assert_eq!(clip.source_in.0, start);
    assert_eq!(clip.duration.0, end - start);
    assert!(s.execute("reel.new", json!({"item":main.0,"sourceInTicks":-1,"sourceOutTicks":end})).is_err());
    assert!(s.execute("reel.new", json!({"item":main.0,"sourceInTicks":1.5,"sourceOutTicks":end})).is_err());
}

#[test]
fn an_edit_of_several_cuts_is_clips_on_the_timeline_inside_the_template() {
    let (mut s, r) = reel("fullBleed");
    let main = s.active_sequence().unwrap().find_item(ClipId(r["mainClip"].as_u64().unwrap())).unwrap().1.item;
    // hook first (3-4 s of the source), then 0.5-2 s and 5-6 s; the subject on the right
    let v = s
        .execute("reel.new", json!({"name": "Edit", "template": "fullBleed", "item": main.0, "cuts": [[3.0, 4.0], [0.5, 2.0], [5.0, 6.0]], "focusX": 0.8}))
        .unwrap();
    let footage: Vec<TrackItem> = track_items(&s, 2).into_iter().filter(|i| i.graphic.is_none()).collect();
    let got: Vec<(f64, f64, f64)> =
        footage.iter().map(|i| ((i.start.seconds() * 10.0).round() / 10.0, (i.source_in.seconds() * 10.0).round() / 10.0, (i.duration.seconds() * 10.0).round() / 10.0)).collect();
    assert_eq!(got, vec![(0.0, 3.0, 1.0), (1.0, 0.5, 1.5), (2.5, 5.0, 1.0)], "back to back, in the asked order");
    assert_eq!(v["reel"]["mainClip"].as_u64(), Some(footage[0].id.0));
    // the template's overlay spans the whole edit
    let overlay = track_items(&s, 3).into_iter().find(|i| i.name == "Reel Overlay").unwrap();
    assert!((overlay.duration.seconds() - 3.5).abs() < 0.05, "{}", overlay.duration.seconds());
    // every cut is framed on the subject: more is cut from the left than the right
    let crop = |i: &TrackItem| {
        let e = i.effects.iter().find(|e| e.effect == "crop").expect("cropped");
        let f = |k: &str| match e.params.get(k).map(|p| &p.value) {
            Some(ParamValue::Float(v)) => *v,
            _ => 0.0,
        };
        (f("left"), f("right"))
    };
    for i in &footage {
        let (l, r) = crop(i);
        assert!(l > r, "left {l} right {r}");
    }
    assert_eq!(crate::reel::current(&s).unwrap().focus_x, Some(0.8));
    // another template keeps the cuts and the framing
    s.execute("reel.apply", json!({"template": "jkr-cinematic-v2"})).unwrap();
    let after: Vec<TrackItem> = track_items(&s, 2).into_iter().filter(|i| i.graphic.is_none()).collect();
    assert_eq!(after.len(), 3);
    let backdrops = track_items(&s, 0).into_iter().filter(|i| i.name == "Reel Backdrop").count();
    assert_eq!(backdrops, 3, "a blurred backdrop under each cut");
}

#[test]
fn every_layer_the_layout_made_stays_editable() {
    let (mut s, r) = reel("headline");
    let overlay = track_items(&s, 3).into_iter().find(|i| i.name == "Reel Overlay").unwrap();
    let layers = filmcraft_project::graphic::layer_indices(&overlay.effects);
    assert!(!layers.is_empty());
    // the headline: moved, resized, recoloured, rewritten, hidden and shown
    let title = layers.iter().position(|&i| overlay.effects[i].effect == filmcraft_project::graphic::TEXT_LAYER).unwrap();
    for props in [json!({"position": [100.0, 400.0]}), json!({"size": 50.0}), json!({"fill_color": "#ff0000"}), json!({"opacity": 60.0}), json!({"enabled": false})] {
        s.execute("graphics.set", json!({"clip": overlay.id.0, "layer": title, "props": props})).unwrap();
    }
    s.execute("graphics.setText", json!({"clip": overlay.id.0, "layer": title, "text": "Edited"})).unwrap();
    let after = track_items(&s, 3).into_iter().find(|i| i.name == "Reel Overlay").unwrap();
    let e = &after.effects[layers[title]];
    assert_eq!(e.params["position"].value, ParamValue::Vec2(filmcraft_geom::Vec2::new(100.0, 400.0)));
    assert_eq!(e.params["size"].value, ParamValue::Float(50.0));
    assert_eq!(e.params["opacity"].value, ParamValue::Float(60.0));
    assert!(!e.enabled);
    assert_eq!(e.params["text"].value, ParamValue::Text("Edited".into()));
    // a shape layer resizes
    if let Some(shape) = layers.iter().position(|&i| overlay.effects[i].effect == filmcraft_project::graphic::SHAPE_LAYER) {
        s.execute("graphics.set", json!({"clip": overlay.id.0, "layer": shape, "props": {"size": [300.0, 20.0]}})).unwrap();
    }
    // the footage moves and scales
    let main = r["mainClip"].as_u64().unwrap();
    s.execute("effects.setParam", json!({"clip": main, "effect": "motion", "param": "scale", "value": 150.0})).unwrap();
    let foot = s.active_sequence().unwrap().find_item(ClipId(main)).unwrap().1.clone();
    let m = foot.effects.iter().find(|e| e.effect == "motion").unwrap();
    assert_eq!(m.params["scale"].value, ParamValue::Float(150.0));
    // and it all undoes
    s.execute("edit.undo", json!({})).unwrap();
}
