// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for RESUMING a page token of a listing whose filter stops admitting
//! the records it admitted when the token was issued.
//!
//! The rule: a page token this service issued indexes the sequence of records the store
//! HOLDS for that listing, not the shorter subsequence one request's filter leaves of it,
//! so the token stays resumable while that filter's membership changes under it — a
//! status the clock advances with no caller involved — and the page it resumes carries
//! the records that still match, down to an empty last page with no token. A refusal
//! stays reserved for a token this service could not have issued: one issued for another
//! sequence, or one whose offset lies outside the held sequence.
//!
//! Each test asserts that rule over a TABLE whose rows differ in what the rule
//! generalises over — which of the two filtered listings is paged, which transient status
//! the filter names, how many records are held and how big a page is — never over one
//! cited request. Every row takes the same shape: records are brought into a transient
//! status, the first page is served under the filter naming it, the clock is then carried
//! past the instant each record's next status is owed from, and the token the service
//! handed out is followed to exhaustion.
//!
//! The HTTP harness (`emulator_with_engine`, `call`, `Answer`) and the export-listing
//! helpers (`seed_job`, `passable_role`) are the ones the sibling suites already use:
//! every suite drives the same emulator the same way.

use std::time::UNIX_EPOCH;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_listing::{passable_role, seed_job};
use crate::search_jobs::{
    call, emulator_with_engine, Answer, PAST_THE_CREATING_INSTANT, PAST_THE_DUE_INSTANT,
};

/// The region every call in this suite is signed for — the one `seed_job` mints its ARNs
/// in, so a seeded record and a called one belong to the same listing.
const REGION: &str = "us-west-2";

/// Which of the two filtered job listings of this service a row pages.
#[derive(Clone, Copy, Debug)]
enum Listing {
    SearchJobs,
    ExportJobs,
}

impl Listing {
    /// The resource path the listing is served at.
    fn path(self) -> &'static str {
        match self {
            Listing::SearchJobs => "/search-jobs",
            Listing::ExportJobs => "/export-search-jobs",
        }
    }

    /// The output member carrying the page.
    fn page_member(self) -> &'static str {
        match self {
            Listing::SearchJobs => "SearchJobs",
            Listing::ExportJobs => "ExportJobs",
        }
    }

    /// The summary member naming the record, which is what a walk collects to check that
    /// no record is served twice.
    fn identifier_member(self) -> &'static str {
        match self {
            Listing::SearchJobs => "SearchJobIdentifier",
            Listing::ExportJobs => "ExportJobIdentifier",
        }
    }
}

/// One row of the table: a listing, the TRANSIENT status its filter names, how many
/// records are held in that status and the page size the walk uses.
#[derive(Clone, Copy, Debug)]
struct Row {
    listing: Listing,
    /// A status the clock moves a record out of on its own: `SearchJobState` RUNNING and
    /// STOPPING for a search job, `ExportJobStatus` RUNNING for an export job (the
    /// non-terminal values of those model enums, i.e. the ones outside
    /// `state::TERMINAL_STATES`).
    transient_status: &'static str,
    record_count: usize,
    max_results: usize,
}

/// The rows of the table: both listings, every transient status each of them reports, two
/// record counts and two page sizes.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (listing, transient_status) in [
        (Listing::SearchJobs, state::RUNNING),
        (Listing::SearchJobs, state::STOPPING),
        (Listing::ExportJobs, state::RUNNING),
    ] {
        for record_count in [2_usize, 5] {
            for max_results in [1_usize, 2] {
                rows.push(Row {
                    listing,
                    transient_status,
                    record_count,
                    max_results,
                });
            }
        }
    }
    rows
}

/// Start `count` search jobs and answer their identifiers, newest-created LAST.
async fn start_search_jobs(emulator: &MultiServiceRouter, case: &str, count: usize) -> Vec<String> {
    let mut identifiers = Vec::new();
    for index in 0..count {
        let started = call(
            emulator,
            REGION,
            "PUT",
            "/search-jobs",
            Some(json!({
                "Name": format!("job-{index}"),
                "SearchScope": {"BackupResourceTypes": ["S3"]}
            })),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartSearchJob must succeed to set up the row, got {} ({:?})",
            started.status, started.body
        );
        identifiers.push(started.string("SearchJobIdentifier"));
    }
    identifiers
}

