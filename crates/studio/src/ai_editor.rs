//! AI editors: Claude and Codex (ChatGPT's coding agent) driving the running app.
//!
//! One MCP server serves both: `filmcraft-cli mcp --bridge 127.0.0.1:<port>` exposes every
//! editor command (`command_run`, `command_batch`, `sequence_inspect`, `render_frame`,
//! `ui_screenshot` …) of the app that is open, so the agent edits on the real timeline. [`setup`]
//! turns on "Let Claude edit" (the app's localhost control server, next start), registers the
//! server as `shorts-factory` with Claude Code, the Claude desktop app (when installed) and Codex,
//! and installs the `shorts-factory-edit` skill (the quality-edit playbook) for Claude and Codex.
//! [`launch`] opens an agent session on a project with a brief.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, Studio, StudioError, err};

/// The server's name in every client.
pub const SERVER: &str = "shorts-factory";
/// The quality-edit playbook.
pub const SKILL: &str = include_str!("../assets/shorts-factory-edit/SKILL.md");

/// Where things stand.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Status {
    /// The app starts its control server.
    pub enabled: bool,
    pub cli: Option<PathBuf>,
    pub claude_code: bool,
    pub claude_desktop: bool,
    pub codex: bool,
    pub skill: bool,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)
}

/// The command-line tool beside the app (the MCP server).
pub fn cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join(if cfg!(windows) { "filmcraft-cli.exe" } else { "filmcraft-cli" });
    p.is_file().then_some(p)
}

fn claude_desktop_config() -> Option<PathBuf> {
    let dir = if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Claude"))
    } else {
        home().map(|h| h.join("Library").join("Application Support").join("Claude"))
    }?;
    dir.is_dir().then(|| dir.join("claude_desktop_config.json"))
}

fn codex_config() -> Option<PathBuf> {
    let dir = home()?.join(".codex");
    dir.is_dir().then(|| dir.join("config.toml"))
}

fn has_server(json_file: &Path) -> bool {
    std::fs::read_to_string(json_file).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).is_some_and(|v| v.pointer(&format!("/mcpServers/{SERVER}")).is_some())
}

pub fn status(studio: &Studio) -> Status {
    let h = home();
    Status {
        enabled: studio.app_settings().claude_editor,
        cli: cli(),
        claude_code: h.as_ref().is_some_and(|h| has_server(&h.join(".claude.json"))),
        claude_desktop: claude_desktop_config().is_some_and(|f| has_server(&f)),
        codex: codex_config().and_then(|f| std::fs::read_to_string(f).ok()).is_some_and(|t| t.contains(&format!("[mcp_servers.{SERVER}]"))),
        skill: h.is_some_and(|h| h.join(".claude").join("skills").join("shorts-factory-edit").join("SKILL.md").is_file()),
    }
}

fn quiet(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

/// A TOML basic string.
fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Codex's config with the server's block replaced (or added); everything else is kept.
pub fn codex_with_server(config: &str, cli: &str, port: u16) -> String {
    let header = format!("[mcp_servers.{SERVER}]");
    let mut out = String::new();
    let mut skipping = false;
    for line in config.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            // Our block and its sub-tables ([mcp_servers.shorts-factory.env]) are replaced.
            skipping = t == header || t.starts_with(&format!("[mcp_servers.{SERVER}."));
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "\n# Shorts Factory Desktop: the running app's editor (Plugin Settings > AI > AI editors)\n{header}\ncommand = {}\nargs = [\"mcp\", \"--bridge\", \"127.0.0.1:{port}\"]\nstartup_timeout_sec = 30\ntool_timeout_sec = 600\n",
        toml_str(cli)
    ));
    out
}

/// A JSON config (Claude desktop) with the server added under `mcpServers`.
pub fn json_with_server(config: &str, cli: &str, port: u16) -> Result<String> {
    let mut v: Value = if config.trim().is_empty() { json!({}) } else { serde_json::from_str(config).map_err(|e| StudioError::Other(format!("the config is not valid JSON: {e}")))? };
    let Some(obj) = v.as_object_mut() else { return err("the config is not a JSON object") };
    let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else { return err("mcpServers is not an object") };
    servers.insert(SERVER.into(), json!({"command": cli, "args": ["mcp", "--bridge", format!("127.0.0.1:{port}")]}));
    serde_json::to_string_pretty(&v).map_err(|e| StudioError::Other(e.to_string()))
}

