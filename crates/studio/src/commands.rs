//! `studio.*`: the channel library, AI, downloads and publishing as engine commands, so they run
//! from menus' dispatch, the control channel and the MCP server (`command_run`) exactly as the
//! Reel Studio panels run them. [`register`] adds them to the engine at start-up.
//!
//! Every command that publishes or schedules acts on approved renders only, with the same
//! once-only claims as the panels. Keys are never part of any result.

use filmcraft_engine::{CommandSpec, EngineError, Result, Session};
use serde_json::{Value, json};

use crate::Studio;

fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}
fn bad(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}
fn studio() -> Result<Studio> {
    Studio::open_default().map_err(|e| bad(e.to_string()))
}
fn int(p: &Value, k: &str) -> Result<i64> {
    p.get(k).and_then(Value::as_i64).ok_or_else(|| bad(format!("need `{k}`")))
}
fn text<'a>(p: &'a Value, k: &str) -> Result<&'a str> {
    p.get(k).and_then(Value::as_str).ok_or_else(|| bad(format!("need `{k}`")))
}
fn ints(p: &Value, k: &str) -> Vec<i64> {
    p.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default()
}
fn to_value<T: serde::Serialize>(v: T) -> Result<Value> {
    serde_json::to_value(v).map_err(|e| bad(e.to_string()))
}
fn err(e: crate::StudioError) -> EngineError {
    bad(e.to_string())
}

fn spec(id: &'static str, label: &'static str, params: &'static str, journal: bool, run: fn(&mut Session, &Value) -> Result<Value>) -> CommandSpec {
    CommandSpec { id, label, menu: &[], shortcut: None, params, enabled: always, run, journal }
}

