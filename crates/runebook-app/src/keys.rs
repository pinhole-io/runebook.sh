//! Runebook command chords.
//!
//! GPUI matches the key. This module owns the command list, the default
//! chords, and the user overrides from [`runebook::AppConfig::keys`].
//! A later `Unbind` hides an older binding for that action on the same chord.
//! An open dialog keeps the chord because each binding also sets `NoAction`
//! in the `Dialog` context.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui_kit::{
    Action, App, Global, KeyBinding, KeyBindingContextPredicate, Keystroke, NoAction,
    OperatingSystem, Unbind,
};

use crate::locale;
use crate::{NewNote, OpenVault, SaveNote, ToggleNoteView};

const CONTEXT: &str = "Runebook";
const DIALOG: &str = "Dialog";
const MAX_CHORD_CHARS: usize = 64;
const MAX_STROKES: usize = 4;

struct Command {
    id: &'static str,
    label_key: &'static str,
    detail_key: &'static str,
    field_id: &'static str,
    defaults: &'static [&'static str],
    spawn: fn() -> Box<dyn Action>,
}

const COMMANDS: &[Command] = &[
    Command {
        id: "new_note",
        label_key: "command.new_note",
        detail_key: "command.new_note.desc",
        field_id: "new-note-key",
        defaults: &["ctrl-n"],
        spawn: || Box::new(NewNote),
    },
    Command {
        id: "save_note",
        label_key: "command.save_note",
        detail_key: "command.save_note.desc",
        field_id: "save-note-key",
        defaults: &["cmd-s"],
        spawn: || Box::new(SaveNote),
    },
    Command {
        id: "open_vault",
        label_key: "command.open_vault",
        detail_key: "command.open_vault.desc",
        field_id: "open-vault-key",
        defaults: &["cmd-o"],
        spawn: || Box::new(OpenVault),
    },
    Command {
        id: "toggle_note_view",
        label_key: "command.toggle_note_view",
        detail_key: "command.toggle_note_view.desc",
        field_id: "toggle-view-key",
        defaults: &["cmd-e"],
        spawn: || Box::new(ToggleNoteView),
    },
];

/// Why a chord field cannot be saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChordFault {
    /// The text is not a GPUI chord.
    Bad,
    /// The text exceeds the stroke or character limit.
    TooLong,
    /// Another command in the same context already uses the chord.
    Conflict {
        /// Locale key for the command that keeps the chord.
        command: &'static str,
    },
}

struct Slot {
    chords: Vec<String>,
    fault: Option<ChordFault>,
}

/// One settings row.
pub(crate) struct KeyRow {
    /// Text for the chord field. An override shows the saved text.
    pub(crate) field: String,
    pub(crate) fault: Option<ChordFault>,
}

enum Change {
    Unbind { index: usize, chord: String },
    Bind { index: usize, chord: String },
}

/// Chords installed in this process. GPUI only appends bindings.
struct Installed {
    chords: Vec<Vec<String>>,
}

impl Global for Installed {}

/// Command id at `index`, when that row exists.
pub(crate) fn command_id(index: usize) -> Option<&'static str> {
    COMMANDS.get(index).map(|command| command.id)
}

/// Locale key for the command name at `index`.
pub(crate) fn label_key(index: usize) -> Option<&'static str> {
    COMMANDS.get(index).map(|command| command.label_key)
}

/// Locale key for the command sentence at `index`.
pub(crate) fn detail_key(index: usize) -> Option<&'static str> {
    COMMANDS.get(index).map(|command| command.detail_key)
}

/// Element id for the settings label at `index`.
pub(crate) fn label_id(index: usize) -> &'static str {
    COMMANDS
        .get(index)
        .map(|command| command.field_id)
        .unwrap_or("key-row")
}

/// Built-in chord text for `id`.
pub(crate) fn default_field(id: &str) -> String {
    COMMANDS
        .iter()
        .find(|command| command.id == id)
        .map(|command| command.defaults.join(", "))
        .unwrap_or_default()
}

/// Sentence for `fault`.
pub(crate) fn fault_text(fault: &ChordFault) -> String {
    match fault {
        ChordFault::Bad => locale::text("settings.keys.bad_chord").to_string(),
        ChordFault::TooLong => locale::text("settings.keys.too_long").to_string(),
        ChordFault::Conflict { command } => locale::format(
            "settings.keys.conflict",
            &[("command", locale::text(command))],
        ),
    }
}

