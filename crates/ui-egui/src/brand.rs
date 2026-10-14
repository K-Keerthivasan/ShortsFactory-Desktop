//! The Shorts Factory Desktop wordmark: the app's 9:16 play mark and its name, drawn in code
//! (no image asset). The app is made with FilmCraft; the ArtCraft marks FilmCraft ships are not
//! used in this fork, as their terms require.

use egui::{Color32, Pos2, Rect, Stroke, StrokeKind, pos2, vec2};

/// The accent the mark is drawn in.
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x2e, 0x3f);

/// Draws the mark (a 9:16 frame with a play head) and "SHORTS FACTORY" `height` points tall with
/// its left edge at `left_center`. Returns the drawn rect.
pub fn paint_wordmark(ui: &egui::Ui, left_center: Pos2, height: f32, dark: bool) -> Option<Rect> {
    let p = ui.painter();
    let ink = if dark { Color32::WHITE } else { Color32::from_rgb(0x15, 0x15, 0x14) };
    let h = height.max(4.0);
    let frame = Rect::from_center_size(pos2(left_center.x + h * 0.3, left_center.y), vec2(h * 0.56, h));
    p.rect_filled(frame, h * 0.14, ACCENT);
    let c = frame.center();
    let s = h * 0.2;
    p.add(egui::Shape::convex_polygon(vec![pos2(c.x - s * 0.7, c.y - s), pos2(c.x - s * 0.7, c.y + s), pos2(c.x + s, c.y)], Color32::WHITE, Stroke::NONE));
    let galley = p.layout_no_wrap("SHORTS FACTORY".into(), egui::FontId::proportional(h * 0.62), ink);
    let text_pos = pos2(frame.max.x + h * 0.35, left_center.y - galley.size().y / 2.0);
    let width = galley.size().x;
    p.galley(text_pos, galley, ink);
    let r = Rect::from_min_max(pos2(left_center.x, left_center.y - h / 2.0), pos2(text_pos.x + width, left_center.y + h / 2.0));
    let _ = StrokeKind::Inside;
    Some(r)
}
