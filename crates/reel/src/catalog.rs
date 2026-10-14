//! The Reel templates. Seven card layouts (a card carrying the channel's identity around the
//! clip) and fourteen footage-first designs. Each is a function from the brand and the card to a
//! [`ReelPlan`]; all geometry is in pixels of the 1080×1920 frame.

use filmcraft_project::{Interpolation, ParamValue};

use crate::layers::{BLACK, Builder, Ctx, MONO, Rgba, Style, WHITE, Z, fit_size, text_height, text_height_w, with_alpha};
use crate::{Backdrop, Fit, LogoSpot, Rect, ReelPlan};

/// One entry of the template gallery.
pub struct TemplateInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub note: &'static str,
    /// "Card" layouts or "Design" templates.
    pub family: &'static str,
    pub(crate) build: fn(&Ctx<'_>) -> ReelPlan,
}

pub static TEMPLATES: &[TemplateInfo] = &[
    TemplateInfo { id: "quoteCard", name: "Quote card", note: "Card over a cropped clip", family: "Card", build: quote_card },
    TemplateInfo { id: "fullBleed", name: "Full bleed", note: "Clip fills 9:16, headline in a lower third", family: "Card", build: full_bleed },
    TemplateInfo { id: "headline", name: "Headline hero", note: "Big headline, rounded clip, mono caption", family: "Card", build: headline },
    TemplateInfo { id: "numbered", name: "Numbered point", note: "Badge, clip on top, bullets below", family: "Card", build: numbered },
    TemplateInfo { id: "framed", name: "Framed", note: "Hairline frame, clean header, footer lockup", family: "Card", build: framed },
    TemplateInfo { id: "brief", name: "Brief", note: "Contact line, numbered points", family: "Card", build: brief },
    TemplateInfo { id: "logoOnly", name: "Logo only", note: "Full-screen clip with only your logo", family: "Card", build: logo_only },
    TemplateInfo { id: "jkr-clean-v2", name: "JKR Clean", note: "Video first · crisp type · quiet accents", family: "Design", build: design },
    TemplateInfo { id: "jkr-cinematic-v2", name: "JKR Cinematic", note: "Video blur · floating frame · film light", family: "Design", build: design },
    TemplateInfo { id: "jkr-esports-v2", name: "JKR Esports", note: "Cut corners · precision rails · punchy type", family: "Design", build: design },
    TemplateInfo { id: "jkr-hud-v2", name: "JKR HUD", note: "Tactical corners · telemetry · reveal", family: "Design", build: design },
    TemplateInfo { id: "jkr-editorial-v2", name: "JKR Editorial", note: "Magazine type · thin rules · monochrome", family: "Design", build: design },
    TemplateInfo { id: "jkr-minimal-v2", name: "JKR Minimal", note: "Pure footage · quiet signature", family: "Design", build: design },
    TemplateInfo { id: "jkr-neon-v2", name: "JKR Neon", note: "Light edge · glowing keyword · orbits", family: "Design", build: design },
    TemplateInfo { id: "jkr-retro-v2", name: "JKR Retro Gaming", note: "Arcade markers · stepped type", family: "Design", build: design },
    TemplateInfo { id: "jkr-dark-glass-v2", name: "JKR Dark Glass", note: "Frosted caption · soft depth · fine edge", family: "Design", build: design },
    TemplateInfo { id: "jkr-cyber-v2", name: "JKR Cyber", note: "Digital grid · HUD", family: "Design", build: design },
    TemplateInfo { id: "jkr-technical-v2", name: "JKR Technical", note: "Blueprint · precise labels · offset outline", family: "Design", build: design },
    TemplateInfo { id: "jkr-highlight-v2", name: "JKR Highlight", note: "Opening hook · clear gameplay · closing CTA", family: "Design", build: design },
    TemplateInfo { id: "jkr-story-v2", name: "JKR Story", note: "Narrative hook · emphasis · ending card", family: "Design", build: design },
    TemplateInfo { id: "jkr-meme-v2", name: "JKR Meme", note: "Expressive outline · bold hook", family: "Design", build: design },
];

