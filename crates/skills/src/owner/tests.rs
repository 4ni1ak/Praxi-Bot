use std::io::Write;
use std::sync::{Arc, Barrier};

use super::*;

fn markdown(name: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: Owner skill\n---\n{body}\n")
}

fn archive(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data, mode) in entries {
        zip.start_file(
            *name,
            zip::write::SimpleFileOptions::default()
                .system(zip::System::Unix)
                .compression_method(zip::CompressionMethod::Stored)
                .unix_permissions(*mode),
        )
        .unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

#[test]
fn owner_create_update_delete_preserves_extras_and_versions() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir_all(source.join("references")).unwrap();
    fs::write(
        source.join(SKILL_FILE),
        markdown("hello", "Original instructions"),
    )
    .unwrap();
    fs::write(source.join("references/guide.txt"), b"reference data").unwrap();
    let prepared = PreparedSkill::folder(&source).unwrap();
    assert_eq!(prepared.file_count(), 2);
    let store = OwnerSkillStore::new(dir.path().join("skills"));
    let created = store.create(&prepared).unwrap();
    assert_eq!(created.source, Source::User);
    assert_eq!(created.version, "0.1.0");
    assert_eq!(store.create(&prepared).unwrap_err(), OwnerError::Conflict);
    let updated = PreparedSkill::markdown(&markdown("hello", "New instructions")).unwrap();
    assert_eq!(
        store.update("hello", &updated, Some("0.0.0")).unwrap_err(),
        OwnerError::Conflict
    );
    let result = store.update("hello", &updated, Some("0.1.0")).unwrap();
    assert_eq!(result.version, "0.1.1");
    assert_eq!(result.body, "New instructions");
    assert_eq!(
        fs::read(dir.path().join("skills/hello/references/guide.txt")).unwrap(),
        b"reference data"
    );
    store.delete("hello").unwrap();
    assert!(!dir.path().join("skills/hello").exists());
    assert_eq!(store.delete("hello").unwrap_err(), OwnerError::NotFound);
}

#[test]
fn simultaneous_owner_creates_have_exactly_one_success() {
    let dir = tempfile::tempdir().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let root = dir.path().join("skills");
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let skill = PreparedSkill::markdown(&markdown("one", "Complete body")).unwrap();
                barrier.wait();
                OwnerSkillStore::new(root).create(&skill).map(|_| ())
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| **r == Err(OwnerError::Conflict))
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("skills/one/SKILL.md")).unwrap(),
        markdown("one", "Complete body")
    );
}

#[test]
fn zip_import_accepts_inert_extras_and_rejects_unsafe_paths_and_modes() {
    let md = markdown("hello", "Instructions");
    let positive = archive(&[
        (SKILL_FILE, md.as_bytes(), 0o644),
        ("refs/data.txt", b"data", 0o644),
    ]);
    let loaded = PreparedSkill::zip(&positive).unwrap();
    assert_eq!(loaded.name(), "hello");
    assert_eq!(loaded.file_count(), 2);
    for bad in [
        "../outside",
        "/absolute",
        "C:/drive",
        "x\\escape",
        "refs/../escape",
        "refs/CON.txt",
        ".env",
        "hidden.",
        "x:stream",
    ] {
        let bytes = archive(&[(SKILL_FILE, md.as_bytes(), 0o644), (bad, b"unsafe", 0o644)]);
        assert!(
            matches!(PreparedSkill::zip(&bytes), Err(OwnerError::Invalid)),
            "{bad}"
        );
    }
    let bytes = archive(&[
        (SKILL_FILE, md.as_bytes(), 0o644),
        ("script.sh", b"do not run", 0o755),
    ]);
    // Preserve the tested metadata on Windows too; its native ZIP origin
    // otherwise discards the executable permission bits before our reader.
    let mut fixture = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    assert_eq!(
        fixture.by_name("script.sh").unwrap().unix_mode().unwrap() & 0o777,
        0o755
    );
    assert!(matches!(
        PreparedSkill::zip(&bytes),
        Err(OwnerError::Invalid)
    ));
}

#[test]
fn zip_import_rejects_case_collisions_and_file_directory_collisions() {
    let md = markdown("hello", "Instructions");
    for entries in [
        vec![
            (SKILL_FILE, md.as_bytes(), 0o644),
            ("REF.txt", b"one".as_slice(), 0o644),
            ("ref.txt", b"two".as_slice(), 0o644),
        ],
        vec![
            (SKILL_FILE, md.as_bytes(), 0o644),
            ("refs", b"file".as_slice(), 0o644),
            ("refs/child", b"child".as_slice(), 0o644),
        ],
    ] {
        assert!(matches!(
            PreparedSkill::zip(&archive(&entries)),
            Err(OwnerError::Invalid)
        ));
    }
}

