// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for WHICH scans the account-wide scan listing reports.
//!
//! `ListScans` is documented as "Returns a list of all scans in an account. Does not
//! return `EXPRESS` scans", so the rule this suite asserts is that the listing reports
//! exactly those scans whose current run is not an Express run: an Express scan appears
//! in no `summaries` page, fills no page slot and shifts no page cursor, while staying
//! reachable by name through `GetScan`.
//!
//! Every request goes through the real router (`MultiServiceRouter::dispatch`), the same
//! path a client takes.

use std::time::Duration;

use serde_json::json;
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    EXPRESS_SCAN_SETTLE_MILLIS, SCAN_TYPE_EXPRESS, SCAN_TYPE_STANDARD, STANDARD_SCAN_SETTLE_MILLIS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scan_lifecycle::{artifact_for, call, create_scan, harness, Answer};

/// The page size a `ListScans` request that names no `maxResults` is served at, which is
/// also the modelled ceiling (`ListScansRequest$maxResults` range 1..100).
const LIST_SCANS_DEFAULT_PAGE: usize = 100;

/// One row of the table: a scan started with the `scanType` the row names, and whether
/// the account's scan listing reports it.
#[derive(Debug, Clone, Copy)]
struct Case {
    /// What this row varies, named in every assertion.
    label: &'static str,
    /// The `scanName` the row drives, unique per row. The names deliberately sort so
    /// that a scan the listing omits falls before, between and after the scans it
    /// reports.
    scan_name: &'static str,
    /// `scanType` as the request carries it, or `None` for the documented default
    /// ("Defaults to Standard type if missing", `CreateScanRequest$scanType`).
    scan_type: Option<&'static str>,
    /// Whether the listing reports this scan.
    listed: bool,
}

/// The table. Rows span the modelled `ScanType` values
/// (`types::shared_types::SCAN_TYPE_VALUES` — `Express | Standard`) and the absent case,
/// at every position in the store's name order.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "an Express scan sorting before every listed scan",
            scan_name: "aa-express",
            scan_type: Some(SCAN_TYPE_EXPRESS),
            listed: false,
        },
        Case {
            label: "an explicitly Standard scan",
            scan_name: "bb-standard",
            scan_type: Some(SCAN_TYPE_STANDARD),
            listed: true,
        },
        Case {
            label: "an Express scan between two listed scans",
            scan_name: "cc-express",
            scan_type: Some(SCAN_TYPE_EXPRESS),
            listed: false,
        },
        Case {
            label: "the documented default scan type",
            scan_name: "dd-default",
            scan_type: None,
            listed: true,
        },
        Case {
            label: "a Standard scan immediately after another reported scan",
            scan_name: "ee-standard",
            scan_type: Some(SCAN_TYPE_STANDARD),
            listed: true,
        },
        Case {
            label: "an Express scan sorting after every listed scan",
            scan_name: "ff-express",
            scan_type: Some(SCAN_TYPE_EXPRESS),
            listed: false,
        },
    ]
}

async fn list_scans(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/scans{query}"), None).await
}

async fn get_scan(router: &MultiServiceRouter, scan_name: &str) -> Answer {
    call(router, "GET", &format!("/scans/{scan_name}"), None).await
}

/// `CreateUploadUrl` + `CreateScan` for `scan_name`, started with `scan_type` (absent for
/// the documented default).
async fn start_scan_of_type(
    router: &MultiServiceRouter,
    scan_name: &str,
    scan_type: Option<&str>,
) {
    let artifact = artifact_for(router, scan_name).await;
    let mut body = json!({ "scanName": scan_name });
    if let Some(scan_type) = scan_type {
        body["scanType"] = json!(scan_type);
    }
    let created = create_scan(router, body, &artifact).await;
    assert_eq!(
        created.status, 200,
        "CreateScan({scan_name}, scanType={scan_type:?}): expected 200, got {} {}",
        created.status, created.json
    );
}

/// Wait past the clock-owed transition of a run of either scan type, so the scan can take
/// another revision. The handlers store the due instant rather than sleeping, so only the
/// test waits.
async fn wait_for_settle() {
    let span = STANDARD_SCAN_SETTLE_MILLIS.max(EXPRESS_SCAN_SETTLE_MILLIS);
    tokio::time::sleep(Duration::from_millis(u64::try_from(span).unwrap_or(500) + 200)).await;
}

