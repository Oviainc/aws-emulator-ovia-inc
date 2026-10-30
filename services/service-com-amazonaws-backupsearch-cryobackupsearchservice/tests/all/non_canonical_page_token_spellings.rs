// SPDX-License-Identifier: CC0-1.0

//! A `NextToken` is resumable only AS THE STRING this service handed out.
//!
//! The rule: a paginating listing resumes a token only when the token is one this service
//! could have issued — its resume position spelled exactly as `state::encode_next_token`
//! renders a `usize`, in canonical decimal — and refuses every other spelling of that
//! number with the modelled `ValidationException` (400), because a token the service never
//! minted says nothing about where a walk of the held sequence stands.
//!
//! The spellings a decimal parse admits BESIDE the canonical one are an optional leading
//! `+`, any number of leading zeros, and the combination of the two; surrounding
//! whitespace is in the table as the neighbouring form of the same mistake. Every row is
//! derived from a token the service really issued, so each case differs from an accepted
//! token in nothing but the spelling of its position.
//!
//! Both arms of the rule are asserted over a TABLE whose rows differ in what the rule
//! generalises over — which of the four paginating listings is paged and which end its
//! token is anchored at (`state::PageAnchor`), how many records it holds, the page size,
//! how deep into the walk the token was issued, and the position's magnitude — never over
//! one cited request.
//!
//! The HTTP harness (`emulator_with_engine`, `call`, `Answer`) and the seeding helpers
//! (`seed_job`, `passable_role`) are the sibling suites': every suite drives the same
//! emulator the same way.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupsearch_cryobackupsearchservice::ops::types;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::export_listing::{passable_role, seed_job};
use crate::search_jobs::{call, emulator_with_engine, recovery_point, Answer};

/// The region every call in this suite is signed for — the one `seed_job` mints its ARNs
/// in, so a seeded record and a called one belong to the same listing.
const REGION: &str = "us-west-2";

/// Which paginating listing a row pages. All four resolve their `NextToken` through the
/// one helper, and they cover both ends a token can be anchored at: the two job listings
/// report a creation order in reverse, the two listings bound to one search job report it
/// as it is held.
#[derive(Clone, Copy, Debug)]
enum Listing {
    SearchJobs,
    ExportJobs,
    JobResults,
    JobBackups,
}

/// The listings every table below is driven over.
const LISTINGS: [Listing; 4] = [
    Listing::SearchJobs,
    Listing::ExportJobs,
    Listing::JobResults,
    Listing::JobBackups,
];

/// One emulator with one listing in it, and how that listing is addressed: the two query
/// members' names differ between the job listings and the two bound to a search job (the
/// model's `httpQuery` spellings), and so does the member carrying the page.
struct Paged {
    emulator: MultiServiceRouter,
    path: String,
    max_results_member: &'static str,
    next_token_member: &'static str,
    page_member: &'static str,
}

impl Paged {
    /// One page of the listing, `token` resuming it when given.
    async fn page(&self, max_results: usize, token: Option<&str>) -> Answer {
        let uri = match token {
            Some(token) => format!(
                "{}?{}={max_results}&{}={token}",
                self.path, self.max_results_member, self.next_token_member
            ),
            None => format!("{}?{}={max_results}", self.path, self.max_results_member),
        };
        call(&self.emulator, REGION, "GET", &uri, None).await
    }

