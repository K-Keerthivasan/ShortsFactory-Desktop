use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod assets;
#[cfg(feature = "ui")]
mod panel;
#[cfg(feature = "ui")]
mod previews;
mod raster;
#[cfg(feature = "ui")]
pub use panel::gallery_button;

type Result<T> = std::result::Result<T, String>;
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Format {
    pub account: String,
    pub series: String,
    pub name: String,
    pub revision: u64,
    pub font: String,
    pub logo: String,
    pub graphic: String,
    pub reference: String,
    pub layout: String,
    pub reel_layout: String,
    pub description: String,
    pub skills: String,
    pub examples: Vec<String>,
    pub color: String,
    pub headline_size: u32,
}
impl Default for Format {
    fn default() -> Self {
        Self {
            account: "JKR".into(),
            series: "New games".into(),
            name: "Gameplay cover".into(),
            revision: 0,
            font: "Arial".into(),
            logo: String::new(),
            graphic: String::new(),
            reference: String::new(),
            layout: "image".into(),
            reel_layout: "quoteCard".into(),
            description: "{title}\n\n{summary}\n\nCreated by {creator}.\nFootage: {source}".into(),
            skills: "Choose a distinctive frame from the source. Credit the original creator and footage source.".into(),
            examples: Vec::new(),
            color: "#ffffff".into(),
            headline_size: 88,
        }
    }
}
fn validate(f: &Format) -> Result<()> {
    if [&f.account, &f.series, &f.name, &f.font].iter().any(|s| s.trim().is_empty() || s.len() > 160) {
        return Err("Enter an account, series, format name and font (up to 160 bytes each).".into());
    }
    if !["image", "top", "bottom"].contains(&f.layout.as_str()) || !(24..=180).contains(&f.headline_size) {
        return Err("Choose a valid headline layout and size (24–180).".into());
    }
    if f.color.len() != 7 || !f.color.starts_with('#') || !f.color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit) {
        return Err("Choose a #RRGGBB text color.".into());
    }
    if serde_json::to_vec(f).map_err(error)?.len() > 65536 {
        return Err("Format exceeds 64 KiB.".into());
    }
    Ok(())
}
fn open() -> Result<Connection> {
    let dir = assets::directory()?;
    let db = Connection::open(dir.join("formats.sqlite3")).map_err(error)?;
    initialize(&db)?;
    Ok(db)
}
fn initialize(db: &Connection) -> Result<()> {
    db.busy_timeout(std::time::Duration::from_secs(4)).map_err(error)?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS formats(account TEXT,series TEXT,name TEXT,rev INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(account,series,name));",
    )
    .map_err(error)?;
    for (series, name) in [("JP series", "Original series"), ("GTA VI", "Gameplay cover"), ("New games", "Gameplay cover"), ("New movies", "Trailer cover")] {
        let f = Format { series: series.into(), name: name.into(), revision: 1, ..Default::default() };
        db.execute("INSERT OR IGNORE INTO formats VALUES(?1,?2,?3,1,?4)", params![f.account, f.series, f.name, serde_json::to_string(&f).map_err(error)?])
            .map_err(error)?;
    }
    Ok(())
}
fn save(db: &Connection, mut f: Format) -> Result<Format> {
    validate(&f)?;
    if f.examples.len() > 64 {
        return Err("A format supports up to 64 examples".into());
    }
    let old = f.revision;
    f.revision = old.checked_add(1).ok_or("Invalid revision")?;
    let body = serde_json::to_string(&f).map_err(error)?;
    let changed = if old == 0 {
        db.execute("INSERT OR IGNORE INTO formats VALUES(?1,?2,?3,1,?4)", params![f.account, f.series, f.name, body])
    } else {
        db.execute(
            "UPDATE formats SET rev=?4,body=?5 WHERE account=?1 AND series=?2 AND name=?3 AND rev=?6",
            params![f.account, f.series, f.name, f.revision, body, old],
        )
    }
    .map_err(error)?;
    if changed != 1 {
        return Err("Format already exists or changed in the other app. Reload before saving.".into());
    }
    Ok(f)
}
fn list(db: &Connection) -> Result<Vec<Format>> {
    let mut q = db.prepare("SELECT body FROM formats ORDER BY account,series,name").map_err(error)?;
    q.query_map([], |r| r.get::<_, String>(0)).map_err(error)?.map(|s| serde_json::from_str(&s.map_err(error)?).map_err(error)).collect()
}
pub fn command(p: &Value) -> Result<Value> {
    let db = open()?;
    dispatch(&db, p, false)
}
fn dispatch(db: &Connection, p: &Value, desktop: bool) -> Result<Value> {
    match p["action"].as_str() {
        Some("list") => serde_json::to_value(list(&db)?).map_err(error),
        Some("save") => {
            let f: Format = serde_json::from_value(p["format"].clone()).map_err(error)?;
            for a in [&f.logo, &f.graphic, &f.reference].into_iter().chain(f.examples.iter()) {
                assets::managed(a)?
            }
            serde_json::to_value(save(db, f)?).map_err(error)
        }
        Some("apply") => {
            let field = |k: &str| p[k].as_str().ok_or_else(|| format!("Missing {k}"));
            let body: Option<String> = db
                .query_row(
                    "SELECT body FROM formats WHERE account=?1 AND series=?2 AND name=?3",
                    params![field("account")?, field("series")?, field("name")?],
                    |r| r.get(0),
                )
                .optional()
                .map_err(error)?;
            let f: Format = serde_json::from_str(&body.ok_or("Unknown format")?).map_err(error)?;
            let mut description = String::new();
            let mut rest = f.description.as_str();
            while let Some((prefix, after)) = rest.split_once('{') {
                description.push_str(prefix);
                let (key, tail) = after.split_once('}').ok_or("Unclosed description placeholder")?;
                description.push_str(field(key)?);
                rest = tail;
            }
            description.push_str(rest);
            let cover = if let Some(output) = p["output"].as_str() {
                let frame = field("frame")?;
                let dest = if desktop {
                    std::path::PathBuf::from(output)
                } else {
                    assets::managed(frame)?;
                    assets::output(output)?
                };
                let dest = dest.to_string_lossy();
                raster::cover(&f, frame, field("title")?, &dest)?;
                Some(dest.into_owned())
            } else {
                None
            };
            Ok(json!({"description":description,"reelLayout":f.reel_layout,"skills":f.skills,"cover":cover,"revision":f.revision}))
        }
        _ => Err("Expected action list, save or apply".into()),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revisions_and_duplicate_names() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        let f = list(&db).unwrap().remove(0);
        assert!(save(&db, f.clone()).is_ok());
        assert!(save(&db, f).is_err());
        let f = Format { account: "Other".into(), ..Default::default() };
        assert!(save(&db, f.clone()).is_ok());
        assert!(save(&db, f).is_err());
    }
}
