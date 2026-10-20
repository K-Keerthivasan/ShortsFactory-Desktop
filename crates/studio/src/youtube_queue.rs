//! A quota-aware queue of YouTube Shorts.
//!
//! [`add`] books Shorts for a YouTube account at given times: one `SCHEDULED` publishing job each,
//! without touching the post's library status (a post already live on Instagram stays as it is).
//! [`run`] uploads the jobs whose time is within the next day, oldest time first, at most three
//! in any 24 hours (the channel's pace, inside Google's daily quota): a job whose time is still ahead
//! is uploaded Private with YouTube's own publish time, so YouTube makes it Public then even with
//! this PC off; one whose time has come is published at once. When Google reports the quota is
//! used up, the job goes back to waiting and the queue pauses until the quota has had time to
//! refill. The app runs it in the background every few minutes.

use rusqlite::params;

use crate::{Result, Studio, clock, err};

/// Uploads in any 24 hours: the channel's pace of three Shorts a day (well inside Google's
/// default 10,000 units at about 1,600 per upload).
pub const DAILY_UPLOADS: usize = 3;
/// How far ahead of their time Shorts are uploaded.
const AHEAD_MS: i64 = 24 * 3_600_000;
/// How long the queue rests after Google reports the quota is used up.
const QUOTA_REST_MS: i64 = 6 * 3_600_000;

/// Books `items` (render id, UTC ISO time) on the YouTube `account` of `channel`. Already booked
/// or published renders are skipped. Returns how many were added.
pub fn add(studio: &Studio, channel: i64, account: i64, items: &[(i64, String)]) -> Result<usize> {
    let (owner, a) = studio.account(account)?;
    if owner != channel || a.platform != "YOUTUBE" || !a.enabled {
        return err("that is not an enabled YouTube account of this workspace");
    }
    let mut c = studio.connect()?;
    let tx = c.transaction()?;
    let now = clock::now_iso();
    let mut n = 0;
    for (render, at) in items {
        let at = clock::parse_iso(at).map(clock::iso).ok_or_else(|| crate::StudioError::Other(format!("unreadable time for render {render}")))?;
        let ok: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM renders WHERE id = ?1 AND channel_id = ?2 AND status = 'APPROVED' AND approved_at IS NOT NULL AND file_path IS NOT NULL AND demo = 0)",
            params![render, channel],
            |r| r.get(0),
        )?;
        if !ok {
            return err(format!("render {render} is not an approved video of this workspace"));
        }
        n += tx.execute(
            "INSERT OR IGNORE INTO publishing_jobs (created_at, updated_at, channel_id, render_id, social_account_id, platform, status, scheduled_at)
             VALUES (?1, ?1, ?2, ?3, ?4, 'YOUTUBE', 'SCHEDULED', ?5)",
            params![now, channel, render, account, at],
        )?;
    }
    tx.commit()?;
    Ok(n)
}

/// Where the queue stands.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Status {
    /// Shorts waiting to be uploaded.
    pub waiting: usize,
    /// Uploads in the last 24 hours.
    pub uploaded_today: usize,
    /// The next waiting Short's time (UTC ISO).
    pub next: Option<String>,
    /// Resting after a quota error until (UTC ISO).
    pub paused_until: Option<String>,
}

fn paused_until(studio: &Studio) -> Option<i64> {
    studio.app_settings().youtube_paused_until.as_deref().and_then(clock::parse_iso).filter(|t| *t > clock::now_ms())
}

pub fn status(studio: &Studio) -> Result<Status> {
    let c = studio.connect()?;
    let day_ago = clock::iso(clock::now_ms() - 86_400_000);
    let waiting: i64 = c.query_row("SELECT COUNT(*) FROM publishing_jobs WHERE platform = 'YOUTUBE' AND status = 'SCHEDULED'", [], |r| r.get(0))?;
    let uploaded: i64 = c.query_row("SELECT COUNT(*) FROM publishing_jobs WHERE platform = 'YOUTUBE' AND started_at >= ?1", [day_ago], |r| r.get(0))?;
    let next: Option<String> =
        c.query_row("SELECT MIN(scheduled_at) FROM publishing_jobs WHERE platform = 'YOUTUBE' AND status = 'SCHEDULED'", [], |r| r.get(0))?;
    Ok(Status { waiting: waiting as usize, uploaded_today: uploaded as usize, next, paused_until: paused_until(studio).map(clock::iso) })
}

/// Uploads what is due soon, within the quota. Returns a line per upload. Blocking.
pub fn run(studio: &Studio) -> Result<Vec<String>> {
    let mut log = Vec::new();
    if paused_until(studio).is_some() {
        return Ok(log);
    }
    let s = status(studio)?;
    let room = DAILY_UPLOADS.saturating_sub(s.uploaded_today);
    if room == 0 {
        return Ok(log);
    }
    let horizon = clock::iso(clock::now_ms() + AHEAD_MS);
    let jobs: Vec<(i64, i64, i64, i64, String)> = {
        let c = studio.connect()?;
        let mut q = c.prepare(
            "SELECT id, channel_id, render_id, social_account_id, scheduled_at FROM publishing_jobs
             WHERE platform = 'YOUTUBE' AND status = 'SCHEDULED' AND scheduled_at <= ?1 ORDER BY scheduled_at LIMIT ?2",
        )?;
        q.query_map(params![horizon, room as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?.collect::<std::result::Result<_, _>>()?
    };
    for (job, channel, render, account, at) in jobs {
        // The same once-only claim as every publication.
        let claimed = studio.connect()?.execute(
            "UPDATE publishing_jobs SET status = 'UPLOADING', started_at = COALESCE(started_at, ?1), attempt_count = attempt_count + 1, updated_at = ?1 WHERE id = ?2 AND status = 'SCHEDULED'",
            params![clock::now_iso(), job],
        )?;
        if claimed == 0 {
            continue;
        }
        match crate::run_job(studio, job, channel, render, account) {
            Ok(link) => log.push(format!("YouTube: render {render} uploaded for {}{}", clock::local_time(&at), link.map(|l| format!(" ({l})")).unwrap_or_default())),
            Err(e) => {
                let text = e.to_string();
                if text.contains("quotaExceeded") || text.contains("uploadLimitExceeded") || text.contains("rateLimitExceeded") {
                    // Not the post's fault: it waits again, and the queue rests.
                    studio.connect()?.execute(
                        "UPDATE publishing_jobs SET status = 'SCHEDULED', last_error = NULL, updated_at = ?1 WHERE id = ?2",
                        params![clock::now_iso(), job],
                    )?;
                    let mut a = studio.app_settings();
                    a.youtube_paused_until = Some(clock::iso(clock::now_ms() + QUOTA_REST_MS));
                    studio.save_app_settings(&a)?;
                    log.push("YouTube: daily quota used up; the queue continues later".into());
                    break;
                }
                log.push(format!("YouTube: render {render} failed: {text}"));
            }
        }
    }
    Ok(log)
}
