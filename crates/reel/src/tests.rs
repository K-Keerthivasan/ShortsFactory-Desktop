use super::*;
use filmcraft_project::graphic::{SHAPE_LAYER, TEXT_LAYER};

fn full_card() -> Card {
    Card {
        title: "A clip worth watching twice".into(),
        caption: "Filmed on the first day of the season".into(),
        attribution: "Original: @creator".into(),
        badge: "02".into(),
        bullets: vec!["First point".into(), "Second point".into()],
        cta: "Follow for more".into(),
        ..Default::default()
    }
}

#[test]
fn every_template_builds_for_empty_long_and_full_cards() {
    let brand = Brand { channel_name: "JKR".into(), social_handle: "jkr".into(), ..Default::default() };
    let long = Card { title: "word ".repeat(120), caption: "x".repeat(190), bullets: vec!["y".repeat(200); 6], ..Default::default() };
    for t in TEMPLATES {
        for card in [Card::default(), long.clone(), full_card()] {
            for seconds in [0.0, 3.0, 30.0, f64::NAN, 1.0e12] {
                let p = plan(t.id, &brand, &card, seconds).unwrap_or_else(|e| panic!("{}: {e}", t.id));
                assert_eq!(p.template, t.id);
                assert!(p.slot.w > 0.0 && p.slot.h > 0.0, "{} has an empty slot", t.id);
                assert!(p.slot.x >= 0.0 && p.slot.y >= 0.0 && p.slot.x + p.slot.w <= 1080.0 + 1e-9, "{} slot leaves the frame", t.id);
                for l in p.under.iter().chain(&p.over) {
                    assert!(l.effect == TEXT_LAYER || l.effect == SHAPE_LAYER);
                    assert!(l.layer.as_ref().is_some_and(|x| x.uid > 0));
                }
                for c in &p.controls {
                    assert!(p.over.iter().any(|l| l.layer.as_ref().is_some_and(|x| x.uid == c.layer)), "{}: control {} names no layer", t.id, c.id);
                }
            }
        }
    }
}

#[test]
fn every_card_template_exposes_the_headline() {
    for t in TEMPLATES.iter().filter(|t| t.id != "logoOnly") {
        let p = plan(t.id, &Brand::default(), &full_card(), 30.0).unwrap_or_else(|e| panic!("{e}"));
        assert!(p.controls.iter().any(|c| c.id == "title"), "{} has no headline control", t.id);
    }
}

#[test]
fn there_are_twenty_one_templates_with_unique_ids() {
    assert_eq!(TEMPLATES.len(), 21);
    let mut ids: Vec<_> = TEMPLATES.iter().map(|t| t.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 21);
    assert!(find("plainRepost").is_some_and(|t| t.id == "quoteCard"));
    assert!(plan("nope", &Brand::default(), &Card::default(), 10.0).is_err());
}

#[test]
fn brand_reads_stored_channel_settings() {
    let json = r##"{"approvalMode":"SEMI_AUTO","primaryColor":"#12ab34","cardBackground":"#101010","font":"Inter","socialHandle":"k2digital","defaultLayout":"framed","templates":[],"postingSlots":["10:00"]}"##;
    let b = Brand::from_settings_json(json).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(b.primary_color, "#12ab34");
    assert_eq!(b.default_layout, "framed");
    assert_eq!(b.handle_line(), "@k2digital");
    assert_eq!(b.background, "#0b0b0e", "missing fields take the defaults");
    assert!(Brand::from_settings_json("not json").is_err());
}

#[test]
fn colours_parse_and_fall_back() {
    assert_eq!(color("#ff0000", [0.0; 4]), [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(color("00ff0080", [0.0; 4])[3], 128.0 / 255.0);
    assert_eq!(color("#zz", [0.5; 4]), [0.5; 4]);
    assert_eq!(color("", [0.25; 4]), [0.25; 4]);
    assert_eq!(color("#ffé000", [0.25; 4]), [0.25; 4], "a multi-byte character never slices mid-character");
}

#[test]
fn long_reels_switch_the_design_headline_to_the_call_to_action() {
    let p = plan("jkr-clean-v2", &Brand::default(), &full_card(), 30.0).unwrap_or_else(|e| panic!("{e}"));
    let title = p.over.iter().find(|l| l.params.get("name").is_some_and(|n| n.value == filmcraft_project::ParamValue::Text("Title".into())));
    let text = title.and_then(|l| l.params.get("text")).unwrap_or_else(|| panic!("no title"));
    assert_eq!(text.keyframes.len(), 2);
    assert_eq!(text.keyframes[1].value, filmcraft_project::ParamValue::Text("Follow for more".into()));
}

#[test]
fn every_template_fits_a_post() {
    for t in TEMPLATES {
        let p = plan_for(t.id, &Brand::default(), &full_card(), 20.0, POST_HEIGHT).unwrap_or_else(|e| panic!("{e}"));
        assert!(p.slot.y + p.slot.h <= f64::from(POST_HEIGHT) + 1e-6, "{} slot leaves the 4:5 frame", t.id);
    }
    assert_eq!(frame_for("post"), (1080, 1350));
    assert_eq!(frame_for("reel"), (1080, 1920));
}
