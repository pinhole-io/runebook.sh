use std::collections::BTreeMap;

use crate::links::{ResolveResult, WikiTarget};
use crate::note::NoteId;

/// Notes and the wiki targets each note contains.
///
/// The map is rebuilt from disk. It is not stored in the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultIndex {
    notes: BTreeMap<NoteId, Vec<WikiTarget>>,
}

impl VaultIndex {
    pub(crate) fn from_notes(notes: BTreeMap<NoteId, Vec<WikiTarget>>) -> Self {
        Self { notes }
    }

    /// `true` when `id` was in the last scan.
    pub fn contains(&self, id: &NoteId) -> bool {
        self.notes.contains_key(id)
    }

    /// Number of notes in the scan.
    pub fn len(&self) -> usize {
        self.notes.len()
    }

    /// `true` when the scan found no notes.
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }

    /// Note ids in sorted order.
    pub fn ids(&self) -> impl Iterator<Item = &NoteId> {
        self.notes.keys()
    }

    /// Wiki targets written in `id`. Missing notes return an empty slice.
    pub fn outgoing(&self, id: &NoteId) -> &[WikiTarget] {
        self.notes.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Resolve `target` by exact id, then by a unique file stem.
    ///
    /// A target that contains `/` never uses the stem rule.
    pub fn resolve(&self, target: &WikiTarget) -> ResolveResult {
        if let Ok(id) = NoteId::parse(target.as_str())
            && self.notes.contains_key(&id)
        {
            return ResolveResult::Exact(id);
        }
        if target.as_str().contains('/') {
            return ResolveResult::Missing;
        }
        let matches: Vec<NoteId> = self
            .notes
            .keys()
            .filter(|id| id.name() == target.as_str())
            .cloned()
            .collect();
        match matches.as_slice() {
            [] => ResolveResult::Missing,
            [only] => ResolveResult::UniqueStem(only.clone()),
            many => ResolveResult::Ambiguous(many.to_vec()),
        }
    }
}
