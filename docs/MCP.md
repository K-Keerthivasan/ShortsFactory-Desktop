# Driving Shorts Factory Desktop from an AI agent (MCP)

Everything the app does is an engine command, and every command is an MCP tool. On top of
FilmCraft's editing commands (`reel.*`, `timeline.*`, `export.*`, …) the app adds `studio.*`:
the workspaces, library, AI cards, yt-dlp downloads and publishing.

```
filmcraft-cli commands studio      # list them with their params
filmcraft-cli exec studio.workspaces
```

| Command | What it does |
| --- | --- |
| `studio.workspaces`, `studio.brand`, `studio.media`, `studio.library`, `studio.accounts` | Read the shared library (account rows name the key file, never its contents) |
| `studio.health` | The start-up connection check (read-only; never changes account status) |
| `studio.writeCard` | Writes a card (headline, caption, hashtags) with Hermes / Ollama |
| `studio.download` | yt-dlp download → checks → library → AI draft (blocks until done; `noDraft` skips the draft) |
| `studio.plan` | The AI reads the video's chapters and captions and picks the parts worth a Reel, with a card each |
| `reel.new`, `reel.apply`, `reel.setCard`, `reel.duplicate`, `reel.usesOf` | Build and edit Reels and Posts on the timeline |
| `studio.sendToLibrary` | Renders the active Reel and adds it to its workspace's library |
| `studio.approve` | Approves a render; nothing unapproved is ever published |
| `studio.publishNow`, `studio.schedule` | Direct Instagram / YouTube; `schedule` queues Instagram locally and uploads YouTube now with YouTube's own publish time |
| `studio.cloudSchedule`, `studio.cloud`, `studio.calendar` | Supabase + B2 scheduling and the calendar |

## One server for Claude and Codex (set up from the app)

Plugin Settings ▸ AI ▸ **AI editors** ▸ **Set up AI editors** (MCP: `studio.aiEditor`
`{"action":"setup"}`) does it all once:

- turns on **Let Claude edit**: the app starts its localhost control server on port 9876 from
  its next start (**Turn off the control server** undoes it);
- registers the same server, `shorts-factory` → `filmcraft-cli mcp --bridge 127.0.0.1:9876`,
  with **Claude Code** (user scope), the **Claude desktop app** (`claude_desktop_config.json`)
  and **Codex** (`~/.codex/config.toml`), keeping every other setting (the previous files are
  kept as `.bak`);
- installs the **`shorts-factory-edit` skill**, the quality-edit playbook (brief, watch, cut plan
  for approval, rough cut, framing, captions, design, sound, colour, review, deliver), for
  Claude (`~/.claude/skills`) and Codex (`~/.codex/skills`).

Then Create ▸ **Edit with Claude** / **Edit with Codex** opens the agent in a terminal on the open
project with a brief (`AI edit/ai-edit-brief.md` in the project's folder); talk to it there and
watch it edit the live timeline. The ChatGPT app itself only connects to MCP servers on the
internet (HTTPS), not to local ones; Codex is ChatGPT's agent that uses this local server.

The server's tools: `command_run` / `command_batch` (any of the editor's ~750 commands),
`command_list`, `project_inspect`, `sequence_inspect`, `render_frame`, `render_preview`,
`doc_inspect`, `media_import`, `workflow_*`, and with the app running `ui_screenshot`,
`ui_click`, `ui_type`, `ui_key`, `ui_drag`, `ui_elements`, `ui_inspect`.

## Two ways to connect

**Headless** – the agent edits project files without the app open:
`filmcraft-cli mcp`

**Live** – the agent drives the running app (you watch the edits happen and can step in):
start the app with `shorts-factory-desktop --control 9876`, then use
`filmcraft-cli mcp --bridge 127.0.0.1:9876`. Bridge mode adds `ui_screenshot`, `ui_click`, … .

## Client set-up

Use the full path to `filmcraft-cli.exe` (in `target\release` after `cargo build --release`).

Claude Code:

```bash
claude mcp add shorts-factory -- "E:\AI\IG_ReelAutomation_Desktop\target\release\filmcraft-cli.exe" mcp --bridge 127.0.0.1:9876
```

Codex (`%USERPROFILE%\.codex\config.toml`):

```toml
[mcp_servers.shorts-factory]
command = 'E:\AI\IG_ReelAutomation_Desktop\target\release\filmcraft-cli.exe'
args = ["mcp", "--bridge", "127.0.0.1:9876"]
```

Hermes (`~/.hermes/config.yaml`):

```yaml
mcp_servers:
  shorts-factory:
    command: 'E:\AI\IG_ReelAutomation_Desktop\target\release\filmcraft-cli.exe'
    args: ["mcp", "--bridge", "127.0.0.1:9876"]
```

## A full run, as an agent would do it

1. `studio.download {"channel":1,"url":"https://youtu.be/…","noDraft":true}` → the file and its length
2. `studio.plan {"channel":1,"url":"…","duration":<length>,"wants":{"mode":"auto","maxReels":3}}` → segments
3. `studio.brand {"channel":1}`, then per segment `reel.new {"paths":["…mp4"],"sourceIn":start,"sourceOut":end,"card":card,"template":"jkr-clean-v2","channelId":1,"brand":<that brand>}`
   (`"format":"post"` for a 4:5 Post; the file is imported once and every Reel uses it)
4. `studio.sendToLibrary {}` (with each Reel open: `sequence.open`) → `{render, post}`
5. You approve in the app (Export ▸ Library), or the agent calls `studio.approve` if you allow it
6. `studio.publishNow` / `studio.schedule` / `studio.cloudSchedule`

The app's **AI** panel runs this same workflow with buttons: paste links, choose single /
several / let the AI decide, review the parts it picked, then queue them. Every Reel lands in the
library unapproved unless you approve it.
