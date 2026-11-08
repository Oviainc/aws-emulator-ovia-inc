// SPDX-License-Identifier: CC0-1.0

//! DISCOVERING a resource scan: what `ListResourceScans` reports of a scan this account and
//! Region started, and what the reads of a scan's result answer while that scan is still
//! running — end to end over the awsQuery wire.
//!
//! Two rules are under test here, each with its own table.
//!
//! 1. **A scan this account and Region started is reported by the listing whatever the call
//!    that started it covered.** `ListResourceScans` narrows to one `ScanType` only where the
//!    request NAMES one, so a request naming no `ScanTypeFilter` reports the account's scans
//!    of either type and the same scan is visible from the item read
//!    (`DescribeResourceScan`) and from the listing, with the status it holds at the moment it
//!    is listed.
//!
//! 2. **A request that operates ON a scan that has not finished is refused 400
//!    `ResourceScanInProgress`, telling the caller to wait for THAT scan** — a different
//!    sentence from the one a request to START a second scan is refused with, which is about
//!    starting a new scan instead. A read of a scan this account and Region holds no
//!    record of is refused 400 `ResourceScanNotFound` with a sentence that names nothing of
//!    the request.
//!
//! The first table is one row per shape the `ScanFilters` of a start can take — none, one
//! type exactly, the documented wildcard, two types at once, and a type the account holds
//! none of — since that is what decides the `ScanType` a scan is recorded under, and each row
//! is listed with no scan type named, with the scan's own type named, and with the other type
//! named. The second table is one row per operation that reads a scan's result. Every value
//! asserted is one the service answered: the scan's id comes from the start, and the
//! resources a related-resources request addresses are the ones `ListStackResources` reports.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use service_com_amazonaws_cloudformation_cloudformation::state::resource_scans as scans;
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::error_code;
use crate::harness::{member, router, xml_blocks, xml_text, xml_texts};
use crate::resource_scan_related_resources::{requested_members, stack_catalogue, Scanned};
use crate::stack_lifecycle::{call, deploy};

/// A stack whose resources span two services, so a filter naming one type leaves part of the
/// account outside the scan's rows.
const TOPIC_AND_BUCKET: &str = r#"{"Resources":{
    "Alpha":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"discovered-alpha-topic"}},
    "Beta":{"Type":"AWS::S3::Bucket","Properties":{"BucketName":"discovered-beta-bucket"}}
}}"#;

/// The stack the cases deploy before they scan.
const STACK: &str = "scan-discovery";

/// Wait past the instant a started scan's conclusion becomes due.
async fn wait_for_the_scan() {
    tokio::time::sleep(scans::SCAN_DELAY + Duration::from_millis(250)).await;
}

/// `StartResourceScan` with one `ScanFilters` entry naming `types`, in the awsQuery flattened
/// form; a row naming no type sends no filter at all. `ScanFilters` is `@length(min: 1,
/// max: 1)`, so there is never more than one filter.
async fn start_scan(router: &MultiServiceRouter, types: &[&str]) -> (u16, String) {
    let members: Vec<(String, String)> = types
        .iter()
        .enumerate()
        .map(|(position, value)| {
            member(
                &format!("ScanFilters.member.1.Types.member.{}", position + 1),
                value,
            )
        })
        .collect();
    call(router, "StartResourceScan", &members).await
}

/// The id of the scan a successful start answered.
fn started_scan_id(case: &str, body: &str) -> String {
    xml_text(body, "ResourceScanId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceScanId, got {body}"))
}

/// `ListResourceScans`, naming a `ScanTypeFilter` only where the case has one.
async fn list_scans(router: &MultiServiceRouter, scan_type: Option<&str>) -> (u16, String) {
    let members: Vec<(String, String)> = scan_type
        .map(|scan_type| vec![member("ScanTypeFilter", scan_type)])
        .unwrap_or_default();
    call(router, "ListResourceScans", &members).await
}

