//! Building blocks the templates are written with: brand colours, text and shape layers, timing.

use filmcraft_geom::Vec2;
use filmcraft_project::graphic::{new_shape_layer, new_text_layer};
use filmcraft_project::graphic_design::{LayerExtra, Pin, PinTarget};
use filmcraft_project::gtemplate::{ControlKind, TemplateControl};
use filmcraft_project::{EffectInstance, Interpolation, Keyframe, Param, ParamValue};

use crate::{Backdrop, Brand, Card, Fit, LogoSpot, Rect, ReelPlan, secs};

/// An sRGB colour with alpha, 0..1.
pub type Rgba = [f32; 4];

/// `#rrggbb` (or `#rrggbbaa`) as a colour; anything unreadable is `fallback`.
pub fn color(hex: &str, fallback: Rgba) -> Rgba {
    let h = hex.trim().trim_start_matches('#');
    let byte = |i: usize| h.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).map(|b| f32::from(b) / 255.0);
    match (h.len(), byte(0), byte(2), byte(4)) {
        (6, Some(r), Some(g), Some(b)) => [r, g, b, 1.0],
        (8, Some(r), Some(g), Some(b)) => [r, g, b, byte(6).unwrap_or(1.0)],
        _ => fallback,
    }
}
pub(crate) const fn with_alpha(c: Rgba, a: f32) -> Rgba {
    [c[0], c[1], c[2], a]
}
pub(crate) const WHITE: Rgba = [1.0, 1.0, 1.0, 1.0];
pub(crate) const BLACK: Rgba = [0.0, 0.0, 0.0, 1.0];
/// A monospace face present on every Windows and macOS system; the text engine falls back if not.
pub(crate) const MONO: &str = "Consolas";

/// Everything a template reads: the brand's colours resolved once, the card and the length.
pub(crate) struct Ctx<'a> {
    pub id: &'static str,
    pub brand: &'a Brand,
    pub card: &'a Card,
    pub seconds: f64,
    pub accent: Rgba,
    pub background: Rgba,
    pub card_bg: Rgba,
    pub band: Rgba,
    pub text: Rgba,
    pub font: String,
}

impl<'a> Ctx<'a> {
    pub fn new(id: &'static str, brand: &'a Brand, card: &'a Card, seconds: f64) -> Self {
        let font = if brand.font.trim().is_empty() { "Arial".to_string() } else { brand.font.trim().to_string() };
        Ctx {
            id,
            brand,
            card,
            seconds,
            accent: color(&brand.primary_color, [1.0, 0.18, 0.25, 1.0]),
            background: color(&brand.background, [0.04, 0.04, 0.05, 1.0]),
            card_bg: color(&brand.card_background, BLACK),
            band: color(&brand.band_color, [0.49, 0.08, 0.13, 1.0]),
            text: color(&brand.card_text_color, WHITE),
            font,
        }
    }
    pub fn title(&self) -> &str {
        let t = self.card.title.trim();
        if t.is_empty() { "Your headline goes here" } else { t }
    }
    pub fn cta(&self) -> String {
        let c = self.card.cta.trim();
        if !c.is_empty() {
            return c.to_string();
        }
        self.brand.cta.as_deref().map(str::trim).filter(|c| !c.is_empty()).map_or_else(|| self.title().to_string(), str::to_string)
    }
    pub fn channel(&self) -> String {
        let n = self.brand.channel_name.trim();
        if n.is_empty() { "Your channel".into() } else { n.to_string() }
    }
    /// Supporting lines: the card's bullets, else its caption.
    pub fn bullets(&self) -> Vec<String> {
        let b: Vec<String> = self.card.bullets.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).take(4).collect();
        if b.is_empty() && !self.card.caption.trim().is_empty() { vec![self.card.caption.trim().to_string()] } else { b }
    }
    pub fn badge(&self) -> String {
        let b = self.card.badge.trim();
        if b.is_empty() { "01".into() } else { b.to_string() }
    }
}

/// Shrinks a headline as it gets longer, so a long one never runs off its area.
pub(crate) fn fit_size(text: &str, base: f64) -> f64 {
    let n = text.chars().count();
    (base
        * if n > 150 {
            0.62
        } else if n > 100 {
            0.74
        } else if n > 60 {
            0.86
        } else {
            1.0
        })
    .round()
}

