use runebook::{
    Error, FolderId, NoteId, NoteKind, ResolveResult, TreeEntry, Vault, WikiTarget, encode_note,
};

fn note(raw: &str) -> NoteId {
    NoteId::parse(raw).unwrap()
}

fn folder(raw: &str) -> FolderId {
    FolderId::parse(raw).unwrap()
}

#[test]
fn create_and_open_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let created = Vault::create(dir.path(), " Personal ").unwrap();
    assert_eq!(created.name(), "Personal");
    let opened = Vault::open(dir.path()).unwrap();
    assert_eq!(opened.name(), "Personal");
    assert!(opened.notes().unwrap().is_empty());
}

#[test]
fn open_rejects_a_non_vault_and_create_rejects_a_second_vault() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-dir");
    std::fs::write(&file, "x").unwrap();
    assert!(matches!(
        Vault::open(&file),
        Err(Error::NotADirectory { .. })
    ));

    let empty = tempfile::tempdir().unwrap();
    assert!(matches!(
        Vault::open(empty.path()),
        Err(Error::NotAVault { .. })
    ));

    Vault::create(empty.path(), "Notes").unwrap();
    assert!(matches!(
        Vault::create(empty.path(), "Notes"),
        Err(Error::AlreadyExists { .. })
    ));
    assert!(matches!(
        Vault::create(empty.path(), "  "),
        Err(Error::InvalidVaultName)
    ));
}

#[test]
fn open_rejects_an_unsupported_schema() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("runebook.toml"),
        "schema_version = 2\nname = \"Old\"\n",
    )
    .unwrap();
    assert!(matches!(
        Vault::open(dir.path()),
        Err(Error::UnsupportedSchema {
            found: 2,
            expected: 1
        })
    ));
}

#[test]
fn note_crud_and_rename_update_the_id() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(dir.path(), "Notes").unwrap();
    let from = note("journal/day");
    vault.create_note(&from, "See [[inbox]].").unwrap();
    assert_eq!(vault.read(&from).unwrap(), "See [[inbox]].");

    vault.write(&from, "See [[inbox|Inbox]].").unwrap();
    let to = note("archive/day");
    vault.rename_note(&from, &to).unwrap();
    assert!(matches!(vault.read(&from), Err(Error::NoteNotFound { .. })));
    assert_eq!(vault.read(&to).unwrap(), "See [[inbox|Inbox]].");
    assert!(dir.path().join("archive/day.md").is_file());

    let dotted = note("my.notes");
    vault.create_note(&dotted, "dots").unwrap();
    assert!(dir.path().join("my.notes.md").is_file());

    vault.delete_note(&to).unwrap();
    assert!(matches!(vault.read(&to), Err(Error::NoteNotFound { .. })));
    assert!(matches!(
        vault.create_note(&dotted, "again"),
        Err(Error::NoteExists { .. })
    ));
}

#[test]
fn folder_rename_moves_the_note_id() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(dir.path(), "Notes").unwrap();
    let journal = folder("journal");
    vault.create_folder(&journal).unwrap();
    vault.create_note(&note("journal/day"), "hello").unwrap();
    vault.rename_folder(&journal, &folder("archive")).unwrap();
    assert_eq!(vault.read(&note("archive/day")).unwrap(), "hello");
    assert!(matches!(
        vault.read(&note("journal/day")),
        Err(Error::NoteNotFound { .. })
    ));

    std::fs::write(dir.path().join("archive/readme.txt"), b"text").unwrap();
    assert!(matches!(
        vault.delete_folder(&folder("archive")),
        Err(Error::ForeignFile { .. })
    ));
    std::fs::remove_file(dir.path().join("archive/readme.txt")).unwrap();
    vault.delete_folder(&folder("archive")).unwrap();
    assert!(!dir.path().join("archive").exists());
}

#[test]
fn delete_folder_removes_mp4_and_still_blocks_other_files() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(dir.path(), "Notes").unwrap();
    let journal = folder("journal");
    vault.create_folder(&journal).unwrap();
    vault.create_note(&note("journal/day"), "hello").unwrap();
    std::fs::write(dir.path().join("journal/day-20261024T052200Z.mp4"), b"mp4").unwrap();
    std::fs::create_dir_all(dir.path().join("journal/extra")).unwrap();
    std::fs::write(dir.path().join("journal/extra/clip.mp4"), b"mp4").unwrap();
    std::fs::write(dir.path().join("journal/extra/pic.pdf"), b"pdf").unwrap();
    assert!(matches!(
        vault.delete_folder(&journal),
        Err(Error::ForeignFile { .. })
    ));
    std::fs::remove_file(dir.path().join("journal/extra/pic.pdf")).unwrap();
    vault.delete_folder(&journal).unwrap();
    assert!(!dir.path().join("journal").exists());
}

