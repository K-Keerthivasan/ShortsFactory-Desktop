use super::*;
#[derive(Clone, Default)]
struct Editor {
    items: Vec<Format>,
    draft: Format,
    loaded: bool,
    message: String,
    frame: String,
    title: String,
    summary: String,
    creator: String,
    source: String,
    description: String,
}
fn pick(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.label(label);
    ui.horizontal(|ui| {
        ui.text_edit_singleline(value);
        if ui.button("Browse").clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("Image", &["png", "jpg", "jpeg"]).pick_file()
        {
            *value = p.to_string_lossy().into_owned();
        }
    });
}
fn refresh(e: &mut Editor) {
    match open().and_then(|db| list(&db)) {
        Ok(rows) => {
            e.items = rows;
            e.loaded = true
        }
        Err(s) => e.message = s,
    }
}
pub fn gallery_button(ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("Shared format gallery").show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("gallery-scroll").max_height(ui.available_height().clamp(120.0, 600.0)).show(ui, draw_editor);
    });
}
fn draw_editor(ui: &mut egui::Ui) {
    let key = ui.id().with("gallery-editor");
    let mut e = ui.data_mut(|d| d.get_temp::<Editor>(key)).unwrap_or_default();
    if !e.loaded {
        refresh(&mut e);
        if let Some(f) = e.items.first() {
            e.draft = f.clone()
        }
    }
    if ui.button("Refresh gallery").clicked() {
        refresh(&mut e)
    }
    egui::ComboBox::from_id_salt("formats").selected_text(format!("{} / {} / {}", e.draft.account, e.draft.series, e.draft.name)).show_ui(ui, |ui| {
        for f in &e.items {
            if ui.selectable_label(false, format!("{} / {} / {}", f.account, f.series, f.name)).clicked() {
                e.draft = f.clone()
            }
        }
    });
    if ui.button("New business, series or variation").clicked() {
        e.draft.revision = 0;
        e.draft.name.clear()
    }
    ui.horizontal_wrapped(|ui| {
        for image in std::iter::once(&e.draft.reference).chain(e.draft.examples.iter()) {
            previews::show(ui, image);
        }
    });
    if ui.button("Add example thumbnails").clicked()
        && let Some(files) = rfd::FileDialog::new().add_filter("Thumbnail", &["png", "jpeg", "jpg"]).pick_files()
    {
        for path in files.into_iter().take(64usize.saturating_sub(e.draft.examples.len())) {
            match assets::store(&path.to_string_lossy()) {
                Ok(path) => {
                    if !e.draft.examples.contains(&path) {
                        e.draft.examples.push(path)
                    }
                }
                Err(msg) => e.message = msg,
            }
        }
    }
    let fresh = e.draft.revision == 0;
    ui.add_enabled_ui(fresh, |ui| {
        for (name, val) in [("Business or account", &mut e.draft.account), ("Series or work", &mut e.draft.series), ("Format name", &mut e.draft.name)] {
            ui.label(name);
            ui.text_edit_singleline(val);
        }
    });
    ui.label("Headline placement");
    ui.horizontal(|ui| {
        for (v, l) in [("image", "Image only"), ("top", "Top"), ("bottom", "Bottom")] {
            ui.selectable_value(&mut e.draft.layout, v.into(), l);
        }
    });
    for (name, val) in [("Font family", &mut e.draft.font), ("Text color", &mut e.draft.color), ("Reel layout", &mut e.draft.reel_layout)] {
        ui.label(name);
        ui.text_edit_singleline(val);
    }
    ui.add(egui::Slider::new(&mut e.draft.headline_size, 24..=180).text("Headline size"));
    pick(ui, "Company logo", &mut e.draft.logo);
    pick(ui, "Graphic overlay", &mut e.draft.graphic);
    pick(ui, "Reference thumbnail", &mut e.draft.reference);
    ui.label("Description format · {title} {summary} {creator} {source}");
    ui.text_edit_multiline(&mut e.draft.description);
    ui.label("Editorial skills and instructions");
    ui.text_edit_multiline(&mut e.draft.skills);
    if ui.button("Save format").clicked() {
        match open().and_then(|db| {
            let mut f = e.draft.clone();
            f.logo = assets::store(&f.logo)?;
            f.graphic = assets::store(&f.graphic)?;
            f.reference = assets::store(&f.reference)?;
            save(&db, f)
        }) {
            Ok(f) => {
                e.draft = f;
                refresh(&mut e);
                e.message = "Format saved".into()
            }
            Err(s) => e.message = s,
        }
    }
    ui.collapsing("Create thumbnail and description",|ui|{
            pick(ui,"Source frame",&mut e.frame);
            for (label,v) in [("Headline",&mut e.title),("Summary",&mut e.summary),("Creator",&mut e.creator),("Footage source",&mut e.source)]{ui.label(label);ui.text_edit_multiline(v);}
            if ui.button("Generate description").clicked() {
                match command(&json!({"action":"apply","account":e.draft.account,"series":e.draft.series,"name":e.draft.name,"title":e.title,"summary":e.summary,"creator":e.creator,"source":e.source})) {
                    Ok(result)=>e.description=result["description"].as_str().unwrap_or_default().to_owned(),
                    Err(message)=>e.message=message
                }
            }
            if ui.button("Export PNG using saved format").clicked()
            && let Some(p)=rfd::FileDialog::new().add_filter("PNG",&["png"]).save_file(){
                let result=open().and_then(|db|dispatch(&db,&json!({"action":"apply","account":e.draft.account,"series":e.draft.series,"name":e.draft.name,"title":e.title,"summary":e.summary,"creator":e.creator,"source":e.source,"frame":e.frame,"output":p}),true));
                match result{Ok(v)=>{e.description=v["description"].as_str().unwrap_or_default().into();e.message="Thumbnail exported".into()},Err(s)=>e.message=s}
            }
            if !e.description.is_empty(){ui.label("Formatted description");ui.text_edit_multiline(&mut e.description);}
        });
    ui.label(&e.message);
    ui.data_mut(|d| d.insert_temp(key, e));
}
