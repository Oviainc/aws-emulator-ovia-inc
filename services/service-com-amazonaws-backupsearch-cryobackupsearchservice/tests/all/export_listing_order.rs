// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ORDER a job listing of this service reports its jobs in.
//!
//! The rule: a job listing of this service reports its jobs NEWEST-CREATED FIRST, stably
//! across pages and inside every filter, so a single-page read is "the latest job". The
//! model states no order for either listing (`ListSearchJobs`,
//! `ListSearchResultExportJobs` — both `@paginated` over summaries carrying only
//! `CreationTime` as an ordering key), so the service's own recorded order for its job
//! listings is the authority, and that order is newest-first; the two listings of one
//! service cannot disagree about it.
//!
//! Each test asserts that rule over a TABLE whose rows differ in what the rule
//! generalises over — the number of records, the page size, the filter in force, and
//! which of the two listings is read — never over one cited request:
//!
//! * paging a listing to exhaustion yields every record exactly once, newest-created
//!   first, for every record count and page size;
//! * the reported `CreationTime`s never increase down the sequence, which is the rule
//!   stated on the member rather than on insertion order;
//! * the order is the same inside a filter as it is without one;
//! * both job listings of this service agree about it.
//!
//! The HTTP harness (`emulator`, `emulator_with_engine`, `call`, `Answer`) and the
//! export-listing helpers (`seed_job`, `passable_role`, `listed_exports`) are the ones
//! the sibling suites already use: every suite drives the same emulator the same way.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_listing::{listed_exports, passable_role, seed_job};
use crate::search_jobs::{call, emulator_with_engine};

/// The region every call in this suite is signed for — the one `seed_job` mints its ARNs
/// in, so a seeded record and a called one belong to the same listing.
const REGION: &str = "us-west-2";

