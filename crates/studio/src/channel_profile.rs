//! A workspace's channel profile: what kind of channel it is, the voice the AI writes in, who it
//! speaks to and its Reel defaults. Each workspace has its own, edited in Plugin Settings, so a
//! new channel is configured rather than coded. Stored in the channel's settings JSON under
//! `profile`; a workspace without one reads as the kind its earlier `template` setting implied.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use rusqlite::{OptionalExtension, params};

use crate::{Result, Studio, StudioError, err};

/// The kinds a channel starts from: (id, label, the AI's voice).
pub const KINDS: &[(&str, &str, &str)] = &[
    (
        "story",
        "Stories (gaming, tech, human interest)",
        "Write a short video story: fascinating gaming, technology or human-interest story. Use a factual curiosity hook and short conversational sentences. Prioritize surprise, emotion, recognizable people and visual storytelling. No generic clickbait, fake drama, invented quotes or newspaper language. Label the real source. Never simulate a social-media post.",
    ),
    (
        "business",
        "Business advice",
        "Write for small-business owners, office managers and entrepreneurs. Turn the sourced story into practical business technology advice: hook, what happened, why it matters, what a business should do, optional configured CTA. Be professional and conversational. Distinguish recommendations from reported facts. Avoid aggressive sales language and unsupported promises.",
    ),
    (
        "education",
        "Explainers and how-tos",
        "Write a clear explainer short: open with the question or the surprising fact, then explain it in plain words, one idea per sentence, with a concrete example. Accurate and friendly; no jargon without a quick definition. Never invent facts or numbers.",
    ),
    (
        "creator",
        "Personal / creator (own recordings)",
        "Write in the creator's own first-person voice from what they say in the recording: keep their words and personality, tighten them, lead with the most interesting line. Casual and direct. Never put words in their mouth that they did not say.",
    ),
    ("custom", "Custom (write your own voice)", ""),
];

/// A channel's profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Profile {
    /// One of [`KINDS`].
    pub kind: String,
    /// The voice instruction; empty uses the kind's own.
    pub voice: String,
    /// Who the channel speaks to, for the AI.
    pub audience: String,
    /// Hashtags every caption carries.
    pub hashtags: Vec<String>,
    /// AI Reels' length.
    pub reel_min_seconds: f64,
    pub reel_max_seconds: f64,
}

impl Default for Profile {
    fn default() -> Self {
        Profile { kind: "story".into(), voice: String::new(), audience: String::new(), hashtags: Vec::new(), reel_min_seconds: 60.0, reel_max_seconds: 90.0 }
    }
}

impl Profile {
    /// Reads the profile from a channel's settings JSON.
    pub fn from_settings(settings: &Value) -> Profile {
        match settings.get("profile") {
            Some(p) => serde_json::from_value(p.clone()).unwrap_or_default(),
            // Before profiles, the K2 template meant a business channel.
            None if settings.get("template").and_then(Value::as_str) == Some("K2Short") => Profile { kind: "business".into(), ..Default::default() },
            None => Profile::default(),
        }
    }

    pub fn is_business(&self) -> bool {
        self.kind == "business"
    }

    /// The kind's built-in voice.
    pub fn kind_voice(kind: &str) -> &'static str {
        KINDS.iter().find(|k| k.0 == kind).map(|k| k.2).unwrap_or(KINDS[0].2)
    }

    /// The full instruction the AI writes in: the voice, the audience and the hashtags.
    pub fn instruction(&self) -> String {
        let voice = self.voice.trim();
        let mut parts = vec![if voice.is_empty() { Self::kind_voice(&self.kind).to_string() } else { voice.to_string() }];
        if !self.audience.trim().is_empty() {
            parts.push(format!("The audience: {}.", self.audience.trim().trim_end_matches('.')));
        }
        let tags: Vec<String> = self.hashtags.iter().map(|h| h.trim().trim_start_matches('#')).filter(|h| !h.is_empty()).map(|h| format!("#{h}")).collect();
        if !tags.is_empty() {
            parts.push(format!("Always include these hashtags: {}.", tags.join(" ")));
        }
        parts.retain(|p| !p.is_empty());
        parts.join(" ")
    }

    pub fn check(&self) -> Result<()> {
        if !KINDS.iter().any(|k| k.0 == self.kind) {
            return err("Choose a channel kind");
        }
        if self.kind == "custom" && self.voice.trim().is_empty() {
            return err("A custom channel needs its voice: describe how the AI should write");
        }
        if !(self.reel_min_seconds.is_finite() && self.reel_max_seconds.is_finite() && 5.0 <= self.reel_min_seconds && self.reel_min_seconds <= self.reel_max_seconds && self.reel_max_seconds <= 600.0) {
            return err("Reel length: a minimum of at least 5 s, no more than the maximum (up to 600 s)");
        }
        if self.voice.chars().count() > 4000 || self.audience.chars().count() > 500 {
            return err("The voice is limited to 4000 characters and the audience to 500");
        }
        Ok(())
    }
}

