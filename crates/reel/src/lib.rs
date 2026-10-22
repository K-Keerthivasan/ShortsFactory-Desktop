//! Reel Studio's model: a channel's **brand**, a Reel's **card** (the words it carries) and the
//! vertical **templates** that lay both out over the footage.
//!
//! A template does not render anything itself. [`plan`] turns a template, a brand and a card into
//! a [`ReelPlan`]: where the main clip sits in the 9:16 frame, what is drawn behind it, the graphic
//! layers drawn under and over it (ordinary `graphic_text` / `graphic_shape` layers) and where the
//! logo goes. The engine (`reel.*` commands) builds that plan into ordinary clips on ordinary
//! tracks, so everything a template makes stays editable on the timeline like any other clip.
//!
//! The brand is read from the same JSON the earlier Shorts Factory app stored per channel
//! (`channel_settings.json`), so existing channels keep their colours, font, handle and logo.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod catalog;
mod layers;

use filmcraft_project::EffectInstance;
use filmcraft_project::gtemplate::TemplateControl;
use filmcraft_time::{TICKS_PER_SECOND, Tick};
use serde::{Deserialize, Serialize};

pub use catalog::{TEMPLATES, TemplateInfo, find};
pub use layers::{Rgba, color};

/// Width of every Reel, in pixels.
pub const REEL_WIDTH: u32 = 1080;
/// Height of every Reel, in pixels.
pub const REEL_HEIGHT: u32 = 1920;
/// Frame rate new Reels are made at.
pub const REEL_FPS: f64 = 30.0;

/// A channel's look. Field names match the stored channel settings, so a settings row reads
/// straight into this; every field has a default, so a partial or older row still reads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Brand {
    /// Shown on cards that carry the channel's name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub channel_name: String,
    pub primary_color: String,
    pub secondary_color: String,
    pub background: String,
    pub card_background: String,
    pub band_color: String,
    pub card_text_color: String,
    pub card_title_size: f64,
    pub verified: bool,
    pub font: String,
    pub website: String,
    pub contact_line: String,
    pub social_handle: String,
    pub cta: Option<String>,
    /// The template a new Reel starts with.
    pub default_layout: String,
    /// The logo image, as a path the app can open (absolute, or relative to the media library).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_path: Option<String>,
}

impl Default for Brand {
    fn default() -> Self {
        Brand {
            channel_name: String::new(),
            primary_color: "#ff2e3f".into(),
            secondary_color: "#ffffff".into(),
            background: "#0b0b0e".into(),
            card_background: "#000000".into(),
            band_color: "#7d1420".into(),
            card_text_color: "#ffffff".into(),
            card_title_size: 56.0,
            verified: true,
            font: "Arial".into(),
            website: String::new(),
            contact_line: String::new(),
            social_handle: String::new(),
            cta: None,
            default_layout: "quoteCard".into(),
            logo_path: None,
        }
    }
}

impl Brand {
    /// Reads a stored channel settings JSON. Unknown fields are ignored; bad colours fall back
    /// when drawn, never here, so a damaged row still opens.
    pub fn from_settings_json(json: &str) -> Result<Brand, String> {
        serde_json::from_str(json).map_err(|e| format!("channel settings are not readable: {e}"))
    }
    /// The handle as shown on a card (`@name`), or the website when there is no handle.
    pub fn handle_line(&self) -> String {
        let h = self.social_handle.trim();
        if h.is_empty() {
            self.website.trim().to_string()
        } else if h.starts_with('@') {
            h.to_string()
        } else {
            format!("@{h}")
        }
    }
}

/// The words a Reel carries.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Card {
    /// Per-field font families; absent entries retain the template's typography.
    pub fonts: std::collections::BTreeMap<String, String>,
    pub text_styles: std::collections::BTreeMap<String, TextStyle>,
    pub text_timing: TextTiming,
    pub title: String,
    pub caption: String,
    pub attribution: String,
    /// The pill some templates carry ("01", "LEVEL UP").
    pub badge: String,
    /// Up to four supporting lines (numbered and brief templates).
    pub bullets: Vec<String>,
    /// The closing line of templates with an end state; empty uses the brand's call to action.
    pub cta: String,
    /// What the post says on Instagram (not drawn on the video).
    pub instagram_caption: String,
    pub hashtags: Vec<String>,
    /// The Short's title on YouTube (up to 100 characters); empty uses the headline.
    pub youtube_title: String,
    /// The analyzer's virality prediction (0–100) for the moment this Reel was made from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virality: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextTiming {
    #[default]
    Always,
    IntroOutro,
}

