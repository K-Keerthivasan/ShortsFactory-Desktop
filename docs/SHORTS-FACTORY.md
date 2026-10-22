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
| Hermes with its own setup (e.g. Codex) | Done — the app runs Hermes with your Hermes provider, model and sign-in (tools off, rules ignored, an empty folder, `HERMES_TIMEOUT_MS`, default 4 minutes). `HERMES_PROVIDER=ollama` sends it to local Ollama instead; when Hermes fails the local Ollama (`OLLAMA_MODEL`) answers unless `AI_FALLBACK=off` |
| One style for the text on every video | Done — Plugin Settings ▸ **Writing** ▸ On-video text style: a style guide for headline, supporting line, credit, caption and hashtags that the AI follows for every card (and the AI workflow) |
| Claude or ChatGPT | Done — Create ▸ Write the card ▸ **Write with Claude or ChatGPT**: Copy prompt (voice, style guide, notes; MCP `studio.writingPrompt`), paste the JSON reply, Use the reply |

**Standard descriptions.** Plugin Settings ▸ **Writing** holds two templates per workspace: the
Instagram description and the YouTube description (the text on the video stays the same on
both). They use `{caption}` `{title}` `{line}` `{credit}` `{cta}` `{hashtags}` `{channel}`;
hashtags already in the text are not repeated, empty fields leave no gaps, and YouTube always
gets `#Shorts`. Standard: Instagram `{caption}` + hashtags; YouTube `{caption}`, credit,
hashtags, `#Shorts`. A preview shows the open Reel's post. MCP: `studio.writing`.

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

## 8b. Thumbnails for every platform

Every video that reaches the library gets thumbnails from its best moment: the app samples
frames across it (skipping the opening and the end card), scores each for sharpness, colour,
contrast and exposure, and enhances the winner (contrast, colour, sharpening) into four sizes:
1280×720 (YouTube, link previews), 1080×1920 (Shorts, Reels, TikTok, Stories), 1080×1350 (feed)
and 1080×1080 (square), each filled with a blurred copy of the frame where its shape differs.
Long videos get their headline set on them in bold type. They are saved beside the video
(`thumbnails/cover-<render>-<size>.jpg`).

Library ▸ a post ▸ **Thumbnail**: the four sizes; scrub the preview and **Use this moment**, or
**Best moment**, with an optional headline. Publishing uses them: **YouTube** gets the image
(`thumbnails.set`, the 9:16 one for Shorts, 16:9 for long videos; a channel must be verified for
custom thumbnails, and a failure never fails the post), **Instagram** the same moment as the
Reel's cover (`thumb_offset`; the Graph API takes a frame of the video, not an image). Cloud
schedules do the same: cloud posts carry the cover moment and cloud YouTube Shorts their
image, which the worker sets after the upload. MCP: `studio.thumbnails`, and
`studio.youtubeQueue` `"thumbnails"` sends them for Shorts already in the cloud.

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
and retried like Instagram ones.

**Pace.** A channel publishes at most `YOUTUBE_PER_DAY` Shorts a day (default 3) at
`YOUTUBE_SLOTS` (default 10:00, 15:00, 20:00): the cloud refuses a booking for a full day and
uploads at most three a day, within 24 hours of their time; Plugin Settings ▸ Cloud ▸ **Keep to
3 a day** moves any extras to the next free times, on YouTube too (this needs the
`youtube.force-ssl` permission: reconnect YouTube once).

**In the cloud (this PC off).** With Cloud publishing chosen, Schedule books the YouTube Short in
the cloud too: the Supabase worker uploads it up to a day and a half ahead with YouTube's own
publish time, at most six uploads in any 24 hours (Google's default 10,000 units a day; a quota
error waits six hours and continues). A Reel already in the cloud Instagram schedule reuses that
stored video; otherwise the video goes to the cloud once, for YouTube only, and is deleted after
the upload. The first booking gives the cloud the channel's key (client ID, secret and refresh
token, encrypted in your own backend). Locally such a publication shows as `CLOUD` until the app
sees the cloud upload it, then as published with its link. Queue ▸ **Move to the cloud** hands
every Short waiting in the local YouTube queue to the cloud, keeping its time. Over MCP:
`studio.youtubeQueue` with `"action":"cloud"` (items), `"toCloud"` or `"sync"`. Backend:
migration `202610180001_youtube.sql` and the `publishing-api` / `publish-due-posts` /
`cleanup-orphans` functions in the cloud project; both quotas share one Google project, so
avoid running the local queue and the cloud for the same channel at once.