/// A template by id. Ids of retired layouts open as the quote card, as they did before.
pub fn find(id: &str) -> Option<&'static TemplateInfo> {
    let id = match id {
        "plainRepost" | "broadcast" => "quoteCard",
        other => other,
    };
    TEMPLATES.iter().find(|t| t.id == id)
}

// ------------------------------------------------------------------------------------------------
// Card layouts
// ------------------------------------------------------------------------------------------------

fn title_and_caption(b: &mut Builder<'_, '_>, x: f64, y: f64, width: f64, size: f64, line: f64, style: Style<'_>, caption_bottom: Option<f64>) -> f64 {
    let title = b.c.title().to_string();
    let t = b.text(Z::Over, "Title", &title, x, y, width, size, style);
    b.control("title", "Headline", t, "text");
    b.reveal(t, 0.0, 0.6, 18.0);
    let mut bottom = y + text_height(&title, size, width, line);
    let caption = b.c.card.caption.trim().to_string();
    if !caption.is_empty() {
        let cap = b.text(Z::Over, "Caption", &caption, x, bottom + 24.0, width, 32.0, Style { opacity: 60.0, ..Style::regular() });
        b.control("caption", "Caption", cap, "text");
        bottom += 24.0 + text_height(&caption, 32.0, width, 1.3);
        if let Some(edge) = caption_bottom {
            b.bottom_at(cap, edge);
        }
    }
    bottom
}

fn quote_card(c: &Ctx<'_>) -> ReelPlan {
    let top = 650.0;
    let mut b = Builder::new(c, Rect::new(0.0, top, 1080.0, 1920.0 - top));
    b.rect(Z::Under, "Card", Rect::FULL, c.card_bg, 0.0);
    b.brand_row(58.0, 60.0, 104.0, 42.0, 34.0);
    let size = (c.brand.card_title_size * if c.title().chars().count() > 220 { 0.72 } else { 1.0 }).round();
    title_and_caption(&mut b, 58.0, 230.0, 964.0, size, 1.26, Style { weight: "SemiBold", ..Style::regular() }, Some(top - 34.0));
    b.finish()
}

fn full_bleed(c: &Ctx<'_>) -> ReelPlan {
    let mut b = Builder::new(c, Rect::FULL);
    b.gradient(Z::Over, "Top scrim", Rect::new(0.0, 0.0, 1080.0, 430.0), with_alpha(BLACK, 0.8), with_alpha(BLACK, 0.0), 90.0);
    b.gradient(Z::Over, "Bottom scrim", Rect::new(0.0, 990.0, 1080.0, 930.0), with_alpha(BLACK, 0.0), with_alpha(BLACK, 0.92), 90.0);
    b.brand_row(56.0, 54.0, 78.0, 34.0, 30.0);
    let size = fit_size(c.title(), 78.0);
    let caption = c.card.caption.trim();
    let cap_h = if caption.is_empty() { 0.0 } else { text_height(caption, 30.0, 968.0, 1.4) };
    let title_h = text_height(c.title(), size, 968.0, 1.08);
    let y = 1800.0 - if cap_h > 0.0 { cap_h + 22.0 } else { 0.0 } - title_h;
    let bar = b.rect(Z::Over, "Accent", Rect::new(56.0, y - 32.0, 96.0, 6.0), c.accent, 3.0);
    let t = b.text(Z::Over, "Title", c.title(), 56.0, y, 968.0, size, Style { weight: "ExtraBold", leading: -6.0, ..Style::regular() });
    b.control("title", "Headline", t, "text");
    b.reveal(t, 0.0, 0.6, 0.0);
    if cap_h > 0.0 {
        let cap = b.text(Z::Over, "Caption", caption, 56.0, 1800.0 - cap_h, 968.0, 30.0, Style { opacity: 80.0, ..Style::mono() });
        b.control("caption", "Caption", cap, "text");
        b.bottom_at(cap, 1800.0);
        b.pin(t, cap, [false, false, false, true], [0.0, 0.0, 0.0, -(cap_h + 22.0)]);
    } else {
        b.bottom_at(t, 1800.0);
    }
    b.pin(bar, t, [false, true, false, false], [0.0, -32.0, 0.0, 0.0]);
    b.finish()
}