/// Per-field typography, stored with the editable Reel. Zero limit means unrestricted text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TextStyle {
    pub size: Option<f64>,
    pub max_chars: u32,
    pub uppercase: Option<bool>,
    pub bold: Option<bool>,
    pub italic: bool,
    pub tracking: f64,
    pub line_spacing: f64,
    pub align: Option<u32>,
}

impl Card {
    pub fn render_text(&self, field: &str, text: &str) -> String {
        let limit = self.text_styles.get(field).map_or(0, |s| s.max_chars) as usize;
        if limit == 0 || text.chars().count() <= limit {
            return text.to_string();
        }
        let mut result: String = text.chars().take(limit.saturating_sub(1)).collect();
        result.push('…');
        result
    }
}

/// The library post a project publishes as (rows of the shared studio database).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostLink {
    pub channel_id: i64,
    pub story_id: i64,
    pub script_id: i64,
}

/// What a project knows about being a Reel, kept in `Project::reel`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReelInfo {
    pub template: String,
    pub channel_id: Option<i64>,
    pub brand: Brand,
    pub card: Card,
    /// The footage clip the template is built around (a `ClipId`).
    pub main_clip: Option<u64>,
    /// "reel" (1080×1920, the default) or "post" (1080×1350).
    pub format: String,
    pub post: Option<PostLink>,
}

impl ReelInfo {
    /// Reads `Project::reel`; a missing or damaged value is no Reel rather than an error.
    pub fn from_value(v: Option<&serde_json::Value>) -> Option<ReelInfo> {
        v.and_then(|v| serde_json::from_value(v.clone()).ok())
    }
    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// A rectangle of the 1080×1920 frame, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }
    pub const FULL: Rect = Rect::new(0.0, 0.0, REEL_WIDTH as f64, REEL_HEIGHT as f64);
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// How the main clip fills its slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fit {
    /// Fill the slot, cropping what overflows.
    Cover,
    /// Show the whole picture inside the slot.
    Contain,
}

/// What is drawn behind everything, across the whole frame.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Backdrop {
    /// Nothing: the under layers (or black) show.
    None,
    /// A blown-up, blurred and darkened copy of the main clip.
    BlurredClip { blur: f64, brightness: f64, saturation: f64 },
}

/// Where the channel logo goes: its centre and the size of its longer side.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogoSpot {
    pub center: (f64, f64),
    pub size: f64,
    /// Opacity, 0..100.
    pub opacity: f64,
}

/// A template applied to a brand and a card: everything the engine needs to build the Reel.
#[derive(Clone, Debug)]
pub struct ReelPlan {
    pub template: &'static str,
    pub slot: Rect,
    pub fit: Fit,
    pub backdrop: Backdrop,
    /// Graphic layers under the clip (card colour, bands, gradients), back to front.
    pub under: Vec<EffectInstance>,
    /// Graphic layers over the clip (scrims, frames, words, decorations), back to front.
    pub over: Vec<EffectInstance>,
    /// The card fields and colours exposed as template properties of the over graphic.
    pub controls: Vec<TemplateControl>,
    pub logo: Option<LogoSpot>,
}

/// Seconds as ticks, for template timing.
pub fn secs(s: f64) -> Tick {
    if !s.is_finite() {
        return Tick::ZERO;
    }
    Tick((s * TICKS_PER_SECOND as f64).round() as i64)
}

