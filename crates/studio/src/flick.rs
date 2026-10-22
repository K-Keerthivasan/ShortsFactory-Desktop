//! Flick (Creatorberry, MIT): animated Remotion scenes for a Reel, run by the app.
//!
//! Flick is an agent skill that turns a transcript into one original motion animation per
//! transcript beat. The app drives its four steps itself, with Flick's own rules, starter
//! Remotion project, sound effects and saved animations (fetched once at a pinned version):
//!
//! 1. **Transcript** – the Reel's words (local Whisper on its footage, or pasted text).
//! 2. **Plan** – the workspace's AI writes Flick's scene plan (step 2's rules); you approve it
//!    in the app, editing any scene first.
//! 3. **Build** – a coding agent (`FLICK_AGENT`: Claude Code, Codex or Hermes) writes one
//!    Remotion component per approved scene in the run's workspace from the brief and scene
//!    spec the app writes; the app then renders every scene (and its poster) itself.
//! 4. **Deliver** – the scenes go onto the Reel at their times (an overlay track), Remotion
//!    Studio can preview them, one scene can be revised from feedback, and a scene can be saved
//!    to Flick's reusable library.
//!
//! Remotion needs Node.js 20+; setup installs it (and its headless browser) once in the app's
//! tools folder, and each run links to that install.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Result, Studio, StudioError, err};

/// The Flick version the app uses (Creatorberry/flick, main on 2026-08-26).
pub const COMMIT: &str = "4442fe355644317aafbe15c55ea3fbcd0217cd8d";

fn quiet(c: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c
}

/// npm and npx are scripts on Windows.
fn node_tool(name: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", name]);
        c
    } else {
        Command::new(name)
    }
}

/// Where Flick lives: `tools/flick/skill` (its files) and `tools/flick/remotion` (the starter
/// project with Remotion installed).
pub fn home(studio: &Studio) -> PathBuf {
    crate::ytdlp::tools_dir(studio).join("flick")
}
fn skill(studio: &Studio) -> PathBuf {
    home(studio).join("skill")
}
fn template(studio: &Studio) -> PathBuf {
    home(studio).join("remotion")
}

/// What is ready.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Status {
    pub node: Option<String>,
    pub skill: bool,
    pub remotion: bool,
    pub agent: String,
}

pub fn agent(studio: &Studio) -> String {
    studio.var("FLICK_AGENT").map(str::to_ascii_lowercase).filter(|a| ["claude", "codex", "hermes"].contains(&a.as_str())).unwrap_or_else(|| "claude".into())
}

pub fn status(studio: &Studio) -> Status {
    let node = quiet(&mut Command::new("node")).arg("--version").output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    Status {
        node,
        skill: skill(studio).join("SKILL.md").is_file(),
        remotion: template(studio).join("node_modules").join("remotion").is_dir(),
        agent: agent(studio),
    }
}

