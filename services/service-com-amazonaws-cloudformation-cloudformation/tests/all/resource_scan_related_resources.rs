// SPDX-License-Identifier: CC0-1.0

//! `ListResourceScanRelatedResources`, end to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **`ListResourceScanRelatedResources`
//! reports, from the resource scan the request names, the resources RELATED to the ones the
//! request addresses — the other resources that scan listed which the same CloudFormation
//! stack manages — each as a `ScannedResource` carrying its resource type, the primary
//! identifier it is addressed by and whether a stack manages it, one page of at most
//! `MaxResults` of them at a time plus the token that resumes after it; a resource the scan
//! did not list relates nothing, a scan this account and Region holds no record of is refused
//! `ResourceScanNotFound`, and a scan that has not finished is refused
//! `ResourceScanInProgress`.**
//!
//! The tables below span the class that rule generalises over rather than one recorded call:
//! resources whose primary identifier is an ARN, a name the template declared and a name
//! CloudFormation generated; stacks of three, two and one resource; requests addressing one
//! resource, several of one stack, every resource of a stack, resources of two stacks at once,
//! the same resource twice and none at all; identifiers that address nothing (an unknown
//! value, the right value under the wrong type, a key the resource does not carry); and page
//! sizes at the documented default, at one, inside the set and at the published maximum.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out), and
//! the values a case addresses a resource by are the physical ids `ListStackResources` reports
//! for the stacks it deployed — so nothing here is a value written down by hand.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::resource_scans as scans;
use service_com_amazonaws_cloudformation_cloudformation::state::{self, PageOf};
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::error_code;
use crate::harness::{member, router, router_with_engine, store, xml_text, REGION};
use crate::stack_lifecycle::{call, call_in, deploy, list_members, ACCOUNT_ID};

/// A second Region, for the case that holds the read to the Region it was signed for.
const OTHER_REGION: &str = "us-west-2";

/// The members a declared `AWS::DynamoDB::Table` must carry for DynamoDB to create it: a
/// key schema with the attribute it names defined, and a capacity mode — a table declared
/// without `ProvisionedThroughput` has to name the on-demand mode.
const TABLE_KEY: &str = r#""KeySchema":[{"AttributeName":"pk","KeyType":"HASH"}],
    "AttributeDefinitions":[{"AttributeName":"pk","AttributeType":"S"}],
    "BillingMode":"PAY_PER_REQUEST""#;

/// A stack of three resources whose primary identifiers span the forms a type publishes: an
/// ARN (`AWS::SNS::Topic`'s `Ref` "returns the topic ARN"), a name the template declared
/// (`AWS::S3::Bucket`) and a name CloudFormation generated (`AWS::DynamoDB::Table`).
fn three_resources() -> String {
    format!(
        r#"{{"Resources":{{
    "Alpha":{{"Type":"AWS::SNS::Topic","Properties":{{"TopicName":"alpha-topic"}}}},
    "Beta":{{"Type":"AWS::S3::Bucket","Properties":{{"BucketName":"beta-bucket"}}}},
    "Gamma":{{"Type":"AWS::DynamoDB::Table","Properties":{{{TABLE_KEY}}}}}
}}}}"#
    )
}

/// A second stack: its resources are related to each other and to nothing the first one holds.
const TWO_RESOURCES: &str = r#"{"Resources":{
    "Delta":{"Type":"AWS::S3::Bucket","Properties":{"BucketName":"delta-bucket"}},
    "Epsilon":{"Type":"AWS::Logs::LogGroup","Properties":{"LogGroupName":"epsilon-group"}}
}}"#;

/// A stack of ONE resource: the scan lists it, and nothing in the scan is related to it.
const ONE_RESOURCE: &str = r#"{"Resources":{"Solo":{"Type":"AWS::SNS::Topic"}}}"#;

