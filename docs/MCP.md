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
| `studio.download` | yt-dlp download → checks → library → AI draft (blocks until done) |
| `reel.new`, `reel.apply`, `reel.setCard`, `reel.duplicate`, `reel.usesOf` | Build and edit Reels and Posts on the timeline |
| `studio.sendToLibrary` | Renders the active Reel and adds it to its workspace's library |
| `studio.approve` | Approves a render; nothing unapproved is ever published |
| `studio.publishNow`, `studio.schedule` | Direct Instagram / YouTube, or the local queue |
| `studio.cloudSchedule`, `studio.cloud`, `studio.calendar` | Supabase + B2 scheduling and the calendar |

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

1. `studio.download {"channel":1,"url":"https://youtu.be/…"}` → a library asset and its path
2. `studio.brand {"channel":1}`, then `reel.new {"paths":["…mp4"],"template":"jkr-clean-v2","channelId":1,"brand":<that brand>}`
   (`"format":"post"` for a 4:5 Post; repeat for several Reels from one source – the file is imported once)
3. `studio.writeCard` → `reel.setCard` with the result
4. `studio.sendToLibrary {}` → `{render, post}`
5. You approve in the app (Export ▸ Library), or the agent calls `studio.approve` if you allow it
6. `studio.publishNow` / `studio.schedule` / `studio.cloudSchedule`
