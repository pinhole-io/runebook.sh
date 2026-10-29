use std::path::PathBuf;

use runebook::{AppConfig, AutosavePace, NoteViewChoice, SpeechModel, TextSize};

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
    assert_eq!(again.theme, "dark");
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
    assert_eq!(loaded.theme, "dark");
    assert_eq!(loaded.note_view, NoteViewChoice::Preview);
    assert_eq!(loaded.microphone, None);
    assert!(loaded.line_numbers);
    assert!(loaded.soft_wrap);
    assert!(loaded.reopen_last_note);
    assert_eq!(loaded.text_size, TextSize::Medium);
    assert_eq!(loaded.autosave, AutosavePace::Normal);
    assert_eq!(loaded.speech_model, SpeechModel::Fast);
    assert!(!loaded.media_muted);
    assert_eq!(loaded.locale, "en");
}

#[test]
fn dark_and_light_theme_names_still_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"dark\"\n").unwrap();
    assert_eq!(AppConfig::load_from(&path).unwrap().theme, "dark");
    std::fs::write(&path, "theme = \"light\"\n").unwrap();
    assert_eq!(AppConfig::load_from(&path).unwrap().theme, "light");
    std::fs::write(&path, "theme = \"Dracula\"\n").unwrap();
    assert_eq!(AppConfig::load_from(&path).unwrap().theme, "Dracula");
}

#[test]
fn settings_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let config = AppConfig {
        last_vault: Some(PathBuf::from("/tmp/notes")),
        microphone: Some("Built-in Audio".to_string()),
        theme: "light".to_string(),
        note_view: NoteViewChoice::Source,
        text_size: TextSize::Small,
        autosave: AutosavePace::Slow,
        speech_model: SpeechModel::Accurate,
        speech_language: "en".to_string(),
        locale: "is".to_string(),
        line_numbers: false,
        soft_wrap: false,
        media_muted: true,
        reopen_last_note: false,
        last_note: Some("inbox".to_string()),
        ..AppConfig::default()
    };
    config.save_to(&path).unwrap();
    let again = AppConfig::load_from(&path).unwrap();
    assert_eq!(again, config);
}
