---
name: shorts-factory-edit
description: Edit a video to a finished, high-quality short (Reel / Short / TikTok) or long video in the user's running Shorts Factory Desktop app (FilmCraft) through its MCP server, the way a professional editor would - story, cuts, framing, captions, design, sound and colour - on the real timeline the user can watch and refine. Use when the user asks to "edit this video", "make a quality edit", "cut my recording", or to work on the open Shorts Factory project.
---

# Shorts Factory quality edit

You edit in the user's **running Shorts Factory Desktop app** through the `shorts-factory` MCP
server. Everything you do lands on the real timeline: the user watches it happen and can change
anything afterwards, so build a clean, well-organised edit, never a flattened render.

## The tools

- `command_list` – find editor commands (filter by a word: `captions`, `transcript`, `reel`,
  `effects`, `lumetri`, `essentialSound`, `graphics`, `sequence`, `clip`, `trim` …) and their params.
- `command_run` / `command_batch` – run one command or several in order (`{"id": …, "params": …}`).
- `project_inspect`, `sequence_inspect` – the project's items and the open sequence's tracks/clips.
- `render_frame`, `render_preview` – look at the result; `ui_screenshot` – see the app.
- `doc_inspect` – the command reference. `studio.*` commands reach the workspace library.

Read before writing: inspect the project and sequence first, and check a command's params with
`command_list` before using it for the first time.

## The process (stop at each ★ for the user's go-ahead)

1. **Brief.** Ask what the video is, who it is for, the platform (Reel/Short 9:16, Post 4:5, long
   16:9), target length, the workspace (brand) and any style wishes, unless already given.
2. **Watch.** Import or locate the footage (`project_inspect`). Transcribe it
   (`transcript.generate`), read it (`transcript.inspect`), and look at frames across it
   (`render_frame`) to know what is on screen.
3. ★ **Cut plan.** Propose: the hook (the strongest 1–3 seconds or line, first), the story beats
   in order with source timecodes, what goes (silences, fillers, retakes, tangents, dead air),
   the target length, the framing, and the look (captions style, titles, music, colour). Wait.
4. **Rough cut.** Build it in a dedicated sequence: a Reel with the workspace template
   (`reel.new` with `cuts` – one clip per cut, hook first – and `focusX` for the subject) or a
   plain sequence for long form. Remove filler words (`transcript.removeFillers`), lift dead air
   and retakes (`transcript.lift`, razor/trim/ripple delete). Keep every cut a separate clip.
5. **Framing.** Keep the subject in the 9:16 frame on every cut; add punch-ins (slow 100→110 %
   scale keyframes, `effects.setKeyframe` on Motion) on key lines and reactions, never constant
   zoom. Cut on action or on the beat of speech.
6. **Captions.** Burned-in, readable captions from the transcript: a captions track
   (`captions.*`), 1–3 words per caption for shorts, styled in the brand (`captions.setStyle`),
   kept clear of the template's headline area and platform UI (bottom ~20 % and right edge).
7. **Design.** The workspace template's card (`reel.setCard`: headline = the hook in a few
   words), titles or lower thirds where they help (`graphics.*`, Essential Graphics templates),
   the logo. Optional Flick motion scenes (`studio.flick`) for animated explanations.
8. **Sound.** Even loudness (`sequence.normalizeMixTrack`, about −14 LUFS for social), dialogue
   clear, music under speech with ducking (`essentialSound.generateDucking`), sound effects only
   where something visible happens, no clipping.
9. **Colour.** A light, consistent grade (`lumetri.*`, LUTs): exposure and white balance first,
   then contrast and saturation; skin tones natural.
10. **Review.** Render frames at the hook, every cut and the end (`render_frame`) and a short
    preview (`render_preview`); check text readability, framing, jump cuts, audio levels and the
    first 3 seconds. Fix what you find, then report what you did.
11. ★ **Deliver.** On the user's go-ahead: export with the right preset, or send it to the
    workspace library (`studio.sendToLibrary`) for approval and publishing. Never publish.

## Quality bar

- The first 2 seconds earn the next 10: open on the payoff, a question or the most striking shot.
- Pace: no shot longer than about 4 s without a reason (speech, a reveal); remove every pause
  longer than about 0.4 s in talking-head footage unless it lands a beat.
- Every caption and title is readable on a phone, spelled right, and never covers a face.
- Nothing is invented: names, facts and quotes come from the footage, the transcript or the user.
- Leave the project tidy: named sequences, tracks used for their purpose, no stray clips.
- Say plainly what you could not do and why.