/// A stack of six resources, so a page boundary falls inside one stack's related resources.
fn six_resources() -> String {
    let tables: Vec<String> = [
        ("One", "paged-one"),
        ("Two", "paged-two"),
        ("Three", "paged-three"),
        ("Four", "paged-four"),
        ("Five", "paged-five"),
        ("Six", "paged-six"),
    ]
    .iter()
    .map(|(logical_id, name)| {
        format!(
            r#""{logical_id}":{{"Type":"AWS::DynamoDB::Table",
            "Properties":{{"TableName":"{name}",{TABLE_KEY}}}}}"#
        )
    })
    .collect();
    format!("{{\"Resources\":{{{}}}}}", tables.join(","))
}

/// The property name a resource of `resource_type` carries its primary identifier under.
///
/// Source: the "Return values" section the AWS CloudFormation resource and property type
/// reference publishes for each type — what `Ref` returns is the member that names the
/// resource outside CloudFormation, which is an ARN for `AWS::SNS::Topic` ("Ref returns the
/// topic ARN") and the resource's own name for the other three. `AWS::Logs::LogGroup` is here
/// as an entry no case of this suite's rule was written against.
pub(crate) fn identifier_key(resource_type: &str) -> &'static str {
    match resource_type {
        "AWS::SNS::Topic" => "TopicArn",
        "AWS::S3::Bucket" => "BucketName",
        "AWS::DynamoDB::Table" => "TableName",
        "AWS::Logs::LogGroup" => "LogGroupName",
        other => panic!("no published primary identifier catalogued for {other:?}"),
    }
}

/// One resource a scan listed: the type and primary-identifier pair a request addresses it by,
/// which is also what a response reports it as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Scanned {
    pub(crate) resource_type: String,
    pub(crate) key: String,
    pub(crate) value: String,
}

/// Every resource one stack deployed, keyed by the logical id its template declares it under,
/// read back through `ListStackResources` so the identifier values are the service's own.
pub(crate) async fn stack_catalogue(
    router: &MultiServiceRouter,
    stack: &str,
) -> BTreeMap<String, Scanned> {
    let (status, body) = call(
        router,
        "ListStackResources",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {stack:?}: ListStackResources answered {status} with {body}"
    );
    let mut catalogue = BTreeMap::new();
    for entry in list_members(&body, "StackResourceSummaries") {
        let logical_id = xml_text(&entry, "LogicalResourceId").unwrap_or_else(|| {
            panic!("case {stack:?}: expected a LogicalResourceId, got {entry}")
        });
        let resource_type = xml_text(&entry, "ResourceType")
            .unwrap_or_else(|| panic!("case {stack:?}: expected a ResourceType, got {entry}"));
        let value = xml_text(&entry, "PhysicalResourceId").unwrap_or_else(|| {
            panic!("case {stack:?}: expected a PhysicalResourceId, got {entry}")
        });
        let key = identifier_key(&resource_type).to_string();
        catalogue.insert(
            logical_id,
            Scanned {
                resource_type,
                key,
                value,
            },
        );
    }
    catalogue
}

/// Start a scan of this account and Region and wait past the instant its conclusion is due, so
/// the scan it answers the id of has a result to read.
pub(crate) async fn completed_scan(router: &MultiServiceRouter) -> String {
    let (status, body) = call(router, "StartResourceScan", &[]).await;
    assert_eq!(
        status, 200,
        "case \"starting the scan\": StartResourceScan answered {status} with {body}"
    );
    let resource_scan_id = xml_text(&body, "ResourceScanId").unwrap_or_else(|| {
        panic!("case \"starting the scan\": expected a ResourceScanId, got {body}")
    });
    tokio::time::sleep(scans::SCAN_DELAY + Duration::from_millis(250)).await;
    resource_scan_id
}

/// An account holding the stacks `templates` deploys, scanned: the router, the id of the
/// completed scan, and what the scan listed keyed by logical id.
async fn scanned_account(
    templates: &[(&str, &str)],
) -> (MultiServiceRouter, String, BTreeMap<String, Scanned>) {
    let router = router();
    let mut catalogue = BTreeMap::new();
    for (stack, template) in templates {
        deploy(&router, stack, template).await;
        catalogue.extend(stack_catalogue(&router, stack).await);
    }
    let resource_scan_id = completed_scan(&router).await;
    (router, resource_scan_id, catalogue)
}