/// The `(Status, ScanType)` a listing reports for each scan it carries, keyed by
/// `ResourceScanId`.
///
/// Each summary is read INSIDE its own `<member>` block, so one summary's members cannot be
/// swept up with another's.
fn reported_summaries(body: &str) -> BTreeMap<String, (String, String)> {
    let Some(list) = xml_blocks(body, "ResourceScanSummaries").into_iter().next() else {
        return BTreeMap::new();
    };
    xml_blocks(&list, "member")
        .into_iter()
        .filter_map(|summary| {
            let id = xml_text(&summary, "ResourceScanId")?;
            Some((
                id,
                (
                    xml_text(&summary, "Status").unwrap_or_default(),
                    xml_text(&summary, "ScanType").unwrap_or_default(),
                ),
            ))
        })
        .collect()
}

/// One row of the discovery table: the `ScanFilters` the start names and the `ScanType` the
/// scan it starts is therefore recorded under — a scan given resource-type filters covered
/// PART of the account, and one given none covered all of it.
struct DiscoveryCase {
    case: &'static str,
    filter: &'static [&'static str],
    scan_type: &'static str,
}

/// Every shape the `ScanFilters` of one start can take.
const DISCOVERY_CASES: &[DiscoveryCase] = &[
    DiscoveryCase {
        case: "no filter at all: a scan over the whole account",
        filter: &[],
        scan_type: scans::scan_type::FULL,
    },
    DiscoveryCase {
        case: "a filter naming one type exactly",
        filter: &["AWS::SNS::Topic"],
        scan_type: scans::scan_type::PARTIAL,
    },
    DiscoveryCase {
        // The wildcard form `ScanFilter$Types` documents: "To scan all resource types within
        // a service, you can use a wildcard, represented by an asterisk (*)."
        case: "a wildcard filter over one service",
        filter: &["AWS::S3::*"],
        scan_type: scans::scan_type::PARTIAL,
    },
    DiscoveryCase {
        case: "a filter naming two types at once",
        filter: &["AWS::SNS::Topic", "AWS::S3::Bucket"],
        scan_type: scans::scan_type::PARTIAL,
    },
    DiscoveryCase {
        case: "a filter naming a type the account holds none of",
        filter: &["AWS::DynamoDB::Table"],
        scan_type: scans::scan_type::PARTIAL,
    },
];

/// The scan type that is NOT the one a row's scan was recorded under.
fn the_other_scan_type(scan_type: &str) -> &'static str {
    if scan_type == scans::scan_type::FULL {
        scans::scan_type::PARTIAL
    } else {
        scans::scan_type::FULL
    }
}