#[test]
fn media_file_opens_known_formats_inside_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(dir.path(), "Notes").unwrap();
    let day = note("journal/day");
    vault.create_note(&day, "see the clip").unwrap();
    std::fs::write(dir.path().join("journal/clip.webm"), b"webm").unwrap();
    std::fs::write(dir.path().join("journal/song.MP3"), b"mp3").unwrap();
    std::fs::write(dir.path().join("journal/photo.png"), b"png").unwrap();
    std::fs::write(dir.path().join("journal/notes.txt"), b"text").unwrap();

    assert_eq!(
        vault.media_file(&day, "clip.webm").unwrap(),
        dir.path().join("journal/clip.webm")
    );
    assert_eq!(
        vault.media_file(&day, "song.MP3").unwrap(),
        dir.path().join("journal/song.MP3")
    );
    assert_eq!(
        vault.media_file(&day, "photo.png#t=1").unwrap(),
        dir.path().join("journal/photo.png")
    );
    std::fs::write(dir.path().join("journal/my clip.mp4"), b"mp4").unwrap();
    assert_eq!(
        vault.media_file(&day, "my%20clip.mp4").unwrap(),
        dir.path().join("journal/my clip.mp4")
    );
    let file_url = format!("file://{}", dir.path().join("journal/photo.png").display());
    assert_eq!(
        vault.media_file(&day, &file_url).unwrap(),
        dir.path().join("journal/photo.png")
    );

    assert!(matches!(
        vault.media_file(&day, "notes.txt"),
        Err(Error::NotMedia { .. })
    ));
    assert!(matches!(
        vault.media_file(&day, "../secret.mp4"),
        Err(Error::PathEscape { .. })
    ));
    assert!(matches!(
        vault.media_file(&day, "file:///etc/passwd.mp4"),
        Err(Error::PathEscape { .. })
    ));
    assert!(matches!(
        vault.media_file(&day, "missing.wav"),
        Err(Error::MediaNotFound { .. })
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            dir.path().join("journal/clip.webm"),
            dir.path().join("journal/link.webm"),
        )
        .unwrap();
        assert!(matches!(
            vault.media_file(&day, "link.webm"),
            Err(Error::Symlink { .. })
        ));
        std::fs::remove_file(dir.path().join("journal/link.webm")).unwrap();
    }

    std::fs::write(dir.path().join("journal/extra.ogg"), b"ogg").unwrap();
    std::fs::remove_file(dir.path().join("journal/notes.txt")).unwrap();
    vault.delete_folder(&folder("journal")).unwrap();
    assert!(!dir.path().join("journal").exists());
}

#[test]
fn recording_path_stays_inside_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(dir.path(), "Notes").unwrap();
    let day = note("journal/day");
    let path = vault.recording_path(&day, "20261024T052200Z").unwrap();
    assert_eq!(path, dir.path().join("journal/day-20261024T052200Z.mp4"));
    assert!(path.starts_with(dir.path()));

    let welcome = vault
        .recording_path(&note("welcome"), "20261024T052200Z")
        .unwrap();
    assert_eq!(welcome, dir.path().join("welcome-20261024T052200Z.mp4"));
    assert!(welcome.starts_with(dir.path()));

    assert!(matches!(
        vault.recording_path(&day, ".."),
        Err(Error::InvalidNoteId { .. })
    ));
    assert!(matches!(
        vault.recording_path(&day, "../x"),
        Err(Error::InvalidNoteId { .. })
    ));
    assert!(matches!(
        vault.recording_path(&day, "a/b"),
        Err(Error::InvalidNoteId { .. })
    ));
}

#[test]
fn index_lists_nested_notes_and_skips_hidden_paths() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(dir.path(), "Notes").unwrap();
    vault
        .create_note(&note("inbox"), "[[journal/day]]")
        .unwrap();
    vault
        .create_note(&note("journal/day"), "[[inbox]]")
        .unwrap();
    std::fs::create_dir_all(dir.path().join(".hidden")).unwrap();
    std::fs::write(dir.path().join(".hidden/secret.md"), "nope").unwrap();
    std::fs::write(dir.path().join("journal/.draft.md"), "nope").unwrap();
    std::fs::write(dir.path().join("readme.txt"), "nope").unwrap();

    let index = vault.notes().unwrap();
    let ids: Vec<_> = index.ids().map(ToString::to_string).collect();
    assert_eq!(ids, vec!["inbox".to_string(), "journal/day".to_string()]);
    assert_eq!(
        index.outgoing(&note("inbox")),
        &[WikiTarget::new("journal/day")]
    );

    let tree = vault.tree().unwrap();
    assert!(tree.children.iter().any(|entry| matches!(
        entry,
        TreeEntry::Note { name, kind: NoteKind::Book, .. } if name == "inbox"
    )));
}

