use serde_json::{Value, json};

use crate::Session;
use filmcraft_project::{ClipId, ParamValue, TrackItem};
use filmcraft_reel::TEMPLATES;

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