/// Start `count` export jobs of one settled search job and answer their identifiers. The
/// destination bucket differs per index, so no two starts are the same request.
async fn start_export_jobs(
    emulator: &MultiServiceRouter,
    case: &str,
    search_job_identifier: &str,
    count: usize,
) -> Vec<String> {
    let mut identifiers = Vec::new();
    for index in 0..count {
        let started = call(
            emulator,
            REGION,
            "PUT",
            "/export-search-jobs",
            Some(json!({
                "SearchJobIdentifier": search_job_identifier,
                "ExportSpecification": {"s3ExportSpecification": {
                    "DestinationBucket": format!("exports-{index}")
                }},
                "RoleArn": passable_role("export")
            })),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartSearchResultExportJob must succeed to set up the row, got {} ({:?})",
            started.status, started.body
        );
        identifiers.push(started.string("ExportJobIdentifier"));
    }
    identifiers
}

/// `StopSearchJob` for every identifier, which is what puts a search job in the transient
/// STOPPING status the clock then carries to STOPPED.
async fn cancel_all(emulator: &MultiServiceRouter, case: &str, identifiers: &[String]) {
    for identifier in identifiers {
        let cancelled = call(
            emulator,
            REGION,
            "PUT",
            &format!("/search-jobs/{identifier}/actions/cancel"),
            None,
        )
        .await;
        assert!(
            cancelled.status < 300,
            "case {case:?}: StopSearchJob must succeed to set up the row, got {} ({:?})",
            cancelled.status,
            cancelled.body
        );
    }
}

/// Bring the records of `engine` into the state the transient status of `row` is reported
/// from, and answer the identifiers held in it.
async fn hold_records_in_the_transient_status(
    emulator: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    case: &str,
    row: Row,
) -> Vec<String> {
    match (row.listing, row.transient_status) {
        (Listing::SearchJobs, state::STOPPING) => {
            let identifiers = start_search_jobs(emulator, case, row.record_count).await;
            cancel_all(emulator, case, &identifiers).await;
            identifiers
        }
        (Listing::SearchJobs, _) => {
            let identifiers = start_search_jobs(emulator, case, row.record_count).await;
            // A started job is CREATING and owes RUNNING next, so carrying the clock past
            // that one owed instant is what the next call reports as RUNNING.
            carry_the_clock_past_the_next_owed_status(engine);
            identifiers
        }
        (Listing::ExportJobs, _) => {
            let search_job = seed_job(engine, state::COMPLETED, Vec::new());
            start_export_jobs(emulator, case, &search_job, row.record_count).await
        }
    }
}

/// Carry the clock past the instant the NEXT status of every held record is owed from, so
/// the next call materialises it.
///
/// What a test does instead of waiting for a span to elapse: the transition is still
/// applied by the service's own `settle`, on the next call, out of the due instant the
/// record holds — the same path a span that elapses on its own takes. Only the FRONT of
/// what a record is owed is brought forward, because a search job owes two statuses in
/// turn and a row observes each of them.
fn carry_the_clock_past_the_next_owed_status(engine: &engine::InMemoryEngine) {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let mut store = store.lock();
    for job in store.search_jobs.values_mut() {
        if let Some(owed) = job.owed_statuses.front_mut() {
            owed.0 = UNIX_EPOCH;
        }
    }
    for export in store.export_jobs.values_mut() {
        if let Some(owed) = export.next_status_due.as_mut() {
            owed.0 = UNIX_EPOCH;
        }
    }
}

/// One call of the listing of `row`, with the status filter in force and `token` resuming
/// it when given.
async fn list_page(
    emulator: &MultiServiceRouter,
    row: Row,
    token: Option<&str>,
) -> Answer {
    let filters = format!(
        "?Status={}&MaxResults={}",
        row.transient_status, row.max_results
    );
    let uri = match token {
        Some(token) => format!("{}{filters}&NextToken={token}", row.listing.path()),
        None => format!("{}{filters}", row.listing.path()),
    };
    call(emulator, REGION, "GET", &uri, None).await
}

