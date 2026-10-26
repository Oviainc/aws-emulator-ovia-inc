// SPDX-License-Identifier: CC0-1.0

//! The resource-scan LIFECYCLE, end to end over the awsQuery wire: `StartResourceScan` starts
//! a scan over the resources this account holds in one Region and `DescribeResourceScan` reads
//! THAT scan back. (`resource_scans.rs` is the suite for `ListResourceScans`, the read over
//! the same records.)
//!
//! Three rules are under test here, each with its own table.
//!
//! 1. **A scan is persisted and read back.** `StartResourceScan` answers a `ResourceScanId` in
//!    the ARN form the model documents, and `DescribeResourceScan` reports that record: the
//!    scan still running right after the call (with no counts and no types, which are "only
//!    available" for a concluded scan) and `COMPLETE` once the clock has made its conclusion
//!    due, with the number of resources it read, the number its filters left it listing, the
//!    distinct types among those, and the filters the call supplied.
//!
//! 2. **A scan reads the account once.** The rows belong to the instant the scan STARTED, so a
//!    resource created after that instant belongs to the next scan rather than to this one,
//!    and reading the record never changes what it reports.
//!
//! 3. **The refusals around a scan.** "Only one can be run at a time for an account in a
//!    Region": a second call while one is running is refused 400 `ResourceScanInProgress`,
//!    that refusal lifts on its own once the scan concludes, a retry carrying the
//!    `ClientRequestToken` of the running scan answers THAT scan's id instead, a scan is
//!    addressable only in the Region it was started in (and another Region may run its own
//!    scan at the same time), a read of an id nothing started is refused 400
//!    `ResourceScanNotFound`, and a filter that places the wildcard anywhere but at the end of
//!    the string is refused 400 `ValidationError`.
//!
//! Each table spans the class its rule generalises over rather than one recorded case: an
//! account holding no resources, one, and several of more than one type across more than one
//! stack; a call with no filter, with a filter naming a type exactly, with the documented
//! wildcard form, with a type the account holds none of, and with two entries at once.

use std::time::{Duration, Instant};

use service_com_amazonaws_cloudformation_cloudformation::state::resource_scans as scans;
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::{deploy, error_code};
use crate::stack_lifecycle::{
    call, call_in, list_members, member, router, xml_blocks, xml_text, ACCOUNT_ID, REGION,
};

/// A second Region, for the cases that pin a scan to the Region it was started in.
const OTHER_REGION: &str = "us-west-2";

/// Wait past the instant a scan's conclusion becomes due.
async fn wait_for_the_scan() {
    tokio::time::sleep(scans::SCAN_DELAY + Duration::from_millis(250)).await;
}

/// `StartResourceScan` with whatever members the case supplies.
async fn start_resource_scan(
    router: &MultiServiceRouter,
    extra: &[(String, String)],
) -> (u16, String) {
    call(router, "StartResourceScan", extra).await
}

/// `DescribeResourceScan` for one scan, by the ARN the start answered.
async fn describe_resource_scan(
    router: &MultiServiceRouter,
    resource_scan_id: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeResourceScan",
        &[member("ResourceScanId", resource_scan_id)],
    )
    .await
}

/// The one `ScanFilters` entry naming `types`, in the awsQuery flattened form.
/// `ScanFilters` is `@length(min: 1, max: 1)`, so there is never more than one filter.
fn filter_members(types: &[&str]) -> Vec<(String, String)> {
    types
        .iter()
        .enumerate()
        .map(|(position, value)| {
            member(
                &format!("ScanFilters.member.1.Types.member.{}", position + 1),
                value,
            )
        })
        .collect()
}

