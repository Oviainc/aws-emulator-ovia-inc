// SPDX-License-Identifier: CC0-1.0

//! What an update of one of an S3 Metadata configuration's tables must satisfy about the
//! JOURNAL table, before anything is written.
//!
//! The rule is the configuration's rather than one operation's: a metadata configuration is
//! built around its journal table — "`MetadataConfiguration$JournalTableConfiguration`" is
//! required, the inventory table "requires a journal table with an `ACTIVE` status"
//! ([`crate::state::METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE`], the published V2 failure
//! for one that has not got it) and the annotation table is read through the same
//! configuration — so none of its tables can be reconfigured until the journal table is in
//! service. Every operation that updates one of them
//! (`UpdateBucketMetadataJournalTableConfiguration`,
//! `UpdateBucketMetadataInventoryTableConfiguration`,
//! `UpdateBucketMetadataAnnotationTableConfiguration`) is judged by the one function here, so
//! the status, the code and the sentence a caller branches on are the same whichever table the
//! request was about.

use std::time::SystemTime;

use crate::state::{
    BucketMetadataConfiguration, METADATA_TABLE_STATUS_ACTIVE, METADATA_TABLE_STATUS_CREATING,
};

/// Which table of the configuration an update is about — what decides the sentence the
/// refusal carries, since S3 words the journal table's own update and an update of a table
/// that depends on it differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdatedTable {
    /// The journal table itself: `UpdateBucketMetadataJournalTableConfiguration`.
    Journal,
    /// A table that DEPENDS on the journal table — the inventory table, whose records are
    /// derived from the journal's, and the annotation table of the same configuration.
    DependingOnTheJournal,
}

/// The sentence S3 answers an update of a journal table that has not got an `ACTIVE` status
/// with: fixed text, naming neither the bucket nor the status the table is in.
pub const JOURNAL_TABLE_NOT_ACTIVE_MESSAGE: &str =
    "You can't perform this operation on a journal table that doesn't have an ACTIVE status. \
     Wait for your journal table status to transition to ACTIVE, and then try again. If \
     creation of the journal table fails, you must re-create the metadata configuration for \
     your general purpose bucket.";

/// The sentence S3 answers an update of a table that depends on the journal table with while
/// that journal table is still being created: fixed text, naming neither the bucket nor the
/// table the request was about.
pub const JOURNAL_TABLE_BEING_CREATED_MESSAGE: &str =
    "You can't perform this operation while the journal table is being created. Wait for your \
     journal table status to transition to ACTIVE, and then try again.";

/// Why an update of `table` of `configuration` cannot be carried out at `now`, or `None` when
/// the update may proceed.
///
/// The journal table reaches `ACTIVE` a span after the call that asked for it
/// ([`crate::state::MetadataTable::status_at`] decides which status it is in at an instant),
/// so this is read at the instant of the call and nothing waits for it. A refusal is a 400
/// [`crate::state::INVALID_REQUEST_CODE`]: the configuration is in a state this update cannot
/// be carried out in, not a payload S3 cannot parse.
///
/// The two tables are judged differently, because what the journal table's status means for
/// them differs:
///
/// * the journal table's OWN settings cannot be changed unless that table has an `ACTIVE`
///   status — the sentence says so, and says what to do when its creation failed, which is to
///   re-create the configuration;
/// * a table DEPENDING on the journal table cannot be reconfigured while the journal table is
///   still being created, because there is not yet a journal table for it to read from. A
///   journal table that FAILED is not that case: the dependent table is then created and
///   reports the failure as its OWN status, which is what the published `ErrorDetails` code
///   for a journal table without an `ACTIVE` status
///   ([`crate::state::METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE`]) exists to say — so the update
///   is carried out and the reader reports it.
pub fn journal_table_refusal(
    configuration: &BucketMetadataConfiguration,
    table: UpdatedTable,
    now: SystemTime,
) -> Option<&'static str> {
    let status = configuration.journal.status_at(now);
    match table {
        UpdatedTable::Journal => (status != METADATA_TABLE_STATUS_ACTIVE)
            .then_some(JOURNAL_TABLE_NOT_ACTIVE_MESSAGE),
        UpdatedTable::DependingOnTheJournal => (status == METADATA_TABLE_STATUS_CREATING)
            .then_some(JOURNAL_TABLE_BEING_CREATED_MESSAGE),
    }
}
