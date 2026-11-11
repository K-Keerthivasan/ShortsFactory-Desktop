use super::{Result, error};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
pub(crate) fn directory() -> Result<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .or_else(|| std::env::var_os("XDG_DATA_HOME"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
        .ok_or("Missing user data directory")?;
    let dir = base.join("K2 Shared Gallery");
    fs::create_dir_all(&dir).map_err(error)?;
    Ok(dir)
}
pub(crate) fn store(path: &str) -> Result<String> {
    if path.is_empty() {
        return Ok(String::new());
    }
    let bytes = fs::read(path).map_err(error)?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err("Gallery image exceeds 20 MiB".into());
    }
    let ext = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[255, 216, 255]) {
        "jpg"
    } else {
        return Err("Choose a PNG or JPEG image".into());
    };
    let folder = directory()?.join("assets");
    fs::create_dir_all(&folder).map_err(error)?;
    let dest = folder.join(format!("{:x}.{ext}", Sha256::digest(&bytes)));
    if !dest.is_file() {
        let mut temp = tempfile::NamedTempFile::new_in(&folder).map_err(error)?;
        temp.write_all(&bytes).map_err(error)?;
        if let Err(e) = temp.persist_noclobber(&dest)
            && e.error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(error(e));
        }
    }
    Ok(dest.to_string_lossy().into_owned())
}
pub(crate) fn managed(path: &str) -> Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    let p = fs::canonicalize(path).map_err(error)?;
    if !p.starts_with(fs::canonicalize(directory()?.join("assets")).map_err(error)?) {
        return Err("Select an image imported into the shared gallery".into());
    }
    Ok(())
}
pub(crate) fn output(name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || name.len() > 120
        || !name.ends_with(".png")
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b" -_.".contains(&b))
        || Path::new(name).components().count() != 1
    {
        return Err("Use a PNG filename in the gallery exports folder".into());
    }
    let dir = directory()?.join("exports");
    fs::create_dir_all(&dir).map_err(error)?;
    Ok(dir.join(name))
}
