//! Kind picker for a new note.
//!
//! Control+N and the file-plus button open this dialog. The choice then uses
//! the existing name dialog, or the research wizard.

use std::cell::Cell;

use gpui_kit::component::WindowExt;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Focusable, Window, px};

use super::tree::note_icon;
use super::{EditorPane, kind_title};
use crate::Root;
use crate::locale;
use runebook::NoteKind;

/// Kinds the picker offers.
///
/// Speech and Sight start a capture, so they wait until the current one ends.
pub(super) fn creatable_kinds(recording: bool) -> Vec<NoteKind> {
    let mut kinds = vec![NoteKind::Book];
    if !recording {
        kinds.push(NoteKind::Speech);
        kinds.push(NoteKind::Sight);
    }
    kinds.push(NoteKind::Deed);
    kinds.push(NoteKind::Research);
    kinds
}

pub(super) fn open(pane: &EditorPane, window: &mut Window, cx: &mut Context<Root>) {
    let kinds = creatable_kinds(pane.capture_status().is_some());
    let command = cx.new(|cx| CommandState::new(window, cx));
    let view = cx.weak_entity();
    let initial_focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            command.focus_handle(cx).focus(window, cx);
        }
        let kinds_for_confirm = kinds.clone();
        let view_for_confirm = view.clone();
        let mut list = Command::new(&command)
            .searchable(false)
            .bordered(false)
            .w_full();
        for kind in &kinds {
            list = list.item(
                CommandItem::new()
                    .label(kind_title(*kind))
                    .icon(note_icon(*kind)),
            );
        }
        dialog
            .title(locale::text("toolbar.new_note"))
            .width(px(420.))
            .child(list.on_confirm(move |path, window, cx| {
                let Some(kind) = kinds_for_confirm.get(path.row).copied() else {
                    return;
                };
                window.close_dialog(cx);
                let view = view_for_confirm.clone();
                window.defer(cx, move |window, cx| {
                    view.update(cx, |root, cx| {
                        if let Some(pane) = root.pane.as_mut() {
                            pane.prompt_new_note(kind, window, cx);
                        }
                        cx.notify();
                    })
                    .ok();
                });
            }))
    });
}

#[cfg(test)]
mod tests {
    use super::creatable_kinds;
    use runebook::NoteKind;

    #[test]
    fn a_capture_hides_speech_and_sight() {
        assert_eq!(
            creatable_kinds(true),
            vec![NoteKind::Book, NoteKind::Deed, NoteKind::Research]
        );
        assert_eq!(
            creatable_kinds(false),
            vec![
                NoteKind::Book,
                NoteKind::Speech,
                NoteKind::Sight,
                NoteKind::Deed,
                NoteKind::Research,
            ]
        );
    }
}