/// Adds the `studio.*` commands to the engine. Call once at start-up, before any command runs.
pub fn register() {
    filmcraft_engine::host::register([
        spec("studio.archivePost", "Archive Superseded Library Post", r#"{"channel":n,"story":n} keeps files/history; rejects active jobs"#, true, |_, p| {
            Ok(json!({"previousStatus":studio()?.archive_post(int(p,"channel")?,int(p,"story")?).map_err(err)?}))
        }),
        spec(
            "studio.workflowWorkerFailure",
            "Record Native Workflow Worker Failure",
            r#"{"channel":n,"asset":n} runtime panic guard; holds an interrupted item for reconciliation"#,
            true,
            |_, p| {
                let s = studio()?;
                s.connect().map_err(err)?.execute("UPDATE editorial_workflow SET state='render_failed',error='native worker failed unexpectedly; reconcile before retrying' WHERE channel=?1 AND asset=?2 AND state='rendering'",rusqlite::params![int(p,"channel")?,int(p,"asset")?]).map_err(|e|bad(e.to_string()))?;
                crate::workflow::status(&s, int(p, "channel")?, int(p, "asset")?).map_err(err)
            },
        ),
        spec(
            "studio.workflowRevise",
            "Revise Reviewed Undelivered Edit",
            "strict Editorial object; preserves prior completed render, rejects delivered/uncertain items",
            true,
            |_, p| {
                let plan = serde_json::from_value(p.clone()).map_err(|e| bad(format!("invalid editorial: {e}")))?;
                crate::workflow::revise(&studio()?, plan).map_err(err)
            },
        ),
        spec(
            "studio.workflowSuggest",
            "Draft with Local Provider",
            "Editorial evidence object; returns constrained copy for review, never persists or publishes",
            false,
            |_, p| {
                let input = serde_json::from_value(p.clone()).map_err(|e| bad(format!("invalid evidence: {e}")))?;
                crate::workflow::suggest(&studio()?, input).map_err(err)
            },
        ),
        spec("studio.workflowInventory", "Workflow Inventory", r#"{"channel":n} reconciles cloud before returning candidates"#, false, |_, p| {
            crate::workflow::inventory(&studio()?, int(p, "channel")?).map_err(err)
        }),
        spec(
            "studio.workflowPlan",
            "Save Reviewed Editorial Plan",
            "strict Editorial object; source_in_ticks/source_out_ticks are integer ticks at 254016000000/s",
            true,
            |_, p| {
                let plan = serde_json::from_value(p.clone()).map_err(|e| bad(format!("invalid editorial: {e}")))?;
                crate::workflow::save(&studio()?, plan).map_err(err)
            },
        ),
        spec(
            "studio.workflowEvidence",
            "Correct Source Evidence",
            r#"{"channel":n,"asset":n,"evidence_url":str,"evidence_note":str}; audits reference changes, preserves rendered text/trim/delivery"#,
            true,
            |_, p| {
                crate::workflow::evidence(&studio()?, int(p, "channel")?, int(p, "asset")?, text(p, "evidence_url")?, text(p, "evidence_note")?).map_err(err)
            },
        ),
        spec(
            "studio.workflowVerify",
            "Verify Encoded Workflow Media",
            r#"{"channel":n,"asset":n} native decode of first/last frames and audio; records dimensions/duration/size"#,
            false,
            |_, p| crate::workflow::verify(&studio()?, int(p, "channel")?, int(p, "asset")?).map_err(err),
        ),
        spec("studio.workflowStatus", "Workflow Status", r#"{"channel":n,"asset":n}"#, false, |_, p| {
            crate::workflow::status(&studio()?, int(p, "channel")?, int(p, "asset")?).map_err(err)
        }),
        spec(
            "studio.workflowRender",
            "Render Workflow Item",
            r#"{"channel":n,"asset":n} native render, blocks; use a dedicated headless session"#,
            true,
            |s, p| crate::workflow::render(&studio()?, int(p, "channel")?, int(p, "asset")?, s).map_err(err),
        ),
        spec(
            "studio.workflowSchedule",
            "Schedule Workflow Item",
            r#"{"channel":n,"asset":n,"account":str,"at":UTC ISO Z} verifies remote storage and job"#,
            true,
            |_, p| crate::workflow::schedule(&studio()?, int(p, "channel")?, int(p, "asset")?, text(p, "account")?, text(p, "at")?).map_err(err),
        ),
        spec("studio.workspaces", "Workspaces", "{}", false, |_, _| to_value(studio()?.channels().map_err(err)?)),
        spec("studio.downloaderCheck", "Check Downloader", "{}", false, |_, _| {
            let s = studio()?;
            Ok(json!({"version": crate::ytdlp::version(&s).map_err(err)?, "ffmpeg": crate::ytdlp::ffmpeg(&s)}))
        }),
        spec("studio.downloaderInstall", "Install Downloader", "{} (downloads official yt-dlp release)", true, |_, _| {
            let s = studio()?;
            let path = crate::ytdlp::install(&s).map_err(err)?;
            Ok(json!({"path": path, "version": crate::ytdlp::version(&s).map_err(err)?}))
        }),
        spec("studio.workspaceFolder", "Workspace Folder", r#"{"channel":n}"#, false, |_, p| {
            to_value(studio()?.workspace_folder(int(p, "channel")?).map_err(err)?)
        }),
        spec("studio.removeClip", "Remove Clip from Create", r#"{"channel":n,"asset":n} (keeps file)"#, true, |_, p| {
            studio()?.remove_clip(int(p, "channel")?, int(p, "asset")?).map_err(err)?;
            Ok(json!({"removed": true, "fileKept": true}))
        }),
        spec("studio.restoreClips", "Restore Removed Clips", r#"{"channel":n}"#, true, |_, p| {
            to_value(studio()?.restore_clips(int(p, "channel")?).map_err(err)?)
        }),
        spec("studio.brand", "Workspace Brand", r#"{"channel":n}"#, false, |_, p| to_value(studio()?.brand(int(p, "channel")?).map_err(err)?)),
        spec("studio.media", "Workspace Media", r#"{"channel":n,"filter":str?}"#, false, |_, p| {
            let filter = p.get("filter").and_then(Value::as_str).unwrap_or("").to_lowercase();
            let rows = studio()?.media(int(p, "channel")?).map_err(err)?;
            to_value(rows.into_iter().filter(|m| filter.is_empty() || m.label.to_lowercase().contains(&filter)).collect::<Vec<_>>())
        }),
        spec("studio.library", "Workspace Library", r#"{"channel":n,"limit":n?=40}"#, false, |_, p| {
            let limit = p.get("limit").and_then(Value::as_u64).unwrap_or(40).min(500) as usize;
            to_value(studio()?.library(int(p, "channel")?, limit).map_err(err)?)
        }),
        spec("studio.accounts", "Publishing Accounts", r#"{"channel":n}"#, false, |_, p| {
            // Account rows carry the key file's name only, never its contents.
            to_value(studio()?.accounts(int(p, "channel")?).map_err(err)?)
        }),
        spec("studio.health", "Check Connections", "{}", false, |_, _| {
            let report = crate::settings::health_check(&studio()?);
            Ok(json!({"checks": report.lines.iter().map(|(l, ok)| json!({"ok": ok, "message": l})).collect::<Vec<_>>()}))
        }),
        spec("studio.writeCard", "Write a Card with AI", r#"{"channel":n,"text":str (notes, transcript or article),"source":str?}"#, false, |_, p| {
            let s = studio()?;
            let card = crate::ai::write_card(&s, int(p, "channel")?, text(p, "text")?, p.get("source").and_then(Value::as_str).unwrap_or("Uploaded video"))
                .map_err(err)?;
            to_value(card)
        }),
        spec(
            "studio.download",
            "Download a Video",
            r#"{"channel":n,"url":str,"notes":str?,"noDraft":bool? (skip the one-card draft; for studio.plan),"options":{"quality":"best|1080|720|480","format":str?,"cookiesFrom":str?,"rateLimit":str?,"sectionStart":str?,"sectionEnd":str?}?} (blocks until done: download, checks, library, AI draft)"#,
            true,
            |_, p| {
                let s = studio()?;
                let options = p.get("options").cloned().map(serde_json::from_value).transpose().map_err(|e| bad(format!("options: {e}")))?.unwrap_or_default();
                let job = crate::ytdlp::Import {
                    url: text(p, "url")?.to_string(),
                    notes: p.get("notes").and_then(Value::as_str).unwrap_or("").to_string(),
                    options,
                    no_draft: p.get("noDraft").and_then(Value::as_bool).unwrap_or(false),
                    save_to: None,
                    ..Default::default()
                };
                let done = crate::ytdlp::run(&s, int(p, "channel")?, job, &|_| {});
                if let Some(e) = &done.error {
                    return Err(bad(e.clone()));
                }
                to_value(done)
            },
        ),
        spec(
            "studio.plan",
            "Choose Reels in a Video with AI",
            r#"{"channel":n,"url":str,"duration":f64? (of the downloaded file; default the video's),"wants":{"mode":"auto|single|multiple","maxReels":n=3,"minSeconds":15,"maxSeconds":60,"notes":str}?} → segments [{start,end,card,why}] (blocks); then reel.new with sourceIn/sourceOut per segment"#,
            false,
            |_, p| {
                let s = studio()?;
                let wants: crate::autopilot::Wants =
                    p.get("wants").cloned().map(serde_json::from_value).transpose().map_err(|e| bad(format!("wants: {e}")))?.unwrap_or_default();
                let meta = crate::ytdlp::analyze(&s, text(p, "url")?, &crate::ytdlp::Options::default()).map_err(err)?;
                let duration = p.get("duration").and_then(Value::as_f64).or_else(|| meta.get("duration").and_then(Value::as_f64)).unwrap_or(0.0);
                let (segments, fallback) = crate::autopilot::plan(&s, int(p, "channel")?, &meta, duration, &wants);
                Ok(json!({"segments": to_value(segments)?, "fallback": fallback, "title": meta.get("title")}))
            },
        ),
        spec(
            "studio.sendToLibrary",
            "Render and Send to the Library",
            r#"{"approve":bool?=false,"preset":str?} — renders the active Reel (blocks), adds it to its workspace's library and links the post"#,
            true,
            send_to_library,
        ),
        spec("studio.approve", "Approve a Render", r#"{"render":n}"#, true, |_, p| {
            studio()?.approve(int(p, "render")?).map_err(err)?;
            Ok(json!({"approved": true}))
        }),
        spec("studio.publishNow", "Publish Now", r#"{"channel":n,"render":n,"accounts":[n]} (blocks until each platform answers)"#, true, |_, p| {
            let s = studio()?;
            let (ch, render) = (int(p, "channel")?, int(p, "render")?);
            let accounts = ints(p, "accounts");
            if accounts.is_empty() {
                return Err(bad("need `accounts`"));
            }
            let results: Vec<Value> = accounts
                .iter()
                .map(|a| match crate::publish_now(&s, ch, render, *a) {
                    Ok(link) => json!({"account": a, "ok": true, "link": link}),
                    Err(e) => json!({"account": a, "ok": false, "error": e.to_string()}),
                })
                .collect();
            Ok(json!({"results": results}))
        }),
        spec(
            "studio.schedule",
            "Schedule on the Local Queue",
            r#"{"channel":n,"render":n,"accounts":[n],"at":"ISO UTC or local YYYY-MM-DD HH:MM"}"#,
            true,
            |_, p| {
                let s = studio()?;
                let at = text(p, "at")?;
                let at =
                    if at.ends_with('Z') || at.contains('+') { at.to_string() } else { crate::local_to_utc_iso(at).ok_or_else(|| bad("unreadable `at`"))? };
                // YouTube accounts are uploaded now and published by YouTube at `at`; others wait for it.
            let done = crate::schedule_everywhere(&s, int(p, "channel")?, int(p, "render")?, &ints(p, "accounts"), &at).map_err(err)?;
            let youtube: Vec<Value> = done
                .youtube
                .iter()
                .map(|(a, r)| match r {
                    Ok(link) => json!({"account": a, "ok": true, "link": link}),
                    Err(e) => json!({"account": a, "ok": false, "error": e}),
                })
                .collect();
            Ok(json!({"scheduled": done.waiting, "youtube": youtube, "at": at}))
            },
        ),
        spec(
            "studio.cloudSchedule",
            "Schedule in the Cloud",
            r#"{"channel":n,"render":n,"account":"cloud account id","at":"ISO UTC or local YYYY-MM-DD HH:MM","now":bool?} — uploads to B2 and confirms with Supabase (blocks)"#,
            true,
            |_, p| {
                let s = studio()?;
                let (ch, render) = (int(p, "channel")?, int(p, "render")?);
                let now = p.get("now").and_then(Value::as_bool).unwrap_or(false);
                let at = match p.get("at").and_then(Value::as_str) {
                    Some(a) if a.ends_with('Z') || a.contains('+') => a.to_string(),
                    Some(a) => crate::local_to_utc_iso(a).ok_or_else(|| bad("unreadable `at`"))?,
                    None if now => crate::now_iso(),
                    None => return Err(bad("need `at`")),
                };
                let caption = s.caption_for(render).map_err(err)?;
                crate::cloud::schedule(&s, ch, render, text(p, "account")?, &caption, &at, now).map_err(err)
            },
        ),
        spec(
            "studio.cloud",
            "Cloud Publishing Call",
            r#"{"action":"accounts|listSchedules|getSchedule|cancelPost|publishNow|retryPost|reschedulePost|instagramStatus|extendInstagramToken|history", …params}"#,
            true,
            |_, p| {
                let action = text(p, "action")?.to_string();
                let allowed = [
                    "accounts",
                    "listSchedules",
                    "getSchedule",
                    "cancelPost",
                    "publishNow",
                    "retryPost",
                    "reschedulePost",
                    "instagramStatus",
                    "extendInstagramToken",
                    "history",
                    "reconcile",
                    "deleteDraft",
                ];
                if !allowed.contains(&action.as_str()) {
                    return Err(bad(format!("`{action}` is not a cloud action")));
                }
                let mut body = p.clone();
                if let Some(o) = body.as_object_mut() {
                    o.remove("action");
                }
                crate::cloud::call(&studio()?, &action, body).map_err(err)
            },
        ),
        spec("studio.tokenInfo", "Instagram Token Expiry", r#"{"account":n} → {valid,expiresAt,kind,text} (never the token)"#, false, |_, p| {
            Ok(crate::meta_token::summary(&studio()?.token_info(int(p, "account")?).map_err(err)?))
        }),
        spec(
            "studio.extendToken",
            "Extend an Instagram Token",
            r#"{"account":n} — long-lived, or the Page's never-expiring token; the old one is kept on any failure"#,
            true,
            |_, p| Ok(crate::meta_token::summary(&studio()?.extend_account_token(int(p, "account")?).map_err(err)?)),
        ),
        spec(
            "studio.youtubeQueue",
            "YouTube Upload Queue",
            r#"{"action":"status"|"run"|"add"|"cloud"|"toCloud"|"sync","channel":n?,"account":n?,"items":[{"render":n,"at":"ISO UTC"}]?} — books Shorts at times, then uploads them ahead within Google's daily quota; "cloud" books items in the cloud instead, "toCloud" moves the waiting queue there, "sync" records cloud uploads"#,
            true,
            |_, p| {
                let s = studio()?;
                match text(p, "action")? {
                    "status" => {}
                    "run" => {
                        let log = crate::youtube_queue::run(&s).map_err(err)?;
                        let st = crate::youtube_queue::status(&s).map_err(err)?;
                        return Ok(json!({"log": log, "waiting": st.waiting, "uploadedToday": st.uploaded_today, "next": st.next, "pausedUntil": st.paused_until}));
                    }
                    "add" => {
                        let items: Vec<(i64, String)> = p
                            .get("items")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(|i| Some((i.get("render")?.as_i64()?, i.get("at")?.as_str()?.to_string()))).collect())
                            .unwrap_or_default();
                        let n = crate::youtube_queue::add(&s, int(p, "channel")?, int(p, "account")?, &items).map_err(err)?;
                        return Ok(json!({"added": n}));
                    }
                    "cloud" => {
                        let (channel, account) = (int(p, "channel")?, int(p, "account")?);
                        let mut out = Vec::new();
                        for i in p.get("items").and_then(Value::as_array).into_iter().flatten() {
                            let render = i.get("render").and_then(Value::as_i64).ok_or_else(|| bad("items need render"))?;
                            let at = i.get("at").and_then(Value::as_str).ok_or_else(|| bad("items need at"))?;
                            out.push(match crate::cloud_youtube::schedule(&s, channel, render, account, at) {
                                Ok(v) => json!({"render": render, "id": v.get("id"), "status": v.get("status"), "at": v.get("scheduled_for")}),
                                Err(e) => json!({"render": render, "error": e.to_string()}),
                            });
                        }
                        return Ok(json!({"booked": out}));
                    }
                    "toCloud" => return Ok(json!({"log": crate::cloud_youtube::move_queue(&s).map_err(err)?})),
                    "sync" => return Ok(json!({"changed": crate::cloud_youtube::sync(&s).map_err(err)?, "waiting": crate::cloud_youtube::waiting(&s).map_err(err)?})),
                    other => return Err(bad(format!("unknown action `{other}`"))),
                }
                let st = crate::youtube_queue::status(&s).map_err(err)?;
                Ok(json!({"waiting": st.waiting, "uploadedToday": st.uploaded_today, "next": st.next, "pausedUntil": st.paused_until}))
            },
        ),
        spec("studio.calendar", "Publishing Calendar", r#"{"channel":n?,"from":"ISO","to":"ISO"}"#, false, |_, p| {
            let channel = p.get("channel").and_then(Value::as_i64);
            to_value(studio()?.calendar(channel, text(p, "from")?, text(p, "to")?).map_err(err)?)
        }),
    ]);
}

