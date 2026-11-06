// SPDX-License-Identifier: CC0-1.0

//! `ListResourceScanResources`, end to end over the awsQuery wire.
//!
//! The rule every case here holds the service to: **`ListResourceScanResources` reports the
//! resources the resource scan the request names FOUND — each as a `ScannedResource` carrying
//! its resource type, the primary identifier it is addressed by and whether a stack manages
//! it — narrowed to the rows that satisfy EVERY filter the request named (a primary-identifier
//! value, a resource-type prefix, a tag key, a tag value), one page of at most `MaxResults` of
//! them at a time plus the token that resumes after it; a scan this account and Region holds
//! no record of is refused `ResourceScanNotFound`, and a scan that has not finished is refused
//! `ResourceScanInProgress`.**
//!
//! The tables below span the class that rule generalises over rather than one recorded call:
//! resources whose primary identifier is an ARN, a name the template declared and a name
//! CloudFormation generated; filters naming an identifier value, a type prefix at a service
//! namespace, at a whole type and at the partition-wide namespace, a tag key and a tag value
//! that reach a resource through its own declaration, through the stack that propagated it and
//! through the tags CloudFormation records itself; filters combined, filters that admit
//! everything and filters that admit nothing; and page sizes at the documented default, at
//! one, inside the set, exactly at the set and at the published maximum.
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out), and
//! the values a case filters by are the physical ids `ListStackResources` reports for the
//! stacks it deployed — so nothing here is a value written down by hand.

use std::collections::BTreeMap;
use std::time::Duration;

use service_com_amazonaws_cloudformation_cloudformation::state::resource_scans as scans;
use service_com_amazonaws_cloudformation_cloudformation::state::{self, provisioning, PageOf};
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::error_code;
use crate::harness::{
    create_stack, member, router, router_with_engine, store, wait_for_the_transition, xml_text,
    REGION,
};
use crate::resource_scan_related_resources::{
    addressing, completed_scan, expected_rows, in_flight, reported_rows, requested_members,
    seeded_scan_id, stack_catalogue, Scanned,
};
use crate::stack_lifecycle::{call, call_in, deploy, list_members, ACCOUNT_ID};

/// A second Region, for the case that holds the read to the Region it was signed for.
const OTHER_REGION: &str = "us-west-2";

/// The members a declared `AWS::DynamoDB::Table` must carry for DynamoDB to create it: a
/// key schema with the attribute it names defined, and a capacity mode — a table declared
/// without `ProvisionedThroughput` has to name the on-demand mode.
const TABLE_KEY: &str = r#""KeySchema":[{"AttributeName":"pk","KeyType":"HASH"}],
    "AttributeDefinitions":[{"AttributeName":"pk","AttributeType":"S"}],
    "BillingMode":"PAY_PER_REQUEST""#;

/// A stack whose resources span the forms a primary identifier takes — an ARN
/// (`AWS::SNS::Topic`'s `Ref` "returns the topic ARN"), a name the template declared
/// (`AWS::S3::Bucket`, `AWS::Logs::LogGroup`) and a name CloudFormation generated
/// (`AWS::DynamoDB::Table`) — and whose declarations span a resource that asks for tags of its
/// own and one that asks for none.
fn tagged_stack() -> String {
    format!(
        r#"{{"Resources":{{
    "Alpha":{{"Type":"AWS::SNS::Topic","Properties":{{"TopicName":"scanned-alpha-topic"}}}},
    "Beta":{{"Type":"AWS::S3::Bucket","Properties":{{"BucketName":"scanned-beta-bucket"}}}},
    "Gamma":{{"Type":"AWS::DynamoDB::Table",
        "Properties":{{"Tags":[{{"Key":"Tier","Value":"gold"}}],{TABLE_KEY}}}}},
    "Delta":{{"Type":"AWS::Logs::LogGroup","Properties":{{
        "LogGroupName":"scanned-delta-group","Tags":[{{"Key":"Tier","Value":"silver"}}]}}}}
}}}}"#
    )
}

/// A second stack, carrying no stack-level tags at all: its resource is in the same scan and
/// outside every tag filter the first stack's tags answer.
const PLAIN_STACK: &str = r#"{"Resources":{
    "Epsilon":{"Type":"AWS::S3::Bucket","Properties":{"BucketName":"scanned-epsilon-bucket"}}
}}"#;