/// A conservative estimate of how tall wrapped text set at `size` in a `width` box is. The text
/// engine does the real layout; this only places what stacks below it.
pub(crate) fn text_height(text: &str, size: f64, width: f64, line_height: f64) -> f64 {
    text_height_w(text, size, width, line_height, 0.55)
}
/// [`text_height`] for a face whose average glyph is `em` of the size wide (caps, heavy and
/// monospace faces run near 0.68).
pub(crate) fn text_height_w(text: &str, size: f64, width: f64, line_height: f64, em: f64) -> f64 {
    if size <= 0.0 || width <= 0.0 {
        return 0.0;
    }
    let per_line = (width / (size * em.max(0.3))).max(1.0);
    let lines: f64 = text.split('\n').map(|l| (l.chars().count() as f64 / per_line).ceil().max(1.0)).sum();
    lines * size * line_height
}

/// Collects a template's layers and controls, handing out the layer ids pins and controls use.
pub(crate) struct Builder<'c, 'a> {
    pub c: &'c Ctx<'a>,
    next_uid: u64,
    pub slot: Rect,
    pub fit: Fit,
    pub backdrop: Backdrop,
    pub under: Vec<EffectInstance>,
    pub over: Vec<EffectInstance>,
    pub controls: Vec<TemplateControl>,
    pub logo: Option<LogoSpot>,
}

/// Where a new layer goes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Z {
    Under,
    Over,
}

fn set(e: &mut EffectInstance, id: &str, v: ParamValue) {
    e.params.insert(id.to_string(), Param::new(v));
}

impl<'c, 'a> Builder<'c, 'a> {
    pub fn new(c: &'c Ctx<'a>, slot: Rect) -> Self {
        Builder { c, next_uid: 1, slot, fit: Fit::Cover, backdrop: Backdrop::None, under: Vec::new(), over: Vec::new(), controls: Vec::new(), logo: None }
    }
    pub fn finish(self) -> ReelPlan {
        ReelPlan {
            template: self.c.id,
            slot: self.slot,
            fit: self.fit,
            backdrop: self.backdrop,
            under: self.under,
            over: self.over,
            controls: self.controls,
            logo: self.logo,
        }
    }
    fn push(&mut self, z: Z, name: &str, mut e: EffectInstance) -> u64 {
        let uid = self.next_uid;
        self.next_uid = self.next_uid.saturating_add(1);
        set(&mut e, "name", ParamValue::Text(name.into()));
        e.layer = Some(Box::new(LayerExtra { uid, ..Default::default() }));
        match z {
            Z::Under => self.under.push(e),
            Z::Over => self.over.push(e),
        }
        uid
    }
    fn layer_mut(&mut self, uid: u64) -> Option<&mut EffectInstance> {
        self.under.iter_mut().chain(self.over.iter_mut()).find(|e| e.layer.as_ref().is_some_and(|l| l.uid == uid))
    }

