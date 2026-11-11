use super::{Format, Result, error};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{fs, io::Write, path::Path};
fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
fn image(path: &str, x: u32, y: u32, w: u32, h: u32, fit: &str) -> Result<String> {
    let meta = fs::metadata(path).map_err(error)?;
    if meta.len() > 20 * 1024 * 1024 {
        return Err("Choose an image smaller than 20 MiB".into());
    }
    let bytes = fs::read(path).map_err(error)?;
    let kind = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[255, 216, 255]) {
        "jpeg"
    } else {
        return Err("Choose a PNG or JPEG".into());
    };
    Ok(format!(
        r#"<image x="{x}" y="{y}" width="{w}" height="{h}" preserveAspectRatio="xMidYMid {fit}" xlink:href="data:image/{kind};base64,{}"/>"#,
        STANDARD.encode(bytes)
    ))
}
pub(super) fn cover(f: &Format, frame: &str, title: &str, output: &str) -> Result<()> {
    let path = Path::new(output);
    if !path.extension().is_some_and(|s| s.eq_ignore_ascii_case("png")) {
        return Err("Cover output must end in .png".into());
    }
    let mut svg = String::from(r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="1080" height="1920">"#);
    svg.push_str(&image(frame, 0, 0, 1080, 1920, "slice")?);
    if !f.graphic.is_empty() {
        svg.push_str(&image(&f.graphic, 0, 0, 1080, 1920, "meet")?)
    }
    if f.layout != "image" {
        let lines: Vec<_> = title.lines().collect();
        if lines.is_empty() || lines.len() > 3 {
            return Err("Use one to three headline lines".into());
        }
        let height = f.headline_size * (lines.len() as u32 + 1);
        let top = if f.layout == "top" { 0 } else { 1920 - height };
        svg.push_str(&format!(r#"<rect y="{top}" width="1080" height="{height}" fill="black" opacity=".75"/>"#));
        for (i, line) in lines.iter().enumerate() {
            svg.push_str(&format!(
                r#"<text x="54" y="{}" fill="{}" font-family="{}" font-size="{}" font-weight="bold">{}</text>"#,
                top + f.headline_size * (i as u32 + 1),
                f.color,
                escape(&f.font),
                f.headline_size,
                escape(line)
            ));
        }
    }
    if !f.logo.is_empty() {
        svg.push_str(&image(&f.logo, 860, 80, 160, 160, "meet")?)
    }
    svg.push_str("</svg>");
    let mut opt = resvg::usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_str(&svg, &opt).map_err(error)?;
    let mut pixels = resvg::tiny_skia::Pixmap::new(1080, 1920).ok_or("Cannot allocate thumbnail")?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixels.as_mut());
    let bytes = pixels.encode_png().map_err(error)?;
    let mut out = fs::OpenOptions::new().write(true).create_new(true).open(path).map_err(error)?;
    if let Err(e) = out.write_all(&bytes) {
        drop(out);
        let _ = fs::remove_file(path);
        return Err(error(e));
    }
    Ok(())
}