/// The `ResourceScanId` of a successful call, checked to be the ARN form the model documents:
/// `arn:${Partition}:cloudformation:${Region}:${Account}:resourceScan/${Id}`.
fn assert_resource_scan_id_shape(case: &str, id: &str, region: &str) {
    let prefix = format!("arn:aws:cloudformation:{region}:{ACCOUNT_ID}:resourceScan/");
    assert!(
        id.starts_with(&prefix),
        "case {case:?}: expected a resource scan id under {prefix:?}, got {id:?}"
    );
    let unique = &id[prefix.len()..];
    let widths = [8usize, 4, 4, 4, 12];
    let groups: Vec<&str> = unique.split('-').collect();
    assert_eq!(
        groups.len(),
        widths.len(),
        "case {case:?}: expected an 8-4-4-4-12 id after {prefix:?}, got {unique:?}"
    );
    for (group, width) in groups.iter().zip(widths) {
        assert!(
            group.len() == width && group.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: expected {width} lowercase-hex digits, got {group:?} in {unique:?}"
        );
    }
}

/// The `PercentageCompleted` of a response, as the number it is on the wire.
fn percentage_completed(case: &str, body: &str) -> f64 {
    let text = xml_text(body, "PercentageCompleted")
        .unwrap_or_else(|| panic!("case {case:?}: expected a PercentageCompleted, got {body}"));
    text.parse::<f64>().unwrap_or_else(|_| {
        panic!("case {case:?}: expected PercentageCompleted to be a number, got {text:?}")
    })
}

/// The `Types` of the one `ScanFilters` entry a response echoes, in document order.
fn echoed_filter(body: &str) -> Vec<String> {
    let Some(filters) = xml_blocks(body, "ScanFilters").into_iter().next() else {
        return Vec::new();
    };
    list_members(&filters, "Types")
}

/// A template declaring one resource.
const ONE_TOPIC: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A template declaring two resources of two different types, so a row spans more than one
/// type and more than one stack's worth of resources.
const TOPIC_AND_BUCKET: &str = r#"{"Resources":{
    "Alpha":{"Type":"AWS::SNS::Topic"},
    "Bucket":{"Type":"AWS::S3::Bucket"}
}}"#;

/// One row of the round-trip table.
struct ScanCase {
    case: &'static str,
    /// The templates deployed before the scan, one stack each: the resources the account
    /// holds when it runs.
    templates: &'static [&'static str],
    /// The `Types` the one `ScanFilters` entry names, or nothing for a call with no filter.
    filter: &'static [&'static str],
    /// How many resources the scan READS — every resource of the account and Region.
    resources_read: usize,
    /// How many of them it LISTS, which is what its filters retained.
    resources_scanned: usize,
    /// The distinct types among the listed resources, sorted.
    listed_types: &'static [&'static str],
}

/// The rows of the round-trip table: an account with no resources, with one, and with several
/// of two types across two stacks; a call with no filter, with an exact type, with the
/// documented wildcard form, with a type nothing holds, and with two entries at once.
const SCAN_CASES: &[ScanCase] = &[
    ScanCase {
        case: "an account holding no resources, no filter",
        templates: &[],
        filter: &[],
        resources_read: 0,
        resources_scanned: 0,
        listed_types: &[],
    },
    ScanCase {
        case: "one resource, no filter",
        templates: &[ONE_TOPIC],
        filter: &[],
        resources_read: 1,
        resources_scanned: 1,
        listed_types: &["AWS::SNS::Topic"],
    },
    ScanCase {
        case: "three resources of two types across two stacks, no filter",
        templates: &[ONE_TOPIC, TOPIC_AND_BUCKET],
        filter: &[],
        resources_read: 3,
        resources_scanned: 3,
        listed_types: &["AWS::S3::Bucket", "AWS::SNS::Topic"],
    },
    ScanCase {
        case: "a filter naming one type exactly",
        templates: &[ONE_TOPIC, TOPIC_AND_BUCKET],
        filter: &["AWS::S3::Bucket"],
        resources_read: 3,
        resources_scanned: 1,
        listed_types: &["AWS::S3::Bucket"],
    },
    ScanCase {
        // The wildcard form `ScanFilter$Types` documents: "To scan all resource types within
        // a service, you can use a wildcard, represented by an asterisk (*). You can place an
        // asterisk at only the end of the string, for example, AWS::S3::*."
        case: "a wildcard filter over one service",
        templates: &[ONE_TOPIC, TOPIC_AND_BUCKET],
        filter: &["AWS::SNS::*"],
        resources_read: 3,
        resources_scanned: 2,
        listed_types: &["AWS::SNS::Topic"],
    },
    ScanCase {
        case: "a filter naming a type the account holds none of",
        templates: &[TOPIC_AND_BUCKET],
        filter: &["AWS::DynamoDB::Table"],
        resources_read: 2,
        resources_scanned: 0,
        listed_types: &[],
    },
    ScanCase {
        case: "a filter naming two types, one by wildcard and one exactly",
        templates: &[ONE_TOPIC, TOPIC_AND_BUCKET],
        filter: &["AWS::S3::*", "AWS::SNS::Topic"],
        resources_read: 3,
        resources_scanned: 3,
        listed_types: &["AWS::S3::Bucket", "AWS::SNS::Topic"],
    },
];