fn headline(c: &Ctx<'_>) -> ReelPlan {
    let size = fit_size(c.title(), 92.0);
    let title_y = 70.0 + 72.0 + 46.0;
    let title_h = text_height(c.title(), size, 940.0, 1.05);
    let caption = c.card.caption.trim();
    let caption_h = if caption.is_empty() { 0.0 } else { 34.0 + text_height(caption, 31.0, 940.0, 1.45) };
    let slot_y = title_y + title_h + 44.0;
    let slot_h = (1920.0 - 70.0 - caption_h - slot_y).max(400.0);
    let mut b = Builder::new(c, Rect::new(70.0, slot_y, 940.0, slot_h));
    b.gradient(Z::Under, "Card", Rect::FULL, c.card_bg, c.background, 75.0);
    b.brand_row(70.0, 70.0, 72.0, 32.0, 28.0);
    let t = b.text(Z::Over, "Title", c.title(), 70.0, title_y, 940.0, size, Style { weight: "ExtraBold", leading: -6.0, ..Style::regular() });
    b.control("title", "Headline", t, "text");
    b.reveal(t, 0.0, 0.6, 18.0);
    b.rect(Z::Over, "Accent rule", Rect::new(70.0, slot_y + slot_h - 5.0, 940.0, 5.0), c.accent, 0.0);
    if !caption.is_empty() {
        let cap = b.text(Z::Over, "Caption", caption, 70.0, slot_y + slot_h + 34.0, 940.0, 31.0, Style { align: 1, opacity: 82.0, ..Style::mono() });
        b.control("caption", "Caption", cap, "text");
    }
    b.finish()
}

fn bullets(b: &mut Builder<'_, '_>, x: f64, mut y: f64, width: f64, size: f64) -> f64 {
    let accent = b.c.accent;
    for (i, line) in b.c.bullets().iter().enumerate() {
        b.text(Z::Over, "Dash", "—", x, y, 40.0, size, Style { color: Some(accent), ..Style::mono() });
        let l = b.text(Z::Over, &format!("Point {}", i + 1), line, x + 56.0, y, width - 56.0, size, Style { opacity: 88.0, ..Style::mono() });
        b.control(&format!("point{}", i + 1), &format!("Point {}", i + 1), l, "text");
        y += text_height(line, size, width - 56.0, 1.42) + 16.0;
    }
    y
}

fn numbered(c: &Ctx<'_>) -> ReelPlan {
    let mut b = Builder::new(c, Rect::new(0.0, 0.0, 1080.0, 1060.0));
    b.rect(Z::Under, "Card", Rect::FULL, c.card_bg, 0.0);
    b.gradient(Z::Over, "Fade", Rect::new(0.0, 0.0, 1080.0, 1062.0), with_alpha(BLACK, 0.0), c.card_bg, 90.0);
    let badge = b.text(
        Z::Over,
        "Badge",
        &c.badge(),
        56.0,
        54.0,
        300.0,
        30.0,
        Style { tracking: 70.0, plate: Some(([1.0, 1.0, 1.0, 0.12], 12.0, 30.0)), ..Style::mono() },
    );
    b.control("badge", "Badge", badge, "text");
    let size = fit_size(c.title(), 72.0);
    let t = b.text(Z::Over, "Title", c.title(), 66.0, 1060.0, 948.0, size, Style { weight: "ExtraBold", leading: -4.0, ..Style::regular() });
    b.control("title", "Headline", t, "text");
    let y = 1060.0 + text_height(c.title(), size, 948.0, 1.08) + 26.0;
    bullets(&mut b, 66.0, y, 948.0, 33.0);
    b.logo = Some(LogoSpot { center: (430.0, 1830.0), size: 54.0, opacity: 100.0 });
    let h = b.text(Z::Over, "Handle", &c.brand.handle_line(), 470.0, 1814.0, 500.0, 30.0, Style { opacity: 70.0, ..Style::regular() });
    b.control("handle", "Handle", h, "text");
    b.finish()
}