/// Renders the active Reel with Export's quick export, then sends it to the library.
fn send_to_library(s: &mut Session, p: &Value) -> Result<Value> {
    let st = studio()?;
    let info = filmcraft_engine::reel::current(s).ok_or_else(|| bad("the active sequence is not a Reel"))?;
    // Renders go to the project's own folder (beside the project file), not the app's.
    let dir = crate::project_folder(s.path.as_deref(), "Renders").map_err(err)?;
    let seq = s.state.active_sequence.ok_or(EngineError::NoSequence)?;
    let name: String = s
        .project
        .item(seq)
        .map(|i| i.name.clone())
        .unwrap_or_else(|| "Reel".into())
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .take(60)
        .collect();
    let path = dir.join(format!("{name}-{}.mp4", crate::now_iso().replace([':', '.'], "-")));
    let preset = p.get("preset").and_then(Value::as_str).unwrap_or(if info.format == "post" {
        "Match Source – Adaptive High Bitrate"
    } else {
        "Social Vertical 1080×1920"
    });
    s.execute("export.quick", json!({"preset": preset, "path": path.to_string_lossy(), "range": "entire", "wait": true}))?;
    let thumb = path.with_extension("png");
    let _ = s.execute("file.exportFrame", json!({"path": thumb.to_string_lossy(), "format": "png", "time": filmcraft_reel::secs(1.0).0}));
    let duration = s.active_sequence().map_or(0.0, |q| q.duration().seconds());
    let footage = info.main_clip.and_then(|c| {
        let q = s.active_sequence()?;
        let (_, it) = q.find_item(filmcraft_engine::project::ClipId(c))?;
        let media = s.project.item(it.item)?.as_media()?;
        match &media.media {
            filmcraft_engine::project::MediaRef::File { path } => Some((std::path::PathBuf::from(path), it.source_in.seconds(), it.duration.seconds())),
            _ => None,
        }
    });
    let approve = p.get("approve").and_then(Value::as_bool).unwrap_or(false);
    let f = footage.as_ref().map(|(p, a, b)| (p.as_path(), *a, *b));
    let (link, render) = st.send_to_library(&info, &path, thumb.is_file().then_some(thumb.as_path()), f, duration, approve).map_err(err)?;
    s.execute("reel.linkPost", json!({"post": link}))?;
    Ok(json!({"post": link, "render": render, "file": path.to_string_lossy(), "approved": approve}))
}