/// A scan is persisted by the start and reported by the read: running straight after the call,
/// complete once the clock has made its conclusion due, over the resources the account held
/// and the filters the call named.
#[tokio::test]
async fn a_scan_is_read_back_through_describe_resource_scan() {
    for (index, row) in SCAN_CASES.iter().enumerate() {
        let case = row.case;
        // One router per row: the account's resources are what the row is about, so each row
        // starts from an account holding only the stacks it deploys itself.
        let router = router();
        for (position, template) in row.templates.iter().enumerate() {
            deploy(&router, &format!("scanned-{index}-{position}"), template).await;
        }

        let started = Instant::now();
        let (status, body) = start_resource_scan(&router, &filter_members(row.filter)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartResourceScan answered {status} with {body}"
        );
        let resource_scan_id = xml_text(&body, "ResourceScanId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceScanId, got {body}"));
        assert_resource_scan_id_shape(case, &resource_scan_id, REGION);

        // Read straight after the call: the scan is still running until its conclusion is
        // due, which is what a poller sees first — and a running scan reports neither counts
        // nor types.
        let (status, body) = describe_resource_scan(&router, &resource_scan_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeResourceScan answered {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourceScanId").as_deref(),
            Some(resource_scan_id.as_str()),
            "case {case:?}: expected the id the start answered, got {body}"
        );
        if started.elapsed() < scans::SCAN_DELAY {
            assert_eq!(
                xml_text(&body, "Status").as_deref(),
                Some(scans::status::IN_PROGRESS),
                "case {case:?}: expected the scan still running, got {body}"
            );
            assert_eq!(
                xml_text(&body, "ResourcesRead"),
                None,
                "case {case:?}: expected no count on a running scan, got {body}"
            );
            assert_eq!(
                xml_text(&body, "ResourcesScanned"),
                None,
                "case {case:?}: expected no count on a running scan, got {body}"
            );
            assert_eq!(
                xml_text(&body, "ResourceTypes"),
                None,
                "case {case:?}: expected no types on a running scan, got {body}"
            );
            assert_eq!(
                xml_text(&body, "EndTime"),
                None,
                "case {case:?}: expected no end time on a running scan, got {body}"
            );
            let percentage = percentage_completed(case, &body);
            assert!(
                (percentage - scans::PERCENTAGE_STARTED).abs() < f64::EPSILON,
                "case {case:?}: expected a scan that has only just started to report {}, \
                 got {percentage}",
                scans::PERCENTAGE_STARTED
            );
        }
        assert!(
            xml_text(&body, "StartTime").is_some(),
            "case {case:?}: expected the instant the scan started, got {body}"
        );

        // And once the clock has made the conclusion due, the same read reports the result.
        wait_for_the_scan().await;
        let (status, body) = describe_resource_scan(&router, &resource_scan_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeResourceScan answered {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "Status").as_deref(),
            Some(scans::status::COMPLETE),
            "case {case:?}: expected the scan complete, got {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourcesRead"),
            Some(row.resources_read.to_string()),
            "case {case:?}: expected the resources the scan read, got {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourcesScanned"),
            Some(row.resources_scanned.to_string()),
            "case {case:?}: expected the resources the scan listed, got {body}"
        );
        assert_eq!(
            list_members(&body, "ResourceTypes"),
            row.listed_types,
            "case {case:?}: expected the types among the listed resources, got {body}"
        );
        assert!(
            xml_text(&body, "EndTime").is_some(),
            "case {case:?}: expected the instant the scan finished, got {body}"
        );
        let percentage = percentage_completed(case, &body);
        assert!(
            (percentage - scans::PERCENTAGE_COMPLETE).abs() < f64::EPSILON,
            "case {case:?}: expected a complete scan at {}, got {percentage}",
            scans::PERCENTAGE_COMPLETE
        );
        assert_eq!(
            echoed_filter(&body),
            row.filter,
            "case {case:?}: expected the filters the call supplied, got {body}"
        );
        assert_eq!(
            xml_text(&body, "StatusReason"),
            None,
            "case {case:?}: expected no reason on a scan that did not fail, got {body}"
        );
    }
}

