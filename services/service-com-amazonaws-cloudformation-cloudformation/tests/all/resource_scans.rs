// SPDX-License-Identifier: CC0-1.0

//! `ListResourceScans` over the awsQuery wire: the resource scans an account and Region
//! holds.
//!
//! The rule every case here holds the service to: **`ListResourceScans` reports the resource
//! scans THIS account and Region persists, from newest to oldest by the scan's own
//! `StartTime`, for exactly one `ScanType` — the one the request names, or `FULL` when it
//! names none — with every member of a summary read off the stored scan, one page of at most
//! `MaxResults` summaries at a time (10 by default, 100 at the most) plus the token that
//! resumes after it, answering the empty list for an account that holds no such scan.**
//!
//! The table below is one row per scan-type selection, over seeded records spanning both
//! `ScanType` values, every `ResourceScanStatus` value, scans that have finished and one
//! still running, start instants deliberately out of the order the records were written in,
//! and two Regions — so a row's expected ids distinguish each selection from every other.
//!
//! The store's WRITER is `StartResourceScan`, whose own suite drives it over the wire
//! (`resource_scan_lifecycle.rs`); the records below are seeded through the store's one
//! writing entry point (`record_resource_scan`) — the same device
//! `hook_result_invocations.rs` and `extension_registry.rs` use — because the statuses and
//! start instants these rows span are ones no bound's worth of wire calls could produce.
//! Every READ goes through the real request path (form-encoded body in, XML envelope out),
//! so what is asserted is what a client sees.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use service_com_amazonaws_cloudformation_cloudformation::state::resource_scans::{
    resource_scan_arn, scan_type, status, PendingScanOutcome, StoredResourceScan,
    DEFAULT_MAX_RESULTS, MAX_RESOURCE_SCANS, MAX_RESULTS_CEILING, PERCENTAGE_COMPLETE,
    PERCENTAGE_STARTED, SCAN_DELAY,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{self, PAGE_SIZE};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_in, member, router_with_engine, store, xml_text, xml_texts, REGION,
};

/// Another Region's records, for the rows that hold a read to the Region it was signed for.
const OTHER_REGION: &str = "us-west-2";

/// The base instant the seeded scans are stamped from: 2023-11-14T22:13:20Z, so a case can
/// name the exact timestamp a summary must report ([`rendered`]).
const BASE_EPOCH_SECONDS: u64 = 1_700_000_000;

/// One seeded instant, `offset` seconds past [`BASE_EPOCH_SECONDS`].
fn at(offset: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(BASE_EPOCH_SECONDS + offset)
}

/// How the modelled timestamp of [`at`] renders on the wire. Every offset this suite uses is
/// under 40, so the rendering stays inside the same minute.
fn rendered(offset: u64) -> String {
    assert!(offset < 40, "an offset past this minute: {offset}");
    format!("2023-11-14T22:13:{:02}Z", 20 + offset)
}

/// A scan's `ResourceScanId`: "the Amazon Resource Name (ARN) of the resource scan", built
/// from an index so a case can name the record it expects.
fn scan_id(index: usize, region: &str) -> String {
    resource_scan_arn(region, &format!("00000000-0000-4000-8000-{index:012}"))
}

/// One scan this suite's table is written against: a finished scan, with every member set
/// off this row rather than off any request.
fn finished(
    index: usize,
    region: &str,
    scan: &str,
    scan_status: &str,
    started: u64,
    ended: u64,
    percentage: f64,
) -> StoredResourceScan {
    StoredResourceScan {
        resource_scan_id: scan_id(index, region),
        region: region.to_string(),
        scan_type: scan.to_string(),
        status: scan_status.to_string(),
        status_reason: None,
        start_time: at(started),
        end_time: Some(at(ended)),
        percentage_completed: Some(percentage),
        pending: None,
        client_request_token: None,
        scan_filters: Vec::new(),
        resources_read: 0,
        resources: Vec::new(),
    }
}