/// Rows for Settings, in catalog order.
pub(crate) fn rows(keys: &BTreeMap<String, Vec<String>>, os: OperatingSystem) -> Vec<KeyRow> {
    let slots = resolve(keys, os);
    COMMANDS
        .iter()
        .zip(slots)
        .map(|(command, slot)| KeyRow {
            field: match keys.get(command.id) {
                Some(raw) => raw.join(", "),
                None => slot.chords.join(", "),
            },
            fault: slot.fault,
        })
        .collect()
}

/// Write `field` into `keys` for `id`.
///
/// A blank field unbinds the command. A value equal to the default removes the
/// override. A bad chord or a conflict leaves `keys` unchanged.
pub(crate) fn commit(
    keys: &mut BTreeMap<String, Vec<String>>,
    id: &str,
    field: &str,
    os: OperatingSystem,
) -> Result<(), ChordFault> {
    let Some(index) = COMMANDS.iter().position(|command| command.id == id) else {
        return Err(ChordFault::Bad);
    };
    let unique = normalize_field(field, os)?;
    let mut others = keys.clone();
    others.remove(id);
    let occupied = resolve(&others, os);
    for (other, slot) in occupied.iter().enumerate() {
        if other == index {
            continue;
        }
        for chord in &unique {
            if slot.chords.iter().any(|taken| taken == chord) {
                return Err(ChordFault::Conflict {
                    command: COMMANDS[other].label_key,
                });
            }
        }
    }
    let defaults: Vec<String> = COMMANDS[index]
        .defaults
        .iter()
        .map(|chord| (*chord).to_string())
        .collect();
    if unique == defaults {
        keys.remove(id);
    } else {
        keys.insert(id.to_string(), unique);
    }
    Ok(())
}

/// Install `keys` on top of the chords already bound in this process.
///
/// Does not clear the keymap. Clearing would drop the editor and dialog chords.
pub(crate) fn apply(cx: &mut App, keys: &BTreeMap<String, Vec<String>>) {
    let next: Vec<Vec<String>> = resolve(keys, cx.platform_info().os)
        .into_iter()
        .map(|slot| slot.chords)
        .collect();
    let previous = cx
        .try_global::<Installed>()
        .map(|installed| installed.chords.clone())
        .unwrap_or_else(|| vec![Vec::new(); COMMANDS.len()]);
    let mut recorded = previous.clone();
    let mut bindings = Vec::new();
    for change in changes(&previous, &next) {
        match change {
            Change::Unbind { index, chord } => {
                if let Some(binding) = unbind_binding(cx, index, &chord) {
                    if let Some(row) = recorded.get_mut(index) {
                        row.retain(|saved| saved != &chord);
                    }
                    bindings.push(binding);
                }
            }
            Change::Bind { index, chord } => {
                let Some(binding) = bind_binding(cx, index, &chord) else {
                    continue;
                };
                bindings.push(binding);
                if let Some(block) = dialog_block(cx, &chord) {
                    bindings.push(block);
                }
                if let Some(row) = recorded.get_mut(index) {
                    row.push(chord);
                }
            }
        }
    }
    if !bindings.is_empty() {
        cx.bind_keys(bindings);
    }
    cx.set_global(Installed { chords: recorded });
}

fn normalize_field(field: &str, os: OperatingSystem) -> Result<Vec<String>, ChordFault> {
    let mut unique = Vec::new();
    for piece in field.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let chord = validate_chord(piece, os)?;
        if !unique.contains(&chord) {
            unique.push(chord);
        }
    }
    Ok(unique)
}

fn validate_chord(chord: &str, os: OperatingSystem) -> Result<String, ChordFault> {
    let chord = chord.trim();
    if chord.is_empty() {
        return Err(ChordFault::Bad);
    }
    if chord.len() > MAX_CHORD_CHARS {
        return Err(ChordFault::TooLong);
    }
    let strokes: Vec<&str> = chord.split_whitespace().collect();
    if strokes.is_empty() {
        return Err(ChordFault::Bad);
    }
    if strokes.len() > MAX_STROKES {
        return Err(ChordFault::TooLong);
    }
    for stroke in &strokes {
        if Keystroke::parse_with_os(stroke, os).is_err() {
            return Err(ChordFault::Bad);
        }
    }
    Ok(strokes.join(" "))
}