/// A stack of six resources, so a page boundary falls inside one scan's resources.
fn six_resources() -> String {
    let tables: Vec<String> = [
        ("One", "scanned-one"),
        ("Two", "scanned-two"),
        ("Three", "scanned-three"),
        ("Four", "scanned-four"),
        ("Five", "scanned-five"),
        ("Six", "scanned-six"),
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

/// The stack-level tags the tagged stack is deployed with, which CloudFormation propagates to
/// the resources it creates in that stack.
const STACK_TAGS: &[(&str, &str)] = &[("Owner", "platform"), ("Environment", "staging")];

/// The name of the stack carrying [`STACK_TAGS`].
const TAGGED_STACK_NAME: &str = "scanned-tagged";

/// The `Tags.member.N.Key`/`Value` request members a stack-level tag list is.
fn tag_members(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .enumerate()
        .flat_map(|(position, (key, value))| {
            let index = position + 1;
            [
                member(&format!("Tags.member.{index}.Key"), key),
                member(&format!("Tags.member.{index}.Value"), value),
            ]
        })
        .collect()
}

/// Deploy a stack with stack-level tags, wait past the instant its create settles and assert it
/// completed — [`deploy`] for a case whose resources must carry propagated tags.
async fn deploy_tagged(router: &MultiServiceRouter, name: &str, template: &str) {
    let (status, body) = create_stack(router, name, template, &tag_members(STACK_TAGS)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: CreateStack answered {status} with {body}"
    );
    wait_for_the_transition().await;
    let (status, body) = call(router, "DescribeStacks", &[member("StackName", name)]).await;
    assert_eq!(
        status, 200,
        "case {name:?}: DescribeStacks answered {status} with {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {name:?}: expected the create to complete, got {body}"
    );
}

/// The account the filter cases read: a tagged stack of four resources, an untagged stack of
/// one, and the scan that found all five.
async fn scanned_account() -> (MultiServiceRouter, String, BTreeMap<String, Scanned>) {
    let router = router();
    deploy_tagged(&router, TAGGED_STACK_NAME, &tagged_stack()).await;
    deploy(&router, "scanned-plain", PLAIN_STACK).await;
    let mut catalogue = stack_catalogue(&router, TAGGED_STACK_NAME).await;
    catalogue.extend(stack_catalogue(&router, "scanned-plain").await);
    let resource_scan_id = completed_scan(&router).await;
    (router, resource_scan_id, catalogue)
}

/// The filters one request narrows the scan by.
#[derive(Debug, Clone, Default)]
struct Filters {
    resource_identifier: Option<String>,
    resource_type_prefix: Option<String>,
    tag_key: Option<String>,
    tag_value: Option<String>,
}

impl Filters {
    /// Narrow by one primary-identifier VALUE.
    fn identifier(value: &str) -> Self {
        Self {
            resource_identifier: Some(value.to_string()),
            ..Self::default()
        }
    }

    /// Narrow by a resource-type prefix.
    fn prefix(prefix: &str) -> Self {
        Self {
            resource_type_prefix: Some(prefix.to_string()),
            ..Self::default()
        }
    }

    /// Narrow by a tag key.
    fn key(key: &str) -> Self {
        Self {
            tag_key: Some(key.to_string()),
            ..Self::default()
        }
    }

    /// Narrow by a tag value.
    fn value(value: &str) -> Self {
        Self {
            tag_value: Some(value.to_string()),
            ..Self::default()
        }
    }

    /// The same filters with a tag key added.
    fn and_key(mut self, key: &str) -> Self {
        self.tag_key = Some(key.to_string());
        self
    }

    /// The same filters with a tag value added.
    fn and_value(mut self, value: &str) -> Self {
        self.tag_value = Some(value.to_string());
        self
    }

    /// The same filters with a primary-identifier value added.
    fn and_identifier(mut self, value: &str) -> Self {
        self.resource_identifier = Some(value.to_string());
        self
    }

    /// The request members these filters are.
    fn members(&self) -> Vec<(String, String)> {
        [
            ("ResourceIdentifier", self.resource_identifier.as_deref()),
            ("ResourceTypePrefix", self.resource_type_prefix.as_deref()),
            ("TagKey", self.tag_key.as_deref()),
            ("TagValue", self.tag_value.as_deref()),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| member(name, value)))
        .collect()
    }
}

/// `ListResourceScanResources` over one scan, with whatever else the case names.
async fn list_resources(
    router: &MultiServiceRouter,
    resource_scan_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut members = vec![member("ResourceScanId", resource_scan_id)];
    members.extend_from_slice(extra);
    call(router, "ListResourceScanResources", &members).await
}

/// The resources one response reports, sorted — [`reported_rows`] over this operation's own
/// `Resources` member.
fn reported(case: &str, body: &str) -> Vec<(String, String, String, String)> {
    reported_rows(case, body, "Resources")
}

/// The scan reports every resource it found, and a request's filters narrow that set — one
/// filter at a time and several at once, with a resource admitted only when every filter the
/// request named admits it.
#[tokio::test]
async fn the_filters_narrow_the_resources_the_scan_found() {
    let (router, resource_scan_id, catalogue) = scanned_account().await;
    let everything = ["Alpha", "Beta", "Gamma", "Delta", "Epsilon"];
    let tagged_stack = ["Alpha", "Beta", "Gamma", "Delta"];
    let alpha = addressing(&catalogue, "Alpha");
    let beta = addressing(&catalogue, "Beta");
    let gamma = addressing(&catalogue, "Gamma");
    let delta = addressing(&catalogue, "Delta");

    let cases: Vec<(&str, Filters, Vec<&str>)> = vec![
        (
            "no filter at all: every resource the scan found",
            Filters::default(),
            everything.to_vec(),
        ),
        (
            "ResourceIdentifier: the ARN its type publishes",
            Filters::identifier(&alpha.value),
            vec!["Alpha"],
        ),
        (
            "ResourceIdentifier: the name the template declared",
            Filters::identifier(&beta.value),
            vec!["Beta"],
        ),
        (
            "ResourceIdentifier: the name CloudFormation generated",
            Filters::identifier(&gamma.value),
            vec!["Gamma"],
        ),
        (
            "ResourceIdentifier: a value nothing in the account carries",
            Filters::identifier("no-such-resource"),
            vec![],
        ),
        (
            "ResourceIdentifier: an identifier KEY rather than its value",
            Filters::identifier(&beta.key),
            vec![],
        ),
        (
            "ResourceTypePrefix: one service's namespace",
            Filters::prefix("AWS::S3::"),
            vec!["Beta", "Epsilon"],
        ),
        (
            "ResourceTypePrefix: a whole type",
            Filters::prefix("AWS::DynamoDB::Table"),
            vec!["Gamma"],
        ),
        (
            "ResourceTypePrefix: the namespace every type is under",
            Filters::prefix("AWS::"),
            everything.to_vec(),
        ),
        (
            "ResourceTypePrefix: a namespace the account holds nothing of",
            Filters::prefix("AWS::Kinesis::"),
            vec![],
        ),
        (
            "TagKey: a stack-level tag the stack propagated to its resources",
            Filters::key(STACK_TAGS[0].0),
            tagged_stack.to_vec(),
        ),
        (
            "TagKey: the second stack-level tag",
            Filters::key(STACK_TAGS[1].0),
            tagged_stack.to_vec(),
        ),
        (
            "TagKey: the tag CloudFormation records the stack name under",
            Filters::key(provisioning::STACK_NAME_TAG),
            everything.to_vec(),
        ),
        (
            "TagKey: a tag one resource's own declaration asked for",
            Filters::key("Tier"),
            vec!["Gamma", "Delta"],
        ),
        (
            "TagKey: a key nothing carries",
            Filters::key("NoSuchTagKey"),
            vec![],
        ),
        (
            "TagValue: a stack-level tag's value",
            Filters::value(STACK_TAGS[0].1),
            tagged_stack.to_vec(),
        ),
        (
            "TagValue: the value CloudFormation records the stack name as",
            Filters::value(TAGGED_STACK_NAME),
            tagged_stack.to_vec(),
        ),
        (
            "TagValue: the value one declaration gave its own tag",
            Filters::value("gold"),
            vec!["Gamma"],
        ),
        (
            "TagValue: a value nothing carries",
            Filters::value("no-such-tag-value"),
            vec![],
        ),
        (
            "TagKey and TagValue: both of one declaration",
            Filters::key("Tier").and_value("silver"),
            vec!["Delta"],
        ),
        (
            "TagKey and TagValue: two filters, each matched by a tag of its own",
            Filters::key("Tier").and_value(STACK_TAGS[0].1),
            vec!["Gamma", "Delta"],
        ),
        (
            "TagKey and TagValue: a key some resources carry and a value none does",
            Filters::key("Tier").and_value("no-such-tag-value"),
            vec![],
        ),
        (
            "a type prefix and a tag key that narrow to one resource",
            Filters::prefix("AWS::Logs::").and_key("Tier"),
            vec!["Delta"],
        ),
        (
            "a type prefix and a tag key no resource of that type carries",
            Filters::prefix("AWS::S3::").and_key("Tier"),
            vec![],
        ),
        (
            "every filter at once, all four admitting the same resource",
            Filters::prefix("AWS::Logs::")
                .and_key("Tier")
                .and_value("silver")
                .and_identifier(&delta.value),
            vec!["Delta"],
        ),
        (
            "every filter at once, one of them admitting nothing",
            Filters::prefix("AWS::Logs::")
                .and_key("Tier")
                .and_value("silver")
                .and_identifier(&beta.value),
            vec![],
        ),
    ];

    for (case, filters, expected) in &cases {
        let (status, body) = list_resources(&router, &resource_scan_id, &filters.members()).await;
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
            body.contains("<Resources"),
            "case {case:?}: Resources is reported as an empty element, got {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken"),
            None,
            "case {case:?}: one page holds every resource of this fixture, so no NextToken, \
             got {body}"
        );
    }

    // The read REPORTS: the same request answers the same resources the second time, and the
    // scan it read is the record it was before.
    for (case, filters, expected) in &cases {
        let (status, body) = list_resources(&router, &resource_scan_id, &filters.members()).await;
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
    let case = "the scan after the reads";
    let (status, body) = call(
        &router,
        "DescribeResourceScan",
        &[member("ResourceScanId", &resource_scan_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Status").as_deref(),
        Some(scans::status::COMPLETE),
        "case {case:?}: the reads left the scan as it was, got {body}"
    );
    assert_eq!(
        xml_text(&body, "ResourcesScanned").as_deref(),
        Some(everything.len().to_string().as_str()),
        "case {case:?}: the scan reports the rows the list walked, got {body}"
    );

    // The identifier a row is reported under is the one that ADDRESSES it: the related-resources
    // read of that same row answers the other resources of its stack.
    let case = "a reported row addressed in another read of the same scan";
    let mut members = vec![member("ResourceScanId", &resource_scan_id)];
    members.extend(requested_members(std::slice::from_ref(&alpha)));
    let (status, body) = call(&router, "ListResourceScanRelatedResources", &members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200, got {status} {body}"
    );
    let related: Vec<String> = list_members(&body, "RelatedResources")
        .iter()
        .filter_map(|entry| xml_text(entry, "value"))
        .collect();
    let mut related = related;
    related.sort();
    let mut expected: Vec<String> = ["Beta", "Gamma", "Delta"]
        .iter()
        .map(|logical_id| addressing(&catalogue, logical_id).value)
        .collect();
    expected.sort();
    assert_eq!(
        related, expected,
        "case {case:?}: expected the other resources of that stack, got {body}"
    );
}

/// One response carries one page of resources, as large as the request asked for and no larger
/// than the published maximum, and the pages walk the set exactly once.
#[tokio::test]
async fn one_response_carries_at_most_the_requested_page_of_resources() {
    let router = router();
    deploy(&router, "scanned-paged", &six_resources()).await;
    let catalogue = stack_catalogue(&router, "scanned-paged").await;
    let resource_scan_id = completed_scan(&router).await;
    let found = ["One", "Two", "Three", "Four", "Five", "Six"];

    // "By default the ListResourceScanResources API action will return at most 100 results in
    // each response. The maximum value is 100."
    for (case, max_results, expected_count) in [
        (
            "no MaxResults: the documented default of 100",
            None,
            found.len(),
        ),
        ("MaxResults 1: the smallest page", Some(1), 1),
        ("MaxResults 2: the page asked for", Some(2), 2),
        ("MaxResults 5: one short of the set", Some(5), 5),
        ("MaxResults 6: exactly the set", Some(6), found.len()),
        (
            "MaxResults 100: the published maximum",
            Some(i32::try_from(scans::MAX_RESULTS_CEILING).expect("a page size")),
            found.len(),
        ),
    ] {
        let extra = max_results
            .map(|max| vec![member("MaxResults", &max.to_string())])
            .unwrap_or_default();
        let (status, body) = list_resources(&router, &resource_scan_id, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let page = reported(case, &body);
        assert_eq!(
            page.len(),
            expected_count,
            "case {case:?}: expected {expected_count} resources, got {body}"
        );
        assert_eq!(
            xml_text(&body, "NextToken").is_some(),
            expected_count < found.len(),
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
        let (status, body) = list_resources(&router, &resource_scan_id, &extra).await;
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
    let expected = expected_rows(&catalogue, &found);
    assert_eq!(
        walked, expected,
        "case \"walking the pages\": expected every resource exactly once, got {walked:?}"
    );

    // A page is walked under the filters too: the token resumes the FILTERED set.
    let case = "paging a filtered read";
    let mut filtered: Vec<(String, String, String, String)> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut extra = vec![
            member("MaxResults", "2"),
            member("ResourceTypePrefix", "AWS::DynamoDB::"),
        ];
        if let Some(token) = token.as_deref() {
            extra.push(member("NextToken", token));
        }
        let (status, body) = list_resources(&router, &resource_scan_id, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        filtered.extend(reported(case, &body));
        token = xml_text(&body, "NextToken");
        if token.is_none() {
            break;
        }
    }
    filtered.sort();
    assert_eq!(
        filtered, expected,
        "case {case:?}: every resource of that type exactly once, got {filtered:?}"
    );

    // A cursor this collection never issued is a bad request, not an empty page.
    for (case, bad_token) in [
        (
            "a cursor past the end",
            state::encode_next_token(PageOf::ScannedResources, found.len() + 2),
        ),
        (
            "a cursor issued for another collection",
            state::encode_next_token(PageOf::RelatedResources, 0),
        ),
    ] {
        let (status, body) = list_resources(
            &router,
            &resource_scan_id,
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

/// The refusals around the read: a scan this account and Region holds no record of, and a scan
/// that has not finished.
#[tokio::test]
async fn the_refusals_around_listing_a_scan_s_resources() {
    let router = router();
    deploy(&router, "refused-scanned", PLAIN_STACK).await;
    let resource_scan_id = completed_scan(&router).await;

    // An id nothing started, in the very ARN form a real id carries.
    let case = "an id nothing started";
    let (status, body) = list_resources(
        &router,
        &format!(
            "arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:resourceScan/\
             00000000-0000-0000-0000-000000000000"
        ),
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
    let (status, body) = call_in(
        &router,
        OTHER_REGION,
        "ListResourceScanResources",
        &[member("ResourceScanId", &resource_scan_id)],
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

    // `ResourceScanId` is required: a request that names no scan is a bad request rather than
    // an empty answer.
    let case = "a request naming no ResourceScanId";
    let (status, body) = call(&router, "ListResourceScanResources", &[]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body}"
    );

    // A scan that has not finished has no result to list. Seeded in flight with its conclusion
    // an hour out — through the store's one writing entry point — so the refusal cannot race
    // the clock, and seeded a second time with its conclusion already due, which is what lifts
    // the refusal.
    let (fresh, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        held.record_resource_scan(in_flight(seeded_scan_id(1), Duration::from_secs(3_600)));
        held.record_resource_scan(in_flight(seeded_scan_id(2), Duration::ZERO));
    }

    let case = "a scan that is still running";
    let (status, body) = list_resources(&fresh, &seeded_scan_id(1), &[]).await;
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
    let (status, body) = list_resources(&fresh, &seeded_scan_id(2), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected it accepted, got {status} {body}"
    );
    assert!(
        reported(case, &body).is_empty(),
        "case {case:?}: that scan found no resource, so it lists none, got {body}"
    );
}