/// Fetches Flick's files at [`COMMIT`], installs Remotion in the starter project and its
/// headless browser. Blocking (minutes; downloads a few hundred MB once).
pub fn setup(studio: &Studio, say: &dyn Fn(&str)) -> Result<()> {
    let st = status(studio);
    let major: u32 = st.node.as_deref().and_then(|v| v.trim_start_matches('v').split('.').next()?.parse().ok()).unwrap_or(0);
    if major < 20 {
        return err("Flick needs Node.js 20 or newer (winget install OpenJS.NodeJS.LTS)");
    }
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(120)).user_agent("shorts-factory-desktop").build();
    if !st.skill {
        say("Fetching Flick's files");
        let tree: Value = agent
            .get(&format!("https://api.github.com/repos/Creatorberry/flick/git/trees/{COMMIT}?recursive=1"))
            .call()
            .map_err(|e| StudioError::Other(format!("could not list Flick's files: {e}")))?
            .into_json()
            .map_err(|e| StudioError::Other(e.to_string()))?;
        let files: Vec<String> = tree
            .get("tree")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|t| t.get("type").and_then(Value::as_str) == Some("blob"))
            .filter_map(|t| t.get("path").and_then(Value::as_str))
            .filter_map(|p| p.strip_prefix("skills/flick/").map(str::to_string))
            .filter(|p| !p.contains("..") && !p.starts_with('/'))
            .collect();
        if files.is_empty() {
            return err("Flick's file list is empty");
        }
        let dest = skill(studio);
        for f in &files {
            let target = dest.join(f);
            std::fs::create_dir_all(target.parent().unwrap_or(&dest))?;
            let r = agent
                .get(&format!("https://raw.githubusercontent.com/Creatorberry/flick/{COMMIT}/skills/flick/{f}"))
                .call()
                .map_err(|e| StudioError::Other(format!("could not fetch {f}: {e}")))?;
            let mut out = std::fs::File::create(&target)?;
            std::io::copy(&mut r.into_reader(), &mut out)?;
        }
        // Flick's licence travels with its files.
        if let Ok(r) = agent.get(&format!("https://raw.githubusercontent.com/Creatorberry/flick/{COMMIT}/LICENSE")).call() {
            let mut out = std::fs::File::create(dest.join("LICENSE"))?;
            std::io::copy(&mut r.into_reader(), &mut out)?;
        }
    }
    let t = template(studio);
    if !t.join("package.json").is_file() {
        copy_dir(&skill(studio).join("assets").join("starter-remotion"), &t)?;
    }
    if !status(studio).remotion {
        say("Installing Remotion (once)");
        run(node_tool("npm").args(["install", "--no-audit", "--no-fund"]).current_dir(&t), Duration::from_secs(1800), None)?;
    }
    say("Preparing Remotion's browser");
    run(node_tool("npx").args(["remotion", "browser", "ensure"]).current_dir(&t), Duration::from_secs(1800), None)?;
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        if p.file_name().is_some_and(|n| n == "node_modules") {
            continue;
        }
        if e.file_type()?.is_dir() {
            copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// Runs a command with a time limit; its output goes to `log` (appended) when given.
fn run(c: &mut Command, limit: Duration, log: Option<&Path>) -> Result<String> {
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = quiet(c).spawn().map_err(|e| StudioError::Other(format!("could not start {:?}: {e}", c.get_program())))?;
    wait(child, limit, log)
}

fn wait(mut child: std::process::Child, limit: Duration, log: Option<&Path>) -> Result<String> {
    let take = |p: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = p {
                let _ = p.read_to_string(&mut s);
            }
            s
        })
    };
    let out = take(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let errs = take(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return err(format!("it took longer than {} minutes and was stopped", limit.as_secs() / 60));
        }
        std::thread::sleep(Duration::from_millis(300));
    };
    let (o, e) = (out.join().unwrap_or_default(), errs.join().unwrap_or_default());
    if let Some(log) = log {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
            let _ = writeln!(f, "{o}\n{e}");
        }
    }
    if !status.success() {
        let tail: String = format!("{o}\n{e}").lines().rev().filter(|l| !l.trim().is_empty()).take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
        return err(format!("failed ({status}): {}", tail.chars().take(500).collect::<String>()));
    }
    Ok(o)
}

/// One scene of the plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Scene {
    /// lowercase-kebab composition id and output name.
    pub id: String,
    pub name: String,
    /// Seconds into the Reel.
    pub start: f64,
    pub end: f64,
    pub transcript: String,
    pub on_screen: String,
    pub text: String,
    pub assets: Vec<String>,
    pub sequence: String,
    pub sound: String,
    pub audio_idea: String,
    pub transition: String,
    /// The rendered scene, once built.
    pub file: Option<PathBuf>,
    pub poster: Option<PathBuf>,
}

impl Scene {
    pub fn component(&self) -> String {
        self.id.split('-').filter(|w| !w.is_empty()).map(|w| w[..1].to_uppercase() + &w[1..]).collect::<String>() + "Scene"
    }
}

/// One Flick run for a Reel: its workspace and plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Run {
    pub dir: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// (start, end, text) in seconds of the Reel.
    pub lines: Vec<(f64, f64, String)>,
    pub direction: String,
    pub assets: Vec<String>,
    pub scenes: Vec<Scene>,
    /// Scenes sit over the Reel's footage (transparent, rendered as VP8 WebM with alpha);
    /// false makes full-frame scenes that replace it.
    pub overlay: bool,
    /// Where the Reel's template shows the footage (x, y, w, h in pixels): overlays stay inside
    /// it, clear of the template's own headline and card.
    pub footage_area: Option<[f64; 4]>,
}