/// A colour theme: (name, accent, secondary, background, card background, band, card text).
pub const THEMES: &[(&str, [&str; 6])] = &[
    ("Red on black", ["#FF2E3F", "#FFFFFF", "#0B0B0E", "#000000", "#7D1420", "#FFFFFF"]),
    ("Electric blue", ["#2E8BFF", "#FFFFFF", "#070B14", "#0A1222", "#123A73", "#FFFFFF"]),
    ("Neon", ["#C04BFF", "#3DF5FF", "#0B0614", "#120A22", "#3A1466", "#FFFFFF"]),
    ("Gold", ["#F5B700", "#FFFFFF", "#0E0C08", "#15120A", "#5C4500", "#FFFFFF"]),
    ("Mint", ["#21D19F", "#FFFFFF", "#06120E", "#0A1A15", "#0D5A44", "#FFFFFF"]),
    ("Clean light", ["#1F6FEB", "#111111", "#F4F5F7", "#FFFFFF", "#DDE3EC", "#111111"]),
    ("Mono", ["#FFFFFF", "#BBBBBB", "#000000", "#0D0D0D", "#333333", "#FFFFFF"]),
];

/// Applies a theme's colours to a brand.
pub fn apply_theme(brand: &mut filmcraft_reel::Brand, theme: &str) -> bool {
    let Some((_, c)) = THEMES.iter().find(|t| t.0 == theme) else { return false };
    brand.primary_color = c[0].into();
    brand.secondary_color = c[1].into();
    brand.background = c[2].into();
    brand.card_background = c[3].into();
    brand.band_color = c[4].into();
    brand.card_text_color = c[5].into();
    true
}

impl Studio {
    /// The channel's profile.
    pub fn profile(&self, channel: i64) -> Result<Profile> {
        Ok(Profile::from_settings(&self.settings_json(channel)?))
    }

    /// Saves the channel's profile, keeping every other setting. The earlier app's `template`
    /// follows the kind (K2Short for business channels).
    pub fn save_profile(&self, channel: i64, profile: &Profile) -> Result<()> {
        profile.check()?;
        let mut c = self.connect()?;
        let tx = c.transaction()?;
        let stored: Option<String> = tx.query_row("SELECT json FROM channel_settings WHERE channel_id = ?1", [channel], |r| r.get(0)).optional()?;
        if stored.is_none() && tx.query_row("SELECT 1 FROM channels WHERE id = ?1", [channel], |_| Ok(())).optional()?.is_none() {
            return err(format!("channel {channel} not found"));
        }
        let mut settings: Value = stored.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}));
        let Some(obj) = settings.as_object_mut() else { return err("Settings must be an object") };
        obj.insert("profile".into(), serde_json::to_value(profile).map_err(|e| StudioError::Other(e.to_string()))?);
        if profile.is_business() {
            obj.insert("template".into(), json!("K2Short"));
        } else if obj.get("template").and_then(Value::as_str) == Some("K2Short") {
            obj.remove("template");
        }
        tx.execute(
            "INSERT INTO channel_settings (channel_id, json) VALUES (?1, ?2) ON CONFLICT(channel_id) DO UPDATE SET json = excluded.json",
            params![channel, settings.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// A new workspace, optionally starting from another's brand and profile (not its logo,
    /// media, accounts or publishing settings).
    pub fn create_workspace_from(&self, slug: &str, name: &str, from: Option<i64>, profile: Option<&Profile>) -> Result<crate::data::Channel> {
        let source = match from {
            Some(id) => Some((self.brand(id)?, self.profile(id)?)),
            None => None,
        };
        let ch = self.create_workspace(slug, name)?;
        if let Some((mut brand, p)) = source {
            brand.channel_name = ch.name.clone();
            brand.logo_path = None;
            self.save_brand(ch.id, &brand)?;
            self.save_profile(ch.id, profile.unwrap_or(&p))?;
        } else if let Some(p) = profile {
            self.save_profile(ch.id, p)?;
        }
        Ok(ch)
    }

    /// Renames a workspace (its folder keeps its slug).
    pub fn rename_workspace(&self, channel: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return err("Use a name of 1–100 characters");
        }
        let c = self.connect()?;
        let n = c.execute("UPDATE channels SET name = ?1, updated_at = ?2 WHERE id = ?3", params![name, crate::clock::now_iso(), channel])?;
        if n == 0 {
            return err(format!("channel {channel} not found"));
        }
        Ok(())
    }
}