fn framed(c: &Ctx<'_>) -> ReelPlan {
    let size = fit_size(c.title(), 56.0);
    let caption = c.card.caption.trim();
    let footer = 30.0 + text_height(c.title(), size, 968.0, 1.14) + if caption.is_empty() { 0.0 } else { 16.0 + 40.0 } + 44.0;
    let slot_y = 6.0 + 34.0 + 66.0 + 26.0;
    let slot = Rect::new(56.0, slot_y, 968.0, (1920.0 - slot_y - footer).max(500.0));
    let mut b = Builder::new(c, slot);
    b.rect(Z::Under, "Card", Rect::FULL, c.card_bg, 0.0);
    b.rect(Z::Over, "Top rule", Rect::new(0.0, 0.0, 1080.0, 6.0), c.accent, 0.0);
    b.logo = Some(LogoSpot { center: (56.0 + 33.0, 40.0 + 33.0), size: 66.0, opacity: 100.0 });
    let h = b.text(Z::Over, "Handle", &c.brand.handle_line(), 540.0, 56.0, 484.0, 28.0, Style { align: 2, opacity: 70.0, ..Style::regular() });
    b.control("handle", "Handle", h, "text");
    b.outline(Z::Over, "Frame", slot, with_alpha(c.accent, 0.33), 2.0, 20.0);
    let ty = slot.y + slot.h + 30.0;
    let t = b.text(Z::Over, "Title", c.title(), 56.0, ty, 968.0, size, Style { weight: "ExtraBold", align: 1, ..Style::regular() });
    b.control("title", "Headline", t, "text");
    if !caption.is_empty() {
        let cap = b.text(
            Z::Over,
            "Caption",
            caption,
            56.0,
            ty + text_height(c.title(), size, 968.0, 1.14) + 16.0,
            968.0,
            28.0,
            Style { align: 1, opacity: 72.0, ..Style::mono() },
        );
        b.control("caption", "Caption", cap, "text");
    }
    b.finish()
}

fn brief(c: &Ctx<'_>) -> ReelPlan {
    let contact = c.brand.contact_line.trim();
    let slot_y = if contact.is_empty() { 26.0 } else { 34.0 + 30.0 + 26.0 };
    let mut b = Builder::new(c, Rect::new(0.0, slot_y, 1080.0, 760.0));
    b.rect(Z::Under, "Card", Rect::FULL, c.card_bg, 0.0);
    b.gradient(Z::Over, "Fade in", Rect::new(0.0, slot_y, 1080.0, 170.0), c.card_bg, with_alpha(c.card_bg, 0.0), 90.0);
    b.gradient(Z::Over, "Fade out", Rect::new(0.0, slot_y + 470.0, 1080.0, 292.0), with_alpha(c.card_bg, 0.0), c.card_bg, 90.0);
    if !contact.is_empty() {
        let l = b.text(Z::Over, "Contact", contact, 56.0, 34.0, 968.0, 24.0, Style { align: 1, tracking: 40.0, opacity: 50.0, ..Style::mono() });
        b.control("contact", "Contact Line", l, "text");
    }
    let top = slot_y + 760.0 + 10.0;
    let badge = b.text(Z::Over, "Badge", &c.badge(), 66.0, top, 400.0, 30.0, Style { color: Some(c.accent), tracking: 100.0, ..Style::mono() });
    b.control("badge", "Badge", badge, "text");
    let size = fit_size(c.title(), 68.0);
    let t = b.text(Z::Over, "Title", c.title(), 66.0, top + 48.0, 948.0, size, Style { weight: "ExtraBold", leading: -4.0, ..Style::regular() });
    b.control("title", "Headline", t, "text");
    bullets(&mut b, 66.0, top + 48.0 + text_height(c.title(), size, 948.0, 1.08) + 24.0, 948.0, 31.0);
    b.logo = Some(LogoSpot { center: (380.0, 1846.0), size: 52.0, opacity: 100.0 });
    let n = b.text(Z::Over, "Channel", &c.channel(), 420.0, 1828.0, 600.0, 30.0, Style::bold());
    b.control("channel", "Channel Name", n, "text");
    b.finish()
}