/// The six scans this suite's table is written against, in the order they were recorded —
/// which is deliberately NOT the order their `StartTime`s put them in.
fn seeded_scans() -> Vec<StoredResourceScan> {
    let mut failed = finished(3, REGION, scan_type::FULL, status::FAILED, 3, 4, 42.5);
    failed.status_reason = Some("The scan exceeded the resources it could read".to_string());

    let running = StoredResourceScan {
        resource_scan_id: scan_id(5, REGION),
        region: REGION.to_string(),
        scan_type: scan_type::PARTIAL.to_string(),
        status: status::IN_PROGRESS.to_string(),
        status_reason: None,
        start_time: at(7),
        end_time: None,
        percentage_completed: Some(PERCENTAGE_STARTED),
        pending: None,
        client_request_token: None,
        scan_filters: Vec::new(),
        resources_read: 0,
        resources: Vec::new(),
    };

    vec![
        finished(
            1,
            REGION,
            scan_type::FULL,
            status::COMPLETE,
            1,
            31,
            PERCENTAGE_COMPLETE,
        ),
        finished(
            2,
            REGION,
            scan_type::PARTIAL,
            status::COMPLETE,
            5,
            9,
            PERCENTAGE_COMPLETE,
        ),
        failed,
        finished(
            4,
            REGION,
            scan_type::FULL,
            status::EXPIRED,
            2,
            8,
            PERCENTAGE_COMPLETE,
        ),
        running,
        finished(
            6,
            OTHER_REGION,
            scan_type::FULL,
            status::COMPLETE,
            9,
            10,
            PERCENTAGE_COMPLETE,
        ),
    ]
}

/// The router with the seeded scans already in its store.
fn router_with_seeded_scans() -> MultiServiceRouter {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        for scan in seeded_scans() {
            held.record_resource_scan(scan);
        }
    }
    router
}

/// The `ResourceScanId`s one response reports, in the order it reports them.
fn reported_ids(body: &str) -> Vec<String> {
    xml_texts(body, "ResourceScanId")
}

/// Wait past the instant a started scan's completion becomes due.
async fn wait_for_the_scan() {
    tokio::time::sleep(SCAN_DELAY + Duration::from_millis(250)).await;
}