/// Every `scanName` the listing reports, in the order it reported them, paged at
/// `max_results` (the absent case when `None`) to the last page. Asserts on the way that
/// no page carries more entries than the page size allows.
async fn listed_scan_names(
    router: &MultiServiceRouter,
    max_results: Option<usize>,
    page_budget: usize,
) -> Vec<String> {
    let page_limit = max_results.unwrap_or(LIST_SCANS_DEFAULT_PAGE);
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        pages += 1;
        assert!(
            pages <= page_budget,
            "case maxResults={max_results:?}: paging did not terminate within {page_budget} pages"
        );
        let mut query = String::new();
        if let Some(max) = max_results {
            query.push_str(&format!("?maxResults={max}"));
        }
        if let Some(token) = &token {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("nextToken={token}"));
        }
        let answer = list_scans(router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={max_results:?}: expected 200, got {} {}",
            answer.status, answer.json
        );
        let page = answer.get("summaries").as_array().cloned().unwrap_or_default();
        assert!(
            page.len() <= page_limit,
            "case maxResults={max_results:?}: a page carried {} summaries, more than {page_limit}",
            page.len()
        );
        for summary in page {
            seen.push(summary["scanName"].as_str().unwrap_or_default().to_string());
        }
        match answer.get("nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

#[tokio::test]
async fn the_listing_omits_express_scans_which_stay_reachable_by_name() {
    let (_engine, router) = harness();
    let cases = cases();
    for case in &cases {
        start_scan_of_type(&router, case.scan_name, case.scan_type).await;
    }

    let mut expected: Vec<String> = cases
        .iter()
        .filter(|case| case.listed)
        .map(|case| case.scan_name.to_string())
        .collect();
    expected.sort();

    let listed = listed_scan_names(&router, None, 2).await;
    assert_eq!(
        listed, expected,
        "the listing must report exactly the scans that are not Express scans, got \
         {listed:?}"
    );

    for case in &cases {
        let label = case.label;
        assert_eq!(
            listed.contains(&case.scan_name.to_string()),
            case.listed,
            "case {label:?}: expected the listing to {} {}, got {listed:?}",
            if case.listed { "report" } else { "omit" },
            case.scan_name
        );

        // A scan the listing omits is not a scan that is gone: it is reachable by name.
        let got = get_scan(&router, case.scan_name).await;
        assert_eq!(
            got.status, 200,
            "case {label:?}: GetScan({name}) expected 200, got {status} {json}",
            name = case.scan_name,
            status = got.status,
            json = got.json
        );
        assert_eq!(
            got.str_of("scanName"),
            case.scan_name,
            "case {label:?}: GetScan must report the scan it was asked for, got {}",
            got.json
        );
        assert_eq!(
            got.str_of("scanNameArn"),
            crate::scan_lifecycle::expected_scan_arn(case.scan_name),
            "case {label:?}: GetScan must report the scan's own ARN, got {}",
            got.json
        );
    }
}

#[tokio::test]
async fn paging_the_listing_never_spends_a_page_slot_on_an_express_scan() {
    let (_engine, router) = harness();
    let cases = cases();
    for case in &cases {
        start_scan_of_type(&router, case.scan_name, case.scan_type).await;
    }
    let mut expected: Vec<String> = cases
        .iter()
        .filter(|case| case.listed)
        .map(|case| case.scan_name.to_string())
        .collect();
    expected.sort();

    // Rows: page sizes below, at and above the number of scans the listing reports, the
    // modelled maximum, and the absent case.
    let page_sizes: &[Option<usize>] = &[Some(1), Some(2), Some(3), Some(6), Some(100), None];
    for max_results in page_sizes {
        let budget = expected.len() + 2;
        let seen = listed_scan_names(&router, *max_results, budget).await;
        assert_eq!(
            seen, expected,
            "case maxResults={max_results:?}: paging must yield every reported scan exactly \
             once, in one stable order, and no Express scan, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn the_listing_follows_the_scans_current_run() {
    let (_engine, router) = harness();

    // Rows: a scan re-run from Express to Standard, and one re-run the other way, so the
    // listing follows the scan's CURRENT run rather than the type it was first started
    // with.
    let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
        (
            "an Express scan re-run as Standard",
            "flips-to-standard",
            Some(SCAN_TYPE_EXPRESS),
            Some(SCAN_TYPE_STANDARD),
        ),
        (
            "a Standard scan re-run as Express",
            "flips-to-express",
            Some(SCAN_TYPE_STANDARD),
            Some(SCAN_TYPE_EXPRESS),
        ),
        (
            "a defaulted scan re-run as Express",
            "flips-from-default",
            None,
            Some(SCAN_TYPE_EXPRESS),
        ),
    ];

    for (label, scan_name, first, second) in rows {
        start_scan_of_type(&router, scan_name, *first).await;
        let listed = listed_scan_names(&router, None, 2).await;
        let first_listed = first.unwrap_or(SCAN_TYPE_STANDARD) != SCAN_TYPE_EXPRESS;
        assert_eq!(
            listed.contains(&scan_name.to_string()),
            first_listed,
            "case {label:?}: after the first run (scanType={first:?}) the listing must {} \
             {scan_name}, got {listed:?}",
            if first_listed { "report" } else { "omit" }
        );

        // A revision cannot start while the previous run is still InProgress.
        wait_for_settle().await;
        start_scan_of_type(&router, scan_name, *second).await;
        let listed = listed_scan_names(&router, None, 2).await;
        let second_listed = second.unwrap_or(SCAN_TYPE_STANDARD) != SCAN_TYPE_EXPRESS;
        assert_eq!(
            listed.contains(&scan_name.to_string()),
            second_listed,
            "case {label:?}: after the re-run (scanType={second:?}) the listing must {} \
             {scan_name}, got {listed:?}",
            if second_listed { "report" } else { "omit" }
        );
        wait_for_settle().await;
    }
}
