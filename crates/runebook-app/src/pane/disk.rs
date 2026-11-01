//! Reload the vault and reconcile the open note.

use super::*;

impl EditorPane {
    pub(super) fn reload(&mut self) -> Result<(), Error> {
        let tree = self.vault.tree()?;
        let index = self.vault.notes()?;
        let stamp = self.vault.stamp()?;
        self.tree = tree;
        self.index = index;
        self.stamp = stamp;
        Ok(())
    }

    /// The stamp from the last scan. The file watch compares against this.
    pub(crate) fn file_stamp(&self) -> &VaultStamp {
        &self.stamp
    }

    /// Read the vault again and show files that changed outside the app.
    pub(super) fn refresh_from_disk(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        if self.prompt.is_some() {
            return;
        }
        if let Err(err) = self.reload() {
            if self.fail_disk_scan(err.to_string()) {
                present_scan(window);
            }
            return;
        }
        self.finish_disk_refresh(window, cx);
        present_scan(window);
    }

    /// Install a scan that started from `base`. A newer local edit wins.
    pub(crate) fn apply_disk_refresh(
        &mut self,
        base: &VaultStamp,
        refresh: DiskRefresh,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> bool {
        if self.prompt.is_some() || &self.stamp != base {
            return false;
        }
        self.tree = refresh.tree;
        self.index = refresh.index;
        self.stamp = refresh.stamp;
        self.finish_disk_refresh(window, cx);
        true
    }

    /// Record a failed scan. Returns true when the notice changed.
    pub(crate) fn fail_disk_scan(&mut self, message: String) -> bool {
        let notice = locale::format("tree.refresh_error", &[("message", &message)]);
        if self.notice.as_deref() == Some(notice.as_str()) {
            return false;
        }
        self.notice = Some(notice);
        true
    }

    pub(super) fn finish_disk_refresh(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.clear_disk_notice();
        self.reconcile_open_file(window, cx);
        if self.search.is_some() {
            self.refresh_search(cx);
        }
    }

    pub(super) fn clear_disk_notice(&mut self) {
        if self.notice.as_deref().is_some_and(|text| {
            let prefix = tree_refresh_prefix();
            !prefix.is_empty() && text.starts_with(prefix)
        }) {
            self.notice = None;
        }
    }

    /// Drop a selection the scan removed, then reload a clean open note.
    pub(super) fn reconcile_open_file(&mut self, window: &mut Window, cx: &mut Context<Root>) {
        self.drop_missing_selection();
        let Some(id) = self.open_note.clone() else {
            return;
        };
        let matches_saved = if self.index.contains(&id) {
            match self.vault.read(&id) {
                Ok(text) => {
                    let (kind, body) = note_body(&text);
                    Some(kind == self.open_kind && body == self.saved_text)
                }
                Err(_) => None,
            }
        } else {
            None
        };
        match open_file_sync(self.dirty, self.capture_status().is_some(), matches_saved) {
            OpenFileSync::Keep => {}
            OpenFileSync::Reload => {
                if let Err(err) = self.reload_open_note(id, window, cx) {
                    self.notice = Some(err);
                }
            }
            OpenFileSync::MissingMove => {
                if let Err(err) = self.settle_after_delete(window, cx) {
                    self.notice = Some(err);
                }
            }
            OpenFileSync::Notice(text) => {
                self.notice = Some(text.to_string());
                if matches_saved.is_none() && matches!(self.tree_selection, TreeSelection::Note(_))
                {
                    self.tree_selection = TreeSelection::None;
                }
            }
        }
    }

    pub(super) fn drop_missing_selection(&mut self) {
        match &self.tree_selection {
            TreeSelection::Note(id) if !tree_contains_note(&self.tree.children, id) => {
                self.tree_selection = TreeSelection::None;
            }
            TreeSelection::Folder(id)
                if !id.is_root() && !tree_contains_folder(&self.tree.children, id) =>
            {
                self.tree_selection = TreeSelection::None;
            }
            _ => {}
        }
    }
}

/// Draw the window after a vault scan.
///
/// Wayland stops asking for frames while the window is idle. `notify`
/// marks the view dirty and does not start a frame, so the last picture
/// stays up until a resize. This asks for the next frame.
pub(crate) fn present_scan(window: &mut Window) {
    window.refresh();
    window.on_next_frame(|_, _| {});
}

pub(super) fn tree_refresh_prefix() -> &'static str {
    locale::text("tree.refresh_error")
        .split("{message}")
        .next()
        .unwrap_or("")
}

/// Absolute path of `id` inside the vault at `root`.
#[cfg(any(test, target_os = "macos"))]
pub(super) fn folder_on_disk(root: &Path, id: &FolderId) -> std::path::PathBuf {
    if id.is_root() {
        root.to_path_buf()
    } else {
        root.join(id.as_str())
    }
}

pub(super) fn rewrite_note_id(note: &NoteId, from: &FolderId, to: &FolderId) -> Option<NoteId> {
    let prefix = format!("{}/", from.as_str());
    let rest = note.as_str().strip_prefix(&prefix)?;
    let next = if to.is_root() {
        rest.to_string()
    } else {
        format!("{}/{rest}", to.as_str())
    };
    NoteId::parse(&next).ok()
}

