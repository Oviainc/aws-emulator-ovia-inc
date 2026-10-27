// SPDX-License-Identifier: CC0-1.0

//! The ONE single-record lookup and the ONE listing projection every resource family
//! of this service is served through, driven end to end over the wire by a read and a
//! listing of each family.
//!
//! THE RULE UNDER TEST, single-homed in `crate::state` (`record_mut`, `newest_first`,
//! `materialised_newest_first`) and reached by every family's reads and listings: both
//! a read of one record and a listing of a Region's records report the status the CLOCK
//! already owed each record — materialised before anything is decided or filtered on it
//! — and a listing reports those records newest first. One family's reader keeping the
//! clock step while another's lost it, or one family ordering its page differently,
//! would show up here as a read and a listing of the same record disagreeing about its
//! status, as a status filter selecting on a status the record has already left, or as
//! a page in the wrong order.
//!
//! The rows differ along what the rule generalises over: the resource FAMILY (and so
//! the record type, the operations that read and list it, the member its status is
//! spelled in, the member its listing filters that status with, the state a new record
//! settles in and whether its records have a clock-owed transition at all), and the
//! phase of the lifecycle the same record is observed in.
//!
//! The listings, their creates, the names created and the order each listing owes are
//! `crate::list_paging_rule`'s own table, reused rather than copied — this suite adds
//! only the columns that table does not carry, the status members and the states a
//! record settles in. Every call goes through the same `MultiServiceRouter` the
//! production router dispatches on, reusing the transcription-job suite's harness
//! (`crate::transcription_jobs`). The Region scoping of these same listings is pinned
//! by `crate::regional_resource_scoping`.

use std::time::Duration;

use serde_json::json;

use crate::list_paging_rule::{listings, Listing, CREATED_IN_ORDER};
use crate::transcription_jobs::Transcribe;

/// Long enough for the transition the clock owes a record created during the test to
/// have fallen due: every family's span is 500 ms
/// (`crate::state::JOB_TRANSITION_DELAY`, `VOCABULARY_TRANSITION_DELAY`,
/// `MODEL_TRANSITION_DELAY`), and a job that entered its family's queue would take two
/// of them.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_EVERY_TRANSITION: Duration = Duration::from_millis(1_300);

/// What a family whose records have a clock-owed transition adds to its
/// [`Listing`]: the read that reports one record's status, the member the listing
/// filters that status with, and the two statuses the clock moves such a record
/// between.
struct Clocked {
    /// The listing of [`listings`] this row belongs to.
    list: &'static str,
    /// The operation that reports ONE record of the family.
    read: &'static str,
    /// The member that read names the record in.
    read_name_member: &'static str,
    /// The member of the answer carrying the record, or `None` for a read whose answer
    /// carries the status itself.
    read_record: Option<&'static str>,
    /// The member that read spells the status in.
    read_status_member: &'static str,
    /// The request member the listing's status filter is spelled with.
    status_filter_member: &'static str,
    /// The status a record just created is in, before its transition falls due.
    pending: &'static str,
    /// The status it is in once the clock has caught up with it.
    settled: &'static str,
}