/// A scan reads the account ONCE, when it starts: a resource created after that instant is not
/// in its result, and reading the record again reports the same thing.
#[tokio::test]
async fn a_scan_reports_the_account_as_it_was_when_the_scan_started() {
    let router = router();
    deploy(&router, "already-there", ONE_TOPIC).await;

    let case = "a stack deployed after the scan started";
    let (status, body) = start_resource_scan(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartResourceScan answered {status} with {body}"
    );
    let resource_scan_id = xml_text(&body, "ResourceScanId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceScanId, got {body}"));

    // A second stack, created while the scan was running: it belongs to the NEXT scan.
    deploy(&router, "created-later", TOPIC_AND_BUCKET).await;
    wait_for_the_scan().await;

    for read in ["the first read", "a second read of the same record"] {
        let (status, body) = describe_resource_scan(&router, &resource_scan_id).await;
        assert_eq!(
            status, 200,
            "case {case:?} ({read}): DescribeResourceScan answered {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourcesRead").as_deref(),
            Some("1"),
            "case {case:?} ({read}): expected only the resource held when the scan \
             started, got {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourcesScanned").as_deref(),
            Some("1"),
            "case {case:?} ({read}): expected only the resource held when the scan \
             started, got {body}"
        );
    }

    // The next scan does see it: the account now holds three resources.
    let (status, body) = start_resource_scan(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the next StartResourceScan answered {status} with {body}"
    );
    let next_scan_id = xml_text(&body, "ResourceScanId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceScanId, got {body}"));
    assert_ne!(
        next_scan_id, resource_scan_id,
        "case {case:?}: expected a scan of its own, got the previous scan's id"
    );
    wait_for_the_scan().await;
    let (status, body) = describe_resource_scan(&router, &next_scan_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeResourceScan answered {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "ResourcesRead").as_deref(),
        Some("3"),
        "case {case:?}: expected the next scan to read every resource, got {body}"
    );
}