/// Lays out `template` for `brand` and `card` over a Reel of `seconds`.
pub fn plan(template: &str, brand: &Brand, card: &Card, seconds: f64) -> Result<ReelPlan, String> {
    let info = find(template).ok_or_else(|| format!("unknown Reel template `{template}`"))?;
    let seconds = if seconds.is_finite() && seconds > 0.0 { seconds.min(86_400.0) } else { 30.0 };
    let mut display = card.clone();
    display.title = card.render_text("title", &card.title);
    display.caption = card.render_text("caption", &card.caption);
    display.cta = card.render_text("title", &card.cta);
    for (i, text) in display.bullets.iter_mut().enumerate() {
        *text = card.render_text(&format!("point{}", i + 1), text);
    }
    let mut plan = (info.build)(&layers::Ctx::new(info.id, brand, &display, seconds));
    for control in &plan.controls {
        let standard = matches!(control.id.as_str(), "title" | "caption" | "point1" | "point2" | "point3" | "point4");
        let custom = card.fonts.get(&control.id).filter(|f| !f.trim().is_empty());
        for layer in plan.over.iter_mut().chain(plan.under.iter_mut()).filter(|e| e.layer.as_ref().is_some_and(|l| l.uid == control.layer)) {
            if standard {
                layer.params.insert(
                    "font_style".into(),
                    filmcraft_project::Param::new(filmcraft_project::ParamValue::Text(if control.id == "title" { "Bold" } else { "Regular" }.into())),
                );
            }
            if let Some(font) = custom.map(String::as_str).or(standard.then_some("Inter")) {
                layer.params.insert("font".into(), filmcraft_project::Param::new(filmcraft_project::ParamValue::Text(font.to_string())));
            }
            if control.id == "channel" && brand.logo_path.as_ref().is_some_and(|p| !p.trim().is_empty()) {
                layer.params.insert("text".into(), filmcraft_project::Param::new(filmcraft_project::ParamValue::Text(String::new())));
            }
            let style = card.text_styles.get(&control.id).cloned().unwrap_or_default();
            if standard || card.text_styles.contains_key(&control.id) {
                use filmcraft_project::{Param, ParamValue};
                let title = control.id == "title";
                let bold = style.bold.unwrap_or(title);
                let face = match (bold, style.italic) {
                    (true, true) => "Bold Italic",
                    (true, false) => "Bold",
                    (false, true) => "Italic",
                    (false, false) => "Regular",
                };
                layer.params.insert("font_style".into(), Param::new(ParamValue::Text(face.into())));
                layer.params.insert("caps".into(), Param::new(ParamValue::Choice(u32::from(style.uppercase.unwrap_or(title)))));
                layer.params.insert(
                    "tracking".into(),
                    Param::new(ParamValue::Float(if style.tracking.is_finite() { style.tracking.clamp(-100.0, 500.0) } else { 0.0 })),
                );
                if !title && let Some(size) = style.size.filter(|s| s.is_finite()) {
                    layer.params.insert("size".into(), Param::new(ParamValue::Float(size.clamp(16.0, 160.0))));
                }
                if style.line_spacing.is_finite() && style.line_spacing != 0.0 {
                    layer.params.insert("leading".into(), Param::new(ParamValue::Float(style.line_spacing.clamp(-10.0, 80.0))));
                }
                if let Some(align) = style.align {
                    layer.params.insert("align".into(), Param::new(ParamValue::Choice(align.min(2))));
                }
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests;

/// Height of a Post (4:5).
pub const POST_HEIGHT: u32 = 1350;

/// Frame size for a format id ("post" is 1080×1350; anything else a 1080×1920 Reel).
pub fn frame_for(format: &str) -> (u32, u32) {
    if format == "post" { (REEL_WIDTH, POST_HEIGHT) } else { (REEL_WIDTH, REEL_HEIGHT) }
}

/// Lays out `template` for a frame `height` pixels tall (1080 wide). The templates are designed
/// for 1920; a shorter frame (a 4:5 Post) gets the same design with every vertical position,
/// slot, pin and animation scaled to fit and the type a little smaller.
pub fn plan_for(template: &str, brand: &Brand, card: &Card, seconds: f64, height: u32) -> Result<ReelPlan, String> {
    let mut p = plan(template, brand, card, seconds)?;
    if height == REEL_HEIGHT || height == 0 {
        return Ok(p);
    }
    let k = f64::from(height) / f64::from(REEL_HEIGHT);
    let type_k = k.sqrt();
    p.slot = Rect::new(p.slot.x, p.slot.y * k, p.slot.w, p.slot.h * k);
    if let Some(l) = p.logo.as_mut() {
        l.center.1 *= k;
        l.size *= type_k;
    }
    for e in p.under.iter_mut().chain(p.over.iter_mut()) {
        let text = e.effect == filmcraft_project::graphic::TEXT_LAYER;
        for (id, param) in e.params.iter_mut() {
            match id.as_str() {
                "position" | "anchor" => param.map_values(|v| match v {
                    filmcraft_project::ParamValue::Vec2(q) => filmcraft_project::ParamValue::Vec2(filmcraft_geom::Vec2::new(q.x, q.y * k)),
                    o => o,
                }),
                "size" if text => param.map_values(|v| match v {
                    filmcraft_project::ParamValue::Float(f) => filmcraft_project::ParamValue::Float((f * type_k).round()),
                    o => o,
                }),
                "size" => param.map_values(|v| match v {
                    filmcraft_project::ParamValue::Vec2(q) => filmcraft_project::ParamValue::Vec2(filmcraft_geom::Vec2::new(q.x, (q.y * k).max(1.0))),
                    o => o,
                }),
                _ => {}
            }
        }
        if let Some(pin) = e.layer.as_mut().and_then(|l| l.pin.as_mut()) {
            pin.offsets[1] *= k;
            pin.offsets[3] *= k;
        }
    }
    Ok(p)
}