impl Run {
    /// The overlay rules for this Reel, with its footage area.
    fn overlay_rules(&self) -> String {
        match self.footage_area {
            Some([x, y, w, h]) => format!(
                "{OVERLAY_RULES} On this Reel the template already draws its own headline, caption and logo outside the footage area: draw ONLY inside the footage area x={x:.0} to {:.0}, y={y:.0} to {:.0} (pixels of the {}x{} frame), and leave everything outside it fully transparent. Within that area keep its middle clear and use its top and bottom edges and sides.",
                x + w,
                y + h,
                self.width,
                self.height
            ),
            None => OVERLAY_RULES.to_string(),
        }
    }
}

/// What overlay scenes must respect.
const OVERLAY_RULES: &str = "These scenes are OVERLAYS on top of the Reel's own footage, not full-frame videos: the composition background must stay fully transparent (no full-frame backgrounds, colour washes or vignettes covering the frame). Keep the animated elements to the top third, the bottom third and the edges, and leave the middle of the frame clear so the footage's main subject stays visible; put text on compact translucent panels so it stays readable over any footage. Motion, impacts and reveals still sync to the sound effects.";

impl Run {
    fn remotion(&self) -> PathBuf {
        self.dir.join("remotion")
    }
    pub fn save(&self) -> Result<()> {
        std::fs::write(self.dir.join("flick-run.json"), serde_json::to_string_pretty(self).map_err(|e| StudioError::Other(e.to_string()))?)?;
        Ok(())
    }
    pub fn load(dir: &Path) -> Result<Run> {
        serde_json::from_str(&std::fs::read_to_string(dir.join("flick-run.json"))?).map_err(|e| StudioError::Other(e.to_string()))
    }
}

/// Starts a run in `parent` (the project's media folder): the workspace with Flick's starter
/// Remotion project (linked to the installed packages), the transcript, and the brand assets
/// given (the workspace logo, screenshots…).
pub fn start(studio: &Studio, parent: &Path, lines: Vec<(f64, f64, String)>, size: (u32, u32), direction: &str, brand: &[PathBuf]) -> Result<Run> {
    let st = status(studio);
    if !st.skill || !st.remotion {
        return err("set up Flick first (Plugin Settings ▸ AI ▸ Set up Flick)");
    }
    if lines.iter().all(|l| l.2.trim().is_empty()) {
        return err("there is no transcript to animate");
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d-%H%M%S");
    let dir = parent.join(format!("flick-{stamp}"));
    std::fs::create_dir_all(dir.join("brand-assets"))?;
    std::fs::create_dir_all(dir.join("scenes"))?;
    let remotion = dir.join("remotion");
    copy_dir(&template(studio), &remotion)?;
    link_modules(&template(studio).join("node_modules"), &remotion.join("node_modules"))?;
    let mut assets = Vec::new();
    std::fs::create_dir_all(remotion.join("public").join("brand-assets"))?;
    for f in brand.iter().filter(|f| f.is_file()) {
        if let Some(name) = f.file_name() {
            std::fs::copy(f, dir.join("brand-assets").join(name))?;
            std::fs::copy(f, remotion.join("public").join("brand-assets").join(name))?;
            assets.push(format!("brand-assets/{}", name.to_string_lossy()));
        }
    }
    let text = lines.iter().map(|l| l.2.as_str()).collect::<Vec<_>>().join(" ");
    let transcript = json!({"source": "shorts-factory", "text": text,
        "segments": lines.iter().map(|(s, e, t)| json!({"start": s, "end": e, "text": t})).collect::<Vec<_>>(), "words": []});
    std::fs::write(dir.join("transcript.json"), serde_json::to_string_pretty(&transcript).unwrap_or_default())?;
    std::fs::write(
        dir.join("flick.config.json"),
        r#"{"version":2,"transcript":"transcript.json","plan":"flick-plan.md","brief":"remotion-brief.md","sceneSpec":"scene-spec.json"}"#,
    )?;
    let run = Run { dir, width: size.0, height: size.1, fps: 30, lines, direction: direction.trim().to_string(), assets, scenes: Vec::new(), overlay: true, footage_area: None };
    run.save()?;
    Ok(run)
}

/// node_modules is shared with the install: a directory junction on Windows, a symlink elsewhere.
fn link_modules(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        // mklink wants backslashes.
        let win = |p: &Path| p.to_string_lossy().replace('/', "\\");
        let o = quiet(Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(win(to)).arg(win(from))).output()?;
        if !o.status.success() {
            return err(format!("could not link Remotion into the run: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)));
        }
    }
    #[cfg(not(windows))]
    std::os::unix::fs::symlink(from, to)?;
    Ok(())
}

fn read(studio: &Studio, rel: &str) -> String {
    std::fs::read_to_string(skill(studio).join(rel)).unwrap_or_default()
}

/// Lowercase kebab-case, as Flick names scenes.
pub fn kebab(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(48).collect::<String>().trim_end_matches('-').to_string()
}

fn plan_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"scenes": {"type": "array", "items": {
            "type": "object",
            "properties": {
                "name": {"type": "string"}, "start": {"type": "number"}, "end": {"type": "number"}, "transcript": {"type": "string"},
                "onScreen": {"type": "string"}, "text": {"type": "string"}, "assets": {"type": "array", "items": {"type": "string"}},
                "sequence": {"type": "string"}, "sound": {"type": "string"}, "audioIdea": {"type": "string"}, "transition": {"type": "string"}
            },
            "required": ["name", "start", "end", "transcript", "onScreen", "text", "assets", "sequence", "sound", "audioIdea", "transition"],
            "additionalProperties": false
        }}},
        "required": ["scenes"],
        "additionalProperties": false
    })
}