/// Every family whose records the model gives a status or state the clock moves, with
/// the members each one models (`smithy_query(query="shape", shape="\
/// TranscriptionJobSummary,MedicalTranscriptionJobSummary,MedicalScribeJobSummary,\
/// CallAnalyticsJobSummary,VocabularyInfo,LanguageModel")` and
/// `smithy_query(query="operation", operation="GetVocabulary,GetMedicalVocabulary,\
/// DescribeLanguageModel,ListVocabularies,ListLanguageModels")`).
///
/// A listing of [`listings`] with no row here is one whose records have no transition
/// to materialise at all — a custom vocabulary filter has no `VocabularyState` and a
/// Call Analytics category no status, and neither listing publishes a status filter —
/// so for those the rule is the ordering alone.
///
/// The `pending` and `settled` states are those of this emulator's own recorded
/// lifecycle: a job processes and then completes; a custom vocabulary built from the
/// terms the request listed becomes usable, while one whose terms would have had to be
/// READ from a file (every medical one, which models no inline alternative) ends in the
/// service's modelled failure state, because nothing read that file.
fn clocked_families() -> Vec<Clocked> {
    vec![
        Clocked {
            list: "ListTranscriptionJobs",
            read: "GetTranscriptionJob",
            read_name_member: "TranscriptionJobName",
            read_record: Some("TranscriptionJob"),
            read_status_member: "TranscriptionJobStatus",
            status_filter_member: "Status",
            pending: "IN_PROGRESS",
            settled: "COMPLETED",
        },
        Clocked {
            list: "ListMedicalTranscriptionJobs",
            read: "GetMedicalTranscriptionJob",
            read_name_member: "MedicalTranscriptionJobName",
            read_record: Some("MedicalTranscriptionJob"),
            read_status_member: "TranscriptionJobStatus",
            status_filter_member: "Status",
            pending: "IN_PROGRESS",
            settled: "COMPLETED",
        },
        Clocked {
            list: "ListMedicalScribeJobs",
            read: "GetMedicalScribeJob",
            read_name_member: "MedicalScribeJobName",
            read_record: Some("MedicalScribeJob"),
            read_status_member: "MedicalScribeJobStatus",
            status_filter_member: "Status",
            pending: "IN_PROGRESS",
            settled: "COMPLETED",
        },
        Clocked {
            list: "ListCallAnalyticsJobs",
            read: "GetCallAnalyticsJob",
            read_name_member: "CallAnalyticsJobName",
            read_record: Some("CallAnalyticsJob"),
            read_status_member: "CallAnalyticsJobStatus",
            status_filter_member: "Status",
            pending: "IN_PROGRESS",
            settled: "COMPLETED",
        },
        Clocked {
            list: "ListVocabularies",
            read: "GetVocabulary",
            read_name_member: "VocabularyName",
            // `GetVocabularyResponse` carries `VocabularyState` itself rather than a
            // record member.
            read_record: None,
            read_status_member: "VocabularyState",
            status_filter_member: "StateEquals",
            pending: "PENDING",
            // The reused table creates this family from the terms in `Phrases`, which
            // is a build this emulator can finish.
            settled: "READY",
        },
        Clocked {
            list: "ListMedicalVocabularies",
            read: "GetMedicalVocabulary",
            read_name_member: "VocabularyName",
            read_record: None,
            read_status_member: "VocabularyState",
            status_filter_member: "StateEquals",
            pending: "PENDING",
            settled: "FAILED",
        },
        Clocked {
            list: "ListLanguageModels",
            read: "DescribeLanguageModel",
            read_name_member: "ModelName",
            read_record: Some("LanguageModel"),
            read_status_member: "ModelStatus",
            status_filter_member: "StatusEquals",
            pending: "IN_PROGRESS",
            // No object store answers for the training data the reused create names
            // in this suite's emulator, and nothing is failed on an assumption, so
            // the training run settles in the status that makes a model usable
            // (`crate::state::TrainingDataVerdict::settles`).
            settled: "COMPLETED",
        },
    ]
}

/// The row of [`clocked_families`] for `listing`, or `None` for a listing whose records
/// have no clock-owed transition.
fn clocked_for<'rows>(rows: &'rows [Clocked], listing: &Listing) -> Option<&'rows Clocked> {
    rows.iter().find(|row| row.list == listing.operation)
}

impl Clocked {
    /// The status this family's single-record read reports for `name`.
    async fn read_status(&self, transcribe: &Transcribe, name: &str, case: &str) -> String {
        let reply = transcribe
            .call(self.read, json!({self.read_name_member: name}))
            .await;
        let body = reply.ok(case);
        let record = match self.read_record {
            Some(member) => body
                .get(member)
                .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {body:?}")),
            None => body,
        };
        record[self.read_status_member]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a {} in the answer, got {record:?}",
                    self.read_status_member
                )
            })
            .to_string()
    }
}

