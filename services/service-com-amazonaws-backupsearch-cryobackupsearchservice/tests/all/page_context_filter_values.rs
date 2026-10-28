// SPDX-License-Identifier: CC0-1.0

//! The sequence a `NextToken` names must be named INJECTIVELY: two requests that page
//! different sequences never produce the same page context, whatever their filter values
//! are.
//!
//! The context travels inside every token this service issues and is compared as a plain
//! string when a token comes back (`state::decode_next_token`), so two sequences sharing
//! one context is exactly the confusion the context exists to prevent — a token handed out
//! for one filtered listing resumes another at that offset and the call answers 200 with a
//! page it was never issued for, where the rule is a 400 `ValidationException`.
//!
//! The filter values a context is built from are request data under no syntactic
//! constraint, so the rows below are the values an encoding is most likely to confuse with
//! its own syntax: a word standing for "this filter is absent", the separators between
//! fields, a decimal length, a whole context string, and a value legal for the OTHER
//! filter member of the same operation.
//!
//! The HTTP harness (`emulator_with_engine`, `listed_exports`) is the search-job and
//! export-listing suites': every suite drives the same emulator the same way.

use std::collections::{BTreeSet, HashMap};
use std::time::{Duration, SystemTime};

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_listing::listed_exports;
use crate::search_jobs::emulator_with_engine;

/// The region the seeded records' ARNs name.
const REGION: &str = "us-west-2";

/// A search job none of the filter values below names, so every row has records the
/// filter keeps and records it drops.
const OTHER_OWNER: &str = "other-owner";

/// Every filter value these tables are driven over.
///
/// The first group is adversarial by construction: each entry is a piece an in-band
/// encoding of "no filter" could be spelled with, or a rendering of a whole context. The
/// statuses come from the service's own `types::EXPORT_JOB_STATUS_VALUES` (the model's
/// `ExportJobStatus` enum), so a value that is legal for the operation's OTHER filter
/// member is a row too, and the last entry is the uuid shape this service actually mints —
/// the control row, which must be refused exactly as the adversarial ones are.
fn filter_values() -> Vec<String> {
    let mut values: BTreeSet<String> = [
        "ALL",
        "-",
        ":",
        "::",
        ":-",
        "0:",
        "1:a",
        "3:ALL",
        "export-jobs:-:-",
        "export-jobs:ALL:ALL",
        "jobs",
        "",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect();
    for status in types::EXPORT_JOB_STATUS_VALUES {
        values.insert((*status).to_string());
    }
    values.insert("48434c42-40a0-4057-b04d-3b484c610e2b".to_string());
    values.into_iter().collect()
}

/// Put one export job per row in the store, oldest row first, owned by the search job the
/// row names and in the status it names.
///
/// The records are seeded rather than started because what the rows vary is the filter
/// VALUE a record carries, and `StartSearchResultExportJob` can only ever write the
/// uuid-shaped identifier it minted; seeding also pins each `CreationTime`, so the
/// newest-first sequence a token indexes is a property of the records and not of how fast
/// the test ran.
fn seed_exports(engine: &engine::InMemoryEngine, rows: &[(&str, &str)]) -> Vec<String> {
    let store = engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
    let mut store = store.lock();
    let base = SystemTime::now();
    rows.iter()
        .enumerate()
        .map(|(index, (owner, status))| {
            let identifier = format!("seeded-export-{index}");
            store.insert_export_job(state::ExportJob {
                identifier: identifier.clone(),
                arn: state::export_job_arn("backup-search", REGION, &identifier),
                search_job_identifier: (*owner).to_string(),
                search_job_arn: state::search_job_arn("backup-search", REGION, owner),
                status: (*status).to_string(),
                creation_time: state::timestamp(base + Duration::from_secs(index as u64)),
                completion_time: None,
                status_message: None,
                export_specification: types::ExportSpecification::s3ExportSpecification(
                    types::S3ExportSpecification {
                        destination_bucket: format!("bucket-{index}"),
                        destination_prefix: None,
                    },
                ),
                role_arn: None,
                tags: HashMap::new(),
                client_token: None,
                request_fingerprint: identifier.clone(),
                // Nothing is owed, so no row's status moves out from under a filter while
                // the test pages.
                next_status_due: None,
            });
            identifier
        })
        .collect()
}

/// Refuse the call and say so with the modelled refusal: what a token that was issued for
/// another sequence is answered with.
async fn assert_refused(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    query: &str,
) {
    let answer = listed_exports(emulator, query).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a token issued for another sequence must be refused, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ValidationException"),
        "case {case:?}: the refusal must carry ValidationException, got {:?}",
        answer.error_code
    );
}

#[test]
fn no_two_differently_filtered_listings_share_a_page_context() {
    // The rule, over every listing of this service at once: a page context names exactly
    // one sequence. Each row is a request — a listing with one choice of filters — and no
    // two rows may render to the same context, since that string is all a returning token
    // is judged by.
    let values = filter_values();
    let mut filters: Vec<Option<&str>> = vec![None];
    filters.extend(values.iter().map(|value| Some(value.as_str())));

    let mut cases: Vec<(String, String)> = Vec::new();
    for by_status in &filters {
        cases.push((
            format!("ListSearchJobs ByStatus={by_status:?}"),
            state::jobs_page_context(*by_status),
        ));
        for by_search_job in &filters {
            cases.push((
                format!(
                    "ListSearchResultExportJobs Status={by_status:?} SearchJobIdentifier={by_search_job:?}"
                ),
                state::export_jobs_page_context(*by_status, *by_search_job),
            ));
        }
    }
    // The two listings bound to one search job: their filter is required, so it is always
    // present, but the value is as free-form as the others.
    for value in &values {
        cases.push((
            format!("ListSearchJobResults SearchJobIdentifier={value:?}"),
            state::results_page_context(value),
        ));
        cases.push((
            format!("ListSearchJobBackups SearchJobIdentifier={value:?}"),
            state::backups_page_context(value),
        ));
    }

    let mut seen: HashMap<String, String> = HashMap::new();
    for (case, context) in cases {
        let previous = seen.insert(context.clone(), case.clone());
        assert!(
            previous.is_none(),
            "case {case:?}: a page context must name exactly one sequence, but its context {context:?} is also {previous:?}'s"
        );
    }
}