/// Step 2: the scene plan, by the workspace's AI with Flick's planning rules. Writes
/// `flick-plan.md` for review; nothing is built until [`build`].
pub fn plan(studio: &Studio, channel: i64, run: &mut Run) -> Result<()> {
    let rules = read(studio, "references/step-2-plan.md");
    let catalog = read(studio, "saved-animations/README.md");
    let sounds: Vec<String> = std::fs::read_dir(run.remotion().join("public").join("sounds"))
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    let instruction = [
        crate::ai::voice(studio, channel)?.as_str(),
        "You are Flick's motion director, planning original short-form scene animations for a vertical social video (this is step 2 of Flick; its rules follow).",
        &rules,
        "The video is meant to go viral: open with the strongest visual proof of the first line, keep every scene a concrete animation of what is said, and keep text readable.",
        &if run.overlay { run.overlay_rules() } else { String::new() },
        "Return the storyboard as JSON: one scene per transcript beat, in order, covering the transcript; start/end in seconds from the transcript; name is a short approved-style name; sound is a file from the available sound effects with its visible trigger, or \"none\"; assets lists only supplied brand assets used, or is empty.",
        "Saved animation catalog (reuse only on a strong fit):",
        &catalog,
    ]
    .join("\n\n");
    let input = json!({
        "format": format!("{}x{} at {} fps", run.width, run.height, run.fps),
        "transcript": run.lines.iter().map(|(s, e, t)| json!({"start": s, "end": e, "text": t})).collect::<Vec<_>>(),
        "brandAssets": run.assets, "soundEffects": sounds,
        "creativeDirection": if run.direction.is_empty() { "Fast, bold, readable, built to stop the scroll." } else { &run.direction },
    });
    let Some(reply) = crate::ai::ask(studio, &instruction, &input, &plan_schema())? else { return err("the mock AI provider does not plan scenes") };
    let total = run.lines.iter().map(|l| l.1).fold(0.0, f64::max);
    let mut used = std::collections::HashSet::new();
    run.scenes = reply
        .get("scenes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let t = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
            let name = t("name");
            let mut id = kebab(&name);
            if id.is_empty() {
                return None;
            }
            while !used.insert(id.clone()) {
                id.push_str("-2");
            }
            let start = s.get("start").and_then(Value::as_f64)?.clamp(0.0, total.max(0.0));
            let end = s.get("end").and_then(Value::as_f64)?.clamp(start, total.max(start));
            (end - start >= 0.5).then(|| Scene {
                id,
                name,
                start,
                end,
                transcript: t("transcript"),
                on_screen: t("onScreen"),
                text: t("text"),
                assets: s.get("assets").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).filter(|a| run.assets.iter().any(|x| x == a)).map(str::to_string).collect(),
                sequence: t("sequence"),
                sound: t("sound"),
                audio_idea: t("audioIdea"),
                transition: t("transition"),
                ..Default::default()
            })
        })
        .collect();
    if run.scenes.is_empty() {
        return err("the AI planned no scenes");
    }
    std::fs::write(run.dir.join("flick-plan.md"), plan_md(run))?;
    run.save()
}