    /// A filled rectangle (`r` in frame pixels), optionally rounded.
    pub fn rect(&mut self, z: Z, name: &str, r: Rect, fill: Rgba, radius: f64) -> u64 {
        let (cx, cy) = r.center();
        let mut e = new_shape_layer(0, Vec2::new(cx, cy), Vec2::new(r.w.max(1.0), r.h.max(1.0)), vec![]);
        set(&mut e, "fill_color", ParamValue::Color(fill));
        if radius > 0.0 {
            set(&mut e, "corner_radius", ParamValue::Float(radius));
        }
        self.push(z, name, e)
    }
    /// A rectangle filled with a linear gradient from `a` to `b`; `angle` in degrees (90 = down).
    pub fn gradient(&mut self, z: Z, name: &str, r: Rect, a: Rgba, b: Rgba, angle: f64) -> u64 {
        let uid = self.rect(z, name, r, a, 0.0);
        if let Some(e) = self.layer_mut(uid) {
            set(e, "fill_kind", ParamValue::Choice(1));
            set(e, "gradient_start", ParamValue::Color(a));
            set(e, "gradient_end", ParamValue::Color(b));
            set(e, "gradient_angle", ParamValue::Float(angle));
        }
        uid
    }
    /// An outline (no fill) around `r`.
    pub fn outline(&mut self, z: Z, name: &str, r: Rect, stroke: Rgba, width: f64, radius: f64) -> u64 {
        let uid = self.rect(z, name, r, stroke, radius);
        if let Some(e) = self.layer_mut(uid) {
            set(e, "fill", ParamValue::Bool(false));
            set(e, "stroke", ParamValue::Bool(true));
            set(e, "stroke_color", ParamValue::Color(stroke));
            set(e, "stroke_width", ParamValue::Float(width));
            set(e, "stroke_type", ParamValue::Choice(2));
        }
        uid
    }
    /// Paragraph text in a box `width` wide with its top-left corner at (x, y).
    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, z: Z, name: &str, text: &str, x: f64, y: f64, width: f64, size: f64, style: Style<'_>) -> u64 {
        let mut e = new_text_layer(text, Vec2::new(x, y), size);
        set(&mut e, "box_width", ParamValue::Float(width.max(size)));
        let font = style.font.map_or_else(|| self.c.font.clone(), str::to_string);
        set(&mut e, "font", ParamValue::Text(font));
        set(&mut e, "font_style", ParamValue::Text(style.weight.into()));
        set(&mut e, "fill_color", ParamValue::Color(style.color.unwrap_or(self.c.text)));
        set(&mut e, "align", ParamValue::Choice(style.align));
        if style.tracking != 0.0 {
            set(&mut e, "tracking", ParamValue::Float(style.tracking));
        }
        if style.leading != 0.0 {
            set(&mut e, "leading", ParamValue::Float(style.leading));
        }
        if style.caps {
            set(&mut e, "caps", ParamValue::Choice(1));
        }
        if style.opacity < 100.0 {
            set(&mut e, "opacity", ParamValue::Float(style.opacity));
        }
        if let Some(sh) = style.shadow {
            set(&mut e, "shadow", ParamValue::Bool(true));
            set(&mut e, "shadow_color", ParamValue::Color(sh));
            set(&mut e, "shadow_opacity", ParamValue::Float(f64::from(sh[3]) * 100.0));
            set(&mut e, "shadow_distance", ParamValue::Float(style.shadow_distance));
            set(&mut e, "shadow_blur", ParamValue::Float(style.shadow_blur));
        }
        if let Some((col, w)) = style.stroke {
            set(&mut e, "stroke", ParamValue::Bool(true));
            set(&mut e, "stroke_color", ParamValue::Color(col));
            set(&mut e, "stroke_width", ParamValue::Float(w));
        }
        if let Some((col, pad, radius)) = style.plate {
            set(&mut e, "background", ParamValue::Bool(true));
            set(&mut e, "background_color", ParamValue::Color(with_alpha(col, 1.0)));
            set(&mut e, "background_opacity", ParamValue::Float(f64::from(col[3]) * 100.0));
            set(&mut e, "background_size", ParamValue::Float(pad));
            set(&mut e, "background_radius", ParamValue::Float(radius));
        }
        if style.underline {
            set(&mut e, "underline", ParamValue::Bool(true));
        }
        self.push(z, name, e)
    }

    /// Exposes one parameter of a layer as an editable template property.
    pub fn control(&mut self, id: &str, name: &str, layer: u64, param: &str) {
        self.controls.push(TemplateControl {
            id: id.into(),
            name: name.into(),
            kind: ControlKind::for_param(param),
            layer,
            param: param.into(),
            min: None,
            max: None,
        });
    }

    /// Animates `param` of a layer through `(seconds, value)` keys with one interpolation.
    pub fn animate(&mut self, uid: u64, param: &str, keys: &[(f64, ParamValue)], interp: Interpolation) {
        let Some(e) = self.layer_mut(uid) else { return };
        let Some(first) = keys.first() else { return };
        let mut p = Param::new(first.1.clone());
        p.keyframes =
            keys.iter().map(|(t, v)| Keyframe { time: secs(*t), value: v.clone(), interp, out_influence: 1.0 / 3.0, in_influence: 1.0 / 3.0 }).collect();
        e.params.insert(param.to_string(), p);
    }
    /// Sets one static parameter of a layer.
    pub fn param(&mut self, uid: u64, id: &str, v: ParamValue) {
        if let Some(e) = self.layer_mut(uid) {
            set(e, id, v);
        }
    }
    /// Makes the given edges (left, top, right, bottom) of a layer follow the same edges of
    /// `to` (another layer's uid, or 0 for the frame), at `offsets` (this edge minus the target's).
    /// Text keeps its size and moves; a shape pinned on opposite edges stretches.
    pub fn pin(&mut self, uid: u64, to: u64, edges: [bool; 4], offsets: [f64; 4]) {
        let to = if to == 0 { PinTarget::Frame } else { PinTarget::Layer(to) };
        if let Some(e) = self.layer_mut(uid) {
            e.layer.get_or_insert_with(Default::default).pin = Some(Pin { to, left: edges[0], top: edges[1], right: edges[2], bottom: edges[3], offsets });
        }
    }
    /// Keeps a layer's bottom edge `above` pixels above the bottom of the frame, whatever height
    /// its text turns out to be.
    pub fn bottom_at(&mut self, uid: u64, y: f64) {
        self.pin(uid, 0, [false, false, false, true], [0.0, 0.0, 0.0, y - crate::REEL_HEIGHT as f64]);
    }
    /// The layer's anchor (layer pixels from its origin).
    pub fn anchor(&mut self, uid: u64, x: f64, y: f64) {
        if let Some(e) = self.layer_mut(uid) {
            set(e, "anchor", ParamValue::Vec2(Vec2::new(x, y)));
        }
    }
    pub fn position(&mut self, uid: u64, x: f64, y: f64) {
        if let Some(e) = self.layer_mut(uid) {
            set(e, "position", ParamValue::Vec2(Vec2::new(x, y)));
        }
    }

    /// A bar across the bottom that fills from left to right over the whole Reel.
    pub fn progress(&mut self, y: f64, height: f64, color: Rgba) {
        let w = Rect::FULL.w;
        let uid = self.rect(Z::Over, "Progress", Rect::new(0.0, y, w, height), color, 0.0);
        // Scale about the left edge: the anchor is the shape's left end, placed at x = 0.
        self.anchor(uid, -w / 2.0, 0.0);
        self.position(uid, 0.0, y + height / 2.0);
        let end = self.c.seconds.max(0.1);
        self.param(uid, "uniform_scale", ParamValue::Bool(false));
        self.animate(uid, "scale_width", &[(0.0, ParamValue::Float(0.0)), (end, ParamValue::Float(100.0))], Interpolation::Linear);
    }

    /// Fades a layer in over `fade` seconds from `start`, optionally rising `rise` pixels.
    pub fn reveal(&mut self, uid: u64, start: f64, fade: f64, rise: f64) {
        self.animate(uid, "opacity", &[(start, ParamValue::Float(0.0)), (start + fade, ParamValue::Float(100.0))], Interpolation::EaseOut);
        if rise != 0.0
            && let Some(ParamValue::Vec2(p)) = self.layer_mut(uid).and_then(|e| e.params.get("position")).map(|p| p.value.clone())
        {
            self.animate(
                uid,
                "position",
                &[(start, ParamValue::Vec2(Vec2::new(p.x, p.y + rise))), (start + fade, ParamValue::Vec2(p))],
                Interpolation::EaseOut,
            );
        }
    }

    /// The channel row: logo spot, name and handle, starting at (x, y).
    pub fn brand_row(&mut self, x: f64, y: f64, mark: f64, name_size: f64, handle_size: f64) -> (u64, u64) {
        self.logo = Some(LogoSpot { center: (x + mark / 2.0, y + mark / 2.0), size: mark, opacity: 100.0 });
        let tx = x + mark + mark * 0.24;
        let name = self.text(Z::Over, "Channel", &self.c.channel(), tx, y + mark * 0.5 - name_size * 1.05, 900.0 - tx, name_size, Style::bold());
        let handle = self.text(
            Z::Over,
            "Handle",
            &self.c.brand.handle_line(),
            tx,
            y + mark * 0.5 + handle_size * 0.15,
            900.0 - tx,
            handle_size,
            Style { opacity: 62.0, ..Style::regular() },
        );
        self.control("channel", "Channel Name", name, "text");
        self.control("handle", "Handle", handle, "text");
        (name, handle)
    }
}

