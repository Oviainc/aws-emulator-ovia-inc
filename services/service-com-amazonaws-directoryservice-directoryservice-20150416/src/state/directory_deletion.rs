// SPDX-License-Identifier: CC0-1.0

//! DIRECTORY TEARDOWN: the stages a deleted directory passes through, and the clock that
//! takes its record away once they have run out.
//!
//! `DeleteDirectory` does not take the directory away while the caller waits. The modelled
//! `DirectoryStage` enum declares [`STAGE_DELETING`] and [`STAGE_DELETED`] for exactly this
//! window, and `DescribeDirectories` keeps DESCRIBING the directory in them: a directory
//! being torn down still has servers to decommission, so what the delete does is move the
//! directory OUT of the stage it was serving in and into the teardown — which is the
//! progress every delete-and-wait client polls for, and what lets such a client tell a
//! delete it started from an id that never existed.
//!
//! So the teardown is a CHAIN the clock owes, built the same way every other transient
//! state of this service is (see [`TRANSITION_SPAN`]):
//!
//! 1. the delete records [`STAGE_DELETING`], with the next step owed one span later;
//! 2. that step settles the stage on [`STAGE_DELETED`], the terminal value of the enum, and
//!    holds the record one more span so a caller can read the settled stage;
//! 3. the record then leaves the store, taking with it everything that describes nothing
//!    without it — the directory's snapshots, the trusts it is one party to and the shares
//!    consumer accounts hold on it — and the id is remembered as one this account HAD, which
//!    is what makes a later reporting call filtered by it an empty page rather than a
//!    refusal.
//!
//! Nothing here sleeps: each step stores the instant the next one falls due, and
//! [`ComAmazonawsDirectoryserviceDirectoryservice20150416State::advance_clock`] materialises
//! whatever is already owed, so the chain runs on the delete's own clock however long ago it
//! ran and whether or not anybody calls again.

use super::*;

/// The stage a directory is in while `DeleteDirectory`'s teardown runs: the `Deleting`
/// value of the modelled `DirectoryStage` enum
/// (`smithy_query(query="shape",shape="DirectoryStage")`).
///
/// A directory in this stage is still one `DescribeDirectories` reports, and no longer one
/// any stage-gated operation serves.
pub const STAGE_DELETING: &str = "Deleting";

/// The stage a deleted directory settles on before its record leaves the store: the
/// `Deleted` value of the same modelled enum, the terminal end of the teardown.
pub const STAGE_DELETED: &str = "Deleted";

impl ComAmazonawsDirectoryserviceDirectoryservice20150416State {
    /// Start the teardown of the directory `directory_id`, as of `now`, answering the id of
    /// the directory the delete applies to — or `None` when no directory is stored under it,
    /// which is the caller's cue to refuse the request.
    ///
    /// THE one place a delete is recorded: it moves the directory into [`STAGE_DELETING`]
    /// and owes the rest of the chain to the clock ([`materialise_directory_deletions`],
    /// reached through `advance_clock`), so the record is still there for the next
    /// `DescribeDirectories` to report and the id is not yet free.
    ///
    /// A directory already being torn down is moved no further: the teardown it is in keeps
    /// the instant IT was started from, so a repeated delete neither restarts the chain nor
    /// pushes the removal further out.
    pub fn begin_directory_deletion(
        &mut self,
        directory_id: &str,
        now: SystemTime,
    ) -> Option<String> {
        let directory = self.directories.get_mut(directory_id)?;
        if !DIRECTORY_TEARDOWN_STAGES.contains(&directory.stage.as_str()) {
            directory.stage = STAGE_DELETING.to_string();
            directory.stage_last_updated = now;
            directory.stage_due_at = Some(now + TRANSITION_SPAN);
        }
        Some(directory.directory_id.clone())
    }

    /// Materialise every step of a directory teardown the clock has already made due at
    /// `now`: the move to [`STAGE_DELETED`], and then the record leaving the store.
    ///
    /// Called from `advance_clock` alone, after the stage chain a CREATE owes, so one pass
    /// carries a directory whose delete ran long enough ago all the way out.
    pub(super) fn materialise_directory_deletions(&mut self, now: SystemTime) {
        for directory in self.directories.values_mut() {
            let Some(due) = directory.stage_due_at else {
                continue;
            };
            if now >= due && directory.stage == STAGE_DELETING {
                directory.stage = STAGE_DELETED.to_string();
                directory.stage_last_updated = due;
                // The settled stage is held for one more span, so the terminal value of
                // the teardown is one a caller polling the delete can actually read before
                // the record goes.
                directory.stage_due_at = Some(due + TRANSITION_SPAN);
            }
        }
        // Nothing is allocated unless a record really is owed removal, which is the common
        // case on every other call: the predicate is asked first, and only then is the set
        // of ids collected — BEFORE any of them is taken out, so the cascade below borrows
        // the store on its own.
        let owed = |directory: &&Directory| {
            directory.stage == STAGE_DELETED
                && directory.stage_due_at.is_some_and(|due| now >= due)
        };
        if !self.directories.values().any(|directory| owed(&directory)) {
            return;
        }
        let removed: Vec<(String, u64)> = self
            .directories
            .values()
            .filter(owed)
            .map(|directory| (directory.directory_id.clone(), directory.seq))
            .collect();
        for (directory_id, seq) in removed {
            self.forget_directory(&directory_id, seq);
        }
    }

    /// Take the record of the directory `directory_id`, created at rank `seq`, out of the
    /// store, with everything that describes nothing without it.
    ///
    /// The alias index is deliberately NOT touched: an alias "cannot be deleted or reused",
    /// so it stays reserved against the id it was created for and a later `CreateAlias`
    /// asking for it is refused.
    ///
    /// The id itself is remembered, and nothing else of the directory: an account that HAD
    /// this directory is what makes a later reporting call filtered by this id an empty
    /// result rather than a refusal, while an id this account never had stays a refusal.
    fn forget_directory(&mut self, directory_id: &str, seq: u64) {
        self.directories.remove(directory_id);
        self.remember_deleted_directory(directory_id, seq);
        // A snapshot is a snapshot OF this directory, so it does not outlive it: real
        // Directory Service deletes a directory's snapshots with the directory ("all
        // snapshots of a directory are deleted when the directory is deleted"), and keeping
        // them would leave records naming an id no caller can resolve — and a store a
        // create/delete cycle could grow without end.
        self.snapshots
            .retain(|_, snapshot| snapshot.directory_id != directory_id);
        // A trust is the Amazon Web Services side of a relationship this directory is one
        // party to, so it does not outlive the directory either.
        self.trusts
            .retain(|_, trust| trust.directory_id != directory_id);
        // A directory share is the consumer account's handle on THIS directory, so it does
        // not outlive it either: keeping it would leave records naming an owner id no caller
        // can resolve.
        self.shared_directories
            .retain(|_, share| share.owner_directory_id != directory_id);
    }
}

/// The stages a directory whose teardown has already started is in.
///
/// What they decide: no further delete restarts the chain from one of them, and a directory
/// in one of them is no longer in service — so it is not a directory this service serves a
/// DNS domain for
/// ([`ComAmazonawsDirectoryserviceDirectoryservice20150416State::domains_served`]), even
/// while the control plane still describes it.
pub(super) const DIRECTORY_TEARDOWN_STAGES: [&str; 2] = [STAGE_DELETING, STAGE_DELETED];