/// `flick-plan.md`, in Flick's format.
pub fn plan_md(run: &Run) -> String {
    let clock = |t: f64| format!("{}:{:04.1}", (t / 60.0) as u64, t % 60.0);
    let mut s = format!(
        "# Flick Plan\n\n## Transcript\n{}\n\n## Format\n{}:{} — {}x{} at {} fps\n\n## Brand assets\n{}\n\n## Creative direction\n{}\n\n## Storyboard\n",
        run.lines.iter().map(|(a, b, t)| format!("[{}–{}] {t}", clock(*a), clock(*b))).collect::<Vec<_>>().join("\n"),
        if run.width < run.height { "9" } else { "16" },
        if run.width < run.height { "16" } else { "9" },
        run.width,
        run.height,
        run.fps,
        if run.assets.is_empty() { "none".to_string() } else { run.assets.join(", ") },
        if run.direction.is_empty() { "Fast, bold, readable, built to stop the scroll." } else { &run.direction },
    );
    let or_none = |v: &str| if v.trim().is_empty() { "none".to_string() } else { v.to_string() };
    for (i, sc) in run.scenes.iter().enumerate() {
        s.push_str(&format!(
            "\n### Scene {} — {} — {}–{}\n\nTranscript:\n{}\n\nWhat's on screen:\n{}\n\nText on screen:\n{}\n\nBrand assets / supplied material:\n{}\n\nSequential / interaction:\n{}\n\nSound effect:\n{}\n\nAudio-coupled idea:\n{}\n\nTransition:\n{}\n",
            i + 1,
            sc.name,
            clock(sc.start),
            clock(sc.end),
            sc.transcript,
            sc.on_screen,
            or_none(&sc.text),
            if sc.assets.is_empty() { "none".into() } else { sc.assets.join(", ") },
            or_none(&sc.sequence),
            or_none(&sc.sound),
            or_none(&sc.audio_idea),
            or_none(&sc.transition)
        ));
    }
    s
}