fn resolve(keys: &BTreeMap<String, Vec<String>>, os: OperatingSystem) -> Vec<Slot> {
    let mut taken: Vec<(String, usize)> = Vec::new();
    let mut slots = Vec::with_capacity(COMMANDS.len());
    for (index, command) in COMMANDS.iter().enumerate() {
        let Some(raw) = keys.get(command.id) else {
            slots.push(claim_defaults(command, index, &mut taken, None));
            continue;
        };
        if raw.is_empty() {
            slots.push(Slot {
                chords: Vec::new(),
                fault: None,
            });
            continue;
        }
        let mut kept = Vec::new();
        let mut fault = None;
        for chord in raw {
            match validate_chord(chord, os) {
                Ok(norm) => {
                    if let Some(owner) = owner_label(&taken, &norm) {
                        fault = Some(ChordFault::Conflict { command: owner });
                    } else {
                        taken.push((norm.clone(), index));
                        kept.push(norm);
                    }
                }
                Err(err) => fault = Some(err),
            }
        }
        if kept.is_empty() {
            slots.push(claim_defaults(command, index, &mut taken, fault));
        } else {
            slots.push(Slot {
                chords: kept,
                fault,
            });
        }
    }
    slots
}

fn claim_defaults(
    command: &Command,
    index: usize,
    taken: &mut Vec<(String, usize)>,
    fault: Option<ChordFault>,
) -> Slot {
    let mut chords = Vec::new();
    for chord in command.defaults {
        let chord = (*chord).to_string();
        if owner_label(taken, &chord).is_none() {
            taken.push((chord.clone(), index));
            chords.push(chord);
        }
    }
    Slot { chords, fault }
}

fn owner_label(taken: &[(String, usize)], chord: &str) -> Option<&'static str> {
    taken
        .iter()
        .find(|(owned, _)| owned == chord)
        .map(|(_, index)| COMMANDS[*index].label_key)
}

fn changes(previous: &[Vec<String>], next: &[Vec<String>]) -> Vec<Change> {
    let mut planned = Vec::new();
    for index in 0..COMMANDS.len() {
        let old = previous.get(index).map(Vec::as_slice).unwrap_or(&[]);
        let new = next.get(index).map(Vec::as_slice).unwrap_or(&[]);
        for chord in old {
            if !new.iter().any(|item| item == chord) {
                planned.push(Change::Unbind {
                    index,
                    chord: chord.clone(),
                });
            }
        }
        for chord in new {
            if !old.iter().any(|item| item == chord) {
                planned.push(Change::Bind {
                    index,
                    chord: chord.clone(),
                });
            }
        }
    }
    planned
}

fn fixed_context(name: &str) -> Rc<KeyBindingContextPredicate> {
    // The names are the constants above. A parse failure is a programming error.
    KeyBindingContextPredicate::parse(name)
        .expect("Runebook and Dialog are fixed context names")
        .into()
}

fn bind_binding(cx: &App, index: usize, chord: &str) -> Option<KeyBinding> {
    let command = &COMMANDS[index];
    KeyBinding::load(
        chord,
        (command.spawn)(),
        Some(fixed_context(CONTEXT)),
        false,
        None,
        cx,
    )
    .ok()
}

fn unbind_binding(cx: &App, index: usize, chord: &str) -> Option<KeyBinding> {
    let name = (COMMANDS[index].spawn)().name();
    KeyBinding::load(
        chord,
        Box::new(Unbind(name.into())),
        Some(fixed_context(CONTEXT)),
        false,
        None,
        cx,
    )
    .ok()
}