**Scheduling.** Schedule with a YouTube account ticked uploads the Short to YouTube straight away
as Private with YouTube's own publish time (`publishAt`); YouTube makes it Public at that time,
whether or not this PC is on, and YouTube Studio lists it as Scheduled (change or cancel it
there). Instagram accounts in the same schedule wait in the library as before (this app or the
earlier app's worker posts them, or use cloud scheduling). The upload needs the app open only
while it runs; the time must be at least five minutes ahead. The Schedule calendar shows the
YouTube ones with a link to YouTube Studio. Over MCP, `studio.schedule` does the same.

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

"Access blocked: … can only be used within its organization" (Error 403: `org_internal`) means
the consent screen's audience is Internal (a Google Workspace project): Audience ▸ **Make
external** (In production), then connect again. If the organisation does not allow it, create the
project under "No organization".

"Error 400: redirect_uri_mismatch" means the OAuth client is a **Web application**; the app
answers Google on a free local port, which only **Desktop app** clients accept. Create a Desktop
app client and paste its ID and secret.

"… has not completed the Google verification process" (Error 403: `access_denied`) means the app
is still in Testing and the account is not a test user: Audience ▸ **Publish app** (a Brand
Account channel has no email to add as a test user). No verification submission is needed.

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
| **Hook first, filler cut** (on by default): the AI also edits each part, opening with its strongest 1–5 seconds as a teaser and leaving out dead air, filler, false starts, repeats, intros and outros; FFmpeg joins the ranges into one clip beside the download (`<name> - edit <start>-<end>.mp4`) and the Reel is built from it | Built — needs the video's captions; the review shows the hook and the seconds cut, untick it (or change the times) for the plain range |
| **One long video (16:9)**: the AI edits the whole video into a tighter long video (at most the minutes chosen), opening with its most gripping 5–15 seconds and leaving out filler, tangents, sponsor reads and long intros/outros; **Make the long video** adds it to the library as a long video (kind VIDEO) for approval, with headline thumbnails; **Open the edit in the editor** puts it in a new 16:9 sequence to polish by hand | Built — YouTube posts it as a normal video (no #Shorts) with its 16:9 thumbnail |
| Review: tick parts, adjust their times and headlines | Built |
| One Reel or Post per part from the same file (`reel.new` with `sourceIn`/`sourceOut`), queued to render into the library | Built |
| Approve, then publish now or schedule, from the same panel | Built |
| The same workflow for agents over MCP (`studio.download`, `studio.plan`, `reel.new`, `studio.sendToLibrary`, …) | Built — see [MCP.md](MCP.md) |

## 11a. AI Reels in Create (the Reels maker)

Create ▸ **AI Reels**: tick **Pick** on workspace clips (trailers and the like; one or a batch,
up to 20) or paste links, choose **Reels per video** (1 or 2) and the length (default 60–90 s),
and **Make AI Reels**. For each video the virality analyzer (below) finds the best part, cuts it
on shot changes with the strongest moment first as the hook, leaves out black frames, logos,
rating cards and slates, and the Reels are made at once as ordinary Reel projects: one clip per
cut on the Footage track (back to back, hook first), framed on the subject for 9:16, inside the
workspace's template with the AI's headline, supporting line, caption and hashtags. Everything
stays editable: trim, slip, move or delete any cut, change the template (the cuts and framing
are kept), rewrite the card. **Add Flick motion scenes** adds Flick's animated overlays to each;
**Render into the library too** (off by default) renders them straight away. Progress shows under
the button; the AI panel keeps the same runs for a closer review. MCP: `reel.new` takes
`cuts` ([[in, out], ...] source seconds, in play order) and `focusX` (0–1).

## 11b. Virality analyzer (local first)

AI panel ▸ **Virality analyzer** (on by default) finds the moments of a downloaded video on
this PC and scores them:

1. **Transcript**: the video's captions, else local **Whisper** (FilmCraft's built-in recogniser
   with word timestamps; **Download Whisper** once, about 290 MB, in the AI panel).
2. **Signals** with **FFmpeg**: loudness every half second and scene cuts.
3. **Candidates**: sentence-aligned windows of the asked length, scored on pace, loudness,
   peaks, cuts and hook words.
4. **Vision**: a 2×2 grid of frames of each candidate goes to a vision-language model, local
   Ollama `gemma3` by default (`VISION_PROVIDER`, `OLLAMA_VISION_MODEL`) or NVIDIA NIM
   (`VISION_PROVIDER=nvidia`, `NVIDIA_API_KEY`, `NVIDIA_VISION_MODEL`, default
   `meta/llama-3.2-90b-vision-instruct`; others on the free catalogue include
   `nvidia/nemotron-3.5-super-vl-120b-a12b`, `google/gemma-4-31b-it` and the video model
   `nvidia/cosmos-reason2-8b`). It rates the pictures and finds the subject for the crop.
5. **Ranking** by the workspace's AI (Hermes/Codex, Ollama, or `AI_PROVIDER=nvidia` with
   `NVIDIA_MODEL`): a 0–100 virality score with hook, emotion, clarity, visual and share parts,
   three suggested hooks, the timestamps and a hook-first edit. Without an AI the signals rank.