/// One scan at a time for an account in a Region: the second call is refused while the first
/// runs, a retry under the token of the running scan addresses that scan instead, another
/// Region runs its own, and the refusal lifts as the scan concludes.
#[tokio::test]
async fn one_scan_at_a_time_for_an_account_in_a_region() {
    let router = router();

    let case = "a second scan while one is running";
    let started = Instant::now();
    let (status, body) =
        start_resource_scan(&router, &[member("ClientRequestToken", "first")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartResourceScan answered {status} with {body}"
    );
    let first_scan_id = xml_text(&body, "ResourceScanId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceScanId, got {body}"));

    // A retry under the SAME token, while that scan is still running, is not a new scan: it
    // answers the scan that token started.
    let (status, body) =
        start_resource_scan(&router, &[member("ClientRequestToken", "first")]).await;
    assert_eq!(
        status, 200,
        "case \"a retry under the same token\": expected the retry accepted, got {status} \
         with {body}"
    );
    if started.elapsed() < scans::SCAN_DELAY {
        assert_eq!(
            xml_text(&body, "ResourceScanId").as_deref(),
            Some(first_scan_id.as_str()),
            "case \"a retry under the same token\": expected the scan that token started, \
             got {body}"
        );
    }

    // A call under a DIFFERENT token, and one carrying no token at all, is a second scan — and
    // only one may run at a time.
    for (label, extra) in [
        ("another token", vec![member("ClientRequestToken", "second")]),
        ("no token at all", Vec::new()),
    ] {
        let (status, body) = start_resource_scan(&router, &extra).await;
        if started.elapsed() < scans::SCAN_DELAY {
            assert_eq!(
                status, 400,
                "case {case:?} ({label}): expected the second scan refused, got {status} \
                 with {body}"
            );
            assert_eq!(
                error_code(case, &body),
                "ResourceScanInProgress",
                "case {case:?} ({label}): expected the modelled refusal, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?} ({label}): the first scan had concluded, so the call is \
                 accepted: got {status} with {body}"
            );
        }
    }

    // Another Region's account runs its own scan: the rule is per account AND Region.
    let (status, body) = call_in(&router, OTHER_REGION, "StartResourceScan", &[]).await;
    assert_eq!(
        status, 200,
        "case \"a scan in another Region\": expected it accepted, got {status} with {body}"
    );
    let other_scan_id = xml_text(&body, "ResourceScanId").unwrap_or_else(|| {
        panic!("case \"a scan in another Region\": expected a ResourceScanId, got {body}")
    });
    assert_resource_scan_id_shape("a scan in another Region", &other_scan_id, OTHER_REGION);

    // And a scan is addressable only in the Region it was started in.
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "DescribeResourceScan",
        &[member("ResourceScanId", &first_scan_id)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a scan read from another Region\": expected it unknown there, got {status} \
         with {body}"
    );
    assert_eq!(
        error_code("a scan read from another Region", &body),
        "ResourceScanNotFound",
        "case \"a scan read from another Region\": expected the modelled refusal, got {body}"
    );

    // The refusal counts the scans still running, so it lifts as they conclude.
    wait_for_the_scan().await;
    let (status, body) = start_resource_scan(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"a scan once the running one concluded\": expected it accepted, got {status} \
         with {body}"
    );
    assert_ne!(
        xml_text(&body, "ResourceScanId").as_deref(),
        Some(first_scan_id.as_str()),
        "case \"a scan once the running one concluded\": expected a scan of its own, got \
         {body}"
    );

    // And a token whose scan has CONCLUDED is not retrying anything: it starts the next scan
    // rather than answering the concluded scan's id for ever.
    wait_for_the_scan().await;
    let case = "the first token again, once its scan concluded";
    let (status, body) =
        start_resource_scan(&router, &[member("ClientRequestToken", "first")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected it accepted, got {status} with {body}"
    );
    assert_ne!(
        xml_text(&body, "ResourceScanId").as_deref(),
        Some(first_scan_id.as_str()),
        "case {case:?}: expected a scan of its own, got {body}"
    );
}

/// The refusals around a scan: a read of an id nothing started, and a filter that places the
/// wildcard anywhere but at the end of the string.
#[tokio::test]
async fn the_refusals_around_a_resource_scan() {
    let router = router();

    // An id nothing started, in the very ARN form a real id carries.
    let case = "an id nothing started";
    let (status, body) = describe_resource_scan(
        &router,
        &format!(
            "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:resourceScan/\
             00000000-0000-0000-0000-000000000000"
        ),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(case, &body),
        "ResourceScanNotFound",
        "case {case:?}: expected the modelled not-found refusal, got {body}"
    );

    // A filter may carry the wildcard "at only the end of the string"; one that places it
    // anywhere else is refused, and leaves no scan behind — which the calls after it show, by
    // being accepted rather than refused as a second scan.
    for filter in ["AWS::*::Bucket", "AWS::S3::*Bucket", "*::S3::Bucket"] {
        let case = format!("a filter placing the wildcard at {filter:?}");
        let (status, body) = start_resource_scan(&router, &filter_members(&[filter])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&case, &body),
            "ValidationError",
            "case {case:?}: expected the service's own refusal code, got {body}"
        );
    }

    // The trailing form, a filter that is nothing BUT the trailing wildcard (so it admits
    // every type), and a filter with no wildcard at all are all accepted.
    for filter in ["AWS::S3::*", "*", "AWS::S3::Bucket"] {
        let case = format!("a filter of {filter:?}");
        let (status, body) = start_resource_scan(&router, &filter_members(&[filter])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected it accepted, got {status} with {body}"
        );
        wait_for_the_scan().await;
    }
}
