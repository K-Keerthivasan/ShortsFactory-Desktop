//! Checks every workspace's publishing accounts without changing anything:
//! `cargo run -p filmcraft-studio --example check_accounts`. Prints only results, never keys.
#![allow(clippy::print_stdout)]

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let studio = filmcraft_studio::Studio::open_default()?;
    for (line, ok) in filmcraft_studio::settings::health_check(&studio).lines {
        println!("{} {line}", if ok { "ok " } else { "ERR" });
    }
    Ok(())
}
