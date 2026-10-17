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
| Create new workspaces | Built — Settings ▸ New workspace; validates slug and creates isolated folders |
| Separate assets, branding, projects and Instagram connections | Done for assets, branding and connections (library rows); projects are `.fcproj` files |
| Apply each workspace's branding to its videos | Done — templates take the channel's colours, font, handle and logo |
| Choose the destination Instagram account for cloud scheduling | Built — Publish ▸ Cloud scheduling |

## 2. Video assets

| Feature | Status |
|---|---|
| Use clips and full movies from the workspace folders | Done — the panel lists the workspace's media |
| Refresh the folder to discover new files | Built — Create ▸ Scan workspace folders; bounded scan under workspace assets, skips unreadable videos and symbolic links |
| Browse and search clips | Built — Create ▸ Workspace clips; list/grid views, first 60 matching clips |
| Preview source videos | Done — a separate footage preview with scrubbing; Play with audio opens the native player. Preview does not import into the project |
| Brand assets (logos, graphics) | Done — the channel logo is placed by every template |
| Duration and resolution | Done |
| Drag-and-drop import | Done — FilmCraft imports by drag and drop and File ▸ Import |

## 3. Download videos from links (yt-dlp, built in)

MediaDock is replaced by yt-dlp driven directly by the app (Create ▸ Download videos).

| Feature | Status |
|---|---|
| Up to ten links per batch, saved into the workspace's clips folder | Built — tested with yt-dlp and a locally served video; site-specific extraction depends on yt-dlp and the source site |
| Persistent download shelf | Create ▸ Workspace clips lists completed downloads across projects and app restarts. Default location: library storage/channels/<workspace>/assets/clips. Custom folders are registered in place |
| Find and use downloaded footage | Downloads shows the folder and provides Show in Create, Preview and Open folder. New Reel imports only the selected clip |
| Recover older downloads | Create ▸ Refresh scans workspace assets, the custom download folder and the open project's legacy Downloads folder; files are kept in place |
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
| Quote card, full bleed, headline hero, numbered point and the rest | All 21 now use full-frame footage with compact text overlays; no reserved empty header/footer. Logo only deliberately has no headline |
| Workspace names, handles, colours | Done |
| Template selection and visual adjustments | Built — Design has all 21 layouts with slot diagrams; every layer is editable. Original recipe variation/tuning controls are not yet ported. |
| Edit workspace branding | Built — Settings edits handle, website, contact, font, colours, headline size, verified mark and default template; saves without replacing other settings |

Saved projects retain their editable layers. Select a template again in Design ▸ Layout to apply
its updated layout to an existing Reel. Footage fills 9:16 using cover/crop; use the clip's Motion
controls to reposition its subject. Headline and supporting text remain inside the Instagram
preview guide area. An empty supporting line adds no reserved space.

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
| Search, rename, archive and restore | Built — latest 40 posts per view; archive retains files and history, with Undo and an Archived view; active jobs block archiving |
| Reopen for editing | Built for new native exports, which save an adjacent `.fcproj`; older renders without that snapshot cannot reopen |
| Published page | Partial — publication status shown per post |

## 9. Post directly to Instagram

| Feature | Status |
|---|---|
| Per-workspace connection, connected account shown | Done — the library's accounts and keys, read in place |
| Post immediately; queued / success / failure; link; attempts and errors | Done |
| Duplicate guard and reconciliation | Done — atomic claim; unknown outcomes need reconciliation |
| Test the account connection | Planned |

## 9b. Post the same Reels to YouTube Shorts

Built. A workspace can publish to its YouTube channel too: tick the YouTube account next to
Instagram in Export ▸ Publish ▸ Local publishing, then Publish now or Schedule. The Short gets the
Reel's headline as its title (up to 100 characters) and the Instagram caption and hashtags as its
description, with `#Shorts` added; hashtags also become the video's tags. Uploads are resumable
and retried like Instagram ones. Cloud publishing (Supabase) is Instagram only.

Connecting a channel (one time), in Plugin Settings ▸ Accounts & keys ▸ YouTube, where the app
shows the same steps with buttons that open each page:

1. [Google Cloud console](https://console.cloud.google.com/projectcreate): create a project with
   the Google account that owns the channel.
2. Enable the [YouTube Data API v3](https://console.cloud.google.com/apis/library/youtube.googleapis.com).
3. [Consent screen](https://console.cloud.google.com/auth/overview): app name, your email,
   audience External, your account under Test users.
4. Audience ▸ **Publish app** (In production): in Testing, Google ends the sign-in after 7 days.
   Your own sign-in then shows "Google hasn't verified this app": Advanced ▸ Continue.
5. [Clients](https://console.cloud.google.com/auth/clients) ▸ Create client ▸ **Desktop app**;
   copy the Client ID and Client secret.
6. Paste them into **Google client ID / secret** (saved as `YOUTUBE_CLIENT_ID` and
   `YOUTUBE_CLIENT_SECRET` in the library's `.env`), Save, then **Connect YouTube to this
   workspace** and allow access in the browser. The refresh token is kept in
   `secrets/youtube-<workspace>.json`; the workspace's YouTube account appears below.

Shorts upload as Public. Google's `videos.insert` reference (updated 2026-10-15) states that videos
uploaded from unverified API projects are not restricted to private viewing; the 2020 rule that
locked them until an audit no longer applies. The default quota allows about six uploads a day per
project; the [YouTube API audit form](https://support.google.com/youtube/contact/yt_api_form) asks
Google for more. Vertical videos up to 3 minutes
are Shorts; Publish warns when a Reel is longer.

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

Built as the **AI** panel (beside Reel Studio), with approval before anything is published:

| Step | Status |
|---|---|
| Paste up to ten video links; yt-dlp downloads each once | Built |
| The AI (Hermes, Codex or Ollama) reads the video's title, description, chapters and captions and picks one part, several, or decides itself, with a card per part | Built — falls back to the chapters or even parts when no AI answers |
| Review: tick parts, adjust their times and headlines | Built |
| One Reel or Post per part from the same file (`reel.new` with `sourceIn`/`sourceOut`), queued to render into the library | Built |
| Approve, then publish now or schedule, from the same panel | Built |
| The same workflow for agents over MCP (`studio.download`, `studio.plan`, `reel.new`, `studio.sendToLibrary`, …) | Built — see [MCP.md](MCP.md) |

## 12. Account health and token notifications

| Feature | Status |
|---|---|
| Token expiry and time left per cloud account; warnings at 7 days, 3 days, 24 hours and expired | Built and tested (thresholds) |
| Banner at the top of Reel Studio for the most urgent warning | Built |
| Check account health; **Extend token** from the desktop (`extendInstagramToken`) | Built — new: the earlier app had no desktop button |

## 13. Supporting tools

| Feature | Status |
|---|---|
| AI provider settings | Built — Plugin Settings switches Hermes / Codex / Ollama while preserving scheduler settings; credentials remain in the existing configuration |
| MCP server for AI agents (Claude Code, Codex, Hermes) | Built — every editing command plus `studio.*`; see [MCP.md](MCP.md) |
| Logs, media and settings folders | Done — FilmCraft logs; the library's folders open from the panel |
| Dashboard, system health, feed configuration | Planned |

## Audit, 2026-10-14

The native app does **not** yet have full parity with the Electron app. The statuses
above distinguish implemented code from external-service acceptance testing.

The original app's `src/server/routes` and `src/shared/contracts.ts` also expose
features that this migration checklist previously underreported:

| Original capability | Native status |
|---|---|
| Per-template tuning, five recipe variations and visual-style options | Missing dedicated controls; ordinary FilmCraft layer editing is available but does not reproduce those settings |
| RSS feed management, polling and article extraction | Existing articles can be read; feed administration and ingestion not ported |
| Analytics, dashboard and system/provider readiness | Not ported |
| Backup, cleanup and asset organisation | Not ported |
| Create/edit/remove publishing accounts | Reads existing connections; management not ported |
| Change AI provider and scheduler configuration | Provider selector built; cron/timezone editing not ported |
| Render naming, deletion and reopening original scripts/projects | Rename and reversible archive built; reopening supports new native snapshots, not original Electron projects; permanent deletion omitted |
| MediaDock batch job management | Replaced by a separate yt-dlp download UI; not equivalent feature parity |

### Original screen coverage

Compared against every file in `desktop-app/dashboard/src/pages`:

| Original screen | Native entry point | Remaining difference |
|---|---|---|
| Create | Reel Studio ▸ Create and Design | Full manual-post/source workflow and template tuning are incomplete |
| Studio | FilmCraft timeline, monitors, properties and export queue | Electron editor-project round-trip is missing |
| Assets | Create ▸ clip search and folder scan; FilmCraft media browser | Asset metadata editor, delete, use-as-logo and original folder management are missing |
| Imports | Create ▸ Download videos | MediaDock job history, health and recovery are not equivalent |
| Videos | Library | Rename/archive/restore built; reopening requires a native snapshot; search covers the loaded recent 40 posts |
| Published | Library publication status | Dedicated published history/filter screen missing |
| Publishing | Publish | Cloud actions exist; local reschedule/cancel/retry and uncertain-result reconciliation UI need work |
| Accounts | Settings connection summary; Publish ▸ cloud accounts | Connect, test, disconnect and full account editor missing |
| Settings | Workspace creation, brand editor and AI provider | Cron/timezone settings, approval policy and full channel settings missing |
| Feeds | Design ▸ existing article picker | Feed CRUD, polling and extraction missing |
| System | Settings folder shortcuts; FilmCraft logs/jobs | Original GPU diagnostics, health, job retry, cleanup and audit views missing |

Reel Studio now has Create, Design, Publish, Library and Settings navigation.
Navigation stays fixed while each page scrolls independently. Layout selection,
extra card fields, AI assistance and captions collapse when not in use. Brand
colours have swatches and hex inputs. Background AI and export results are tied
to the originating project so switching Reels cannot overwrite another draft.
Template slot diagrams indicate geometry, not a rendered preview of fonts, colours
or motion. Actual renders for visual QA are generated by:

```powershell
cargo test -p filmcraft-engine reel_frames -- --ignored
```

They are written to `target/reel-frames/`, not committed. The audit rendered all
21 templates at 1080×1920 using procedural Aurora footage and inspected their
contact sheet. Empty/long/full card structural tests and the engine's template,
card edit, undo, sequence isolation, save/open and MP4 export tests passed. This
does not establish pixel parity with Remotion, motion fidelity at every frame,
or suitability for every user clip/font/logo. Live Instagram, YouTube, cloud
scheduling and real-link downloads still need service acceptance tests.

Future FilmCraft patch merges are described in [UPSTREAM.md](UPSTREAM.md).