pub(super) fn rewrite_folder_id(id: &FolderId, from: &FolderId, to: &FolderId) -> Option<FolderId> {
    if id == from {
        return Some(to.clone());
    }
    let prefix = format!("{}/", from.as_str());
    let rest = id.as_str().strip_prefix(&prefix)?;
    let next = if to.is_root() {
        rest.to_string()
    } else {
        format!("{}/{rest}", to.as_str())
    };
    FolderId::parse(&next).ok()
}

pub(super) fn folder_contains(folder: &FolderId, child: &FolderId) -> bool {
    let prefix = format!("{}/", folder.as_str());
    child.as_str().starts_with(&prefix)
}

pub(super) fn note_in_folder(note: &NoteId, folder: &FolderId) -> bool {
    let prefix = format!("{}/", folder.as_str());
    note.as_str().starts_with(&prefix)
}

pub(super) fn disk_changed() -> &'static str {
    locale::text("notice.disk_changed")
}

pub(super) fn disk_removed() -> &'static str {
    locale::text("notice.disk_removed")
}

/// One background scan of the vault. The watch applies it on the UI thread.
pub(crate) struct DiskRefresh {
    tree: VaultTree,
    index: VaultIndex,
    stamp: VaultStamp,
}

/// Read the vault when `base` is stale.
///
/// `Ok(None)` means the file tree is already current.
///
/// # Errors
///
/// Returns the vault error when the folder cannot be read.
pub(crate) fn scan_vault(root: &Path, base: &VaultStamp) -> Result<Option<DiskRefresh>, Error> {
    let vault = Vault::open(root)?;
    let stamp = vault.stamp()?;
    if &stamp == base {
        return Ok(None);
    }
    Ok(Some(DiskRefresh {
        tree: vault.tree()?,
        index: vault.notes()?,
        stamp,
    }))
}

enum OpenFileSync {
    Keep,
    Reload,
    MissingMove,
    Notice(&'static str),
}

/// What to do with the open note after a scan.
///
/// `matches_saved` is `None` when the file is gone.
/// A clean note reloads. Unsaved text and a live recording stay on screen.
fn open_file_sync(dirty: bool, capturing: bool, matches_saved: Option<bool>) -> OpenFileSync {
    match matches_saved {
        Some(true) => OpenFileSync::Keep,
        Some(false) if capturing => OpenFileSync::Notice(recording_changed()),
        Some(false) if dirty => OpenFileSync::Notice(disk_changed()),
        Some(false) => OpenFileSync::Reload,
        None if capturing => OpenFileSync::Notice(recording_removed()),
        None if dirty => OpenFileSync::Notice(disk_removed()),
        None => OpenFileSync::MissingMove,
    }
}

pub(super) fn tree_contains_note(entries: &[runebook::TreeEntry], id: &NoteId) -> bool {
    entries.iter().any(|entry| match entry {
        runebook::TreeEntry::Note { id: note, .. } => note == id,
        runebook::TreeEntry::Folder { children, .. } => tree_contains_note(children, id),
    })
}

pub(super) fn tree_contains_folder(entries: &[runebook::TreeEntry], id: &FolderId) -> bool {
    entries.iter().any(|entry| match entry {
        runebook::TreeEntry::Folder {
            id: folder,
            children,
            ..
        } => folder == id || tree_contains_folder(children, id),
        runebook::TreeEntry::Note { .. } => false,
    })
}

#[cfg(test)]
mod disk_sync_tests {
    use super::{
        OpenFileSync, disk_changed, disk_removed, open_file_sync, recording_changed,
        recording_removed,
    };

    fn notice(sync: OpenFileSync) -> Option<&'static str> {
        match sync {
            OpenFileSync::Notice(text) => Some(text),
            _ => None,
        }
    }

    #[test]
    fn clean_note_reloads_when_the_file_changed() {
        assert!(matches!(
            open_file_sync(false, false, Some(false)),
            super::OpenFileSync::Reload
        ));
        assert!(matches!(
            open_file_sync(false, false, Some(true)),
            super::OpenFileSync::Keep
        ));
    }

    #[test]
    fn unsaved_text_stays_when_the_file_changed() {
        assert_eq!(
            notice(open_file_sync(true, false, Some(false))),
            Some(disk_changed())
        );
        assert!(matches!(
            open_file_sync(true, false, Some(true)),
            super::OpenFileSync::Keep
        ));
    }

    #[test]
    fn missing_clean_note_moves_and_a_dirty_note_stays() {
        assert!(matches!(
            open_file_sync(false, false, None),
            super::OpenFileSync::MissingMove
        ));
        assert_eq!(
            notice(open_file_sync(true, false, None)),
            Some(disk_removed())
        );
    }

    #[test]
    fn recording_blocks_a_reload() {
        assert_eq!(
            notice(open_file_sync(false, true, Some(false))),
            Some(recording_changed())
        );
        assert_eq!(
            notice(open_file_sync(true, true, None)),
            Some(recording_removed())
        );
    }
}

#[cfg(test)]
mod folder_path_tests {
    use super::folder_on_disk;
    use runebook::FolderId;
    use std::path::Path;

    #[test]
    fn the_root_id_is_the_vault_root() {
        let root = Path::new("/tmp/notes");
        assert_eq!(folder_on_disk(root, &FolderId::root()), root);
    }

    #[test]
    fn a_nested_id_joins_the_vault_root() {
        let id = FolderId::parse("journal/2026").unwrap();
        assert_eq!(
            folder_on_disk(Path::new("/tmp/notes"), &id),
            Path::new("/tmp/notes/journal/2026")
        );
    }
}
