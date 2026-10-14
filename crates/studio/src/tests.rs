use super::*;
use filmcraft_reel::{Card, ReelInfo};
use serde_json::json;

/// The tables Reel Studio touches, as the earlier app's migrations create them.
const SCHEMA: &str = "
CREATE TABLE channels (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL DEFAULT '', updated_at text NOT NULL DEFAULT '', slug text NOT NULL UNIQUE, name text NOT NULL, description text NOT NULL DEFAULT '', enabled integer NOT NULL DEFAULT 1);
CREATE TABLE channel_settings (channel_id integer PRIMARY KEY NOT NULL, json text NOT NULL);
CREATE TABLE application_settings (key text PRIMARY KEY NOT NULL, json text NOT NULL);
CREATE TABLE media_assets (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL DEFAULT '', updated_at text NOT NULL DEFAULT '', channel_id integer, file_path text NOT NULL, proxy_path text, asset_type text NOT NULL, label text, tags text NOT NULL DEFAULT '[]', duration real, width integer, height integer, source text, source_url text, creator text, license text, license_notes text);
CREATE TABLE articles (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL, updated_at text NOT NULL, channel_id integer NOT NULL, feed_id integer, url text NOT NULL, canonical_url text NOT NULL, guid text, fingerprint text NOT NULL, title text NOT NULL, author text, published_at text, site_name text, category text, text text, lead_image text, status text DEFAULT 'DISCOVERED' NOT NULL, error text);
CREATE TABLE story_clusters (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL, updated_at text NOT NULL, channel_id integer NOT NULL, canonical_story_title text NOT NULL, summary text, primary_article_id integer NOT NULL, status text DEFAULT 'CLUSTERED' NOT NULL, score real, score_json text, business_angle text, kind text DEFAULT 'RSS' NOT NULL);
CREATE TABLE story_cluster_members (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL, updated_at text NOT NULL, channel_id integer NOT NULL, cluster_id integer NOT NULL, article_id integer NOT NULL);
CREATE TABLE scripts (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL, updated_at text NOT NULL, channel_id integer NOT NULL, story_id integer NOT NULL, version integer NOT NULL, json text NOT NULL, status text DEFAULT 'DRAFT' NOT NULL, demo integer DEFAULT false NOT NULL);
CREATE UNIQUE INDEX script_version ON scripts (story_id, version);
CREATE TABLE renders (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL, updated_at text NOT NULL, channel_id integer NOT NULL, story_id integer NOT NULL, script_id integer NOT NULL, status text DEFAULT 'QUEUED' NOT NULL, file_path text, thumbnail_path text, duration real, props_json text, error text, approved_at text, demo integer DEFAULT false NOT NULL, name text);
CREATE TABLE social_accounts (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL, updated_at text NOT NULL, channel_id integer NOT NULL, platform text NOT NULL, display_name text NOT NULL, external_account_id text NOT NULL, credentials_reference text NOT NULL, enabled integer DEFAULT true NOT NULL, status text DEFAULT 'UNVERIFIED' NOT NULL, last_verified_at text);
CREATE TABLE publishing_jobs (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL, updated_at text NOT NULL, channel_id integer NOT NULL, render_id integer NOT NULL, social_account_id integer NOT NULL, platform text NOT NULL, status text DEFAULT 'SCHEDULED' NOT NULL, scheduled_at text NOT NULL, started_at text, published_at text, attempt_count integer DEFAULT 0 NOT NULL, remote_id text, remote_url text, remote_state_json text, last_error text);
CREATE UNIQUE INDEX publish_once ON publishing_jobs (render_id, social_account_id);
CREATE TABLE publishing_attempts (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL, updated_at text NOT NULL, channel_id integer NOT NULL, publishing_job_id integer NOT NULL, action text NOT NULL, result_json text NOT NULL);
CREATE TABLE audit_logs (id integer PRIMARY KEY AUTOINCREMENT, created_at text NOT NULL, updated_at text NOT NULL, channel_id integer NOT NULL, action text NOT NULL, entity_id integer, details text DEFAULT '{}' NOT NULL);
CREATE TABLE worker_heartbeats (id text PRIMARY KEY NOT NULL, kind text NOT NULL, last_seen_at text NOT NULL);
";

