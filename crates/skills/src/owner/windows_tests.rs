//! Windows handle-lifetime controls use only a temporary import directory.

use super::*;

#[test]
fn windows_file_snapshot_handle_rejects_writes_renames_and_deletion() {
    let temporary = tempfile::tempdir().unwrap();
    let original = temporary.path().join("reference.txt");
    let renamed = temporary.path().join("renamed.txt");
    fs::write(&original, b"immutable reference").unwrap();

    let mut held = windows_open(&original, false).unwrap();
    let mut captured = String::new();
    held.read_to_string(&mut captured).unwrap();
    assert_eq!(captured, "immutable reference");

    assert!(
        fs::OpenOptions::new().write(true).open(&original).is_err(),
        "a snapshot handle must refuse another writer"
    );
    assert!(
        fs::rename(&original, &renamed).is_err(),
        "a snapshot handle must refuse renaming its file"
    );
    assert!(
        fs::remove_file(&original).is_err(),
        "a snapshot handle must refuse deleting its file"
    );
    assert_eq!(fs::read(&original).unwrap(), b"immutable reference");

    drop(held);
    fs::write(&original, b"after release").unwrap();
    fs::rename(&original, &renamed).unwrap();
    assert_eq!(fs::read(&renamed).unwrap(), b"after release");
    fs::remove_file(&renamed).unwrap();
}

#[test]
fn windows_snapshot_ancestor_handles_prevent_directory_replacement_until_drop() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().join("parent");
    let source = parent.join("source");
    let renamed_source = parent.join("source-renamed");
    let renamed_parent = temporary.path().join("parent-renamed");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join(SKILL_FILE),
        "---\nname: windows-import\ndescription: A local fixture\n---\nInert instructions\n",
    )
    .unwrap();
    fs::write(source.join("reference.txt"), b"inert reference").unwrap();
    let baseline = PreparedSkill::folder(&source).unwrap();
    assert_eq!(baseline.name(), "windows-import");
    assert_eq!(baseline.file_count(), 2);

    let held = windows_ancestors(&source).unwrap();
    assert!(
        fs::rename(&source, &renamed_source).is_err(),
        "the held import-root handle must refuse root replacement"
    );
    assert!(
        fs::rename(&parent, &renamed_parent).is_err(),
        "held ancestor handles must refuse ancestor replacement"
    );

    drop(held);
    fs::rename(&source, &renamed_source).unwrap();
    fs::rename(&parent, &renamed_parent).unwrap();
    let relocated = renamed_parent.join("source-renamed");
    let snapshot = PreparedSkill::folder(&relocated).unwrap();
    assert_eq!(snapshot.name(), "windows-import");
    assert_eq!(snapshot.file_count(), 2);
    assert_eq!(
        snapshot.files.get("reference.txt").unwrap(),
        b"inert reference"
    );
}
