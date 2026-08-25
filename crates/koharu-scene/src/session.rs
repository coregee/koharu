use std::{collections::BTreeMap, path::Path, sync::Arc};

use bytes::Bytes;

use crate::{
    Change, Error, Generation, Patch, ProducerId, ProjectId, Result, Snapshot, TextGroup,
    patch::{Operation, apply_operations},
    state::{State, StoredState},
};

struct HistoryEntry {
    inverse: Arc<[Operation]>,
    // A revision's inverse may restore blobs that the current state no longer
    // references. Retain their lease for as long as that revision is undoable.
    _blobs: koharu_storage::Blobs,
}

/// One open project and its in-memory undo history.
///
/// Every successful commit publishes a complete scene snapshot. Undo commands
/// are retained only for the lifetime of this session; they are UI history, not
/// part of the durable project format.
pub struct Session {
    storage: koharu_storage::Session,
    current: Snapshot,
    history: BTreeMap<crate::Revision, HistoryEntry>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Session")
            .field("project", &self.project_id())
            .field("revision", &self.current.revision())
            .field("undoable", &self.history.len())
            .finish_non_exhaustive()
    }
}

impl Session {
    #[tracing::instrument(level = "info", skip_all, fields(path = %path.as_ref().display()))]
    pub async fn create(path: impl AsRef<Path>) -> Result<Self> {
        let document = koharu_storage::DocumentId::new();
        let state = State::empty(document);
        let storage = koharu_storage::Session::create(
            path,
            document,
            Bytes::from(encode_checkpoint(&state)?),
        )
        .await?;
        Self::assemble(storage, state).await
    }

    #[tracing::instrument(level = "info", skip_all, fields(path = %path.as_ref().display()))]
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let storage = koharu_storage::Session::open(path).await?;
        Self::recover(storage).await
    }

    pub async fn memory() -> Result<Self> {
        let document = koharu_storage::DocumentId::new();
        let state = State::empty(document);
        let storage =
            koharu_storage::Session::memory(document, Bytes::from(encode_checkpoint(&state)?))
                .await?;
        Self::assemble(storage, state).await
    }

    #[must_use]
    pub fn project_id(&self) -> ProjectId {
        ProjectId(self.current.state.document)
    }

    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.current.clone()
    }

    #[tracing::instrument(level = "info", skip_all, fields(project = %self.project_id(), base = %patch.base_revision))]
    pub async fn commit(&mut self, patch: Patch) -> Result<Commit> {
        if patch.project != self.current.state.document {
            return Err(Error::invalid("patch belongs to another project"));
        }
        if patch.base_revision != self.current.revision()
            || !Arc::ptr_eq(&patch.base_state, &self.current.state)
        {
            return Err(Error::Storage(koharu_storage::Error::RevisionConflict {
                current: self.current.revision(),
                proposed: patch.base_revision,
            }));
        }
        if patch.is_empty() {
            return Ok(Commit {
                revision: self.current.revision(),
                changes: Change::empty(self.current.revision()),
                snapshot: self.current.clone(),
            });
        }

        let next_revision = self
            .current
            .revision()
            .next()
            .ok_or_else(|| Error::invalid("project revision overflow"))?;
        let mut state = (*patch.state).clone();
        state.revision = next_revision;
        let proposed = self.current.storage.update(
            next_revision,
            Bytes::from(encode_checkpoint(&state)?),
            state.referenced_blobs(),
            patch.attachments.iter().cloned(),
        )?;
        let stored = self.storage.save(&proposed).await?;

        let inverse = patch
            .operations
            .iter()
            .rev()
            .map(Operation::reversed)
            .collect::<Vec<_>>();
        self.history.insert(
            next_revision,
            HistoryEntry {
                inverse: inverse.into(),
                _blobs: self.current.storage.blobs().clone(),
            },
        );

        let state = Arc::new(state);
        let snapshot = Snapshot::new(state, stored)?;
        let changes =
            Change::from_operations(patch.base_revision, next_revision, &patch.operations);
        self.current = snapshot.clone();
        Ok(Commit {
            revision: next_revision,
            changes,
            snapshot,
        })
    }

    pub async fn undo(&mut self, revision: crate::Revision) -> Result<Commit> {
        self.undo_many([revision]).await
    }

    #[tracing::instrument(level = "info", skip_all)]
    pub async fn undo_many(
        &mut self,
        revisions: impl IntoIterator<Item = crate::Revision>,
    ) -> Result<Commit> {
        let mut revisions = revisions.into_iter().collect::<Vec<_>>();
        revisions.sort_unstable_by(|left, right| right.cmp(left));
        revisions.dedup();
        if revisions.is_empty() {
            return Err(Error::invalid("undo requires at least one revision"));
        }
        let mut operations = Vec::new();
        for revision in &revisions {
            let entry = self.history.get(revision).ok_or_else(|| {
                Error::invalid(format!(
                    "revision {revision} is not undoable in this session"
                ))
            })?;
            operations.extend(entry.inverse.iter().cloned());
        }
        let state = apply_operations(&self.current.state, &operations)?;
        let label: Arc<str> = if revisions.len() == 1 {
            format!("Undo revision {}", revisions[0]).into()
        } else {
            format!("Undo {} revisions", revisions.len()).into()
        };
        let patch = Patch::new(
            &self.current,
            state,
            Vec::new(),
            operations,
            Vec::new(),
            Some(label),
        )?;
        self.commit(patch).await
    }

    pub async fn collect_garbage(&self) -> Result<koharu_storage::GcReport> {
        self.storage.collect_garbage().await.map_err(Into::into)
    }

    async fn assemble(storage: koharu_storage::Session, state: State) -> Result<Self> {
        let stored = storage.load().await?;
        let current = Snapshot::new(Arc::new(state), stored)?;
        Ok(Self {
            storage,
            current,
            history: BTreeMap::new(),
        })
    }

    async fn recover(storage: koharu_storage::Session) -> Result<Self> {
        let stored = storage.load().await?;
        let checkpoint: StoredState = revision::from_slice(stored.payload())?;
        let state = State::from_checkpoint(stored.document_id(), stored.revision(), checkpoint)?;
        let migrate_text_groups = state.has_outdated_component::<TextGroup>()?;
        if !migrate_text_groups {
            state.validate()?;
        }
        let current = Snapshot::new(Arc::new(state), stored)?;
        let mut session = Self {
            storage,
            current,
            history: BTreeMap::new(),
        };
        if migrate_text_groups {
            let generation =
                Generation::new(ProducerId::new("dev.koharu.scene.text-group-migration")?);
            let mut edit = session.current.edit_as(generation);
            edit.migrate_legacy_text_groups()?;
            let patch = edit.finish()?;
            patch.state.validate()?;
            session.commit(patch).await?;
            session.history.clear();
        }
        Ok(session)
    }
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub revision: crate::Revision,
    pub changes: Change,
    pub snapshot: Snapshot,
}