fn logo_only(c: &Ctx<'_>) -> ReelPlan {
    let mut b = Builder::new(c, Rect::FULL);
    b.rect(Z::Over, "Logo plate", Rect::new(1080.0 - 54.0 - 162.0, 1920.0 - 86.0 - 162.0, 162.0, 162.0), with_alpha(BLACK, 0.4), 18.0);
    b.logo = Some(LogoSpot { center: (1080.0 - 54.0 - 81.0, 1920.0 - 86.0 - 81.0), size: 126.0, opacity: 100.0 });
    b.finish()
}

// ------------------------------------------------------------------------------------------------
// Design templates
// ------------------------------------------------------------------------------------------------

/// The family's fixed slot: styles never change the crop geometry.
fn design_slot(id: &str) -> Rect {
    match id {
        "jkr-cinematic-v2" | "jkr-editorial-v2" | "jkr-technical-v2" | "jkr-story-v2" => Rect::new(56.0, 290.0, 968.0, 1280.0),
        "jkr-esports-v2" | "jkr-hud-v2" | "jkr-retro-v2" | "jkr-cyber-v2" => Rect::new(40.0, 240.0, 1000.0, 1440.0),
        "jkr-neon-v2" => Rect::new(56.0, 240.0, 968.0, 1440.0),
        _ => Rect::FULL,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Clean,
    Editorial,
    Condensed,
    Keyword,
    Highlight,
    Outline,
    Glow,
    Stacked,
    Underline,
}

/// The parts of a design that differ between the fourteen; everything else is shared.
struct Recipe {
    look: Look,
    max_size: f64,
    top: bool,
    blur_background: bool,
    background: Option<(Rgba, Rgba)>,
    frame: &'static str,
    plate: &'static str,
    decorations: &'static str,
    progress: bool,
}

fn recipe(c: &Ctx<'_>) -> Recipe {
    let dark = [0.035, 0.043, 0.063, 1.0];
    let base = Recipe {
        look: Look::Keyword,
        max_size: 76.0,
        top: false,
        blur_background: false,
        background: Some((dark, dark)),
        frame: "none",
        plate: "none",
        decorations: "line",
        progress: false,
    };
    match c.id {
        "jkr-cinematic-v2" => Recipe { look: Look::Editorial, max_size: 86.0, blur_background: true, background: None, frame: "floating", ..base },
        "jkr-esports-v2" => Recipe {
            look: Look::Condensed,
            max_size: 88.0,
            top: true,
            background: Some((with_alpha(c.accent, 1.0), dark)),
            frame: "cutCorner",
            decorations: "technical",
            ..base
        },
        "jkr-hud-v2" => Recipe { look: Look::Clean, background: Some((c.band, dark)), frame: "hud", plate: "side", decorations: "hud", ..base },
        "jkr-editorial-v2" => {
            Recipe { look: Look::Editorial, max_size: 90.0, blur_background: true, background: None, frame: "thin", decorations: "editorial", ..base }
        }
        "jkr-minimal-v2" => Recipe { look: Look::Clean, max_size: 52.0, ..base },
        "jkr-neon-v2" => Recipe { look: Look::Glow, background: Some(([0.08, 0.06, 0.16, 1.0], dark)), frame: "glow", decorations: "glow", ..base },
        "jkr-retro-v2" => Recipe { look: Look::Stacked, max_size: 70.0, frame: "corners", decorations: "arcade", ..base },
        "jkr-dark-glass-v2" => Recipe { look: Look::Clean, max_size: 64.0, plate: "glass", decorations: "none", ..base },
        "jkr-cyber-v2" => Recipe { look: Look::Underline, background: Some(([0.0, 0.12, 0.1, 1.0], dark)), frame: "hud", decorations: "technical", ..base },
        "jkr-technical-v2" => Recipe {
            look: Look::Clean,
            max_size: 58.0,
            blur_background: true,
            background: None,
            frame: "offset",
            plate: "outline",
            decorations: "technical",
            ..base
        },
        "jkr-highlight-v2" => Recipe { look: Look::Highlight, max_size: 92.0, decorations: "none", progress: true, ..base },
        "jkr-story-v2" => Recipe {
            look: Look::Editorial,
            blur_background: true,
            background: None,
            frame: "rounded",
            plate: "side",
            decorations: "editorial",
            progress: true,
            ..base
        },
        "jkr-meme-v2" => Recipe { look: Look::Outline, max_size: 96.0, top: true, decorations: "none", ..base },
        _ => base,
    }
}

fn title_style(r: &Recipe, c: &Ctx<'_>) -> Style<'static> {
    let shadow = Some([0.0, 0.0, 0.0, 0.7]);
    match r.look {
        Look::Clean => Style { weight: "SemiBold", shadow, ..Style::regular() },
        Look::Editorial => Style { font: Some("Georgia"), weight: "Bold", leading: -4.0, shadow, ..Style::regular() },
        Look::Condensed => Style { weight: "Black", caps: true, tracking: -20.0, shadow, ..Style::regular() },
        Look::Keyword => Style { weight: "ExtraBold", tracking: -15.0, shadow, ..Style::regular() },
        Look::Highlight => Style { weight: "Black", plate: Some((with_alpha(c.accent, 0.92), 14.0, 8.0)), ..Style::regular() },
        Look::Outline => Style { weight: "Black", caps: true, align: 1, stroke: Some((BLACK, 10.0)), ..Style::regular() },
        Look::Glow => Style { weight: "Bold", shadow: Some(with_alpha(c.accent, 0.95)), shadow_distance: 0.0, shadow_blur: 36.0, ..Style::regular() },
        Look::Stacked => Style { font: Some(MONO), weight: "Bold", caps: true, tracking: 60.0, shadow, ..Style::regular() },
        Look::Underline => Style { weight: "Bold", underline: true, color: Some(WHITE), shadow, ..Style::regular() },
    }
}

fn design(c: &Ctx<'_>) -> ReelPlan {
    let r = recipe(c);
    let slot = design_slot(c.id);
    let mut b = Builder::new(c, slot);
    // Behind the slot: the footage itself, blurred, or a dark field.
    if r.blur_background {
        b.backdrop = Backdrop::BlurredClip { blur: 48.0, brightness: -55.0, saturation: if c.id == "jkr-editorial-v2" { 0.0 } else { 75.0 } };
    } else if let Some((a, z)) = r.background {
        b.gradient(Z::Under, "Background", Rect::FULL, a, z, 120.0);
    }
    // The frame around the slot.
    let accent = c.accent;
    match r.frame {
        "thin" => {
            b.outline(Z::Over, "Frame", slot, with_alpha(WHITE, 0.35), 2.0, 0.0);
        }
        "floating" | "rounded" => {
            b.outline(Z::Over, "Frame", slot, with_alpha(WHITE, 0.18), 2.0, 24.0);
        }
        "glow" => {
            let f = b.outline(Z::Over, "Glow frame", slot, accent, 3.0, 24.0);
            b.param(f, "shadow", ParamValue::Bool(true));
            b.param(f, "shadow_color", ParamValue::Color(accent));
            b.param(f, "shadow_distance", ParamValue::Float(0.0));
            b.param(f, "shadow_blur", ParamValue::Float(40.0));
        }
        "offset" => {
            b.outline(Z::Over, "Offset frame", Rect::new(slot.x + 16.0, slot.y + 16.0, slot.w, slot.h), with_alpha(accent, 0.8), 2.0, 0.0);
        }
        "hud" | "corners" | "cutCorner" => {
            let (l, w) = (64.0, if r.frame == "corners" { 8.0 } else { 4.0 });
            for (i, (x, y)) in
                [(slot.x, slot.y), (slot.x + slot.w - l, slot.y), (slot.x, slot.y + slot.h - w), (slot.x + slot.w - l, slot.y + slot.h - w)].iter().enumerate()
            {
                b.rect(Z::Over, &format!("Corner {}", i + 1), Rect::new(*x, *y, l, w), accent, 0.0);
            }
            for (i, (x, y)) in
                [(slot.x, slot.y), (slot.x + slot.w - w, slot.y), (slot.x, slot.y + slot.h - l), (slot.x + slot.w - w, slot.y + slot.h - l)].iter().enumerate()
            {
                b.rect(Z::Over, &format!("Corner {}b", i + 1), Rect::new(*x, *y, w, l), accent, 0.0);
            }
        }
        _ => {}
    }
    // A weighted edge where the words sit.
    if r.top {
        b.gradient(Z::Over, "Top shade", Rect::new(0.0, 0.0, 1080.0, 900.0), with_alpha(BLACK, 0.78), with_alpha(BLACK, 0.0), 90.0);
    } else {
        b.gradient(Z::Over, "Bottom shade", Rect::new(0.0, 960.0, 1080.0, 960.0), with_alpha(BLACK, 0.0), with_alpha(BLACK, 0.86), 90.0);
    }
    // Branding: the logo and the channel name inside the safe area.
    b.logo = Some(LogoSpot { center: (72.0 + 28.0, 96.0 + 28.0), size: 56.0, opacity: 100.0 });
    let name = b.text(
        Z::Over,
        "Channel",
        &c.channel(),
        72.0 + 70.0,
        96.0 + 10.0,
        700.0,
        30.0,
        Style { weight: "Bold", shadow: Some([0.0, 0.0, 0.0, 0.6]), ..Style::regular() },
    );
    b.control("channel", "Channel Name", name, "text");
    // The words. Safe area: 72 px left, 180 px right, 300 px above the bottom. Bottom-placed
    // words are pinned by their bottom edge, so a headline of any length grows upwards from the
    // caption and never runs into it; the plate, rule and bar follow the headline.
    let (left, width) = (72.0, 1080.0 - 72.0 - 180.0);
    let size = fit_size(c.title(), r.max_size).max(32.0);
    let style = title_style(&r, c);
    let em = if matches!(r.look, Look::Condensed | Look::Outline | Look::Stacked) { 0.7 } else { 0.56 };
    let title_h = text_height_w(c.title(), size, width, 1.12, em);
    let caption = c.card.caption.trim();
    let cap_text_h = if caption.is_empty() { 0.0 } else { text_height(caption, 32.0, width, 1.35) };
    let caption_h = if caption.is_empty() { 0.0 } else { 18.0 + cap_text_h };
    let base = 1920.0 - 300.0;
    let y = if r.top { 330.0 + 46.0 } else { base - caption_h - title_h };
    let mut words: Vec<u64> = Vec::new();
    if r.top {
        let label = b.text(
            Z::Over,
            "Label",
            &format!("THE PLAY / {}", c.badge()),
            left,
            330.0,
            width,
            22.0,
            Style { color: Some(accent), tracking: 230.0, ..Style::mono() },
        );
        b.control("badge", "Badge", label, "text");
    }
    // Drawn before the headline so it sits behind it; pinned to it once the headline exists.
    let plate = match r.plate {
        "side" => Some((b.rect(Z::Over, "Side bar", Rect::new(left - 28.0, y, 8.0, title_h + caption_h), accent, 4.0), [0.0, 0.0, 0.0, caption_h])),
        "glass" => Some((
            b.rect(Z::Over, "Glass", Rect::new(left - 32.0, y - 32.0, width + 64.0, title_h + caption_h + 64.0), [0.06, 0.07, 0.1, 0.62], 28.0),
            [0.0, -32.0, 0.0, caption_h + 32.0],
        )),
        "outline" => Some((
            b.outline(Z::Over, "Plate", Rect::new(left - 24.0, y - 24.0, width + 48.0, title_h + caption_h + 48.0), with_alpha(accent, 0.9), 2.0, 0.0),
            [0.0, -24.0, 0.0, caption_h + 24.0],
        )),
        _ => None,
    };
    let rule = match r.decorations {
        "line" | "editorial" => {
            let (w, h) = if r.decorations == "line" { (96.0, 6.0) } else { (width, 1.5) };
            Some((b.rect(Z::Over, "Rule", Rect::new(left, y - 34.0, w, h), accent, 3.0), -34.0))
        }
        "glow" => {
            let g = b.rect(Z::Over, "Light rule", Rect::new(left, y - 40.0, 220.0, 5.0), accent, 3.0);
            b.param(g, "shadow", ParamValue::Bool(true));
            b.param(g, "shadow_color", ParamValue::Color(accent));
            b.param(g, "shadow_distance", ParamValue::Float(0.0));
            b.param(g, "shadow_blur", ParamValue::Float(30.0));
            Some((g, -40.0))
        }
        "arcade" => {
            Some((b.text(Z::Over, "Score", "1UP  000000", left, y - 60.0, 400.0, 24.0, Style { color: Some(accent), tracking: 140.0, ..Style::mono() }), -60.0))
        }
        "technical" | "hud" => {
            b.text(
                Z::Over,
                "Telemetry",
                "REC ● 9:16 · 30P",
                1080.0 - 180.0 - 260.0,
                108.0,
                260.0,
                20.0,
                Style { color: Some(accent), align: 2, tracking: 120.0, ..Style::mono() },
            );
            None
        }
        _ => None,
    };
    let t = b.text(Z::Over, "Title", c.title(), left, y, width, size, style);
    b.control("title", "Headline", t, "text");
    words.push(t);
    let cap = (!caption.is_empty()).then(|| {
        let cap = b.text(
            Z::Over,
            "Caption",
            caption,
            left,
            base - cap_text_h,
            width,
            32.0,
            Style { opacity: 82.0, shadow: Some([0.0, 0.0, 0.0, 0.6]), ..Style::regular() },
        );
        b.control("caption", "Caption", cap, "text");
        b.bottom_at(cap, base);
        cap
    });
    if !r.top {
        match cap {
            Some(cap) => b.pin(t, cap, [false, false, false, true], [0.0, 0.0, 0.0, -(cap_text_h + 18.0)]),
            None => b.bottom_at(t, base),
        }
    }
    if let Some((p, offsets)) = plate {
        b.pin(p, t, [false, true, false, true], offsets);
        words.push(p);
    }
    if let Some((d, top)) = rule {
        b.pin(d, t, [false, true, false, false], [0.0, top, 0.0, 0.0]);
        words.push(d);
    }
    words.extend(cap);
    // The hook shows the words, the body leaves the footage clear, the close is the call to
    // action. Short Reels keep the words throughout.
    let end = c.seconds;
    if end >= 8.0 && c.id != "jkr-minimal-v2" {
        for w in &words {
            b.animate(
                *w,
                "opacity",
                &[
                    (0.0, ParamValue::Float(0.0)),
                    (0.35, ParamValue::Float(100.0)),
                    (2.6, ParamValue::Float(100.0)),
                    (3.0, ParamValue::Float(0.0)),
                    (end - 3.0, ParamValue::Float(0.0)),
                    (end - 2.6, ParamValue::Float(100.0)),
                ],
                Interpolation::Linear,
            );
        }
        let cta = c.cta();
        b.animate(t, "text", &[(0.0, ParamValue::Text(c.title().into())), (end - 3.0, ParamValue::Text(cta))], Interpolation::Hold);
    } else {
        b.reveal(t, 0.0, 0.5, 0.0);
    }
    if r.progress {
        b.progress(1920.0 - 7.0, 7.0, accent);
    }
    b.fit = Fit::Cover;
    b.finish()
}