#[tokio::test]
async fn a_started_scan_is_reported_by_the_listing_whatever_it_covered() {
    for row in DISCOVERY_CASES {
        let case = row.case;
        // One router per row: the scan the row starts is the only scan of that account.
        let router = router();
        deploy(&router, STACK, TOPIC_AND_BUCKET).await;

        let started = Instant::now();
        let (status, body) = start_scan(&router, row.filter).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartResourceScan answered {status} with {body}"
        );
        let scan_id = started_scan_id(case, &body);

        // Listed straight after the call, with no scan type named: the scan the account just
        // started is there, under the scan type it covers, with the status it holds now.
        let (status, body) = list_scans(&router, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListResourceScans answered {status} with {body}"
        );
        let listed = reported_summaries(&body);
        let (listed_status, listed_scan_type) = listed
            .get(&scan_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected {scan_id} listed, got {body}"));
        assert_eq!(
            listed_scan_type, row.scan_type,
            "case {case:?}: expected ScanType {:?}, got {listed_scan_type:?}",
            row.scan_type
        );
        if started.elapsed() < scans::SCAN_DELAY {
            assert_eq!(
                listed_status,
                scans::status::IN_PROGRESS,
                "case {case:?}: the scan is still running, got {listed_status:?}"
            );
        }

        // And once the clock has made its conclusion due, the SAME listing reports that scan
        // complete: the item read and the listing agree about the one record.
        wait_for_the_scan().await;
        let (status, body) = list_scans(&router, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListResourceScans answered {status} with {body}"
        );
        let listed = reported_summaries(&body);
        let (listed_status, listed_scan_type) = listed.get(&scan_id).unwrap_or_else(|| {
            panic!("case {case:?}: expected {scan_id} still listed, got {body}")
        });
        assert_eq!(
            (listed_status.as_str(), listed_scan_type.as_str()),
            (scans::status::COMPLETE, row.scan_type),
            "case {case:?}: expected the concluded scan under its own type, got {body}"
        );

        let (status, body) = call(
            &router,
            "DescribeResourceScan",
            &[member("ResourceScanId", &scan_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeResourceScan answered {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(scans::status::COMPLETE),
            "case {case:?}: the item read reports the status the listing does, got {body}"
        );

        // Naming the scan's own type selects it; naming the other type selects nothing, which
        // is what makes the unnarrowed listing above a report over EVERY scan type.
        let (status, body) = list_scans(&router, Some(row.scan_type)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListResourceScans answered {status} with {body}"
        );
        assert!(
            reported_summaries(&body).contains_key(&scan_id),
            "case {case:?}: expected {scan_id} under ScanTypeFilter {:?}, got {body}",
            row.scan_type
        );

        let other = the_other_scan_type(row.scan_type);
        let (status, body) = list_scans(&router, Some(other)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListResourceScans answered {status} with {body}"
        );
        assert!(
            !reported_summaries(&body).contains_key(&scan_id),
            "case {case:?}: expected {scan_id} outside ScanTypeFilter {other:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn an_unnarrowed_listing_reports_both_scan_types_newest_first() {
    let router = router();
    deploy(&router, STACK, TOPIC_AND_BUCKET).await;

    // Two scans of one account, one of each scan type, each started through the wire and let
    // conclude before the next begins ("only one can be run at a time").
    let case = "a partial scan, then a full one";
    let (status, body) = start_scan(&router, &["AWS::SNS::Topic"]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartResourceScan answered {status} with {body}"
    );
    let partial = started_scan_id(case, &body);
    wait_for_the_scan().await;

    let (status, body) = start_scan(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartResourceScan answered {status} with {body}"
    );
    let full = started_scan_id(case, &body);
    wait_for_the_scan().await;

    // With no scan type named both are reported, newest first.
    let (status, body) = list_scans(&router, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListResourceScans answered {status} with {body}"
    );
    assert_eq!(
        xml_texts(&body, "ResourceScanId"),
        vec![full.clone(), partial.clone()],
        "case {case:?}: expected both scans newest first, got {body}"
    );
    let listed = reported_summaries(&body);
    for (scan_id, scan_type) in [
        (&full, scans::scan_type::FULL),
        (&partial, scans::scan_type::PARTIAL),
    ] {
        assert_eq!(
            listed.get(scan_id),
            Some(&(
                scans::status::COMPLETE.to_string(),
                scan_type.to_string()
            )),
            "case {case:?}: expected {scan_id} complete under {scan_type}, got {body}"
        );
    }

    // And each scan type names exactly its own scan.
    for (scan_type, expected) in [
        (scans::scan_type::FULL, &full),
        (scans::scan_type::PARTIAL, &partial),
    ] {
        let (status, body) = list_scans(&router, Some(scan_type)).await;
        assert_eq!(
            status, 200,
            "case {scan_type:?}: ListResourceScans answered {status} with {body}"
        );
        assert_eq!(
            xml_texts(&body, "ResourceScanId"),
            vec![expected.clone()],
            "case {scan_type:?}: expected only that type's scan, got {body}"
        );
    }
}

#[tokio::test]
async fn an_unfinished_scan_refuses_the_reads_of_its_result() {
    let router = router();
    deploy(&router, STACK, TOPIC_AND_BUCKET).await;
    // The resources a related-resources request addresses, read back through
    // `ListStackResources`, so the request carries identifiers the service itself assigned.
    let addressed: Vec<Scanned> = stack_catalogue(&router, STACK).await.into_values().collect();

    let started = Instant::now();
    let (status, body) = start_scan(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"starting the scan\": StartResourceScan answered {status} with {body}"
    );
    let scan_id = started_scan_id("starting the scan", &body);

    // One row per operation that reads the RESULT of a scan: neither has a result to report
    // while the scan is still finding it.
    let mut related = vec![member("ResourceScanId", &scan_id)];
    related.extend(requested_members(&addressed));
    let gated: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "ListResourceScanResources",
            vec![member("ResourceScanId", &scan_id)],
        ),
        ("ListResourceScanRelatedResources", related),
    ];

    for (action, members) in &gated {
        let case = *action;
        let (status, body) = call(&router, action, members).await;
        if started.elapsed() >= scans::SCAN_DELAY {
            // The clock concluded the scan before the call was served, so the gate was no
            // longer owed: this row says nothing on a machine that slow.
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the in-progress refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "ResourceScanInProgress",
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(scans::OPERATION_WHILE_SCANNING),
            "case {case:?}: expected the sentence a request on an unfinished scan carries, \
             got {body}"
        );
    }

    // A request to START another scan is refused with the SAME modelled error and the OTHER
    // sentence: what it states is the new scan the caller cannot start, not a wait on an
    // operation against this one.
    let case = "a second StartResourceScan";
    let (status, body) = start_scan(&router, &[]).await;
    if started.elapsed() < scans::SCAN_DELAY {
        assert_eq!(
            status, 400,
            "case {case:?}: expected the exclusivity refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "ResourceScanInProgress",
            "case {case:?}: expected the modelled refusal, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(scans::SCAN_IN_PROGRESS),
            "case {case:?}: expected the sentence a second start carries, got {body}"
        );
    }

    // The gate LIFTS on its own as the clock concludes the scan: the same reads answer, and
    // the scan is listed complete.
    wait_for_the_scan().await;
    for (action, members) in &gated {
        let case = *action;
        let (status, body) = call(&router, action, members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read to answer once the scan concluded, got \
             {status} {body}"
        );
    }
    let (status, body) = list_scans(&router, None).await;
    assert_eq!(
        status, 200,
        "case \"the concluded scan\": ListResourceScans answered {status} with {body}"
    );
    assert_eq!(
        reported_summaries(&body).get(&scan_id).map(|(status, _)| status.as_str()),
        Some(scans::status::COMPLETE),
        "case \"the concluded scan\": expected it listed complete, got {body}"
    );
}

#[tokio::test]
async fn a_scan_no_record_answers_for_is_refused_without_naming_the_request() {
    let router = router();

    // Addresses no scan was ever started under, each in the ARN form a scan's id takes — the
    // refusal of a value OUTSIDE that form is an input-constraint violation instead, which
    // `scan_and_registry_reads.rs` is what pins. The refusal is the same fixed sentence for
    // each, since it names nothing of the request.
    let unknown = [
        (
            "an id in the ARN form, under which nothing was started",
            scans::resource_scan_arn("us-east-1", "00000000-0000-4000-8000-000000000001"),
        ),
        (
            "another well-formed id nothing was started under",
            scans::resource_scan_arn("us-east-1", "11111111-2222-3333-4444-555555555555"),
        ),
    ];
    for (case, scan_id) in &unknown {
        for action in ["DescribeResourceScan", "ListResourceScanResources"] {
            let (status, body) =
                call(&router, action, &[member("ResourceScanId", scan_id)]).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {action}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(case, &body),
                "ResourceScanNotFound",
                "case {case:?} on {action}: expected the modelled refusal, got {body}"
            );
            let message = xml_text(&body, "Message").unwrap_or_default();
            assert_eq!(
                message, scans::NOT_FOUND,
                "case {case:?} on {action}: expected {:?}, got {message:?}",
                scans::NOT_FOUND
            );
            assert!(
                !message.contains(scan_id.as_str()),
                "case {case:?} on {action}: the refusal names nothing of the request, got \
                 {message:?}"
            );
        }
    }
}