fn encode_checkpoint(state: &State) -> Result<Vec<u8>> {
    revision::to_vec(&state.to_checkpoint()).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use revision::revisioned;

    use super::*;
    use crate::{
        At, Component, Origin, PageDraft, TextGroupKind, TextLayout, TextLayoutKind, TextRole,
        component::{ValidationContext, encode, key},
    };

    #[revisioned(revision = 1)]
    #[derive(Clone)]
    struct LegacyTextGroup {}

    impl Component for LegacyTextGroup {
        const KIND: &'static str = TextGroup::KIND;
    }

    #[tokio::test]
    async fn opening_revision_one_text_groups_migrates_structure_and_payload() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy-text-groups.khrproj");
        let (page, layer, main, legacy_revision);
        {
            let mut session = Session::create(&path).await.unwrap();
            let mut created = None;
            let patch = session
                .snapshot()
                .patch(|edit| {
                    let page = edit.add_page(PageDraft::new("page", 500.0, 700.0), At::End)?;
                    let content = edit.add_text_content(page, At::End)?;
                    edit.set_text_role(
                        content,
                        Some(&TextRole {
                            origin: Origin::User,
                            role: TextRole::SFX.to_owned(),
                        }),
                    )?;
                    let layer = edit.add_text_layer(
                        page,
                        At::End,
                        content,
                        &TextLayout {
                            origin: Origin::User,
                            kind: TextLayoutKind::Paragraph,
                        },
                    )?;
                    created = Some((page, layer));
                    Ok(())
                })
                .unwrap();
            let snapshot = session.commit(patch).await.unwrap().snapshot;
            (page, layer) = created.unwrap();
            main = snapshot
                .page(page)
                .unwrap()
                .text_group(TextGroupKind::Main)
                .unwrap()
                .unwrap()
                .id();
            let sfx = snapshot
                .page(page)
                .unwrap()
                .text_group(TextGroupKind::Sfx)
                .unwrap()
                .unwrap()
                .id();

            let mut state = (*session.current.state).clone();
            let position = state.child_ids(main).unwrap().len();
            state.move_entity(layer, main, position).unwrap();
            state.remove_leaf(sfx).unwrap();
            let record_exists = |_id| true;
            let blob_exists = |_id| true;
            let legacy = encode(
                &LegacyTextGroup {},
                &ValidationContext::new(&record_exists, &blob_exists),
            )
            .unwrap();
            state
                .set_entity_component(main, key::<TextGroup>().unwrap(), legacy)
                .unwrap();
            legacy_revision = state.revision.next().unwrap();
            state.revision = legacy_revision;
            let proposed = session
                .current
                .storage
                .update(
                    legacy_revision,
                    Bytes::from(encode_checkpoint(&state).unwrap()),
                    state.referenced_blobs(),
                    [],
                )
                .unwrap();
            session.storage.save(&proposed).await.unwrap();
        }

        let session = Session::open(&path).await.unwrap();
        assert_eq!(
            session.snapshot().revision(),
            legacy_revision.next().unwrap()
        );
        let snapshot = session.snapshot();
        let sfx = snapshot
            .page(page)
            .unwrap()
            .text_group(TextGroupKind::Sfx)
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.parent(layer).unwrap(), Some(sfx.id()));
        assert_eq!(
            session
                .current
                .state
                .component(main, &key::<TextGroup>().unwrap())
                .unwrap()
                .unwrap()
                .schema,
            u32::from(<TextGroup as revision::Revisioned>::revision()),
        );
        assert!(session.history.is_empty());
        let migrated_revision = snapshot.revision();
        drop(snapshot);
        drop(session);

        let reopened = Session::open(&path).await.unwrap();
        assert_eq!(reopened.snapshot().revision(), migrated_revision);
    }
}