/// The request members that address `resources`, one `ScannedResourceIdentifier` each in the
/// awsQuery flattened form.
pub(crate) fn requested_members(resources: &[Scanned]) -> Vec<(String, String)> {
    resources
        .iter()
        .enumerate()
        .flat_map(|(position, resource)| {
            let index = position + 1;
            [
                member(
                    &format!("Resources.member.{index}.ResourceType"),
                    &resource.resource_type,
                ),
                member(
                    &format!("Resources.member.{index}.ResourceIdentifier.entry.1.key"),
                    &resource.key,
                ),
                member(
                    &format!("Resources.member.{index}.ResourceIdentifier.entry.1.value"),
                    &resource.value,
                ),
            ]
        })
        .collect()
}

/// The resources one response reports, sorted: each as its type, its one primary-identifier
/// pair and what it says about being managed by a stack.
///
/// `items` is the response member the rows arrive under, so the suites that read a scan's rows
/// (`RelatedResources` here, `Resources` for `ListResourceScanResources`) read them the one way.
pub(crate) fn reported_rows(
    case: &str,
    body: &str,
    items: &str,
) -> Vec<(String, String, String, String)> {
    let mut rows: Vec<(String, String, String, String)> = list_members(body, items)
        .iter()
        .map(|entry| {
            (
                xml_text(entry, "ResourceType")
                    .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceType, got {entry}")),
                xml_text(entry, "key")
                    .unwrap_or_else(|| panic!("case {case:?}: expected an identifier key, got {entry}")),
                xml_text(entry, "value")
                    .unwrap_or_else(|| panic!("case {case:?}: expected an identifier value, got {entry}")),
                xml_text(entry, "ManagedByStack")
                    .unwrap_or_else(|| panic!("case {case:?}: expected a ManagedByStack, got {entry}")),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// [`reported_rows`] over this operation's own `RelatedResources` member.
fn reported(case: &str, body: &str) -> Vec<(String, String, String, String)> {
    reported_rows(case, body, "RelatedResources")
}

/// The rows `expected` logical ids must come back as: every related resource of this emulator's
/// scans is one a stack manages, so each reports `true`.
pub(crate) fn expected_rows(
    catalogue: &BTreeMap<String, Scanned>,
    expected: &[&str],
) -> Vec<(String, String, String, String)> {
    let mut rows: Vec<(String, String, String, String)> = expected
        .iter()
        .map(|logical_id| {
            let resource = catalogue
                .get(*logical_id)
                .unwrap_or_else(|| panic!("the fixture holds no resource {logical_id:?}"));
            (
                resource.resource_type.clone(),
                resource.key.clone(),
                resource.value.clone(),
                "true".to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// The id of a scan this suite SEEDS, in the ARN form the model documents, built from an
/// index so a case can name the record it reads.
pub(crate) fn seeded_scan_id(index: usize) -> String {
    scans::resource_scan_arn(REGION, &format!("00000000-0000-4000-8000-{index:012}"))
}

/// A scan recorded in flight, with its conclusion due `due_in` from now.
///
/// Seeded through the store's one writing entry point, the device `resource_scans.rs` uses for
/// records a bound's worth of wire calls cannot produce: a case that must read a scan while it
/// is still running, or exactly as its conclusion falls due, cannot then race the clock.
pub(crate) fn in_flight(resource_scan_id: String, due_in: Duration) -> scans::StoredResourceScan {
    let now = SystemTime::now();
    scans::StoredResourceScan {
        resource_scan_id,
        region: REGION.to_string(),
        scan_type: scans::scan_type::FULL.to_string(),
        status: scans::status::IN_PROGRESS.to_string(),
        status_reason: None,
        start_time: now,
        end_time: None,
        percentage_completed: Some(scans::PERCENTAGE_STARTED),
        pending: Some(scans::PendingScanOutcome {
            status: scans::status::COMPLETE.to_string(),
            status_reason: None,
            percentage_completed: scans::PERCENTAGE_COMPLETE,
            due_at: now + due_in,
        }),
        client_request_token: None,
        scan_filters: Vec::new(),
        resources_read: 0,
        resources: Vec::new(),
    }
}

/// One resource of the fixture, as a request addresses it.
pub(crate) fn addressing(catalogue: &BTreeMap<String, Scanned>, logical_id: &str) -> Scanned {
    catalogue
        .get(logical_id)
        .unwrap_or_else(|| panic!("the fixture holds no resource {logical_id:?}"))
        .clone()
}

/// `ListResourceScanRelatedResources` over one scan, addressing `resources`, with whatever
/// else the case names.
async fn list_related(
    router: &MultiServiceRouter,
    resource_scan_id: &str,
    resources: &[Scanned],
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("ResourceScanId", resource_scan_id)];
    members.extend(requested_members(resources));
    members.extend_from_slice(extra);
    call(router, "ListResourceScanRelatedResources", &members).await
}

/// The related resources of a set of addressed resources are the OTHER resources of the scan
/// that the same stack manages — never the addressed resources themselves, never a resource
/// another stack manages, and nothing at all for an identifier the scan listed no resource
/// under.
#[tokio::test]
async fn related_resources_are_the_other_resources_the_same_stack_manages() {
    let three = three_resources();
    let (router, resource_scan_id, catalogue) = scanned_account(&[
        ("related-three", &three),
        ("related-two", TWO_RESOURCES),
        ("related-one", ONE_RESOURCE),
    ])
    .await;

    let alpha = addressing(&catalogue, "Alpha");
    let cases: Vec<(&str, Vec<Scanned>, Vec<&str>)> = vec![
        (
            "one resource, addressed by the ARN its type publishes",
            vec![alpha.clone()],
            vec!["Beta", "Gamma"],
        ),
        (
            "one resource, addressed by the name its template declared",
            vec![addressing(&catalogue, "Beta")],
            vec!["Alpha", "Gamma"],
        ),
        (
            "one resource, addressed by the name CloudFormation generated for it",
            vec![addressing(&catalogue, "Gamma")],
            vec!["Alpha", "Beta"],
        ),
        (
            "two resources of the same stack",
            vec![alpha.clone(), addressing(&catalogue, "Beta")],
            vec!["Gamma"],
        ),
        (
            "every resource of a stack: nothing is left to be related to",
            vec![
                alpha.clone(),
                addressing(&catalogue, "Beta"),
                addressing(&catalogue, "Gamma"),
            ],
            vec![],
        ),
        (
            "one resource of each of two stacks",
            vec![alpha.clone(), addressing(&catalogue, "Delta")],
            vec!["Beta", "Gamma", "Epsilon"],
        ),
        (
            "the only resource of its stack",
            vec![addressing(&catalogue, "Solo")],
            vec![],
        ),
        (
            "the same resource addressed twice: each related resource once",
            vec![alpha.clone(), alpha.clone()],
            vec!["Beta", "Gamma"],
        ),
        (
            "a type the account holds, under an identifier value nothing carries",
            vec![Scanned {
                resource_type: "AWS::S3::Bucket".to_string(),
                key: "BucketName".to_string(),
                value: "no-such-bucket".to_string(),
            }],
            vec![],
        ),
        (
            "one resource's identifier under another type",
            vec![Scanned {
                resource_type: "AWS::DynamoDB::Table".to_string(),
                key: alpha.key.clone(),
                value: alpha.value.clone(),
            }],
            vec![],
        ),
        (
            "a resource's own type under a primary identifier it does not carry",
            vec![Scanned {
                resource_type: "AWS::SNS::Topic".to_string(),
                key: "TopicName".to_string(),
                value: "alpha-topic".to_string(),
            }],
            vec![],
        ),
    ];

    for (case, resources, expected) in &cases {
        let (status, body) = list_related(&router, &resource_scan_id, resources, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let expected_rows = expected_rows(&catalogue, expected);
        assert_eq!(
            reported(case, &body),
            expected_rows,
            "case {case:?}: expected {expected_rows:?}, got {body}"
        );
        assert!(
            body.contains("<RelatedResources"),
            "case {case:?}: RelatedResources is reported as an empty element, got {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: one page holds every related resource, so no NextToken, got {body}"
        );
    }

    // The read REPORTS: the same request answers the same resources the second time, and the
    // scan it read is the record it was before.
    for (case, resources, expected) in &cases {
        let (status, body) = list_related(&router, &resource_scan_id, resources, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 on the repeat read, got {status} {body}"
        );
        let expected_rows = expected_rows(&catalogue, expected);
        assert_eq!(
            reported(case, &body),
            expected_rows,
            "case {case:?}: the repeat read must report {expected_rows:?}, got {body}"
        );
    }
    let (status, body) = call(
        &router,
        "DescribeResourceScan",
        &[member("ResourceScanId", &resource_scan_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the scan after the reads\": expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(scans::status::COMPLETE),
        "case \"the scan after the reads\": the reads left the scan as it was, got {body}"
    );
}

/// One response carries one page of related resources, as large as the request asked for and
/// no larger than the published maximum, and the pages walk the set exactly once.
#[tokio::test]
async fn one_response_carries_at_most_the_requested_page_of_related_resources() {
    let (router, resource_scan_id, catalogue) =
        scanned_account(&[("related-paged", &six_resources())]).await;
    let addressed = vec![addressing(&catalogue, "One")];
    let related = ["Two", "Three", "Four", "Five", "Six"];

    // "By default the ListResourceScanRelatedResources API action will return up to 100
    // results in each response. The maximum value is 100."
    for (case, max_results, expected_count) in [
        ("no MaxResults: the documented default of 100", None, related.len()),
        ("MaxResults 1: the smallest page", Some(1), 1),
        ("MaxResults 2: the page asked for", Some(2), 2),
        ("MaxResults 5: exactly the set", Some(5), related.len()),
        (
            "MaxResults 100: the published maximum",
            Some(i32::try_from(scans::MAX_RESULTS_CEILING).expect("a page size")),
            related.len(),
        ),
    ] {
        let extra = max_results
            .map(|max| vec![member("MaxResults", &max.to_string())])
            .unwrap_or_default();
        let (status, body) = list_related(&router, &resource_scan_id, &addressed, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let page = reported(case, &body);
        assert_eq!(
            page.len(),
            expected_count,
            "case {case:?}: expected {expected_count} related resources, got {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken").is_some(),
            expected_count < related.len(),
            "case {case:?}: a page with more behind it carries a NextToken, got {body}"
        );
    }

    // The pages walk the whole set exactly once.
    let mut walked: Vec<(String, String, String, String)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut extra = vec![member("MaxResults", "2")];
        if let Some(token) = token.as_deref() {
            extra.push(member("NextToken", token));
        }
        let (status, body) = list_related(&router, &resource_scan_id, &addressed, &extra).await;
        assert_eq!(
            status, 200,
            "case \"walking the pages\": expected 200, got {status} {body}"
        );
        let page = reported("walking the pages", &body);
        assert!(
            page.len() <= 2,
            "case \"walking the pages\": a page of 2 carries at most 2, got {}",
            page.len()
        );
        walked.extend(page);
        token = xml_text(&body, "NextToken");
        if token.is_none() {
            break;
        }
    }
    walked.sort();
    let expected = expected_rows(&catalogue, &related);
    assert_eq!(
        walked, expected,
        "case \"walking the pages\": expected every related resource exactly once, got {walked:?}"
    );

    // A cursor this collection never issued is a bad request, not an empty page.
    for (case, bad_token) in [
        (
            "a cursor past the end",
            state::encode_next_token(PageOf::RelatedResources, related.len() + 2),
        ),
        (
            "a cursor issued for another collection",
            state::encode_next_token(PageOf::ResourceScanSummaries, 0),
        ),
    ] {
        let (status, body) = list_related(
            &router,
            &resource_scan_id,
            &addressed,
            &[member("NextToken", &bad_token)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(case, &body),
            "ValidationError",
            "case {case:?}: expected the service's own refusal code, got {body}"
        );
    }
}

/// The refusals around the read: a scan this account and Region holds no record of, a scan
/// still running, and a request asking about more resources than one call may carry.
#[tokio::test]
async fn the_refusals_around_listing_related_resources() {
    let (router, resource_scan_id, catalogue) = scanned_account(&[
        ("refused-three", &three_resources()),
    ])
    .await;
    let addressed = vec![addressing(&catalogue, "Alpha")];

    // An id nothing started, in the very ARN form a real id carries.
    let case = "an id nothing started";
    let (status, body) = list_related(
        &router,
        &format!(
            "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:resourceScan/\
             00000000-0000-0000-0000-000000000000"
        ),
        &addressed,
        &[],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(case, &body),
        "ResourceScanNotFound",
        "case {case:?}: expected the modelled refusal, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some(scans::NOT_FOUND),
        "case {case:?}: expected the sentence that refusal carries, got {body}"
    );

    // A scan is addressable only in the Region it was started in.
    let case = "a scan read from another Region";
    let mut members = vec![member("ResourceScanId", &resource_scan_id)];
    members.extend(requested_members(&addressed));
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListResourceScanRelatedResources",
        &members,
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(case, &body),
        "ResourceScanNotFound",
        "case {case:?}: expected the modelled refusal, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some(scans::NOT_FOUND),
        "case {case:?}: expected the sentence that refusal carries, got {body}"
    );

    // A request asking about more resources than one call may carry: "Up to 100 resources can
    // be provided."
    for (case, count) in [
        ("exactly the documented maximum of addressed resources", scans::MAX_REQUESTED_RESOURCES),
        ("one resource past it", scans::MAX_REQUESTED_RESOURCES + 1),
    ] {
        let asked: Vec<Scanned> = (0..count)
            .map(|index| Scanned {
                resource_type: "AWS::S3::Bucket".to_string(),
                key: "BucketName".to_string(),
                value: format!("bulk-bucket-{index}"),
            })
            .collect();
        let (status, body) = list_related(&router, &resource_scan_id, &asked, &[]).await;
        if count <= scans::MAX_REQUESTED_RESOURCES {
            assert_eq!(
                status, 200,
                "case {case:?}: expected it accepted, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} {body}"
            );
            assert_eq!(
                error_code(case, &body),
                "ValidationError",
                "case {case:?}: expected the service's own refusal code, got {body}"
            );
        }
    }

    // A `MaxResults` outside the range the model publishes is a bad request.
    for (case, max_results) in [
        ("MaxResults 0: under the modelled minimum", "0"),
        ("MaxResults 101: past the modelled maximum", "101"),
    ] {
        let (status, body) = list_related(
            &router,
            &resource_scan_id,
            &addressed,
            &[member("MaxResults", max_results)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
    }

    // `Resources` is required: a request that names no resource to relate anything to is a bad
    // request rather than an empty answer.
    let case = "a request naming no Resources at all";
    let (status, body) = call(
        &router,
        "ListResourceScanRelatedResources",
        &[member("ResourceScanId", &resource_scan_id)],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );

    // A scan that has not finished has no result to read. Seeded in flight with its
    // conclusion an hour out — through the store's one writing entry point, the device
    // `resource_scans.rs` uses — so the refusal cannot race the clock, and seeded a second
    // time with its conclusion already due, which is what lifts the refusal.
    let (fresh, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        held.record_resource_scan(in_flight(seeded_scan_id(1), Duration::from_secs(3_600)));
        held.record_resource_scan(in_flight(seeded_scan_id(2), Duration::ZERO));
    }

    let case = "a scan that is still running";
    let (status, body) = list_related(&fresh, &seeded_scan_id(1), &addressed, &[]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(case, &body),
        "ResourceScanInProgress",
        "case {case:?}: expected the modelled refusal, got {body}"
    );
    assert_eq!(
        xml_text(&body, "Message").as_deref(),
        Some(scans::OPERATION_WHILE_SCANNING),
        "case {case:?}: expected the sentence that refusal carries, got {body}"
    );

    // And the refusal lifts on its own as the clock concludes the scan.
    let case = "a scan whose conclusion the clock has made due";
    let (status, body) = list_related(&fresh, &seeded_scan_id(2), &addressed, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected it accepted, got {status} {body}"
    );
    assert!(
        reported(case, &body).is_empty(),
        "case {case:?}: that scan listed no resource, so nothing is related, got {body}"
    );
}
