use super::*;

#[test]
fn headline_styles_limits_and_logo_lockups_work_across_the_catalog() {
    use filmcraft_project::ParamValue;
    let brand = Brand { logo_path: Some("own-logo.png".into()), social_handle: "mychannel".into(), ..Brand::default() };
    let mut card = full_card();
    for template in TEMPLATES {
        let p = plan(template.id, &brand, &card, 30.0).unwrap();
        for control in &p.controls {
            let e = p.over.iter().find(|e| e.layer.as_ref().is_some_and(|l| l.uid == control.layer)).unwrap();
            if control.id == "channel" {
                assert_eq!(e.params["text"].value, ParamValue::Text(String::new()), "{} duplicates its logo name", template.id);
            }
            if control.id == "title" {
                assert_eq!(e.params["caps"].value, ParamValue::Choice(1));
            }
            if template.family == "Design" && matches!(control.id.as_str(), "title" | "caption") {
                assert!(
                    !e.params.get("opacity").is_some_and(|p| p.keyframes.iter().any(|k| k.time > secs(1.0) && k.value == ParamValue::Float(0.0))),
                    "{} hides text during footage",
                    template.id
                );
            }
        }
    }
    card.text_styles
        .insert("title".into(), TextStyle { max_chars: 12, uppercase: Some(false), bold: Some(false), italic: true, size: Some(80.0), ..Default::default() });
    let card: Card = serde_json::from_value(serde_json::to_value(card).unwrap()).unwrap();
    for template in TEMPLATES.iter().filter(|t| t.id != "logoOnly") {
        let p = plan(template.id, &brand, &card, 30.0).unwrap();
        let c = p.controls.iter().find(|c| c.id == "title").unwrap();
        let e = p.over.iter().find(|e| e.layer.as_ref().is_some_and(|l| l.uid == c.layer)).unwrap();
        assert_eq!(e.params["caps"].value, ParamValue::Choice(0));
        assert_eq!(e.params["font_style"].value, ParamValue::Text("Italic".into()));
        let ParamValue::Text(text) = &e.params["text"].value else { panic!("missing text") };
        assert!(text.chars().count() <= 12);
    }
}

#[test]
fn important_text_avoids_the_instagram_right_rail_and_caption_area() {
    use filmcraft_project::ParamValue;
    for template in TEMPLATES {
        let p = plan(template.id, &Brand::default(), &full_card(), 30.0).unwrap();
        for control in p.controls.iter().filter(|c| matches!(c.id.as_str(), "title" | "caption" | "handle" | "channel")) {
            let e = p.over.iter().find(|e| e.layer.as_ref().is_some_and(|l| l.uid == control.layer)).unwrap();
            let ParamValue::Vec2(position) = e.params["position"].value else {
                continue;
            };
            let width = match e.params.get("box_width").map(|p| &p.value) {
                Some(ParamValue::Float(w)) => *w,
                _ => 0.0,
            };
            assert!(position.x >= 56.0 && position.x + width <= 900.01, "{} {} leaves the horizontal text area: {position:?} {width}", template.id, control.id);
            assert!(position.y >= 200.0 && position.y <= 1420.0, "{} {} leaves the vertical text area: {position:?}", template.id, control.id);
        }
    }
}

#[test]
fn standard_card_type_is_bundled_inter_and_keeps_the_channel_brand_font() {
    let brand = Brand { font: "Georgia".into(), ..Brand::default() };
    let card = Card { title: "A clear headline".into(), caption: "A supporting line".into(), ..Card::default() };
    for template in TEMPLATES.iter().filter(|t| t.id != "logoOnly") {
        let p = plan(template.id, &brand, &card, 10.0).unwrap();
        for control in &p.controls {
            let layer = p.over.iter().find(|e| e.layer.as_ref().is_some_and(|l| l.uid == control.layer)).unwrap();
            if matches!(control.id.as_str(), "title" | "caption") {
                assert_eq!(layer.params["font"].value, filmcraft_project::ParamValue::Text("Inter".into()), "{} {}", template.id, control.id);
                assert_eq!(
                    layer.params["font_style"].value,
                    filmcraft_project::ParamValue::Text(if control.id == "title" { "Bold" } else { "Regular" }.into())
                );
            }
            if control.id == "channel" {
                assert_eq!(layer.params["font"].value, filmcraft_project::ParamValue::Text("Georgia".into()));
            }
        }
    }
}
use filmcraft_project::graphic::{SHAPE_LAYER, TEXT_LAYER};

#[test]
fn field_fonts_override_every_template_and_survive_serialization() {
    let mut card = full_card();
    card.fonts.insert("title".into(), "Georgia".into());
    card.fonts.insert("caption".into(), "Consolas".into());
    let card: Card = serde_json::from_value(serde_json::to_value(card).unwrap()).unwrap();
    for template in TEMPLATES {
        let p = plan(template.id, &Brand::default(), &card, 15.0).unwrap();
        for c in p.controls.iter().filter(|c| card.fonts.contains_key(&c.id)) {
            let layer = p.over.iter().find(|e| e.layer.as_ref().is_some_and(|l| l.uid == c.layer)).unwrap();
            assert_eq!(layer.params["font"].value, filmcraft_project::ParamValue::Text(card.fonts[&c.id].clone()), "{} {}", template.id, c.id);
        }
    }
}

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