/// `remotion-brief.md` and `scene-spec.json` for the approved plan (step 3's hand-off).
fn handoff(run: &Run) -> Result<()> {
    let mut brief = format!(
        "# Remotion Composition Brief: Flick\n\n## Objective\nCreate the approved short-form scene animations from the timestamped transcript.\n\n## Output\n- Remotion project: `remotion/`\n- Format: {}x{} at {} fps\n- Rendered scenes:\n{}\n\n## Source Material\n- Transcript: `transcript.json`\n- Approved plan: `flick-plan.md`\n- Selected brand assets: {}\n- Available sound effects: `remotion/public/sounds/`\n\n## Creative Direction\n- User direction: {}\n- Avoid: generic filler visuals, unapproved assets, background music\n\n## Scene Compositions\n",
        run.width,
        run.height,
        run.fps,
        run.scenes.iter().map(|s| format!("  - `{}` → `scenes/{}/{}.mp4`", s.id, s.id, s.id)).collect::<Vec<_>>().join("\n"),
        if run.assets.is_empty() { "none".into() } else { run.assets.join(", ") },
        if run.direction.is_empty() { "Fast, bold, readable, built to stop the scroll." } else { &run.direction },
    );
    for s in &run.scenes {
        brief.push_str(&format!(
            "\n### {}\n- Composition ID: `{}`\n- Component: `{}`\n- Transcript: {}\n- Time: {:.2}s–{:.2}s\n- Output: `scenes/{}/{}.mp4`\n- What is on screen: {}\n- Text on screen: {}\n- Brand assets / supplied material: {}\n- Sequential / interaction: {}\n- Sound effect: {}\n- Audio-coupled idea: {}\n- Transition: {}\n",
            s.name, s.id, s.component(), s.transcript, s.start, s.end, s.id, s.id, s.on_screen,
            if s.text.is_empty() { "none" } else { &s.text },
            if s.assets.is_empty() { "none".into() } else { s.assets.join(", ") },
            if s.sequence.is_empty() { "none" } else { &s.sequence },
            if s.sound.is_empty() { "none" } else { &s.sound },
            if s.audio_idea.is_empty() { "none" } else { &s.audio_idea },
            if s.transition.is_empty() { "clean cut" } else { &s.transition },
        ));
    }
    if run.overlay {
        brief.push_str(&format!("\n## Overlay\n{}\n", run.overlay_rules()));
    }
    brief.push_str("\n## Remotion Instructions\n- Build one dedicated React component for each approved scene under `src/scenes/`.\n- Register each scene as its own `<Composition>` in `src/Root.tsx`.\n- Do not create a combined or all-scenes composition.\n- Derive frame timing from `scene-spec.json`.\n- Use frame-driven Remotion motion. Build the approved visual idea; do not fall back to generic title-card layouts.\n- Use only selected brand assets from `public/brand-assets/` and bundled SFX from `public/sounds/`.\n- Do not add background music. Use an SFX only when it supports the visible action in the approved scene.\n- Keep on-screen text readable.\n");
    std::fs::write(run.dir.join("remotion-brief.md"), brief)?;
    let fps = f64::from(run.fps);
    let spec = json!({
        "width": run.width, "height": run.height, "fps": run.fps,
        "scenes": run.scenes.iter().map(|s| json!({
            "id": s.id, "name": s.name, "transcript": s.transcript,
            "startMs": (s.start * 1000.0).round(), "endMs": (s.end * 1000.0).round(),
            "from": 0, "durationInFrames": ((s.end - s.start) * fps).round().max(1.0),
            "componentName": s.component(), "assets": s.assets, "visualDescription": s.on_screen,
            "text": s.text, "sequence": s.sequence, "soundEffects": if s.sound.is_empty() || s.sound == "none" { vec![] } else { vec![s.sound.clone()] },
            "transition": s.transition,
        })).collect::<Vec<_>>(),
    });
    let text = serde_json::to_string_pretty(&spec).unwrap_or_default();
    std::fs::write(run.dir.join("scene-spec.json"), &text)?;
    std::fs::create_dir_all(run.remotion().join("src").join("data"))?;
    std::fs::write(run.remotion().join("src").join("data").join("scene-spec.json"), &text)?;
    Ok(())
}

