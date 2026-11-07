//! One rule for generated journal IDs in every dump section, at the current checkpoint.
use super::*;

pub(super) fn journal_aliases(refs: &HashMap<String, JournalId>) -> HashMap<JournalId, String> {
    let mut names: HashMap<JournalId, String> = HashMap::new();
    for (name, id) in refs {
        let current = names.entry(*id).or_insert_with(|| name.clone());
        if name < current {
            *current = name.clone();
        }
    }
    names
}

impl Runner<'_> {
    pub(super) fn journal_names(&self) -> HashMap<String, String> {
        journal_aliases(&self.journals)
            .into_iter()
            .map(|(id, name)| (id.to_db(), name))
            .collect()
    }
}