fn fixture(name: &str) -> Studio {
    let root = std::env::temp_dir().join(format!("reel-studio-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir_all(root.join("storage/channels/jkr/assets/brand")).unwrap();
    std::fs::create_dir_all(root.join("secrets")).unwrap();
    std::fs::write(root.join("storage/channels/jkr/assets/brand/logo.png"), b"png").unwrap();
    std::fs::write(root.join("storage/channels/jkr/assets/clip.mp4"), b"mp4").unwrap();
    std::fs::write(root.join("secrets/instagram-jkr.json"), br#"{"accessToken":"never-logged"}"#).unwrap();
    let c = rusqlite::Connection::open(root.join("data/studio.db")).unwrap();
    c.execute_batch(SCHEMA).unwrap();
    c.execute("INSERT INTO channels (slug, name) VALUES ('jkr', 'JKR')", []).unwrap();
    c.execute("INSERT INTO media_assets (channel_id, file_path, asset_type, label) VALUES (1, 'channels/jkr/assets/brand/logo.png', 'LOGO', 'Logo')", [])
        .unwrap();
    c.execute(
        "INSERT INTO media_assets (channel_id, file_path, asset_type, label, duration) VALUES (1, 'channels/jkr/assets/clip.mp4', 'VIDEO', 'Clip', 12.5)",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO channel_settings (channel_id, json) VALUES (1, ?1)",
        [json!({"primaryColor": "#22c55e", "logoAssetId": 1, "socialHandle": "jkr", "enabledPlatforms": ["INSTAGRAM"], "defaultLayout": "framed"}).to_string()],
    )
    .unwrap();
    c.execute("INSERT INTO social_accounts (created_at, updated_at, channel_id, platform, display_name, external_account_id, credentials_reference) VALUES ('', '', 1, 'INSTAGRAM', 'JKR IG', '1784', 'instagram-jkr.json')", []).unwrap();
    Studio::open(&root).unwrap()
}

fn reel() -> ReelInfo {
    ReelInfo {
        template: "jkr-clean-v2".into(),
        channel_id: Some(1),
        card: Card {
            title: "The play of the season".into(),
            caption: "Round two".into(),
            instagram_caption: "Watch this".into(),
            hashtags: vec!["gaming".into()],
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn reads_channels_brand_media_and_accounts_in_place() {
    let s = fixture("read");
    assert_eq!(s.channels().unwrap().len(), 1);
    let b = s.brand(1).unwrap();
    assert_eq!(b.channel_name, "JKR");
    assert_eq!(b.primary_color, "#22c55e");
    assert_eq!(b.default_layout, "framed");
    assert!(b.logo_path.as_deref().is_some_and(|p| p.ends_with("logo.png")));
    assert_eq!(s.media(1).unwrap().len(), 2);
    let a = s.accounts(1).unwrap();
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].credentials_reference, "instagram-jkr.json");
    assert!(s.secret("../data/studio.db").is_err(), "secrets stay inside the secrets folder");
    assert!(s.secret("instagram-jkr.json").is_ok());
}

#[test]
fn workspace_creation_validates_paths_and_keeps_channels_separate() {
    let s = fixture("create-workspace");
    for slug in ["../escape", "UPPER", "a/b", "a--b", "-bad", "bad-"] {
        assert!(s.create_workspace(slug, "Test").is_err());
    }
    assert!(s.create_workspace("new", "  ").is_err());
    let ch = s.create_workspace("new-channel", "New Channel").unwrap();
    assert_ne!(ch.id, 1);
    assert!(s.storage.join("channels/new-channel/assets/clips").is_dir());
    assert_eq!(s.brand(ch.id).unwrap().channel_name, "New Channel");
    assert!(s.accounts(ch.id).unwrap().is_empty());
    assert!(s.create_workspace("new-channel", "Duplicate").is_err());
}

#[test]
fn saving_brand_preserves_other_settings_and_rejects_invalid_values() {
    let s = fixture("save-brand");
    let before = s.settings_json(1).unwrap();
    let mut brand = s.brand(1).unwrap();
    brand.primary_color = "#123456".into();
    brand.social_handle = "newhandle".into();
    s.save_brand(1, &brand).unwrap();
    let after = s.settings_json(1).unwrap();
    assert_eq!(after["enabledPlatforms"], before["enabledPlatforms"]);
    assert_eq!(after["logoAssetId"], before["logoAssetId"]);
    assert_eq!(s.brand(1).unwrap().primary_color, "#123456");
    brand.card_title_size = f64::NAN;
    assert!(s.save_brand(1, &brand).is_err());
    brand.card_title_size = 56.0;
    brand.primary_color = "invalid".into();
    assert!(s.save_brand(1, &brand).is_err());
    assert_eq!(s.settings_json(1).unwrap(), after);
}

#[test]
fn media_scan_skips_unreadable_videos_and_rejects_stored_path_traversal() {
    let s = fixture("scan-media");
    let dir = s.storage.join("channels/jkr/assets/clips");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("broken.mp4"), b"not video").unwrap();
    assert_eq!(s.scan_media(1).unwrap(), (0, 1));
    assert_eq!(s.media(1).unwrap().len(), 2);
    s.connect().unwrap().execute("UPDATE channels SET slug = '../outside' WHERE id = 1", []).unwrap();
    assert!(s.scan_media(1).is_err());
}

#[test]
fn library_names_archive_and_restore_are_scoped_and_keep_files() {
    let s = fixture("library-actions");
    let video = s.root.join("export.mp4");
    std::fs::write(&video, b"render").unwrap();
    std::fs::write(video.with_extension("fcproj"), b"editable project").unwrap();
    let (post, render) = s.send_to_library(&reel(), &video, None, None, 8.0, false).unwrap();
    let path = s.library(1, 10).unwrap()[0].render.as_ref().unwrap().path.clone().unwrap();
    assert_eq!(std::fs::read(path.with_extension("fcproj")).unwrap(), b"editable project");
    assert!(s.rename_post(999, post.story_id, "Wrong workspace").is_err());
    assert!(s.rename_post(1, post.story_id, "   ").is_err());
    s.rename_post(1, post.story_id, "My saved Reel").unwrap();
    assert_eq!(s.library(1, 10).unwrap()[0].title, "My saved Reel");
    let c = s.connect().unwrap();
    c.execute("INSERT INTO publishing_jobs (created_at, updated_at, channel_id, render_id, social_account_id, platform, status, scheduled_at) VALUES ('', '', 1, ?1, 1, 'INSTAGRAM', 'SCHEDULED', '')", [render]).unwrap();
    assert!(s.archive_post(1, post.story_id).is_err());
    c.execute("UPDATE publishing_jobs SET status = 'CANCELLED'", []).unwrap();
    assert!(s.archive_post(999, post.story_id).is_err());
    s.archive_post(1, post.story_id).unwrap();
    assert!(s.library(1, 10).unwrap().is_empty());
    assert_eq!(s.archived_library(1, 10).unwrap().len(), 1);
    assert!(path.is_file() && path.with_extension("fcproj").is_file());
    assert!(s.restore_post(999, post.story_id).is_err());
    s.restore_post(1, post.story_id).unwrap();
    assert_eq!(s.library(1, 10).unwrap()[0].status, "RENDERED");
    assert!(s.archived_library(1, 10).unwrap().is_empty());
}

#[test]
fn changing_ai_provider_preserves_scheduler_settings() {
    let s = fixture("provider-settings");
    let c = s.connect().unwrap();
    c.execute(
        "INSERT INTO application_settings (key, json) VALUES ('general', ?1)",
        [json!({"aiProvider": "hermes", "timezone": "America/New_York", "publishCron": "* * * * *"}).to_string()],
    )
    .unwrap();
    s.set_ai_provider("ollama").unwrap();
    assert_eq!(ai::provider(&s), "ollama");
    let saved: String = c.query_row("SELECT json FROM application_settings WHERE key = 'general'", [], |r| r.get(0)).unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(saved["publishCron"], "* * * * *");
    assert_eq!(saved["timezone"], "America/New_York");
    assert!(s.set_ai_provider("invalid").is_err());
    assert_eq!(ai::provider(&s), "ollama");
    c.execute("UPDATE application_settings SET json = 'broken' WHERE key = 'general'", []).unwrap();
    assert!(s.set_ai_provider("hermes").is_err());
}

#[test]
fn a_reel_becomes_a_post_with_a_render_then_a_new_version() {
    let s = fixture("library");
    let video = s.root.join("export.mp4");
    std::fs::write(&video, b"video").unwrap();
    let clip = s.storage.join("channels/jkr/assets/clip.mp4");
    let (link, render) = s.send_to_library(&reel(), &video, None, Some((&clip, 1.0, 8.0)), 8.0, false).unwrap();
    let script = s.script(link.script_id, 1).unwrap();
    assert_eq!(script["card"]["title"], "The play of the season");
    assert_eq!(script["layout"], "jkr-clean-v2");
    assert_eq!(script["clip"]["assetId"], 2, "the footage maps back to its library row");
    assert_eq!(script["clip"]["trimEnd"], 9.0);
    let lib = s.library(1, 10).unwrap();
    assert_eq!(lib.len(), 1);
    let r = lib[0].render.as_ref().unwrap();
    assert_eq!((r.id, r.status.as_str(), r.approved), (render, "RENDERED", false));
    assert!(r.path.as_ref().is_some_and(|p| p.is_file()));
    // A second send is a new version of the same post.
    let mut again = reel();
    again.post = Some(link);
    let (link2, _) = s.send_to_library(&again, &video, None, None, 8.0, true).unwrap();
    assert_eq!(link2.story_id, link.story_id);
    assert_ne!(link2.script_id, link.script_id);
    assert_eq!(s.library(1, 10).unwrap().len(), 1);
}

#[test]
fn publishing_is_claimed_once_and_needs_approval() {
    let s = fixture("claim");
    let video = s.root.join("export.mp4");
    std::fs::write(&video, b"video").unwrap();
    let (_, render) = s.send_to_library(&reel(), &video, None, None, 8.0, false).unwrap();
    assert!(s.claim(1, render, 1).is_err(), "not approved yet");
    s.approve(render).unwrap();
    assert_eq!(s.schedule(1, render, &[1], "2000-01-01T00:00:00.000Z").unwrap(), 1);
    assert_eq!(s.due_publications().unwrap().len(), 1);
    let job = s.claim(1, render, 1).unwrap();
    assert!(s.claim(1, render, 1).is_err(), "a second claim while uploading is refused");
    s.finish_publication(job, Ok(("178".into(), Some("https://instagram.com/reel/x".into())))).unwrap();
    assert!(s.claim(1, render, 1).is_err(), "published videos are never published again");
    assert!(s.due_publications().unwrap().is_empty());
    assert_eq!(s.caption_for(render).unwrap(), "Watch this\n\n#gaming");
}

#[test]
fn the_earlier_worker_owns_schedules_while_it_runs() {
    let s = fixture("heartbeat");
    assert!(!s.pipeline_online().unwrap());
    s.connect().unwrap().execute("INSERT INTO worker_heartbeats (id, kind, last_seen_at) VALUES ('w', 'pipeline', ?1)", [now_iso()]).unwrap();
    assert!(s.pipeline_online().unwrap());
    assert!(run_due(&s).unwrap().is_empty());
}

#[test]
fn helpers() {
    assert_eq!(data::fingerprint("The Best of Times, in 2026!"), "2026 best times");
    assert_eq!(ai::extract_json("Sure! ```json\n{\"a\": \"}\"}\n``` done").unwrap(), "{\"a\": \"}\"}");
    assert!(ai::extract_json("no json").is_err());
    assert!(ai::extract_json("{\"unterminated\": 1").is_err());
    assert_eq!(ai::ollama_host(Some("0.0.0.0")), "http://127.0.0.1:11434");
    assert_eq!(ai::ollama_host(Some("http://box:8080")), "http://box:8080");
    assert_eq!(ai::ollama_host(None), "http://127.0.0.1:11434");
    let t = clock::parse_iso("2026-10-14T13:05:09.250Z").unwrap();
    assert_eq!(clock::iso(t), "2026-10-14T13:05:09.250Z");
    assert!(clock::parse_iso("2026-13-40T99:00").is_none());
    let env = read_env(std::path::Path::new("definitely-missing.env"));
    assert!(env.is_empty());
}

#[test]
fn the_script_matches_the_earlier_apps_schema_limits() {
    let mut r = reel();
    r.card.title = "x".repeat(900);
    r.card.bullets = vec!["y".repeat(500); 9];
    r.card.badge = "z".repeat(50);
    let v = data::script_json(&r, None, f64::NAN);
    assert_eq!(v["card"]["title"].as_str().unwrap().chars().count(), 400);
    assert_eq!(v["shortHeadline"].as_str().unwrap().chars().count(), 80);
    assert_eq!(v["bullets"].as_array().unwrap().len(), 4);
    assert_eq!(v["badge"].as_str().unwrap().chars().count(), 24);
    assert_eq!(v["durationTargetSeconds"], 30.0);
    r.card.title = String::new();
    assert_eq!(data::script_json(&r, None, 500.0)["card"]["title"], "Reel");
}

/// Reads (never writes) the real data folder: `cargo test -p filmcraft-studio real -- --ignored --nocapture`.
#[test]
#[ignore = "reads the local Shorts Factory data folder"]
fn real_library_is_readable() {
    let s = Studio::open_default().unwrap();
    for ch in s.channels().unwrap() {
        let b = s.brand(ch.id).unwrap();
        let accounts = s.accounts(ch.id).unwrap();
        println!(
            "{} ({}): accent {}, layout {}, logo {}, {} media, {} posts, accounts: {:?}, AI: {}",
            ch.name,
            ch.slug,
            b.primary_color,
            b.default_layout,
            b.logo_path.is_some(),
            s.media(ch.id).unwrap().len(),
            s.library(ch.id, 500).unwrap().len(),
            accounts
                .iter()
                .map(|a| format!(
                    "{} {} ({}, secret file present: {})",
                    a.platform,
                    a.display_name,
                    a.status,
                    s.secrets.join(&a.credentials_reference).is_file()
                ))
                .collect::<Vec<_>>(),
            ai::provider(&s),
        );
    }
}

#[test]
fn cloud_uploads_only_go_to_the_reservations_own_object() {
    use cloud::valid_upload_target as ok;
    let p = "https://abcd.supabase.co";
    assert!(ok("https://s3.us-west-004.backblazeb2.com/reels/u/1.mp4?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Signature=x", p, "b2", "reels", "u/1.mp4"));
    assert!(!ok("https://s3.us-west-004.backblazeb2.com/reels/u/2.mp4?X-Amz-Algorithm=AWS4-HMAC-SHA256", p, "b2", "reels", "u/1.mp4"), "another object");
    assert!(!ok("https://evil.example/reels/u/1.mp4?X-Amz-Algorithm=AWS4-HMAC-SHA256", p, "b2", "reels", "u/1.mp4"), "another host");
    assert!(!ok("https://user:pw@s3.x.backblazeb2.com/reels/u/1.mp4?X-Amz-Algorithm=AWS4-HMAC-SHA256", p, "b2", "reels", "u/1.mp4"), "credentials in the URL");
    assert!(!ok("http://s3.x.backblazeb2.com/reels/u/1.mp4?X-Amz-Algorithm=AWS4-HMAC-SHA256", p, "b2", "reels", "u/1.mp4"), "not HTTPS");
    assert!(ok("https://abcd.supabase.co/storage/v1/object/upload/sign/reels/u/1.mp4?token=t", p, "supabase", "reels", "u/1.mp4"));
    assert!(!ok("https://other.supabase.co/storage/v1/object/upload/sign/reels/u/1.mp4", p, "supabase", "reels", "u/1.mp4"));
}

#[test]
fn token_health_warns_at_seven_days_three_days_a_day_and_expiry() {
    let at = |hours: i64| clock::iso(clock::now_ms() + hours * 3_600_000);
    assert_eq!(cloud::token_health(Some(&at(24 * 30))).1, 0);
    assert_eq!(cloud::token_health(Some(&at(24 * 6))).1, 1);
    assert_eq!(cloud::token_health(Some(&at(48))).1, 2);
    assert_eq!(cloud::token_health(Some(&at(5))).1, 3);
    assert_eq!(cloud::token_health(Some(&at(-1))).1, 4);
    assert_eq!(cloud::token_health(None).1, 0);
}

#[test]
fn ytdlp_formats_follow_the_options() {
    let o = ytdlp::Options { quality: "720".into(), ..Default::default() };
    assert!(ytdlp::format_for(&o, true).starts_with("bv*[height<=720][ext=mp4]+ba"));
    assert_eq!(ytdlp::format_for(&o, false), "b[height<=720][ext=mp4]/b[height<=720]", "no FFmpeg: single-file only");
    let custom = ytdlp::Options { format: "18".into(), ..Default::default() };
    assert_eq!(ytdlp::format_for(&custom, true), "18");
}

#[test]
fn a_logo_is_copied_registered_once_and_used_by_the_brand() {
    let s = fixture("logo");
    let img = s.root.join("New Logo.png");
    std::fs::write(&img, b"png").unwrap();
    let id = s.set_logo(1, &img).unwrap();
    assert_eq!(s.set_logo(1, &img).unwrap(), id, "setting the same logo again reuses its row");
    let b = s.brand(1).unwrap();
    assert!(b.logo_path.as_deref().is_some_and(|p| p.ends_with("New Logo.png")));
    assert!(s.set_logo(1, &s.root.join("notes.txt")).is_err());
}

#[test]
fn timestamps_with_offsets_parse() {
    let z = clock::parse_iso("2026-12-13T17:00:00Z").unwrap();
    assert_eq!(clock::parse_iso("2026-12-13T17:00:00+00:00"), Some(z));
    assert_eq!(clock::parse_iso("2026-12-13T17:00:00.123456+00:00"), Some(z + 123));
    assert_eq!(clock::parse_iso("2026-12-13T12:00:00-05:00"), Some(z));
    assert!(cloud::token_health(Some("2099-01-01T00:00:00+00:00")).0.ends_with("days left"));
}