/// How a text layer is set.
#[derive(Clone, Copy)]
pub(crate) struct Style<'f> {
    pub font: Option<&'f str>,
    pub weight: &'static str,
    pub color: Option<Rgba>,
    /// 0 left, 1 centre, 2 right.
    pub align: u32,
    pub tracking: f64,
    pub leading: f64,
    pub caps: bool,
    pub opacity: f64,
    pub shadow: Option<Rgba>,
    pub shadow_distance: f64,
    pub shadow_blur: f64,
    pub stroke: Option<(Rgba, f64)>,
    /// Background plate: colour (alpha = opacity), padding, corner radius.
    pub plate: Option<(Rgba, f64, f64)>,
    pub underline: bool,
}

impl Style<'static> {
    pub const fn regular() -> Self {
        Style {
            font: None,
            weight: "Regular",
            color: None,
            align: 0,
            tracking: 0.0,
            leading: 0.0,
            caps: false,
            opacity: 100.0,
            shadow: None,
            shadow_distance: 4.0,
            shadow_blur: 24.0,
            stroke: None,
            plate: None,
            underline: false,
        }
    }
    pub const fn bold() -> Self {
        Style { weight: "Bold", ..Style::regular() }
    }
    pub const fn mono() -> Self {
        Style { font: Some(MONO), ..Style::regular() }
    }
}