#[test]
fn tree_reads_kind_and_the_index_uses_the_body() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(dir.path(), "Notes").unwrap();
    vault
        .create_note(
            &note("talk"),
            &encode_note(NoteKind::Speech, "See [[inbox]].\n"),
        )
        .unwrap();
    vault.create_note(&note("inbox"), "hi").unwrap();

    let index = vault.notes().unwrap();
    assert_eq!(index.outgoing(&note("talk")), &[WikiTarget::new("inbox")]);
    let tree = vault.tree().unwrap();
    assert_eq!(
        note_kind(&tree.children, &note("talk")),
        Some(NoteKind::Speech)
    );
}

fn note_kind(entries: &[TreeEntry], id: &NoteId) -> Option<NoteKind> {
    for entry in entries {
        match entry {
            TreeEntry::Note {
                id: note_id, kind, ..
            } if note_id == id => return Some(*kind),
            TreeEntry::Folder { children, .. } => {
                if let Some(kind) = note_kind(children, id) {
                    return Some(kind);
                }
            }
            TreeEntry::Note { .. } => {}
        }
    }
    None
}

#[test]
fn resolve_exact_then_unique_stem_then_ambiguous() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(dir.path(), "Notes").unwrap();
    vault.create_note(&note("inbox"), "").unwrap();
    vault.create_note(&note("a/day"), "").unwrap();
    vault.create_note(&note("b/day"), "").unwrap();
    let index = vault.notes().unwrap();

    assert_eq!(
        index.resolve(&WikiTarget::new("inbox")),
        ResolveResult::Exact(note("inbox"))
    );
    assert_eq!(
        index.resolve(&WikiTarget::new("missing")),
        ResolveResult::Missing
    );
    assert_eq!(
        index.resolve(&WikiTarget::new("day")),
        ResolveResult::Ambiguous(vec![note("a/day"), note("b/day")])
    );
    assert_eq!(
        index.resolve(&WikiTarget::new("a/day")),
        ResolveResult::Exact(note("a/day"))
    );
    assert_eq!(
        index.resolve(&WikiTarget::new("nope/missing")),
        ResolveResult::Missing
    );

    let only = tempfile::tempdir().unwrap();
    let vault = Vault::create(only.path(), "Notes").unwrap();
    vault.create_note(&note("nested/inbox"), "").unwrap();
    let index = vault.notes().unwrap();
    assert_eq!(
        index.resolve(&WikiTarget::new("inbox")),
        ResolveResult::UniqueStem(note("nested/inbox"))
    );
}

#[test]
fn note_id_rejects_parent_segments_and_keeps_dots_in_the_stem() {
    assert!(NoteId::parse("../x").is_err());
    assert!(NoteId::parse("/abs").is_err());
    assert!(NoteId::parse(".hidden").is_err());
    assert!(NoteId::parse("foo/").is_err());
    assert!(NoteId::parse("foo\\bar").is_err());
    let id = note("my.notes");
    assert_eq!(id.relative_path(), std::path::PathBuf::from("my.notes.md"));
}

#[cfg(unix)]
#[test]
fn index_skips_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(dir.path(), "Notes").unwrap();
    vault.create_note(&note("inbox"), "body").unwrap();
    std::os::unix::fs::symlink(dir.path().join("inbox.md"), dir.path().join("link.md")).unwrap();
    let ids: Vec<_> = vault
        .notes()
        .unwrap()
        .ids()
        .map(ToString::to_string)
        .collect();
    assert_eq!(ids, vec!["inbox".to_string()]);
}

#[test]
fn search_matches_the_body_and_caps_the_hits() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Vault::create(dir.path(), "Notes").unwrap();
    vault
        .create_note(&note("journal/day"), "The Blue Lantern.")
        .unwrap();
    vault
        .create_note(
            &note("speech/talk"),
            &encode_note(NoteKind::Speech, "No match here."),
        )
        .unwrap();

    let empty = vault.search("  ").unwrap();
    assert!(empty.hits.is_empty());
    assert!(!empty.truncated);

    let missed = vault.search("speech").unwrap();
    assert!(missed.hits.is_empty());

    let found = vault.search("blue lantern").unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].id, note("journal/day"));
    assert!(found.hits[0].excerpt.to_lowercase().contains("blue"));
    assert!(!found.truncated);

    let long = format!("prefix {}", "x".repeat(200));
    vault.create_note(&note("long"), &long).unwrap();
    let clipped = vault.search("prefix").unwrap();
    assert_eq!(clipped.hits[0].excerpt.chars().count(), 121);
    assert!(clipped.hits[0].excerpt.starts_with("prefix "));
    assert!(clipped.hits[0].excerpt.ends_with('…'));

    for index in 0..21 {
        vault
            .create_note(&note(&format!("batch/{index}")), "shared token")
            .unwrap();
    }
    let page = vault.search("shared token").unwrap();
    assert_eq!(page.hits.len(), 20);
    assert!(page.truncated);
}
