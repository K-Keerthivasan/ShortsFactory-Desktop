use super::{Result, error};
use egui::{ColorImage, TextureHandle};
fn load(ctx: &egui::Context, path: &str) -> Result<TextureHandle> {
    let mut reader = image::ImageReader::open(path).map_err(error)?.with_guessed_format().map_err(error)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().map_err(error)?.thumbnail(120, 180).to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(ctx.load_texture(path, ColorImage::from_rgba_unmultiplied(size, image.as_raw()), Default::default()))
}
pub(crate) fn show(ui: &mut egui::Ui, path: &str) {
    if path.is_empty() {
        return;
    }
    let key = egui::Id::new(("format-preview", path));
    let texture = ui.data_mut(|d| d.get_temp::<Result<TextureHandle>>(key)).unwrap_or_else(|| {
        let result = load(ui.ctx(), path);
        ui.data_mut(|d| d.insert_temp(key, result.clone()));
        result
    });
    match texture {
        Ok(t) => {
            ui.image((t.id(), t.size_vec2()));
        }
        Err(s) => {
            ui.label(s);
        }
    }
}