/// The entries of one answered page, as (identifier, status) pairs in the order the page
/// holds them.
fn page_entries(answer: &Answer, row: Row) -> Vec<(String, String)> {
    answer
        .get(row.listing.page_member())
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            (
                summary[row.listing.identifier_member()]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                summary["Status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Assert the invariants that hold of EVERY page of a filtered listing, and answer its
/// entries and the token it carried.
fn page_invariants(answer: &Answer, row: Row, case: &str) -> (Vec<(String, String)>, Option<String>) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a page of a filtered listing must be answered, got {} ({:?})",
        answer.status, answer.body
    );
    let entries = page_entries(answer, row);
    assert!(
        entries.len() <= row.max_results,
        "case {case:?}: a page must hold at most MaxResults entries, got {}",
        entries.len()
    );
    for (identifier, status) in &entries {
        assert_eq!(
            status, row.transient_status,
            "case {case:?}: the filter admits only records in that status, got {identifier:?} in {status:?}"
        );
    }
    let token = answer.get("NextToken").as_str().map(ToString::to_string);
    assert_eq!(
        token.is_some(),
        entries.len() == row.max_results,
        "case {case:?}: a page of {} entries must {}carry a NextToken, got {:?}",
        entries.len(),
        if entries.len() == row.max_results { "" } else { "not " },
        answer.get("NextToken")
    );
    (entries, token)
}

/// The sequence part of a token this service issued: a token is an offset into the held
/// sequence and the name of that sequence, joined by `@` (`state::encode_next_token`).
fn token_sequence(token: &str) -> &str {
    token
        .split_once('@')
        .map(|(_, sequence)| sequence)
        .unwrap_or_default()
}

#[tokio::test]
async fn a_token_issued_for_a_transient_status_filter_resumes_after_the_clock_moves_records_out_of_it(
) {
    for row in rows() {
        let case = format!(
            "{:?} filtered by {}, {} records, MaxResults={}",
            row.listing, row.transient_status, row.record_count, row.max_results
        );
        let (emulator, engine) = emulator_with_engine();
        let held = hold_records_in_the_transient_status(&emulator, &engine, &case, row).await;

        // The first page, served while the filter still admits every held record.
        let first = list_page(&emulator, row, None).await;
        let (first_entries, token) = page_invariants(&first, row, &case);
        assert_eq!(
            first_entries.len(),
            row.max_results.min(row.record_count),
            "case {case:?}: the first page must carry as many records as the filter and the page size allow, got {first_entries:?}"
        );
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a full first page must carry a NextToken, got {:?}", first.body)
        });

        // The clock now makes each record's next status due, with no caller involved, so
        // the filter stops admitting the records the token was issued over.
        carry_the_clock_past_the_next_owed_status(&engine);

        // The token the service issued is followed to exhaustion. Every page is answered,
        // every entry it carries still matches the filter, and no record is served twice.
        let mut seen: Vec<String> = first_entries
            .iter()
            .map(|(identifier, _)| identifier.clone())
            .collect();
        let mut token = Some(token);
        let mut pages = 0;
        while let Some(resuming) = token {
            let answer = list_page(&emulator, row, Some(&resuming)).await;
            assert_ne!(
                answer.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: a token this service issued must not be refused once the clock moves records out of the filter, got {:?}",
                answer.body
            );
            let (entries, next) = page_invariants(&answer, row, &case);
            for (identifier, _) in entries {
                assert!(
                    !seen.contains(&identifier),
                    "case {case:?}: paging must serve a record at most once, saw {identifier:?} twice"
                );
                seen.push(identifier);
            }
            token = next;
            pages += 1;
            assert!(
                pages <= row.record_count + 2,
                "case {case:?}: paging must terminate, took {pages} pages after the status moved"
            );
        }
        for identifier in &seen {
            assert!(
                held.contains(identifier),
                "case {case:?}: a page may only carry records this store holds, got {identifier:?}"
            );
        }

        // The control: the records really did leave the filter, so a fresh walk of the
        // same filtered listing now finds nothing.
        let fresh = list_page(&emulator, row, None).await;
        let (fresh_entries, fresh_token) = page_invariants(&fresh, row, &case);
        assert!(
            fresh_entries.is_empty() && fresh_token.is_none(),
            "case {case:?}: the clock has moved every record out of the filter, so the filtered listing must now be empty, got {:?}",
            fresh.body
        );
    }
}