fn dialog_block(cx: &App, chord: &str) -> Option<KeyBinding> {
    KeyBinding::load(
        chord,
        Box::new(NoAction),
        Some(fixed_context(DIALOG)),
        false,
        None,
        cx,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use gpui_kit::{
        Action, KeyBinding, KeyContext, Keymap, Keystroke, NoAction, OperatingSystem, Unbind,
    };

    use super::{
        COMMANDS, CONTEXT, Change, ChordFault, DIALOG, changes, commit, resolve, validate_chord,
    };
    use crate::{NewNote, OpenVault, SaveNote, ToggleNoteView};

    #[test]
    fn ctrl_n_in_runebook_runs_new_note() {
        let keymap = keymap_after(&[defaults()]);
        let matched = match_chord(&keymap, "ctrl-n", &[CONTEXT]);
        assert!(matched[0].action().partial_eq(&NewNote));
    }

    #[test]
    fn ctrl_n_inside_a_dialog_does_not_run_new_note() {
        let keymap = keymap_after(&[defaults()]);
        let matched = match_chord(&keymap, "ctrl-n", &[CONTEXT, DIALOG]);
        assert!(matched.is_empty());
    }

    #[test]
    fn a_second_rebind_leaves_only_the_latest_chord() {
        let defaults = defaults();
        let mut custom = defaults.clone();
        custom[0] = vec!["ctrl-m".to_string()];
        let keymap = keymap_after(&[defaults.clone(), custom, defaults]);
        let matched = match_chord(&keymap, "ctrl-n", &[CONTEXT]);
        assert_eq!(matched.len(), 1);
        assert!(matched[0].action().partial_eq(&NewNote));
        assert!(match_chord(&keymap, "ctrl-m", &[CONTEXT]).is_empty());
        assert!(match_chord(&keymap, "ctrl-n", &[CONTEXT, DIALOG]).is_empty());
    }

    #[test]
    fn a_shared_chord_is_rejected() {
        let mut keys = BTreeMap::new();
        let err = commit(&mut keys, "save_note", "ctrl-n", OperatingSystem::Linux).unwrap_err();
        assert_eq!(
            err,
            ChordFault::Conflict {
                command: "command.new_note",
            }
        );
        assert!(keys.is_empty());
    }

    #[test]
    fn a_chord_longer_than_the_limit_is_rejected() {
        let mut keys = BTreeMap::new();
        let err = commit(
            &mut keys,
            "new_note",
            &"a".repeat(65),
            OperatingSystem::Linux,
        )
        .unwrap_err();
        assert_eq!(err, ChordFault::TooLong);
        let err = validate_chord("ctrl-a ctrl-b ctrl-c ctrl-d ctrl-e", OperatingSystem::Linux)
            .unwrap_err();
        assert_eq!(err, ChordFault::TooLong);
        assert!(keys.is_empty());
    }

    #[test]
    fn an_unknown_id_stays_in_the_map_and_is_skipped() {
        let mut keys = BTreeMap::new();
        keys.insert("not_a_command".to_string(), vec!["ctrl-1".to_string()]);
        let slots = resolve(&keys, OperatingSystem::Linux);
        assert_eq!(slots[0].chords, vec!["ctrl-n".to_string()]);
        assert!(keys.contains_key("not_a_command"));
    }

    fn defaults() -> Vec<Vec<String>> {
        COMMANDS
            .iter()
            .map(|command| {
                command
                    .defaults
                    .iter()
                    .map(|chord| (*chord).to_string())
                    .collect()
            })
            .collect()
    }

    fn keymap_after(tables: &[Vec<Vec<String>>]) -> Keymap {
        let mut keymap = Keymap::default();
        let mut previous = vec![Vec::new(); COMMANDS.len()];
        for table in tables {
            for change in changes(&previous, table) {
                apply_change(&mut keymap, &change);
            }
            previous.clone_from(table);
        }
        keymap
    }

    fn apply_change(keymap: &mut Keymap, change: &Change) {
        match change {
            Change::Bind { index, chord } => {
                let chord = leak(chord);
                match index {
                    0 => keymap.add_bindings([
                        KeyBinding::new(chord, NewNote, Some(CONTEXT)),
                        KeyBinding::new(chord, NoAction, Some(DIALOG)),
                    ]),
                    1 => keymap.add_bindings([
                        KeyBinding::new(chord, SaveNote, Some(CONTEXT)),
                        KeyBinding::new(chord, NoAction, Some(DIALOG)),
                    ]),
                    2 => keymap.add_bindings([
                        KeyBinding::new(chord, OpenVault, Some(CONTEXT)),
                        KeyBinding::new(chord, NoAction, Some(DIALOG)),
                    ]),
                    _ => keymap.add_bindings([
                        KeyBinding::new(chord, ToggleNoteView, Some(CONTEXT)),
                        KeyBinding::new(chord, NoAction, Some(DIALOG)),
                    ]),
                }
            }
            Change::Unbind { index, chord } => {
                let chord = leak(chord);
                let name = match index {
                    0 => NewNote.name(),
                    1 => SaveNote.name(),
                    2 => OpenVault.name(),
                    _ => ToggleNoteView.name(),
                };
                keymap.add_bindings([KeyBinding::new(chord, Unbind(name.into()), Some(CONTEXT))]);
            }
        }
    }

    fn match_chord(keymap: &Keymap, chord: &'static str, stack: &[&str]) -> Vec<KeyBinding> {
        let contexts: Vec<KeyContext> = stack
            .iter()
            .map(|name| KeyContext::parse(name).expect("test context"))
            .collect();
        keymap
            .bindings_for_input(&[Keystroke::parse(chord).expect("test chord")], &contexts)
            .0
            .into_iter()
            .collect()
    }

    fn leak(chord: &str) -> &'static str {
        Box::leak(chord.to_string().into_boxed_str())
    }
}