The review shows each moment's score, parts and hooks (**Use** puts a hook on the headline);
**Make** builds the Reels from the edit, cropped to 9:16 on the subject, into the editor, where
captions and the GPU export work as before. MCP: `studio.analyze`.

**A score is a prediction, not a guarantee of views.** Reels made with the analyzer carry their
score into the library; AI panel ▸ **4 · Predictions vs results** ▸ Check results lists the
published ones with their real YouTube views, likes and comments (and Instagram's where the
token allows) and the rank correlation between score and views (MCP:
`studio.viralityResults`). Trust it for JKR only once that correlation holds up.

**Klap (optional, paid)**: with `KLAP_API_KEY`, **Compare with Klap** sends the same link to
Klap and lists its shorts with Klap's own scores and explanations; each can be downloaded and
opened as a Reel. Klap does not return source times, so its clips are finished videos rather
than editable ranges. Uses Klap credits. MCP: `studio.klap`.

## 11c. Flick motion scenes (Remotion), merged with the analyzer

[Flick](https://github.com/Creatorberry/flick) (Creatorberry, MIT) turns a video's words into
original animated scenes, one per line, built in Remotion. The app runs Flick's workflow itself
with Flick's own rules, starter project, sound effects and saved animations, fetched once at a
pinned version (no Claude Code plugin needed):

1. **Words**: the analyzer's transcript of the moment, mapped into the Reel's time (the hook
   piece first), or Whisper on the Reel's footage, or pasted lines (`[12.5] text`).
2. **Plan**: the workspace's AI writes Flick's storyboard (step 2's rules): per scene the
   transcript, what is on screen, text, sequence, sound effect and transition. From the
   analyzer, the direction is the moment's best hook and why it scored.
3. **Build**: a coding agent writes one Remotion component per approved scene from the brief and
   scene spec the app writes (`FLICK_AGENT`: `claude`, the default, `codex` or `hermes`;
   `FLICK_TIMEOUT_MS`), then the app renders each scene and its poster.
4. **Onto the Reel**: the scenes go on a new top video track at their times, their sound effects
   on a new audio track, the first time the Reel is open; caption rendering and the GPU export
   stay the editor's.

AI panel: **Add Flick motion scenes** queues this for every Reel the analyzer makes (one job at a
time); **Review Flick's plan first** stops at the plan. Reel Studio ▸ Design ▸ **Motion (Flick)**
shows each Reel's plan to edit and approve, the rendered scenes, **Revise this scene** from
feedback (only that scene), **Preview in Remotion Studio**, and **Save as reusable** (adds the
component to Flick's library for later runs). **Set up Flick** (once) fetches Flick and installs
Remotion and its headless browser in the app's tools folder; it needs Node.js 20+. Each run's
files are in the project's `Motion` folder (`flick-<time>/`: `transcript.json`,
`flick-plan.md`, `remotion-brief.md`, `scene-spec.json`, `remotion/`, `scenes/`). MCP:
`studio.flick` (`status`, `setup`, `run`).

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

