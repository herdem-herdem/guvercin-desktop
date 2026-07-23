//! Pure decision logic for two-way (local ⇄ Google) sync.
//!
//! Splitting the decisions out of the I/O makes the risky part — deciding what to
//! push, pull, or delete — unit-testable without a live Google account. The engine
//! in `google_sync` gathers state, calls these, and performs the resulting action.
//!
//! Conflict resolution is last-write-wins by millisecond timestamp. Deletes are
//! conservative: a remote item is only deleted when the user explicitly tombstoned
//! the local row, and a local row is only dropped for a remote deletion when it is
//! in-scope, clean, and confirmed absent from a successful full remote fetch.

/// Snapshot of a local row's sync bookkeeping.
#[derive(Clone, Copy, Debug)]
pub struct LocalState {
    pub dirty: bool,
    pub deleted: bool,
    pub has_remote_id: bool,
    /// Remote `updated` we recorded at the last successful sync (ms).
    pub remote_updated_ms: i64,
    /// When the row was last edited locally (ms).
    pub local_updated_ms: i64,
}

/// What to do with a local row that has a matching remote item this round.
#[derive(Debug, PartialEq, Eq)]
pub enum MatchAction {
    /// Nothing changed on either side.
    Noop,
    /// Remote is authoritative → overwrite the local row and clear `dirty`.
    PullOverwrite,
    /// Local edit wins → PATCH the remote item.
    UpdateRemote,
    /// Local row is tombstoned → delete the remote item, then drop the row.
    DeleteRemote,
}

/// What to do with a local row that has *no* matching remote item this round.
#[derive(Debug, PartialEq, Eq)]
pub enum OrphanAction {
    /// Locally created and never pushed → create it on Google.
    CreateRemote,
    /// Tombstoned and either never pushed or already gone remotely → just drop it.
    DropLocal,
    /// Had a remote id, still clean, now absent remotely → the remote was deleted,
    /// so drop the local row too.
    DeleteLocal,
    /// Had a remote id, absent remotely, but edited locally → local wins; recreate
    /// it on Google (as a fresh item).
    RecreateRemote,
    /// Nothing to do (defensive; e.g. a clean row with no remote id and no scope).
    Noop,
}

/// Decide for a local row matched to a remote item whose `updated` is `remote_ms`.
pub fn reconcile_matched(local: LocalState, remote_ms: i64) -> MatchAction {
    if local.deleted {
        return MatchAction::DeleteRemote;
    }
    if local.dirty {
        // Both sides may have changed since the last sync → last write wins.
        if remote_ms > local.local_updated_ms {
            MatchAction::PullOverwrite
        } else {
            MatchAction::UpdateRemote
        }
    } else if remote_ms > local.remote_updated_ms {
        // Remote moved on and we have no local edits.
        MatchAction::PullOverwrite
    } else {
        MatchAction::Noop
    }
}

/// Decide for a local row with no matching remote item. `in_scope` is whether the
/// row falls inside the fetched remote window (always true for full fetches; for a
/// windowed calendar pull, false rows are left untouched).
pub fn reconcile_orphan(local: LocalState, in_scope: bool) -> OrphanAction {
    if local.deleted {
        // Tombstone with the remote already gone (or never pushed): just drop it.
        return OrphanAction::DropLocal;
    }
    if !local.has_remote_id {
        // Locally created — push it regardless of window.
        return OrphanAction::CreateRemote;
    }
    if !in_scope {
        // Out of the fetched window: absence tells us nothing. Leave it alone.
        return OrphanAction::Noop;
    }
    if local.dirty {
        OrphanAction::RecreateRemote
    } else {
        OrphanAction::DeleteLocal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(dirty: bool, deleted: bool, has_remote_id: bool, remote_ms: i64, local_ms: i64) -> LocalState {
        LocalState { dirty, deleted, has_remote_id, remote_updated_ms: remote_ms, local_updated_ms: local_ms }
    }

    #[test]
    fn matched_clean_unchanged_is_noop() {
        assert_eq!(reconcile_matched(st(false, false, true, 100, 100), 100), MatchAction::Noop);
    }

    #[test]
    fn matched_remote_newer_pulls() {
        assert_eq!(reconcile_matched(st(false, false, true, 100, 100), 200), MatchAction::PullOverwrite);
    }

    #[test]
    fn matched_local_dirty_pushes_when_local_newer() {
        // dirty, remote unchanged since last sync → push local.
        assert_eq!(reconcile_matched(st(true, false, true, 100, 300), 100), MatchAction::UpdateRemote);
    }

    #[test]
    fn matched_conflict_remote_wins_when_remote_newer() {
        // Both changed; remote timestamp beats local edit time → remote wins.
        assert_eq!(reconcile_matched(st(true, false, true, 100, 250), 300), MatchAction::PullOverwrite);
    }

    #[test]
    fn matched_conflict_local_wins_when_local_newer() {
        assert_eq!(reconcile_matched(st(true, false, true, 100, 400), 300), MatchAction::UpdateRemote);
    }

    #[test]
    fn matched_tombstone_deletes_remote() {
        assert_eq!(reconcile_matched(st(false, true, true, 100, 100), 999), MatchAction::DeleteRemote);
        // Deleted takes priority even if dirty.
        assert_eq!(reconcile_matched(st(true, true, true, 100, 100), 999), MatchAction::DeleteRemote);
    }

    #[test]
    fn orphan_local_created_pushes() {
        assert_eq!(reconcile_orphan(st(true, false, false, 0, 100), true), OrphanAction::CreateRemote);
        // Even out of scope, a never-pushed local item should be created.
        assert_eq!(reconcile_orphan(st(true, false, false, 0, 100), false), OrphanAction::CreateRemote);
    }

    #[test]
    fn orphan_tombstone_drops() {
        assert_eq!(reconcile_orphan(st(false, true, true, 100, 100), true), OrphanAction::DropLocal);
        assert_eq!(reconcile_orphan(st(false, true, false, 0, 100), true), OrphanAction::DropLocal);
    }

    #[test]
    fn orphan_remote_deleted_clean_drops_local() {
        assert_eq!(reconcile_orphan(st(false, false, true, 100, 100), true), OrphanAction::DeleteLocal);
    }

    #[test]
    fn orphan_remote_deleted_but_local_dirty_recreates() {
        assert_eq!(reconcile_orphan(st(true, false, true, 100, 200), true), OrphanAction::RecreateRemote);
    }

    #[test]
    fn orphan_out_of_scope_is_left_alone() {
        // A synced, clean row outside the fetch window must never be deleted.
        assert_eq!(reconcile_orphan(st(false, false, true, 100, 100), false), OrphanAction::Noop);
    }
}
