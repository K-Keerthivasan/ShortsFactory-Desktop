//! Sets a workspace's logo in the Shorts Factory library:
//! `cargo run -p filmcraft-studio --example set_logo -- <workspace slug> <image file>`.
#![allow(clippy::print_stdout)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(slug), Some(file)) = (args.next(), args.next()) else {
        return Err("usage: set_logo <workspace slug> <image file>".into());
    };
    let studio = filmcraft_studio::Studio::open_default()?;
    let channel = studio.channels()?.into_iter().find(|c| c.slug == slug).ok_or_else(|| format!("no workspace `{slug}`"))?;
    let id = studio.set_logo(channel.id, std::path::Path::new(&file))?;
    let brand = studio.brand(channel.id)?;
    println!("{}: logo asset {id} -> {}", channel.name, brand.logo_path.unwrap_or_default());
    Ok(())
}
