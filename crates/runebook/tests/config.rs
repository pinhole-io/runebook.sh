use std::path::PathBuf;

use runebook::{AppConfig, NoteViewChoice, ThemeChoice};

#[test]
fn missing_config_is_the_default_and_save_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runebook").join("config.toml");
    let loaded = AppConfig::load_from(&path).unwrap();
    assert_eq!(loaded, AppConfig::default());

    let config = AppConfig {
        last_vault: Some(PathBuf::from("/tmp/notes")),
        ..AppConfig::default()
    };
    config.save_to(&path).unwrap();
    let again = AppConfig::load_from(&path).unwrap();
    assert_eq!(again.last_vault.unwrap(), PathBuf::from("/tmp/notes"));
    assert_eq!(again.theme, ThemeChoice::Dark);
    assert_eq!(again.note_view, NoteViewChoice::Preview);
    assert_eq!(again.microphone, None);
}

#[test]
fn old_config_keeps_defaults_for_new_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "last_vault = \"/tmp/notes\"\n").unwrap();
    let loaded = AppConfig::load_from(&path).unwrap();
    assert_eq!(loaded.last_vault.unwrap(), PathBuf::from("/tmp/notes"));
    assert_eq!(loaded.theme, ThemeChoice::Dark);
    assert_eq!(loaded.note_view, NoteViewChoice::Preview);
    assert_eq!(loaded.microphone, None);
}

#[test]
fn settings_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let config = AppConfig {
        last_vault: Some(PathBuf::from("/tmp/notes")),
        microphone: Some("Built-in Audio".to_string()),
        theme: ThemeChoice::Light,
        note_view: NoteViewChoice::Source,
    };
    config.save_to(&path).unwrap();
    let again = AppConfig::load_from(&path).unwrap();
    assert_eq!(again, config);
}