/// Runs the coding agent in the run's workspace with `prompt`. Its output goes to `agent.log`.
fn agent_run(studio: &Studio, run: &Run, prompt: &str) -> Result<()> {
    let limit = Duration::from_millis(studio.var("FLICK_TIMEOUT_MS").and_then(|v| v.parse().ok()).unwrap_or(40 * 60_000));
    let log = run.dir.join("agent.log");
    let prompt_file = run.dir.join("agent-prompt.md");
    std::fs::write(&prompt_file, prompt)?;
    let mut c = match agent(studio).as_str() {
        "codex" => {
            let mut c = Command::new(studio.var("CODEX_BIN").unwrap_or("codex"));
            c.args(["exec", "--full-auto", "--skip-git-repo-check", "-C"]).arg(&run.dir).arg("-");
            c
        }
        "hermes" => {
            let mut c = Command::new(studio.var("HERMES_BIN").unwrap_or("hermes"));
            // Windows limits a command line to about 32 KB: Hermes reads the prompt from its file.
            c.args(["-z", "Read agent-prompt.md in this folder and do exactly what it says.", "--ignore-rules"]);
            c
        }
        _ => {
            let mut c = Command::new(studio.var("CLAUDE_BIN").unwrap_or("claude"));
            c.args([
                "-p",
                "--permission-mode",
                "acceptEdits",
                "--allowedTools",
                "Read,Write,Edit,Glob,Grep,Bash(npx tsc:*),Bash(npx remotion:*),Bash(node:*),Bash(ls:*),Bash(dir:*)",
            ]);
            c
        }
    };
    c.current_dir(&run.dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = quiet(&mut c).spawn().map_err(|e| StudioError::Other(format!("could not start the {} agent: {e}", agent(studio))))?;
    if agent(studio) != "hermes"
        && let Some(mut stdin) = child.stdin.take()
    {
        use std::io::Write;
        stdin.write_all(prompt.as_bytes())?;
    }
    drop(child.stdin.take());
    wait(child, limit, Some(&log)).map(|_| ()).map_err(|e| StudioError::Other(format!("the {} agent {e} (see {})", agent(studio), log.display())))
}

fn rules_for_agent(studio: &Studio) -> String {
    [
        "You are Flick's Remotion builder, run by the Shorts Factory desktop app in this folder (a Flick output directory). Work only inside this folder. Do not start Remotion Studio, do not render, do not install packages and do not ask questions: the app renders every scene itself after you finish.",
        "Flick's skill and its build rules follow; the plan is already approved (flick-plan.md), and remotion-brief.md and scene-spec.json are written.",
        &read(studio, "SKILL.md"),
        &read(studio, "references/step-3-compose.md"),
        "If remotion-brief.md has an Overlay section, follow it strictly: transparent background, the middle of the frame left clear.",
        "Saved animation catalog (the components are under the folder given; adapt one only on a strong fit):",
        &read(studio, "saved-animations/README.md"),
        &format!("Saved animations folder: {}", skill(studio).join("saved-animations").display()),
    ]
    .join("\n\n")
}

/// Step 3: the agent builds the approved scenes, then the app renders each one (and its
/// poster). Returns the scenes with their files. Blocking (many minutes).
pub fn build(studio: &Studio, run: &mut Run, say: &dyn Fn(&str)) -> Result<()> {
    if run.scenes.is_empty() {
        return err("plan the scenes first");
    }
    std::fs::write(run.dir.join("flick-plan.md"), plan_md(run))?;
    handoff(run)?;
    run.save()?;
    say(&format!("{} is building {} scene(s) in Remotion", agent_label(studio), run.scenes.len()));
    let prompt = format!(
        "{}\n\nBuild every approved scene now: one component per scene under remotion/src/scenes/, each registered as its own <Composition> in remotion/src/Root.tsx with the id, size, fps and durationInFrames of scene-spec.json. Then run `npx tsc --noEmit` in remotion/ and fix any errors. Finish with a one-line summary.",
        rules_for_agent(studio)
    );
    agent_run(studio, run, &prompt)?;
    render_all(studio, run, say, None)
}

fn agent_label(studio: &Studio) -> &'static str {
    match agent(studio).as_str() {
        "codex" => "Codex",
        "hermes" => "Hermes",
        _ => "Claude Code",
    }
}

/// Renders the run's scenes (or only `only`) and their posters.
fn render_all(studio: &Studio, run: &mut Run, say: &dyn Fn(&str), only: Option<&str>) -> Result<()> {
    let mut failed = Vec::new();
    let n = run.scenes.len();
    for i in 0..n {
        let id = run.scenes[i].id.clone();
        if only.is_some_and(|o| o != id) {
            continue;
        }
        say(&format!("Rendering scene {} of {n}: {}", i + 1, run.scenes[i].name));
        let out_dir = run.dir.join("scenes").join(&id);
        std::fs::create_dir_all(&out_dir)?;
        let out = out_dir.join(format!("{id}.{}", if run.overlay { "webm" } else { "mp4" }));
        let r = run_render(run, &id, &out);
        match r {
            Ok(()) if out.is_file() => {
                run.scenes[i].file = Some(out.clone());
                // A settled frame two-thirds in, as Flick's poster.
                let poster = out_dir.join("poster.jpg");
                let at = (run.scenes[i].end - run.scenes[i].start) * 0.66;
                if let Some(ff) = crate::ytdlp::ffmpeg(studio) {
                    let ok = quiet(Command::new(ff).args(["-y", "-hide_banner", "-loglevel", "error", "-ss", &format!("{at:.2}"), "-i"]).arg(&out).args(["-frames:v", "1", "-q:v", "3"]).arg(&poster))
                        .status()
                        .is_ok_and(|s| s.success());
                    if ok {
                        run.scenes[i].poster = Some(poster);
                    }
                }
            }
            Ok(()) => failed.push(format!("{id}: no file")),
            Err(e) => failed.push(format!("{id}: {e}")),
        }
        run.save()?;
    }
    if failed.is_empty() { Ok(()) } else { err(format!("some scenes did not render: {}", failed.join("; "))) }
}

