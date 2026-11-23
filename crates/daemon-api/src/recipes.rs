//! Goal-based setup recipes (issue #578): "I want a daily briefing" instead of
//! "make a bot, pick tools, set permissions". A recipe is a versioned data file
//! (`templates/recipes/*.json`, schema in `docs/recipes.md`) that bundles
//!
//! * a bot profile (the same [`SharedProfile`] the profile gallery uses),
//! * the apps it needs, by MCP catalog id,
//! * an optional schedule, and
//! * the permission level, with a plain-language "can / cannot" list per language.
//!
//! Security rules:
//!
//! * A recipe is **data**. Reading one never installs, connects, schedules or
//!   creates anything.
//! * The bot is created through the profile importer, which clamps the recipe's
//!   permissions to the ceiling the user confirmed (never more). The recipe's
//!   `level` must equal the profile's own preset and can never be `full`.
//! * Apps are only *named*; the app asks to add them switched off, scoped to
//!   the new bot, and the user connects and approves them in Apps. Recipes
//!   cannot carry secrets, commands, URLs or folders.
//! * A schedule is created switched off; the user turns it on.
//! * The persona text is shown in full in the preview (it is instructions).

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::profile_share::{
    FORMAT as PROFILE_FORMAT, SharePreset, SharedProfile, clean_text, ident_ok,
};

/// Value of the `format` field.
pub const FORMAT: &str = "praxi-recipe";
/// Schema version written by this build and the only one it reads.
pub const FORMAT_VERSION: u32 = 1;
/// Largest recipe file accepted (bytes).
pub const MAX_FILE_BYTES: usize = 64 * 1024;
/// Languages every recipe must be written in.
pub const LANGS: [&str; 3] = ["en", "tr", "de"];

const MAX_APPS: usize = 4;
const MAX_LINES: usize = 8;

/// A named text per language.
pub type Text = BTreeMap<String, String>;

/// An app the recipe needs, by MCP catalog id. Never a command, URL or secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RecipeApp {
    /// Id in the MCP catalog (`fetch`, `brave-search`, ...).
    pub catalog_id: String,
    /// Why the recipe wants it, per language.
    pub why: Text,
}

/// When a recipe's schedule runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RecipeFreq {
    /// Every day.
    Daily,
    /// Monday to Friday.
    Weekdays,
    /// Once a week on `weekday`.
    Weekly,
}

/// The optional schedule of a recipe. Created switched off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RecipeSchedule {
    /// Title per language.
    pub title: Text,
    /// What the bot is asked to do, per language.
    pub prompt: Text,
    /// How often.
    pub freq: RecipeFreq,
    /// Local time, `HH:MM`.
    pub time: String,
    /// 0 (Sunday) to 6, for `weekly`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekday: Option<u8>,
}

/// A goal-based setup recipe (`*.praxi-recipe.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// Always `praxi-recipe`.
    pub format: String,
    /// Schema version (`1`).
    pub format_version: u32,
    /// Stable slug (`daily-briefing`).
    pub id: String,
    /// lucide icon name, resolved by the app.
    pub icon: String,
    /// Goal in the user's words ("Give me a morning briefing"), per language.
    pub names: Text,
    /// One sentence per language.
    pub descriptions: Text,
    /// Permission level the recipe asks for. Equal to `profile.permissions.preset`; never `full`.
    pub level: SharePreset,
    /// What the bot will be able to do, in plain words, per language.
    pub can: BTreeMap<String, Vec<String>>,
    /// What it will not do without asking, in plain words, per language.
    pub cannot: BTreeMap<String, Vec<String>>,
    /// Apps it needs (added switched off).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<RecipeApp>,
    /// Optional schedule (created switched off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<RecipeSchedule>,
    /// The bot. `meta` and `schedules` stay empty (the recipe has its own).
    pub profile: SharedProfile,
}

fn texts(field: &str, map: &Text, max: usize) -> Result<(), String> {
    for lang in LANGS {
        let v = map
            .get(lang)
            .ok_or_else(|| format!("{field} is missing the {lang} text"))?;
        if v.trim().is_empty() {
            return Err(format!("{field} ({lang}) must not be empty"));
        }
        clean_text(field, v, max, true)?;
    }
    if map.keys().any(|k| !LANGS.contains(&k.as_str())) {
        return Err(format!("{field} has a language other than en, tr, de"));
    }
    Ok(())
}

fn lines(field: &str, map: &BTreeMap<String, Vec<String>>) -> Result<(), String> {
    for lang in LANGS {
        let list = map
            .get(lang)
            .ok_or_else(|| format!("{field} is missing the {lang} list"))?;
        if list.is_empty() || list.len() > MAX_LINES {
            return Err(format!("{field} ({lang}) needs 1 to {MAX_LINES} lines"));
        }
        for l in list {
            if l.trim().is_empty() {
                return Err(format!("{field} ({lang}) has an empty line"));
            }
            clean_text(field, l, 200, false)?;
        }
    }
    if map.keys().any(|k| !LANGS.contains(&k.as_str())) {
        return Err(format!("{field} has a language other than en, tr, de"));
    }
    Ok(())
}