/// Assert that `listing`'s page, narrowed to the records whose status is `status`, is
/// exactly `expected` — in the order this listing owes.
async fn filtered_page_is(
    transcribe: &Transcribe,
    listing: &Listing,
    clocked: &Clocked,
    status: &str,
    expected: &[String],
    case: &str,
) {
    let case = format!("{case} / {}={status}", clocked.status_filter_member);
    let (names, token) = listing
        .page(
            transcribe,
            json!({clocked.status_filter_member: status, "MaxResults": 100}),
            &case,
        )
        .await;
    assert_eq!(
        names, expected,
        "case {case:?}: the listing must select on the status the clock owed each \
         record, and report them in its own order, got {names:?}"
    );
    assert!(
        token.is_none(),
        "case {case:?}: a page holding every match carries no cursor, got {token:?}"
    );
}

/// RULE: the read of one record and the listing of its family both report the status
/// the clock already owed it — before the transition falls due and after it — and the
/// listing reports its records in its own order whichever status they are in. A family
/// whose records have no transition at all is served by that ordering alone.
#[tokio::test]
async fn every_family_s_read_and_listing_report_the_clock_owed_status() {
    let listings = listings();
    let rows = clocked_families();
    let transcribe = Transcribe::new();
    // The record every family's read is driven through: the first name created, which
    // is the OLDEST of its family, so the order the listing reports is observed on a
    // record the page does not happen to start with.
    let read_back = CREATED_IN_ORDER[0];
    let nothing: Vec<String> = Vec::new();

    // Each family is created and then observed straight away, before any span of it
    // has elapsed: nothing sleeps inside the service, so a record just created is
    // still in the status it started in, through either operation.
    for listing in &listings {
        listing.create_the_records(&transcribe).await;
        let expected = listing.expected_order();
        let case = format!("{} before the transition", listing.operation);

        let Some(clocked) = clocked_for(&rows, listing) else {
            // No status to materialise, so the whole rule is the order: the records
            // this Region holds, newest first.
            let (names, _) = listing
                .page(&transcribe, json!({"MaxResults": 100}), &case)
                .await;
            assert_eq!(
                names, expected,
                "case {case:?}: the listing must report its records in its own order, \
                 got {names:?}"
            );
            continue;
        };

        let read = clocked.read_status(&transcribe, read_back, &case).await;
        assert_eq!(
            read, clocked.pending,
            "case {case:?}: {} must report {:?} for a record just created, got {read:?}",
            clocked.read, clocked.pending
        );
        // The listing agrees with the read, for every record of the family: filtering
        // on the status they are in keeps them all, and filtering on the one they have
        // not reached yet keeps none.
        filtered_page_is(
            &transcribe,
            listing,
            clocked,
            clocked.pending,
            &expected,
            &case,
        )
        .await;
        filtered_page_is(
            &transcribe,
            listing,
            clocked,
            clocked.settled,
            &nothing,
            &case,
        )
        .await;
    }

    // One span for every family: the transition was due at an instant stored on each
    // record whether or not anybody called, so waiting it out once settles them all.
    std::thread::sleep(PAST_EVERY_TRANSITION);

    for listing in &listings {
        let Some(clocked) = clocked_for(&rows, listing) else {
            continue;
        };
        let expected = listing.expected_order();
        let case = format!("{} after the transition", listing.operation);

        let read = clocked.read_status(&transcribe, read_back, &case).await;
        assert_eq!(
            read, clocked.settled,
            "case {case:?}: {} must report {:?} once the transition fell due, got \
             {read:?}",
            clocked.read, clocked.settled
        );
        // And the listing has materialised the same transition for every record it
        // reports, before its filter selected on the status: the settled status now
        // keeps them all and the status they have left keeps none.
        filtered_page_is(
            &transcribe,
            listing,
            clocked,
            clocked.settled,
            &expected,
            &case,
        )
        .await;
        filtered_page_is(
            &transcribe,
            listing,
            clocked,
            clocked.pending,
            &nothing,
            &case,
        )
        .await;
    }
}