    /// The entries a page carried, as the JSON values the listing reports.
    fn entries(&self, answer: &Answer) -> Vec<serde_json::Value> {
        answer
            .get(self.page_member)
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
}

/// One result item a scan recorded, in the shape `ListSearchJobResults` pages.
fn result_item(index: usize) -> types::ResultItem {
    types::ResultItem::S3ResultItem(types::S3ResultItem {
        object_key: Some(format!("objects/{index}")),
        object_size: Some(1024 + index as i64),
        ..Default::default()
    })
}

/// One recovery point a scan covered, in the shape `ListSearchJobBackups` pages.
fn covered_backup(index: usize) -> types::SearchJobBackupsResult {
    recovery_point(
        "S3",
        &format!(
            "arn:aws:backup:{REGION}:{}:recovery-point:rp-{index}",
            engine::emulator_account_id()
        ),
        &format!("arn:aws:s3:::bucket-{index}"),
        std::time::SystemTime::now(),
    )
}

/// An emulator holding `records` entries of `listing`, ready to be paged.
async fn paged(listing: Listing, records: usize, case: &str) -> Paged {
    let (emulator, engine) = emulator_with_engine();
    match listing {
        Listing::SearchJobs => {
            for index in 0..records {
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
                    "case {case:?}: StartSearchJob must succeed to set up the row, got {} ({:?})",
                    started.status, started.body
                );
            }
            Paged {
                emulator,
                path: "/search-jobs".to_string(),
                max_results_member: "MaxResults",
                next_token_member: "NextToken",
                page_member: "SearchJobs",
            }
        }
        Listing::ExportJobs => {
            // An export can only be started from a search job whose results are final, so
            // one settled job is seeded and exported `records` times.
            let exportable = seed_job(&engine, state::COMPLETED, Vec::new());
            for index in 0..records {
                let started = call(
                    &emulator,
                    REGION,
                    "PUT",
                    "/export-search-jobs",
                    Some(json!({
                        "SearchJobIdentifier": exportable,
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
            }
            Paged {
                emulator,
                path: "/export-search-jobs".to_string(),
                max_results_member: "MaxResults",
                next_token_member: "NextToken",
                page_member: "ExportJobs",
            }
        }
        Listing::JobResults => {
            let identifier = seed_job(&engine, state::COMPLETED, Vec::new());
            // The recorded matches are seeded on the job record itself, which is what this
            // listing pages: what the rows vary is the paging, not how the scan ran.
            let store =
                engine.state::<state::ComAmazonawsBackupsearchCryobackupsearchserviceState>();
            let mut store = store.lock();
            store
                .job_mut(&identifier)
                .expect("the seeded job is held")
                .results = (0..records).map(result_item).collect();
            drop(store);
            Paged {
                emulator,
                path: format!("/search-jobs/{identifier}/search-results"),
                max_results_member: "maxResults",
                next_token_member: "nextToken",
                page_member: "Results",
            }
        }
        Listing::JobBackups => {
            let identifier = seed_job(
                &engine,
                state::COMPLETED,
                (0..records).map(covered_backup).collect(),
            );
            Paged {
                emulator,
                path: format!("/search-jobs/{identifier}/backups"),
                max_results_member: "maxResults",
                next_token_member: "nextToken",
                page_member: "Results",
            }
        }
    }
}

/// The text before a token's `@` (its resume position) and the text after it (the sequence
/// the token names).
fn split_token(token: &str, case: &str) -> (String, String) {
    let (position, context) = token
        .split_once('@')
        .unwrap_or_else(|| panic!("case {case:?}: a token this service issued carries a position and the sequence it indexes, got {token:?}"));
    (position.to_string(), context.to_string())
}

/// Every spelling of `position` a decimal parse admits BESIDE the canonical one, each
/// percent-encoded for a query string so the server reads exactly the spelling the row
/// means (`+` in a query is otherwise a space).
fn non_canonical_spellings(position: &str) -> Vec<(String, String)> {
    let mut spellings = vec![
        (format!("+{position}"), format!("%2B{position}")),
        (format!("0{position}"), format!("0{position}")),
        (format!("00{position}"), format!("00{position}")),
        (format!("+0{position}"), format!("%2B0{position}")),
        (format!("{position:0>8}"), format!("{position:0>8}")),
        (format!(" {position}"), format!("%20{position}")),
        (format!("{position} "), format!("{position}%20")),
    ];
    // A zero-padded rendering of a number already that wide is the canonical spelling
    // itself, which is the row the two tables below assert is ACCEPTED.
    spellings.retain(|(spelling, _)| spelling != position);
    spellings
}

/// Refuse the call with the modelled refusal: what a token this service did not issue is
/// answered with.
fn assert_refused(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a token this service did not issue must be refused, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ValidationException"),
        "case {case:?}: the refusal must carry ValidationException, got {:?}",
        answer.error_code
    );
}

#[tokio::test]
async fn a_token_whose_position_is_not_the_spelling_this_service_writes_is_refused() {
    // Every row: page a listing, take a token it really issued, and re-spell ONLY the
    // position inside it. The canonical token is the control — it must resume the walk —
    // and every other spelling of the same number must be refused, since
    // `state::encode_next_token` can have written none of them.
    for listing in LISTINGS {
        for (records, max_results) in [(2_usize, 1_usize), (3, 1), (3, 2), (5, 2)] {
            let case = format!("{listing:?}, {records} records, page size {max_results}");
            let paged = paged(listing, records, &case).await;

            let mut token = {
                let first = paged.page(max_results, None).await;
                assert_eq!(
                    first.status, 200,
                    "case {case:?}: the first page must be answered, got {} ({:?})",
                    first.status, first.body
                );
                first.get("NextToken").as_str().map(ToString::to_string)
            };
            let mut depth = 0_usize;
            assert!(
                token.is_some(),
                "case {case:?}: a full first page must carry a NextToken to re-spell"
            );

            // Each token of the whole walk is a row, so the positions under test are not
            // just the first page's but every depth the listing hands out.
            while let Some(issued) = token {
                depth += 1;
                let (position, context) = split_token(&issued, &case);
                let case = format!("{case}, token {depth} ({issued:?})");
                assert_eq!(
                    position,
                    position
                        .parse::<usize>()
                        .map(|number| number.to_string())
                        .unwrap_or_default(),
                    "case {case:?}: this service only ever writes a position in canonical decimal"
                );

                for (spelling, encoded) in non_canonical_spellings(&position) {
                    let case = format!("{case}, position spelled {spelling:?}");
                    let answer = paged
                        .page(max_results, Some(&format!("{encoded}@{context}")))
                        .await;
                    assert_refused(&answer, &case);
                }

                // The control, against the same emulator and the same page size: the
                // token as ISSUED resumes the walk.
                let resumed = paged.page(max_results, Some(&issued)).await;
                assert_eq!(
                    resumed.status, 200,
                    "case {case:?}: the token as issued must resume the walk, got {} ({:?})",
                    resumed.status, resumed.body
                );
                assert!(
                    paged.entries(&resumed).len() <= max_results,
                    "case {case:?}: a page must hold at most MaxResults entries, got {}",
                    paged.entries(&resumed).len()
                );
                token = resumed.get("NextToken").as_str().map(ToString::to_string);
                assert!(
                    depth <= records + 1,
                    "case {case:?}: paging must terminate, took {depth} tokens"
                );
            }
        }
    }
}

#[test]
fn the_helper_admits_the_position_it_writes_and_no_other_spelling_of_it() {
    // The rule at the helper both ends share, over magnitudes an end-to-end walk cannot
    // reach cheaply (a position of zero, and positions of two, three and four digits) and
    // over both anchors: the string `encode_next_token` wrote decodes, and every other
    // spelling of the same number is an error.
    let contexts = [
        state::jobs_page_context(None),
        state::jobs_page_context(Some(state::RUNNING)),
        state::export_jobs_page_context(None, None),
        state::results_page_context("48434c42-40a0-4057-b04d-3b484c610e2b"),
        state::backups_page_context("48434c42-40a0-4057-b04d-3b484c610e2b"),
    ];
    let len = 1000_usize;
    for context in &contexts {
        for position in [0_usize, 1, 2, 9, 10, 99, 100, 1000] {
            for anchor in [state::PageAnchor::CreationOrder, state::PageAnchor::NewestFirst] {
                let issued = state::encode_next_token(context, position);
                let case = format!("{issued:?} at {anchor:?}");
                assert!(
                    state::decode_next_token(Some(&issued), context, len, anchor).is_ok(),
                    "case {case:?}: the token this service issued must decode, got {:?}",
                    state::decode_next_token(Some(&issued), context, len, anchor)
                );

                for (spelling, _) in non_canonical_spellings(&position.to_string()) {
                    let token = format!("{spelling}@{context}");
                    let case = format!("{case}, position spelled {spelling:?}");
                    let decoded = state::decode_next_token(Some(&token), context, len, anchor);
                    assert!(
                        decoded.is_err(),
                        "case {case:?}: a position spelled as no token this service issued must be an error, got {decoded:?}"
                    );
                }
            }
        }
    }
}