/// Writes `text` to `file`, keeping the previous one as `<file>.bak`.
fn write_with_backup(file: &Path, text: &str) -> Result<()> {
    if file.is_file() {
        std::fs::copy(file, file.with_extension(format!("{}.bak", file.extension().and_then(|e| e.to_str()).unwrap_or("old"))))?;
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, text)?;
    Ok(())
}

/// Turns on the AI editors: the control server (next start), the `shorts-factory` MCP server
/// in Claude Code, the Claude desktop app and Codex, and the editing skill. Returns a line per
/// step. Blocking (a few seconds).
pub fn setup(studio: &Studio) -> Result<Vec<String>> {
    let cli = cli().ok_or_else(|| StudioError::Other("filmcraft-cli was not found beside the app".into()))?;
    let cli_s = cli.to_string_lossy().into_owned();
    let port = crate::settings::CLAUDE_PORT;
    let mut log = Vec::new();
    let mut a = studio.app_settings();
    a.claude_editor = true;
    studio.save_app_settings(&a)?;
    log.push(format!("The app's control server is on (port {port}) from the next start"));
    // Claude Code (user scope): replace any earlier registration.
    let claude = studio.var("CLAUDE_BIN").unwrap_or("claude").to_string();
    let _ = quiet(Command::new(&claude).args(["mcp", "remove", "-s", "user", SERVER])).output();
    match quiet(Command::new(&claude).args(["mcp", "add", "-s", "user", SERVER, "--"]).arg(&cli).args(["mcp", "--bridge", &format!("127.0.0.1:{port}")])).output() {
        Ok(o) if o.status.success() => log.push("Claude Code: the shorts-factory server is added".into()),
        Ok(o) => log.push(format!("Claude Code: not added ({})", String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("failed"))),
        Err(_) => log.push("Claude Code: not installed (skipped)".into()),
    }
    match claude_desktop_config() {
        Some(f) => {
            let text = json_with_server(&std::fs::read_to_string(&f).unwrap_or_default(), &cli_s, port)?;
            write_with_backup(&f, &text)?;
            log.push("Claude desktop app: added (restart it)".into());
        }
        None => log.push("Claude desktop app: not installed (skipped)".into()),
    }
    match codex_config() {
        Some(f) => {
            let text = codex_with_server(&std::fs::read_to_string(&f).unwrap_or_default(), &cli_s, port);
            write_with_backup(&f, &text)?;
            log.push("Codex: added".into());
        }
        None => log.push("Codex: not installed (skipped)".into()),
    }
    let h = home().ok_or_else(|| StudioError::Other("no home folder".into()))?;
    for dir in [h.join(".claude").join("skills"), h.join(".codex").join("skills")] {
        if dir.parent().is_some_and(Path::is_dir) {
            let d = dir.join("shorts-factory-edit");
            std::fs::create_dir_all(&d)?;
            std::fs::write(d.join("SKILL.md"), SKILL)?;
        }
    }
    log.push("The shorts-factory-edit skill (the editing playbook) is installed".into());
    Ok(log)
}

/// Opens a Claude Code or Codex session in a terminal on `dir` (the project's folder), with
/// `brief` written there and handed to the agent.
pub fn launch(studio: &Studio, agent: &str, dir: &Path, brief: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let file = dir.join("ai-edit-brief.md");
    std::fs::write(&file, brief)?;
    let prompt = "Use the shorts-factory-edit skill and the shorts-factory MCP server to edit in the running Shorts Factory Desktop app. Read ai-edit-brief.md in this folder first and follow it.";
    let bin = match agent {
        "codex" => studio.var("CODEX_BIN").unwrap_or("codex").to_string(),
        _ => studio.var("CLAUDE_BIN").unwrap_or("claude").to_string(),
    };
    if cfg!(windows) {
        // A new console window the user can talk to the agent in.
        Command::new("cmd").args(["/C", "start", "Shorts Factory editor", "/D"]).arg(dir).arg(&bin).arg(prompt).spawn()?;
    } else {
        Command::new(&bin).arg(prompt).current_dir(dir).spawn()?;
    }
    Ok(())
}