/// One row of the scan-type table: the case name, the members the request carries, and the
/// indexes of the seeded scans the response must report, in order.
type SelectionCase = (&'static str, Vec<(String, String)>, Vec<usize>);

#[tokio::test]
async fn each_scan_type_selection_reports_its_scans_newest_first() {
    let router = router_with_seeded_scans();

    // One row per selection a request can make, against the seeded records: the expected
    // order is by `StartTime` descending, which is not the order the records were written.
    let cases: Vec<SelectionCase> = vec![
        (
            "no ScanTypeFilter: the documented default scan type",
            vec![],
            vec![3, 4, 1],
        ),
        (
            "ScanTypeFilter FULL: the same scans, named explicitly",
            vec![member("ScanTypeFilter", scan_type::FULL)],
            vec![3, 4, 1],
        ),
        (
            "ScanTypeFilter PARTIAL: the other scan type's scans",
            vec![member("ScanTypeFilter", scan_type::PARTIAL)],
            vec![5, 2],
        ),
    ];

    for (case, members, expected) in &cases {
        let expected_ids: Vec<String> = expected.iter().map(|i| scan_id(*i, REGION)).collect();
        let (status_code, body) = call(&router, "ListResourceScans", members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        assert_eq!(
            reported_ids(&body),
            expected_ids,
            "case {case:?}: expected {expected_ids:?}, got {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: one page holds every match, so no NextToken, got {body}"
        );
    }

    // The read reports, it does not consume: the same request answers the same scans the
    // second time, and each selection is unchanged by every other.
    for (case, members, expected) in &cases {
        let expected_ids: Vec<String> = expected.iter().map(|i| scan_id(*i, REGION)).collect();
        let (status_code, body) = call(&router, "ListResourceScans", members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200 on the repeat read, got {status_code} {body}"
        );
        assert_eq!(
            reported_ids(&body),
            expected_ids,
            "case {case:?}: the repeat read must report {expected_ids:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn one_summary_carries_every_member_the_record_holds() {
    let router = router_with_seeded_scans();

    // The `FAILED` scan is the newest `FULL` one, so it is the first summary of the default
    // selection: every member below is read off that record.
    let (status_code, body) = call(&router, "ListResourceScans", &[]).await;
    assert_eq!(
        status_code, 200,
        "case \"one record's members\": expected 200, got {status_code} {body}"
    );
    for (tag, expected) in [
        ("ResourceScanId", scan_id(3, REGION)),
        ("Status", status::FAILED.to_string()),
        (
            "StatusReason",
            "The scan exceeded the resources it could read".to_string(),
        ),
        ("StartTime", rendered(3)),
        ("EndTime", rendered(4)),
        ("ScanType", scan_type::FULL.to_string()),
    ] {
        assert_eq!(
            xml_text(&body, tag).as_deref(),
            Some(expected.as_str()),
            "case {tag:?}: expected {expected:?}, got {body}"
        );
    }
    let percentage = xml_text(&body, "PercentageCompleted")
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or_else(|| {
            panic!("case \"PercentageCompleted\": expected a double, got {body}")
        });
    assert!(
        (percentage - 42.5).abs() < f64::EPSILON,
        "case \"PercentageCompleted\": expected 42.5, got {percentage}"
    );

    // A scan that has not finished reports no `EndTime`: the member is "the time that the
    // resource scan was finished", and that scan has not.
    let (status_code, running) = call(
        &router,
        "ListResourceScans",
        &[member("ScanTypeFilter", scan_type::PARTIAL)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"a running scan\": expected 200, got {status_code} {running}"
    );
    assert_eq!(
        reported_ids(&running).first().map(String::as_str),
        Some(scan_id(5, REGION).as_str()),
        "case \"a running scan\": it is the newest PARTIAL scan, got {running}"
    );
    assert_eq!(
        xml_texts(&running, "EndTime").len(),
        1,
        "case \"a running scan\": only the finished scan of the two reports an EndTime, got {running}"
    );
    assert_eq!(
        xml_text(&running, "Status").as_deref(),
        Some(status::IN_PROGRESS),
        "case \"a running scan\": the newest summary carries its own status, got {running}"
    );
}

#[tokio::test]
async fn a_read_reports_only_the_region_it_was_signed_for() {
    let router = router_with_seeded_scans();

    // A scan covers "the resources in this account in this Region", so the Region the
    // request was signed for is the one whose scans it reports.
    for (case, region, expected) in [
        ("the Region the records were seeded in", REGION, vec![3, 4, 1]),
        ("another Region, holding one scan of its own", OTHER_REGION, vec![6]),
        ("a Region holding no scan at all", "eu-west-1", vec![]),
    ] {
        let expected_ids: Vec<String> = expected.iter().map(|i| scan_id(*i, region)).collect();
        let (status_code, body) = call_in(&router, region, "ListResourceScans", &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        assert_eq!(
            reported_ids(&body),
            expected_ids,
            "case {case:?}: expected {expected_ids:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn an_account_that_holds_no_such_scan_is_an_empty_list() {
    // Each selection is a query over the recorded scans, so one that selects none answers
    // the empty list — never a refusal, and never another selection's scans.
    for (case, seed_them, members) in [
        (
            "an account that has never been scanned",
            false,
            vec![] as Vec<(String, String)>,
        ),
        (
            "a scan type this account has no scan of",
            true,
            vec![member("ScanTypeFilter", scan_type::PARTIAL)],
        ),
    ] {
        let (router, engine) = router_with_engine();
        if seed_them {
            let state = store(&engine);
            let mut held = state.lock();
            // Only the other Region's FULL scan, so the request's Region holds none.
            held.record_resource_scan(finished(
                6,
                OTHER_REGION,
                scan_type::FULL,
                status::COMPLETE,
                9,
                10,
                PERCENTAGE_COMPLETE,
            ));
        }

        let (status_code, body) = call(&router, "ListResourceScans", &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        assert!(
            reported_ids(&body).is_empty(),
            "case {case:?}: expected no summaries, got {body}"
        );
        assert!(
            body.contains("<ResourceScanSummaries"),
            "case {case:?}: ResourceScanSummaries is reported as an empty element, got {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: an empty page carries no NextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn a_started_scan_completes_on_the_clock_rather_than_because_it_was_listed() {
    let (router, engine) = router_with_engine();
    let started_at = SystemTime::now();
    {
        let state = store(&engine);
        let mut held = state.lock();
        held.record_resource_scan(StoredResourceScan::starting(
            scan_id(1, REGION),
            REGION.to_string(),
            scan_type::FULL.to_string(),
            started_at,
        ));
    }

    // Listing straight after the call that started it still sees the scan running: the
    // completion is due a moment later, and no number of reads brings it forward.
    for attempt in 0..2 {
        let (status_code, body) = call(&router, "ListResourceScans", &[]).await;
        assert_eq!(
            status_code, 200,
            "case \"in flight\": expected 200, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(status::IN_PROGRESS),
            "case \"in flight\" (read {attempt}): the scan is still running, got {body}"
        );
        assert_eq!(
            xml_text(&body, "EndTime"),
            None,
            "case \"in flight\" (read {attempt}): a running scan has not finished, got {body}"
        );
    }

    wait_for_the_scan().await;
    let (status_code, body) = call(&router, "ListResourceScans", &[]).await;
    assert_eq!(
        status_code, 200,
        "case \"past the due instant\": expected 200, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(status::COMPLETE),
        "case \"past the due instant\": the scan has completed, got {body}"
    );
    assert!(
        xml_text(&body, "EndTime").is_some(),
        "case \"past the due instant\": a finished scan reports its EndTime, got {body}"
    );
    let percentage = xml_text(&body, "PercentageCompleted")
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or_else(|| panic!("case \"past the due instant\": expected a double, got {body}"));
    assert!(
        (percentage - PERCENTAGE_COMPLETE).abs() < f64::EPSILON,
        "case \"past the due instant\": expected {PERCENTAGE_COMPLETE}, got {percentage}"
    );
}

#[tokio::test]
async fn a_scan_whose_outcome_is_not_due_yet_keeps_the_status_it_holds() {
    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        // A scan headed for `FAILED` an hour from now: nothing may report that outcome
        // before the instant it is due.
        held.record_resource_scan(StoredResourceScan {
            resource_scan_id: scan_id(1, REGION),
            region: REGION.to_string(),
            scan_type: scan_type::FULL.to_string(),
            status: status::IN_PROGRESS.to_string(),
            status_reason: None,
            start_time: SystemTime::now(),
            end_time: None,
            percentage_completed: Some(PERCENTAGE_STARTED),
            pending: Some(PendingScanOutcome {
                status: status::FAILED.to_string(),
                status_reason: Some("The scan failed".to_string()),
                percentage_completed: 10.0,
                due_at: SystemTime::now() + Duration::from_secs(3_600),
            }),
            client_request_token: None,
            scan_filters: Vec::new(),
            resources_read: 0,
            resources: Vec::new(),
        });
    }

    let (status_code, body) = call(&router, "ListResourceScans", &[]).await;
    assert_eq!(
        status_code, 200,
        "case \"not due yet\": expected 200, got {status_code} {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(status::IN_PROGRESS),
        "case \"not due yet\": the outcome is not reported before it is due, got {body}"
    );
    assert_eq!(
        xml_text(&body, "StatusReason"),
        None,
        "case \"not due yet\": neither is its reason, got {body}"
    );
}

/// One row of the page-size table: the case name, the `MaxResults` the request names, and
/// how many summaries the response must carry.
type PageSizeCase = (&'static str, Option<i32>, usize);

#[tokio::test]
async fn one_response_carries_at_most_the_requested_page_of_summaries() {
    let (router, engine) = router_with_engine();
    // More scans than the documented ceiling, so a row asking for more than 100 is
    // distinguishable from one asking for exactly 100.
    let total = MAX_RESULTS_CEILING + 5;
    {
        let state = store(&engine);
        let mut held = state.lock();
        for index in 0..total {
            held.record_resource_scan(finished(
                index,
                REGION,
                scan_type::FULL,
                status::COMPLETE,
                // Each scan starts a second after the one before it, so "newest to oldest"
                // is the reverse of the order they were recorded in.
                u64::try_from(index % 40).expect("an offset inside the minute"),
                39,
                PERCENTAGE_COMPLETE,
            ));
        }
    }

    // "The default value is 10. The maximum value is 100."
    let cases: Vec<PageSizeCase> = vec![
        ("no MaxResults: the documented default", None, DEFAULT_MAX_RESULTS),
        ("MaxResults 1: the smallest page", Some(1), 1),
        ("MaxResults 7: the page asked for", Some(7), 7),
        (
            "MaxResults 100: the documented maximum",
            Some(100),
            MAX_RESULTS_CEILING,
        ),
        (
            "MaxResults 101: past the documented maximum",
            Some(101),
            MAX_RESULTS_CEILING,
        ),
        (
            "MaxResults 0: no maximum at all, so the documented default",
            Some(0),
            DEFAULT_MAX_RESULTS,
        ),
        (
            "MaxResults -5: not a maximum either, so the documented default",
            Some(-5),
            DEFAULT_MAX_RESULTS,
        ),
    ];
    for (case, max_results, expected) in cases {
        let members = max_results
            .map(|max| vec![member("MaxResults", &max.to_string())])
            .unwrap_or_default();
        let (status_code, body) = call(&router, "ListResourceScans", &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected 200, got {status_code} {body}"
        );
        assert_eq!(
            reported_ids(&body).len(),
            expected,
            "case {case:?}: expected {expected} summaries, got {}",
            reported_ids(&body).len()
        );
        assert!(
            xml_text(&body, "NextToken").is_some(),
            "case {case:?}: a page with more behind it carries a NextToken, got {body}"
        );
    }

    // The pages walk the whole collection exactly once, newest first.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let members = token
            .as_deref()
            .map(|token| vec![member("NextToken", token)])
            .unwrap_or_default();
        let (status_code, body) = call(&router, "ListResourceScans", &members).await;
        assert_eq!(
            status_code, 200,
            "case \"walking the pages\": expected 200, got {status_code} {body}"
        );
        let page = reported_ids(&body);
        assert!(
            page.len() <= DEFAULT_MAX_RESULTS,
            "case \"walking the pages\": a default page carries at most {DEFAULT_MAX_RESULTS}, got {}",
            page.len()
        );
        walked.extend(page);
        token = xml_text(&body, "NextToken");
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked.len(),
        total,
        "case \"walking the pages\": every scan exactly once, got {} ids",
        walked.len()
    );
    let distinct: std::collections::BTreeSet<&String> = walked.iter().collect();
    assert_eq!(
        distinct.len(),
        total,
        "case \"walking the pages\": no scan reported twice, got {} distinct of {}",
        distinct.len(),
        walked.len()
    );

    // A cursor this collection never issued is a bad request, not an empty page.
    for (case, bad_token) in [
        (
            "a cursor past the end",
            state::encode_next_token(state::PageOf::ResourceScanSummaries, total + 1),
        ),
        (
            "a cursor issued for another collection",
            state::encode_next_token(state::PageOf::HookResults, 0),
        ),
    ] {
        let (status_code, body) =
            call(&router, "ListResourceScans", &[member("NextToken", &bad_token)]).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected 400, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

#[tokio::test]
async fn the_store_keeps_at_most_the_retention_bound_of_scans() {
    let (router, engine) = router_with_engine();
    let beyond_the_bound = 3;
    {
        let state = store(&engine);
        let mut held = state.lock();
        for index in 0..MAX_RESOURCE_SCANS + beyond_the_bound {
            held.record_resource_scan(finished(
                index,
                REGION,
                scan_type::FULL,
                status::COMPLETE,
                // One instant for every record, so the reclaimed ones are the only thing
                // that decides which ids survive.
                1,
                2,
                PERCENTAGE_COMPLETE,
            ));
        }
        assert_eq!(
            held.resource_scans.len(),
            MAX_RESOURCE_SCANS,
            "case \"past the bound\": the store keeps at most {MAX_RESOURCE_SCANS}, got {}",
            held.resource_scans.len()
        );
    }

    // The OLDEST records are the reclaimed ones, so the first id of the newest-first page is
    // the last record written and the oldest surviving one closes the walk.
    let (status_code, body) = call(
        &router,
        "ListResourceScans",
        &[member("MaxResults", &PAGE_SIZE.to_string())],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case \"past the bound\": expected 200, got {status_code} {body}"
    );
    let reported = reported_ids(&body);
    assert_eq!(
        reported.len(),
        PAGE_SIZE,
        "case \"past the bound\": one bounded page of the surviving records, got {}",
        reported.len()
    );
    assert!(
        !reported.contains(&scan_id(0, REGION)),
        "case \"past the bound\": the oldest record was reclaimed, got {reported:?}"
    );
}