fn run_render(r: &Run, id: &str, out: &Path) -> Result<()> {
    let mut c = node_tool("npx");
    c.args(["remotion", "render", "src/index.tsx", id]).arg(out);
    if r.overlay {
        // Transparent: PNG frames into VP8 with an alpha channel (the editor reads WebM alpha).
        c.args(["--codec=vp8", "--image-format=png", "--pixel-format=yuva420p"]);
    }
    run(
        c.current_dir(r.remotion()),
        Duration::from_secs(1800),
        Some(&r.dir.join("render.log")),
    )
    .map(|_| ())
}

/// Revises one scene from feedback (only that scene), then renders it again. Blocking.
pub fn revise(studio: &Studio, run: &mut Run, id: &str, feedback: &str, say: &dyn Fn(&str)) -> Result<()> {
    let scene = run.scenes.iter().find(|s| s.id == id).ok_or_else(|| StudioError::Other("no such scene".into()))?.clone();
    say(&format!("{} is revising {}", agent_label(studio), scene.name));
    let prompt = format!(
        "{}\n\nRevise only the scene `{}` (component {}, composition id `{}`) from this feedback, keeping its approved timing and every other scene unchanged:\n\n{}\n\nThen run `npx tsc --noEmit` in remotion/ and fix any errors. Finish with a one-line summary.",
        rules_for_agent(studio),
        scene.name,
        scene.component(),
        scene.id,
        feedback.trim()
    );
    agent_run(studio, run, &prompt)?;
    render_all(studio, run, say, Some(id))
}

/// Starts Remotion Studio for the run; returns its address.
pub fn open_studio(run: &Run) -> Result<String> {
    let mut c = node_tool("npx");
    c.args(["remotion", "studio", "src/index.tsx", "--port", "3123", "--no-open"]).current_dir(run.remotion()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    quiet(&mut c).spawn().map_err(|e| StudioError::Other(format!("could not start Remotion Studio: {e}")))?;
    let url = "http://localhost:3123".to_string();
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(90) {
        if ureq::AgentBuilder::new().timeout(Duration::from_secs(2)).build().get(&url).call().is_ok() {
            return Ok(url);
        }
        std::thread::sleep(Duration::from_millis(700));
    }
    err("Remotion Studio did not start within 90 seconds")
}

/// Saves a scene's component to Flick's reusable library (code only; no video, poster or
/// brand assets), with its catalog entry.
pub fn save_reusable(studio: &Studio, run: &Run, id: &str, pattern: &str, use_for: &str, avoid_for: &str) -> Result<PathBuf> {
    let scene = run.scenes.iter().find(|s| s.id == id).ok_or_else(|| StudioError::Other("no such scene".into()))?;
    let scenes = run.remotion().join("src").join("scenes");
    let wanted = scene.component();
    let file = std::fs::read_dir(&scenes)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x == "tsx") && std::fs::read_to_string(p).is_ok_and(|t| t.contains(&wanted)))
        .ok_or_else(|| StudioError::Other(format!("no component {wanted} found")))?;
    let dest = skill(studio).join("saved-animations").join(&scene.id);
    std::fs::create_dir_all(&dest)?;
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    std::fs::copy(&file, dest.join(&name))?;
    let mut catalog = read(studio, "saved-animations/README.md");
    catalog.push_str(&format!("\n## {}\n**File:** `{}/{name}` — `{wanted}`\n**Pattern:** {}\n**Use for:** {}\n**Avoid for:** {}\n", scene.id, scene.id, pattern.trim(), use_for.trim(), avoid_for.trim()));
    std::fs::write(skill(studio).join("saved-animations").join("README.md"), catalog)?;
    Ok(dest)
}