#[test]
fn zip_and_folder_resource_limits_fail_closed() {
    let md = markdown("hello", "Instructions");
    let large = vec![b'a'; MAX_IMPORT_FILE_BYTES + 1];
    let bytes = archive(&[
        (SKILL_FILE, md.as_bytes(), 0o644),
        ("large.txt", &large, 0o644),
    ]);
    assert!(matches!(
        PreparedSkill::zip(&bytes),
        Err(OwnerError::TooLarge)
    ));
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(SKILL_FILE), &md).unwrap();
    for i in 0..MAX_IMPORT_FILES {
        fs::write(dir.path().join(format!("f{i}")), "").unwrap();
    }
    assert!(matches!(
        PreparedSkill::folder(dir.path()),
        Err(OwnerError::TooLarge)
    ));
    let large_folder = tempfile::tempdir().unwrap();
    fs::write(large_folder.path().join(SKILL_FILE), &md).unwrap();
    let block = vec![b'x'; MAX_IMPORT_FILE_BYTES];
    for i in 0..=MAX_IMPORT_BYTES / MAX_IMPORT_FILE_BYTES {
        fs::write(large_folder.path().join(format!("part{i}.txt")), &block).unwrap();
    }
    assert!(matches!(
        PreparedSkill::folder(large_folder.path()),
        Err(OwnerError::TooLarge)
    ));
    for name in ["nul", "con", "aux.txt", "import", "../x", "x..y"] {
        assert!(
            PreparedSkill::markdown(&markdown(name, "Body")).is_err(),
            "{name}"
        );
    }
}

#[test]
fn zip_entry_count_and_depth_budgets_reject_otherwise_inert_archives() {
    let md = markdown("hello", "Instructions");
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        SKILL_FILE,
        zip::write::SimpleFileOptions::default().unix_permissions(0o644),
    )
    .unwrap();
    zip.write_all(md.as_bytes()).unwrap();
    for i in 0..MAX_IMPORT_ENTRIES {
        zip.add_directory(
            format!("dir{i}"),
            zip::write::SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
    }
    assert!(matches!(
        PreparedSkill::zip(&zip.finish().unwrap().into_inner()),
        Err(OwnerError::TooLarge)
    ));
    let deep = format!("{}reference.txt", "dir/".repeat(16));
    let zip = archive(&[(SKILL_FILE, md.as_bytes(), 0o644), (&deep, b"data", 0o644)]);
    assert!(matches!(
        PreparedSkill::zip(&zip),
        Err(OwnerError::TooLarge)
    ));
}

#[test]
fn filtered_content_covers_auxiliary_files_and_snapshot_is_immutable() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join(SKILL_FILE),
        markdown("hello", "Instructions"),
    )
    .unwrap();
    fs::write(dir.path().join("reference.txt"), "known-secret-123").unwrap();
    let prepared = PreparedSkill::folder(dir.path()).unwrap();
    assert!(prepared.contains_filtered_content(|t| t.replace("known-secret-123", "[REDACTED]")));
    fs::write(dir.path().join("reference.txt"), "changed after preview").unwrap();
    let target = tempfile::tempdir().unwrap();
    OwnerSkillStore::new(target.path().join("skills"))
        .create(&prepared)
        .unwrap();
    assert_eq!(
        fs::read_to_string(target.path().join("skills/hello/reference.txt")).unwrap(),
        "known-secret-123"
    );
}

#[cfg(unix)]
#[test]
fn folder_import_rejects_symlinks_ancestors_and_executable_files() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join(SKILL_FILE), markdown("hello", "Instructions")).unwrap();
    symlink(&source, dir.path().join("alias")).unwrap();
    assert!(PreparedSkill::folder(&dir.path().join("alias")).is_err());
    fs::write(dir.path().join("private"), "private").unwrap();
    symlink(dir.path().join("private"), source.join("link")).unwrap();
    assert!(PreparedSkill::folder(&source).is_err());
    fs::remove_file(source.join("link")).unwrap();
    fs::write(source.join("script.sh"), "do not run").unwrap();
    fs::set_permissions(source.join("script.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(PreparedSkill::folder(&source).is_err());
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, dir.path().join("skills")).unwrap();
    let prepared = PreparedSkill::markdown(&markdown("hello", "Body")).unwrap();
    assert!(
        OwnerSkillStore::new(dir.path().join("skills"))
            .create(&prepared)
            .is_err()
    );
    assert!(!outside.join("hello").exists());
}