#[tokio::test]
async fn a_token_whose_offset_lies_outside_the_held_sequence_is_still_refused() {
    // The rule's other half: widening what a GENUINE token can resume does not make an
    // offset the service never handed out resumable. The rows take the sequence name out
    // of a token the service DID issue for each listing and put an offset no page of it
    // could have ended at in front of it.
    for row in rows() {
        let case = format!(
            "{:?} filtered by {}, {} records, MaxResults={}",
            row.listing, row.transient_status, row.record_count, row.max_results
        );
        let (emulator, engine) = emulator_with_engine();
        hold_records_in_the_transient_status(&emulator, &engine, &case, row).await;

        let first = list_page(&emulator, row, None).await;
        let (_, token) = page_invariants(&first, row, &case);
        let token = token.unwrap_or_else(|| {
            panic!("case {case:?}: a full first page must carry a NextToken, got {:?}", first.body)
        });
        let sequence = token_sequence(&token);
        let beyond = format!("{}@{sequence}", row.record_count + 1_000);

        let refused = list_page(&emulator, row, Some(&beyond)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an offset outside the held sequence must be refused, got {} ({:?})",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal must carry ValidationException, got {:?}",
            refused.error_code
        );
    }
}

#[tokio::test]
async fn the_wall_clock_alone_carries_a_search_job_listing_out_of_its_filter_mid_walk() {
    // The same rule, with nothing but the wall clock moving the records: the spans the
    // service owes a search job elapse while the caller holds the token it was handed.
    let row = Row {
        listing: Listing::SearchJobs,
        transient_status: state::RUNNING,
        record_count: 3,
        max_results: 1,
    };
    let case = "three started search jobs, filtered by RUNNING, MaxResults=1";
    let (emulator, _engine) = emulator_with_engine();
    start_search_jobs(&emulator, case, row.record_count).await;

    // Past the span a job spends CREATING and inside the one its scan runs for, so every
    // job reports the transient RUNNING status the filter names.
    tokio::time::sleep(PAST_THE_CREATING_INSTANT).await;
    let first = list_page(&emulator, row, None).await;
    let (first_entries, token) = page_invariants(&first, row, case);
    assert_eq!(
        first_entries.len(),
        1,
        "case {case:?}: the first page must carry the one job the page size allows, got {first_entries:?}"
    );
    let token = token.unwrap_or_else(|| {
        panic!("case {case:?}: a full first page must carry a NextToken, got {:?}", first.body)
    });

    // Past the span the scan takes, so every job has settled to its terminal status with
    // nobody calling.
    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let resumed = list_page(&emulator, row, Some(&token)).await;
    let (resumed_entries, resumed_token) = page_invariants(&resumed, row, case);
    assert!(
        resumed_entries.is_empty() && resumed_token.is_none(),
        "case {case:?}: no job is RUNNING any more, so the token must resume an empty last page, got {:?}",
        resumed.body
    );

    // And the jobs are still listed, in the terminal status the clock moved them to: the
    // walk ended because the filter no longer admits them, not because they are gone.
    let unfiltered = call(
        &emulator,
        REGION,
        "GET",
        "/search-jobs?MaxResults=1000",
        None,
    )
    .await;
    let listed: Vec<Value> = unfiltered
        .get("SearchJobs")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        row.record_count,
        "case {case:?}: the unfiltered listing must still report every job, got {:?}",
        unfiltered.body
    );
    for summary in listed {
        let status = summary["Status"].as_str().unwrap_or_default().to_string();
        assert!(
            state::TERMINAL_STATES.contains(&status.as_str()),
            "case {case:?}: every job must have settled to a terminal status, got {status:?}"
        );
    }
}
