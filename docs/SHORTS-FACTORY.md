# Shorts Factory Desktop — feature parity

The earlier Shorts Factory Desktop (Electron + Node, `E:\AI\IG_ReelAutomation\desktop-app`) is being
rebuilt here on FilmCraft. This list follows that app's feature list item by item.

Status: **Done** works in this app · **Partial** works with the noted gap · **Planned** not built yet.
Until an item is Done, the earlier app keeps doing it: both apps share one library, so they can run
side by side.

## Architecture

| Layer | Crate | What it holds |
|---|---|---|
| L2 | `crates/reel` (`filmcraft-reel`) | Brand, card, the 21 templates as layer plans |
| L4 | `crates/engine/src/reel.rs` | `reel.*` commands: build a template into tracks and clips |
| L5 | `crates/studio` (`filmcraft-studio`) | The library database, keys, AI, Instagram / YouTube, scheduling |
| L5 | `crates/ui-egui/src/panels/reel.rs` | The Reel Studio panel |

What runs where is unchanged: editing, previews, rendering and local assets on the PC; Ollama on the
PC; direct posting from the desktop; MediaDock in its own Docker install; cloud schedules in
Supabase; temporary cloud video in Backblaze B2.

## 1. Workspaces

| Feature | Status |
|---|---|
| Switch between JKR and K2 Digital Media | Done — Reel Studio's workspace picker reads the library's channels |
| Create new workspaces | Planned |
| Separate assets, branding, projects and Instagram connections | Done for assets, branding and connections (library rows); projects are `.fcproj` files |
| Apply each workspace's branding to its videos | Done — templates take the channel's colours, font, handle and logo |
| Choose the destination Instagram account for cloud scheduling | Planned (with §10) |

## 2. Video assets

| Feature | Status |
|---|---|
| Use clips and full movies from the workspace folders | Done — the panel lists the workspace's media |
| Refresh the folder to discover new files | Planned |
| Browse and search clips | Partial — list; search planned |
| Preview source videos | Done — FilmCraft's Source monitor |
| Brand assets (logos, graphics) | Done — the channel logo is placed by every template |
| Duration and resolution | Done |
| Drag-and-drop import | Done — FilmCraft imports by drag and drop and File ▸ Import |

## 3. Download videos from links (yt-dlp, built in)

MediaDock is replaced by yt-dlp driven directly by the app (Reel Studio ▸ Download from links).

| Feature | Status |
|---|---|
| Up to ten links per batch, copied into the workspace's clips folder | Built — not yet run against real links on this PC |
| Options: quality (best / 1080p / 720p / 480p), custom `-f` format, section start–end, rate limit, browser cookies | Built |
| Best video + audio merged to MP4 with FFmpeg (`FFMPEG_PATH` or the earlier app's bundled copy); single-file MP4 without it | Built |
| Readability and duration checks (FilmCraft's own demuxers), progress, errors, retry, replace link | Built |
| Keep downloaded footage when AI fails; draft post for review; Open as Reel | Built |
| Install yt-dlp from the panel (official GitHub release, on request) | Built |

## 4. Local AI assistance

| Feature | Status |
|---|---|
| Headline, subtitle, Instagram caption, hashtags, attribution | Done — Hermes or Ollama, the library's provider and channel voice |
| From source metadata and notes | Done for notes and articles; MediaDock metadata with §3 |

## 5. Video editing

| Feature | Status |
|---|---|
| Preview, play, in/out, cut scenes, queue exports | Done — FilmCraft (Source monitor, three-point editing, export queue) |
| Crop, position, scale, transform | Done — Motion, Crop and keyframes; templates fit footage to their slot |
| Direct controls and the timeline | Done — Reel Studio panel plus the timeline |
| Headline and supporting text | Done — card fields, also as template properties in Essential Graphics |
| Layouts and templates, vertical preview | Done — 21 templates on a 1080×1920 sequence |
| Save with unsaved-change safeguards | Done — FilmCraft projects, autosave and crash recovery |

## 6. Templates and branding

| Feature | Status |
|---|---|
| Quote card, full bleed, headline hero, numbered point and the rest | Done — all 21 |
| Workspace names, handles, colours | Done |
| Template selection and visual adjustments | Done — re-apply any template; every layer is editable |

## 7. Rendering and exporting

| Feature | Status |
|---|---|
| Render locally, queue, status | Done — FilmCraft export queue (Social Vertical 1080×1920 H.264) |
| Scene exports, MP4 download, preview | Done |
| Exports reach the library as the post's render | Done — Reel Studio ▸ Send to library |

## 8. Video library

| Feature | Status |
|---|---|
| Drafts and finished videos, approve, publication results | Done — Reel Studio's Library section |
| Search, name, delete, reopen for editing | Planned |
| Published page | Partial — publication status shown per post |

## 9. Post directly to Instagram

| Feature | Status |
|---|---|
| Per-workspace connection, connected account shown | Done — the library's accounts and keys, read in place |
| Post immediately; queued / success / failure; link; attempts and errors | Done |
| Duplicate guard and reconciliation | Done — atomic claim; unknown outcomes need reconciliation |
| Test the account connection | Planned |

## 10. Schedule through Supabase

Same `publishing-api` edge function, same `.env` credentials (`CLOUD_SUPABASE_URL`,
`CLOUD_SUPABASE_ANON_KEY`, `CLOUD_AUTH_EMAIL`, `CLOUD_AUTH_PASSWORD`); the session is kept in memory.

| Feature | Status |
|---|---|
| Reserve, upload the MP4 straight to the signed B2 / Storage URL, finalize ("safe to close") | Built — not yet run end to end against the deployed backend |
| Upload destination checked against the reservation (host, bucket, key, signature, HTTPS) | Built and tested |
| Idempotency key reused for a retry of the same render | Built |
| Choose the cloud Instagram account; time typed locally, sent with the IANA time zone | Built |
| List schedules; publish now, cancel, retry, delete an unfinished upload; Instagram link | Built |
| Local schedules (PC on) | Done — written to the library; run by the earlier app's worker, or here when it is not running |

## 11. Automatic import workflow

Link → download → AI → render → local schedule: **Planned** (after §3).

## 12. Account health and token notifications

| Feature | Status |
|---|---|
| Token expiry and time left per cloud account; warnings at 7 days, 3 days, 24 hours and expired | Built and tested (thresholds) |
| Banner at the top of Reel Studio for the most urgent warning | Built |
| Check account health; **Extend token** from the desktop (`extendInstagramToken`) | Built — new: the earlier app had no desktop button |

## 13. Supporting tools

| Feature | Status |
|---|---|
| AI provider settings | Done — read from the library (`application_settings`, `.env`) |
| Logs, media and settings folders | Done — FilmCraft logs; the library's folders open from the panel |
| Dashboard, system health, feed configuration | Planned |