#[tokio::test]
async fn an_export_token_is_refused_across_the_filter_whatever_the_filter_value_is() {
    // The rule as a client sees it, in both directions: a token issued for the unfiltered
    // export listing is refused under a `SearchJobIdentifier` filter, and a token issued
    // under that filter is refused for the unfiltered listing — for EVERY filter value,
    // including one that spells whatever the context says "no filter" with.
    //
    // The empty value is exercised by the context table above only: an empty query value
    // is not a filter value a client can tell from an absent member on the wire.
    for value in filter_values().into_iter().filter(|value| !value.is_empty()) {
        let (emulator, engine) = emulator_with_engine();
        // Two export jobs the filter keeps and two it drops, so the filtered walk and the
        // unfiltered one really are different sequences.
        seed_exports(
            &engine,
            &[
                (value.as_str(), state::RUNNING),
                (OTHER_OWNER, state::RUNNING),
                (value.as_str(), state::COMPLETED),
                (OTHER_OWNER, state::COMPLETED),
            ],
        );

        let unfiltered = listed_exports(&emulator, "?MaxResults=1").await;
        let unfiltered_token = unfiltered.string("NextToken");
        assert!(
            !unfiltered_token.is_empty(),
            "case {value:?}: the unfiltered first page of four at MaxResults=1 must carry a NextToken, got {:?}",
            unfiltered.body
        );

        let filter = format!("SearchJobIdentifier={value}");
        let filtered = listed_exports(&emulator, &format!("?{filter}&MaxResults=1")).await;
        assert_eq!(
            filtered.status, 200,
            "case {value:?}: a filter value is never refused for what it names, got {} ({:?})",
            filtered.status, filtered.body
        );
        let filtered_token = filtered.string("NextToken");
        assert!(
            !filtered_token.is_empty(),
            "case {value:?}: a full page of the two matching export jobs must carry a NextToken, got {:?}",
            filtered.body
        );

        // Each token resumes the listing it WAS issued for...
        for (case, query) in [
            (
                format!("{value:?}: the unfiltered token resumes the unfiltered listing"),
                format!("?MaxResults=1&NextToken={unfiltered_token}"),
            ),
            (
                format!("{value:?}: the filtered token resumes the filtered listing"),
                format!("?{filter}&MaxResults=1&NextToken={filtered_token}"),
            ),
        ] {
            let answer = listed_exports(&emulator, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a token must resume the listing it was issued for, got {} ({:?})",
                answer.status, answer.body
            );
        }

        // ...and neither resumes the other.
        assert_refused(
            &emulator,
            &format!("{value:?}: the unfiltered token under the filter"),
            &format!("?{filter}&MaxResults=1&NextToken={unfiltered_token}"),
        )
        .await;
        assert_refused(
            &emulator,
            &format!("{value:?}: the filtered token with no filter"),
            &format!("?MaxResults=1&NextToken={filtered_token}"),
        )
        .await;
    }
}

#[tokio::test]
async fn an_export_token_is_refused_across_the_two_filter_members() {
    // The same rule between the operation's two filters: one value carried by `Status` and
    // the same value carried by `SearchJobIdentifier` select different sequences, so
    // neither one's token resumes the other's listing. Every status the model declares is
    // a row, each with a search job named exactly like it.
    let (emulator, engine) = emulator_with_engine();
    let mut rows: Vec<(&str, &str)> = Vec::new();
    for status in types::EXPORT_JOB_STATUS_VALUES {
        rows.push((status, status));
        rows.push((OTHER_OWNER, status));
    }
    seed_exports(&engine, &rows);

    for status in types::EXPORT_JOB_STATUS_VALUES {
        let by_status = listed_exports(&emulator, &format!("?Status={status}&MaxResults=1")).await;
        let status_token = by_status.string("NextToken");
        assert!(
            !status_token.is_empty(),
            "case {status:?}: a full page of the status-filtered listing must carry a NextToken, got {:?}",
            by_status.body
        );

        let by_owner =
            listed_exports(&emulator, &format!("?SearchJobIdentifier={status}&MaxResults=1")).await;
        let owner_token = by_owner.string("NextToken");
        assert!(
            !owner_token.is_empty(),
            "case {status:?}: a full page of the search-job-filtered listing must carry a NextToken, got {:?}",
            by_owner.body
        );

        assert_refused(
            &emulator,
            &format!("{status:?}: a Status token under SearchJobIdentifier"),
            &format!("?SearchJobIdentifier={status}&MaxResults=1&NextToken={status_token}"),
        )
        .await;
        assert_refused(
            &emulator,
            &format!("{status:?}: a SearchJobIdentifier token under Status"),
            &format!("?Status={status}&MaxResults=1&NextToken={owner_token}"),
        )
        .await;
    }
}