#[test]
fn published_alias_update_preserves_version_and_future_publication() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("skills");
    fs::create_dir_all(root.join("hello@0.1.0")).unwrap();
    fs::write(root.join("hello@0.1.0/SKILL.md"), markdown("hello", "Old")).unwrap();
    fs::write(root.join("hello@0.1.0/extra.txt"), "Keep this reference").unwrap();
    let prepared = PreparedSkill::markdown(&markdown("hello", "New")).unwrap();
    let store = OwnerSkillStore::new(root.clone());
    assert_eq!(store.create(&prepared).unwrap_err(), OwnerError::Conflict);
    let updated = store.update("hello", &prepared, None).unwrap();
    assert_eq!(updated.body, "New");
    assert_eq!(updated.version, "0.1.1");
    assert_eq!(updated.dir, Some(root.join("hello@0.1.1")));
    assert!(!root.join("hello@0.1.0").exists());
    assert_eq!(
        fs::read_to_string(root.join("hello@0.1.1/extra.txt")).unwrap(),
        "Keep this reference"
    );
    assert!(!root.join("hello").exists());
    let index = crate::SkillIndex::load_roots(crate::SkillRoots::from_data_dir(dir.path()));
    let bot = index.for_bot(Some("a")).unwrap();
    bot.write("hello", &markdown("hello", "Bot revision"))
        .unwrap();
    bot.publish("hello", true).unwrap();
    assert!(!root.join("hello@0.1.1").exists());
    assert_eq!(
        index.for_bot(None).unwrap().read("hello", None).unwrap(),
        "Bot revision"
    );
}

#[test]
fn published_update_crash_states_remain_visible_and_recover_before_bot_publication() {
    for crash_stage in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skills");
        let old = root.join("hello@0.1.0");
        let next = root.join("hello@0.1.1");
        fs::create_dir_all(&old).unwrap();
        fs::write(
            old.join(SKILL_FILE),
            markdown("hello", "Old complete document"),
        )
        .unwrap();
        fs::write(old.join("reference.txt"), "Preserved").unwrap();
        version::begin(&old, "0.1.0", "0.1.1").unwrap();
        if crash_stage > 0 {
            fs::write(
                old.join(SKILL_FILE),
                with_version(&markdown("hello", "New complete document"), "0.1.1").unwrap(),
            )
            .unwrap();
        }
        if crash_stage == 2 {
            fs::rename(&old, &next).unwrap();
        }
        // Reconstruct the runtime index from exactly the on-disk crash state.
        // Reading never repairs, and a complete skill remains visible.
        let index = crate::SkillIndex::load_roots(crate::SkillRoots::from_data_dir(dir.path()));
        assert_eq!(
            index.for_bot(None).unwrap().read("hello", None).unwrap(),
            if crash_stage == 0 {
                "Old complete document"
            } else {
                "New complete document"
            }
        );
        let visible = if crash_stage == 2 { &next } else { &old };
        assert!(
            visible.join(version::MARKER).exists(),
            "read-only load changed disk"
        );
        assert_eq!(
            fs::read_to_string(visible.join("reference.txt")).unwrap(),
            "Preserved"
        );
        let bot = index.for_bot(Some("a")).unwrap();
        bot.write("hello", &markdown("hello", "Bot revision"))
            .unwrap();
        bot.publish("hello", true).unwrap();
        assert_eq!(
            index.for_bot(None).unwrap().read("hello", None).unwrap(),
            "Bot revision"
        );
        assert!(!next.exists());
        assert!(!old.join(version::MARKER).exists());
        assert!(old.join(SKILL_FILE).is_file());
    }
}

#[test]
fn published_update_refuses_destination_collision_before_changing_document() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("skills");
    let old = root.join("hello@0.1.0");
    fs::create_dir_all(&old).unwrap();
    fs::create_dir(root.join("hello@0.1.1")).unwrap();
    let original = markdown("hello", "Original");
    fs::write(old.join(SKILL_FILE), &original).unwrap();
    let prepared = PreparedSkill::markdown(&markdown("hello", "New")).unwrap();
    assert_eq!(
        OwnerSkillStore::new(root)
            .update("hello", &prepared, None)
            .unwrap_err(),
        OwnerError::Conflict
    );
    assert_eq!(fs::read_to_string(old.join(SKILL_FILE)).unwrap(), original);
    assert!(!old.join(version::MARKER).exists());
}

#[test]
fn published_update_recovery_rejects_invalid_version_markers_without_moving_data() {
    for marker in [
        r#"{"old":"0.1.0","next":"../../outside"}"#,
        r#"{"old":"0.1.0","next":"0.1.9"}"#,
        r#"{"old":"0.1.0","next":"0.1.1","path":"../../outside"}"#,
        r#"{"old":"0.2.0","next":"0.2.1"}"#,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("skills");
        let old = root.join("hello@0.1.0");
        fs::create_dir_all(&old).unwrap();
        let original = markdown("hello", "Original");
        fs::write(old.join(SKILL_FILE), &original).unwrap();
        fs::write(old.join(version::MARKER), marker).unwrap();
        let prepared = PreparedSkill::markdown(&markdown("another", "New")).unwrap();
        assert!(
            OwnerSkillStore::new(root.clone())
                .create(&prepared)
                .is_err()
        );
        assert_eq!(fs::read_to_string(old.join(SKILL_FILE)).unwrap(), original);
        assert!(!root.join("another").exists());
    }
}