impl Recipe {
    /// Check every field, the embedded profile included.
    ///
    /// # Errors
    /// A short message naming the first offending field.
    pub fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT {
            return Err("this is not a Praxi-Bot recipe file".into());
        }
        if self.format_version != FORMAT_VERSION {
            return Err(format!(
                "recipe format version {} is not supported (this app reads version {FORMAT_VERSION})",
                self.format_version
            ));
        }
        if !ident_ok(&self.id, "-", 40) {
            return Err("id must be a short slug".into());
        }
        if !ident_ok(&self.icon, "-", 32) {
            return Err("icon must be an icon name".into());
        }
        texts("names", &self.names, 80)?;
        texts("descriptions", &self.descriptions, 400)?;
        lines("can", &self.can)?;
        lines("cannot", &self.cannot)?;
        if self.level == SharePreset::Full {
            return Err("a recipe can never ask for the full level".into());
        }
        if self.profile.permissions.preset != Some(self.level) {
            return Err("level must equal profile.permissions.preset".into());
        }
        if self.profile.format != PROFILE_FORMAT {
            return Err("profile is not a Praxi-Bot profile".into());
        }
        self.profile.validate()?;
        if self.profile.meta.is_some() {
            return Err("profile.meta must be left out of a recipe".into());
        }
        if !self.profile.schedules.is_empty() {
            return Err("put the schedule in `schedule`, not in profile.schedules".into());
        }
        if self.apps.len() > MAX_APPS {
            return Err(format!("a recipe may name at most {MAX_APPS} apps"));
        }
        let mut seen = BTreeSet::new();
        for a in &self.apps {
            if !ident_ok(&a.catalog_id, "-", 40) || !seen.insert(a.catalog_id.as_str()) {
                return Err(format!("bad or repeated app {:?}", a.catalog_id));
            }
            texts("apps.why", &a.why, 200)?;
        }
        if let Some(s) = &self.schedule {
            texts("schedule.title", &s.title, 80)?;
            texts("schedule.prompt", &s.prompt, 2_000)?;
            let ok_time = s.time.split_once(':').is_some_and(|(h, m)| {
                h.len() == 2
                    && m.len() == 2
                    && h.parse::<u8>().is_ok_and(|h| h < 24)
                    && m.parse::<u8>().is_ok_and(|m| m < 60)
            });
            if !ok_time {
                return Err("schedule.time must look like 08:00".into());
            }
            match (s.freq, s.weekday) {
                (RecipeFreq::Weekly, Some(d)) if d <= 6 => {}
                (RecipeFreq::Weekly, _) => return Err("a weekly schedule needs weekday 0-6".into()),
                (_, None) => {}
                (_, Some(_)) => return Err("weekday is only for weekly schedules".into()),
            }
        }
        Ok(())
    }
}

/// Parse and validate a recipe file. Reads nothing but the text.
///
/// # Errors
/// Too large, not JSON, wrong format or version, unknown field, failed check.
pub fn parse(text: &str) -> Result<Recipe, String> {
    if text.len() > MAX_FILE_BYTES {
        return Err(format!(
            "the recipe file is larger than {} KiB",
            MAX_FILE_BYTES / 1024
        ));
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not a valid recipe file: {e}"))?;
    match value.get("format").and_then(|f| f.as_str()) {
        Some(FORMAT) => {}
        _ => return Err("this is not a Praxi-Bot recipe file".into()),
    }
    // Check the version before the strict field check so a newer file gets the right message.
    match value
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
    {
        Some(v) if v == u64::from(FORMAT_VERSION) => {}
        Some(v) => {
            return Err(format!(
                "recipe format version {v} is not supported (this app reads version {FORMAT_VERSION})"
            ));
        }
        None => return Err("recipe format_version is missing".into()),
    }
    let recipe: Recipe =
        serde_json::from_value(value).map_err(|e| format!("not a valid recipe file: {e}"))?;
    recipe.validate()?;
    Ok(recipe)
}

const RECIPE_FILES: [&str; 5] = [
    include_str!("../templates/recipes/daily-briefing.json"),
    include_str!("../templates/recipes/inbox-triage.json"),
    include_str!("../templates/recipes/price-watcher.json"),
    include_str!("../templates/recipes/weekly-review.json"),
    include_str!("../templates/recipes/research-assistant.json"),
];

/// The built-in recipes, parsed and validated.
///
/// # Panics
/// Never at run time: a test parses every file, so a broken recipe fails CI.
#[must_use]
pub fn recipes() -> Vec<Recipe> {
    RECIPE_FILES.iter().filter_map(|t| parse(t).ok()).collect()
}

/// The built-in goal recipes (read-only data; nothing is created).
#[utoipa::path(get, path = "/v1/profile-share/recipes", tag = "bots",
    security(("bearer" = [])),
    responses((status = 200, body = Vec<Recipe>)))]
pub(crate) async fn list_recipes() -> Json<Vec<Recipe>> {
    Json(recipes())
}

#[cfg(test)]
#[path = "recipes_tests.rs"]
mod tests;