/// Start one export job for `search_job_identifier` and return its
/// `ExportJobIdentifier`. The destination bucket differs per `index` so no two starts
/// are the same request.
async fn start_export(
    emulator: &MultiServiceRouter,
    search_job_identifier: &str,
    index: usize,
) -> String {
    let started = call(
        emulator,
        REGION,
        "PUT",
        "/export-search-jobs",
        Some(json!({
            "SearchJobIdentifier": search_job_identifier,
            "ExportSpecification": {"s3ExportSpecification": {
                "DestinationBucket": format!("exports-{search_job_identifier}-{index}")
            }},
            "RoleArn": passable_role("export")
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "export {index} of {search_job_identifier:?} must start, got {} ({:?})",
        started.status, started.body
    );
    started.string("ExportJobIdentifier")
}

/// The `ExportJobIdentifier`s of one page, in the order the page holds them.
fn page_identifiers(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|summary| {
            summary["ExportJobIdentifier"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every `CreationTime` of one page, as the epoch seconds the protocol renders them as.
fn page_creation_times(page: &[Value]) -> Vec<f64> {
    page.iter()
        .map(|summary| summary["CreationTime"].as_f64().unwrap_or_default())
        .collect()
}

/// Follow `ListSearchResultExportJobs` to exhaustion with `filters` in force and
/// `max_results` per page, asserting the paging invariants that hold for any page size,
/// and return every identifier seen and every `CreationTime` seen, in order.
async fn page_to_exhaustion(
    emulator: &MultiServiceRouter,
    case: &str,
    filters: &str,
    max_results: usize,
    expected_total: usize,
) -> (Vec<String>, Vec<f64>) {
    let mut seen: Vec<String> = Vec::new();
    let mut times: Vec<f64> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let query = match &token {
            Some(token) => format!("?{filters}MaxResults={max_results}&NextToken={token}"),
            None => format!("?{filters}MaxResults={max_results}"),
        };
        let listed = listed_exports(emulator, &query).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListSearchResultExportJobs, got {} ({:?})",
            listed.status, listed.body
        );
        let page = listed
            .get("ExportJobs")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            page.len() <= max_results,
            "case {case:?}: a page must hold at most MaxResults export jobs, got {}",
            page.len()
        );
        seen.extend(page_identifiers(&page));
        times.extend(page_creation_times(&page));
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case {case:?}: paging must terminate, took {pages} pages"
        );
        match listed.get("NextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    (seen, times)
}

#[tokio::test]
async fn export_jobs_are_paged_newest_created_first_whatever_the_page_size() {
    // The rule, over a table of record counts and page sizes: the first page opens with
    // the most recently created export job, following NextToken to exhaustion yields
    // each held export job exactly once newest-first, and the CreationTimes reported
    // never increase down that sequence.
    for export_count in [1_usize, 2, 3, 5] {
        for max_results in [1_usize, 2, 3, 5, 1000] {
            let (emulator, engine) = emulator_with_engine();
            seed_job(&engine, "exportable", state::COMPLETED, Vec::new());
            let mut created: Vec<String> = Vec::new();
            for index in 0..export_count {
                created.push(start_export(&emulator, "exportable", index).await);
            }
            let newest_first: Vec<String> = created.iter().rev().cloned().collect();
            let case = format!("{export_count} exports, MaxResults={max_results}");

            let first = listed_exports(&emulator, &format!("?MaxResults={max_results}")).await;
            let first_page = first
                .get("ExportJobs")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                page_identifiers(&first_page).first().map(String::as_str),
                newest_first.first().map(String::as_str),
                "case {case:?}: the first page must open with the newest export job, got {:?}",
                first.body
            );

            let (seen, times) =
                page_to_exhaustion(&emulator, &case, "", max_results, export_count).await;
            assert_eq!(
                seen, newest_first,
                "case {case:?}: paging must yield every export job exactly once, newest-created first, got {seen:?}"
            );
            assert!(
                times.windows(2).all(|pair| pair[0] >= pair[1]),
                "case {case:?}: CreationTime must never increase down a newest-first listing, got {times:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_newest_first_order_is_the_same_inside_every_filter() {
    // The rule holds per filtered sequence, not only for the unfiltered one: filtering
    // selects which records are reported, never the order they are reported in.
    //
    // The records are seeded rather than started, because what varies across rows is the
    // STATUS and the owning search job, and `FAILED` is a status no sequence of calls
    // reaches here; seeding also pins each CreationTime, so "newest-created" is a
    // property of the record and not of how fast the test ran.
    let (emulator, engine) = emulator_with_engine();
    let owners = ["search-job-a", "search-job-b"];
    for owner in owners {
        seed_job(&engine, owner, state::COMPLETED, Vec::new());
    }
    // Every value the model's `ExportJobStatus` enum declares, from the service's own
    // `types::EXPORT_JOB_STATUS_VALUES`.
    let statuses = types::EXPORT_JOB_STATUS_VALUES;
    let base = SystemTime::now();
    let mut created: Vec<(String, String, String)> = Vec::new();
    {
        let store =
            engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
        let mut store = store.lock();
        // Nine records, cycling through both owners and all three statuses so no filter
        // selects a contiguous run of the creation order: an order that only looked
        // right because the filter kept a prefix would fail here.
        for index in 0..9_usize {
            let identifier = format!("seeded-export-{index}");
            let owner = owners[index % owners.len()];
            let status = statuses[index % statuses.len()];
            store.insert_export_job(state::ExportJob {
                identifier: identifier.clone(),
                arn: state::export_job_arn("backup-search", REGION, &identifier),
                search_job_identifier: owner.to_string(),
                search_job_arn: state::search_job_arn("backup-search", REGION, owner),
                status: status.to_string(),
                // One second apart and increasing with the insertion order, so the
                // reported order can be checked against the member itself.
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
                // Terminal as far as the clock is concerned: nothing is owed, so
                // `settle` cannot move a row's status out from under the filter.
                next_status_due: None,
            });
            created.push((identifier, owner.to_string(), status.to_string()));
        }
    }

    // One row per filter the operation models — each on its own and both together,
    // plus the unfiltered listing and a filter value that selects nothing.
    let mut filters: Vec<(Option<&str>, Option<&str>)> = vec![(None, None)];
    for status in statuses {
        filters.push((None, Some(status)));
    }
    for owner in owners {
        filters.push((Some(owner), None));
        for status in statuses {
            filters.push((Some(owner), Some(status)));
        }
    }
    // A search job this store holds no export for: the filter selects nothing, which is
    // still an order (the empty one) and must not be a refusal.
    filters.push((Some("search-job-c"), None));

    for (wanted_owner, wanted_status) in filters {
        let case = format!(
            "SearchJobIdentifier={} Status={}",
            wanted_owner.unwrap_or("<unset>"),
            wanted_status.unwrap_or("<unset>")
        );
        let mut query = String::new();
        if let Some(owner) = wanted_owner {
            query.push_str(&format!("SearchJobIdentifier={owner}&"));
        }
        if let Some(status) = wanted_status {
            query.push_str(&format!("Status={status}&"));
        }
        // What the rule says the answer is: the matching records, in the reverse of the
        // order they were created in.
        let expected: Vec<String> = created
            .iter()
            .rev()
            .filter(|(_, owner, status)| {
                wanted_status.is_none_or(|wanted| wanted == status)
                    && wanted_owner.is_none_or(|wanted| wanted == owner)
            })
            .map(|(identifier, _, _)| identifier.clone())
            .collect();

        // Every page size that splits this sequence differently, including one that
        // serves it whole.
        for max_results in [1_usize, 2, 4, 1000] {
            let row = format!("{case} at MaxResults={max_results}");
            let (seen, times) =
                page_to_exhaustion(&emulator, &row, &query, max_results, created.len()).await;
            assert_eq!(
                seen, expected,
                "case {row:?}: a filtered listing must report its matches newest-created first, got {seen:?}"
            );
            assert!(
                times.windows(2).all(|pair| pair[0] > pair[1]),
                "case {row:?}: CreationTime must strictly decrease down a newest-first listing of records created a second apart, got {times:?}"
            );
        }
    }
}

#[tokio::test]
async fn both_job_listings_of_this_service_report_newest_created_first() {
    // The rule is the SERVICE's, not one listing's: the search-job listing and the
    // export-job listing of the same emulator answer in the same order, so a client
    // reading "the latest job" gets the latest one from either.
    let (emulator, engine) = emulator_with_engine();
    let mut search_jobs: Vec<String> = Vec::new();
    for index in 0..3_usize {
        let started = call(
            &emulator,
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
            "search job {index} must start, got {} ({:?})",
            started.status, started.body
        );
        search_jobs.push(started.string("SearchJobIdentifier"));
    }

    seed_job(&engine, "exportable", state::COMPLETED, Vec::new());
    let mut exports: Vec<String> = Vec::new();
    for index in 0..3_usize {
        exports.push(start_export(&emulator, "exportable", index).await);
    }

    let listed_jobs = call(&emulator, REGION, "GET", "/search-jobs", None).await;
    let listed_search_jobs: Vec<String> = listed_jobs
        .get("SearchJobs")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["SearchJobIdentifier"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    // The seeded "exportable" job is listed too, and it was inserted after the three
    // started ones, so it opens the newest-first sequence.
    let mut expected_jobs: Vec<String> = search_jobs.clone();
    expected_jobs.push("exportable".to_string());
    expected_jobs.reverse();
    assert_eq!(
        listed_search_jobs, expected_jobs,
        "ListSearchJobs must report search jobs newest-created first, got {listed_search_jobs:?}"
    );

    let listed = listed_exports(&emulator, "").await;
    let listed_export_jobs = page_identifiers(
        &listed
            .get("ExportJobs")
            .as_array()
            .cloned()
            .unwrap_or_default(),
    );
    let expected_exports: Vec<String> = exports.iter().rev().cloned().collect();
    assert_eq!(
        listed_export_jobs, expected_exports,
        "ListSearchResultExportJobs must report export jobs newest-created first, exactly as ListSearchJobs does, got {listed_export_jobs:?}"
    );
}
