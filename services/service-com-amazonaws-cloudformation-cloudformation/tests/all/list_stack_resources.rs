// SPDX-License-Identifier: CC0-1.0

//! `ListStackResources`, end to end over the awsQuery wire.
//!
//! The rule under test: `ListStackResources` reports one `StackResourceSummary` per
//! resource the stack named by `StackName` materialised in its last completed deployment
//! — carrying the logical id and type the template declared, the physical id
//! CloudFormation's documented naming rules gave it, and the resource-level status that
//! operation left the resource in with the instant it reached that status — one bounded
//! page at a time, resuming exactly where the previous page's `NextToken` left off. The
//! stack is named by name or by stack id while it is running and by stack id once it has
//! been deleted, and a `StackName` that names no stack is refused rather than answered
//! with an empty list.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client calling `ListStackResources` sees. The table spans the class the rule
//! generalises over: both template spellings, a template declaring no resources at all,
//! resource types the return-value catalogue carries and one it does not, named and
//! generated physical ids, the declaration order of the resources, their count across a
//! page boundary, and the lifecycle status the stack's last operation left them in.

use std::collections::BTreeSet;

use crate::stack_lifecycle::{
    call, create_stack, member, router, wait_for_the_transition, xml_text,
};
// The XML readers and the physical-id expectation this suite needs already exist, shared,
// in the `DescribeStackResources` suite: one fix to how a response is read serves both.
use crate::stack_resources::{element, is_iso8601_instant, sns_arn, wire_resources, PhysicalId};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    self, resource_status, PAGE_SIZE,
};
use smithy_server_runtime::MultiServiceRouter;

/// One `StackResourceSummary` as the response carries it.
///
/// Read member by member rather than tag by tag, so each summary's logical id stays
/// paired with its own physical id, status and timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WireSummary {
    pub(crate) logical_id: Option<String>,
    physical_id: Option<String>,
    pub(crate) resource_type: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) status_reason: Option<String>,
    pub(crate) last_updated: Option<String>,
    drift_status: Option<String>,
}

/// Every `StackResourceSummary` in a `ListStackResources` response, in wire order.
///
/// Shared with `rolled_back_resource_records.rs`, which drains the pages of one stack's
/// summaries the same way.
pub(crate) fn wire_summaries(xml: &str) -> Vec<WireSummary> {
    let Some(start) = xml.find("<StackResourceSummaries>") else {
        return Vec::new();
    };
    let after = &xml[start + "<StackResourceSummaries>".len()..];
    let end = after
        .find("</StackResourceSummaries>")
        .unwrap_or(after.len());
    let block = &after[..end];

    let mut summaries = Vec::new();
    let mut rest = block;
    // The first member of a list carries the serializer's `xmlns` attribute
    // (`<member xmlns="…">`), so the open tag is matched by prefix rather than literally.
    while let Some(open) = rest.find("<member") {
        let after_tag = &rest[open + "<member".len()..];
        let Some(tag_end) = after_tag.find('>') else {
            break;
        };
        let after_open = &after_tag[tag_end + 1..];
        let Some(close) = after_open.find("</member>") else {
            break;
        };
        let entry = &after_open[..close];
        summaries.push(WireSummary {
            logical_id: element(entry, "LogicalResourceId"),
            physical_id: element(entry, "PhysicalResourceId"),
            resource_type: element(entry, "ResourceType"),
            status: element(entry, "ResourceStatus"),
            status_reason: element(entry, "ResourceStatusReason"),
            last_updated: element(entry, "LastUpdatedTimestamp"),
            drift_status: element(entry, "StackResourceDriftStatus"),
        });
        rest = &after_open[close + "</member>".len()..];
    }
    summaries
}

pub(crate) async fn list_stack_resources(
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (u16, String) {
    call(router, "ListStackResources", members).await
}

/// `ListStackResources` for one stack, the common case.
async fn resources_of(router: &MultiServiceRouter, name_or_id: &str) -> (u16, String) {
    list_stack_resources(router, &[member("StackName", name_or_id)]).await
}

/// The logical ids a response reports, in wire order.
fn logical_ids(summaries: &[WireSummary]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| summary.logical_id.clone().unwrap_or_default())
        .collect()
}

/// What one declared resource must be summarised as.
#[derive(Debug, Clone)]
struct ExpectedSummary {
    logical_id: String,
    resource_type: &'static str,
    physical_id: PhysicalId,
}

/// One row of the listed-resources table: a stack, and what listing its resources must
/// report.
struct ListCase {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    /// In the order the response must carry them — by logical id, which is what makes one
    /// answer the same on every build.
    expected: Vec<ExpectedSummary>,
}

fn expect(
    logical_id: &str,
    resource_type: &'static str,
    physical_id: PhysicalId,
) -> ExpectedSummary {
    ExpectedSummary {
        logical_id: logical_id.to_string(),
        resource_type,
        physical_id,
    }
}

/// The class the rule covers. The recorded case — a template deployed through
/// `CreateStack` whose resources a client then lists — is every row.
///
/// The resource types are entries of this service's documented return-value catalogue
/// (`state::template`'s `RETURN_VALUE_TYPES`), plus `AWS::CloudFormation::WaitConditionHandle`,
/// which the catalogue does not carry and whose physical id is therefore the generated
/// physical NAME.
fn list_cases() -> Vec<ListCase> {
    vec![
        ListCase {
            // A template may declare no resources at all, which is an empty list and not
            // a refusal.
            case: "a json template with no Resources section",
            stack_name: "list-no-resources",
            template: "{}".to_string(),
            expected: vec![],
        },
        ListCase {
            case: "one resource the template names, of a type the catalogue carries",
            stack_name: "list-named-topic",
            template:
                r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"list-topic"}}}}"#
                    .to_string(),
            expected: vec![expect(
                "Topic",
                "AWS::SNS::Topic",
                PhysicalId::Exact(sns_arn("list-topic")),
            )],
        },
        ListCase {
            case: "one resource the yaml template does not name",
            stack_name: "list-unnamed",
            template: "Resources:\n  Unnamed:\n    Type: AWS::SNS::Topic\n".to_string(),
            expected: vec![expect(
                "Unnamed",
                "AWS::SNS::Topic",
                PhysicalId::Generated(sns_arn("list-unnamed-Unnamed-")),
            )],
        },
        ListCase {
            // The ORDER dimension: declared Topic, Bucket, Alpha and reported by logical
            // id. Alpha is also a type the return-value catalogue does not carry, which
            // reports its generated physical NAME as its physical resource id, while the
            // topic's documented `Ref` is its ARN rather than its name. The topic and the
            // bucket are really created in Amazon SNS and Amazon S3, which this crate's
            // harness mounts beside CloudFormation.
            case: "three resources of three types, declared out of logical-id order",
            stack_name: "list-mixed",
            template: concat!(
                "Resources:\n",
                "  Topic:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: list-mixed-topic\n",
                "  Bucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: list-mixed-bucket\n",
                "  Alpha:\n",
                "    Type: AWS::CloudFormation::WaitConditionHandle\n",
            )
            .to_string(),
            expected: vec![
                expect(
                    "Alpha",
                    "AWS::CloudFormation::WaitConditionHandle",
                    PhysicalId::Generated("list-mixed-Alpha-".to_string()),
                ),
                expect(
                    "Bucket",
                    "AWS::S3::Bucket",
                    PhysicalId::Exact("list-mixed-bucket".to_string()),
                ),
                expect(
                    "Topic",
                    "AWS::SNS::Topic",
                    PhysicalId::Exact(sns_arn("list-mixed-topic")),
                ),
            ],
        },
        ListCase {
            // A catalogue entry the issue's own fixtures do not cite, whose documented
            // `Ref` value is the resource's own name.
            case: "a named log group, a catalogue entry the recorded units do not cite",
            stack_name: "list-catalogued",
            template: r#"{"Resources":{
                 "Group":{"Type":"AWS::Logs::LogGroup","Properties":{"LogGroupName":"list-logs"}}}}"#
                .to_string(),
            expected: vec![expect(
                "Group",
                "AWS::Logs::LogGroup",
                PhysicalId::Exact("list-logs".to_string()),
            )],
        },
        ListCase {
            // The COUNT dimension inside one page: every declaration reported.
            case: "twelve unnamed resources",
            stack_name: "list-twelve",
            template: {
                let declarations: Vec<String> = (0..12)
                    .map(|index| format!(r#""R{index:02}":{{"Type":"AWS::SNS::Topic"}}"#))
                    .collect();
                format!(r#"{{"Resources":{{{}}}}}"#, declarations.join(","))
            },
            expected: (0..12)
                .map(|index| {
                    expect(
                        &format!("R{index:02}"),
                        "AWS::SNS::Topic",
                        PhysicalId::Generated(sns_arn(&format!("list-twelve-R{index:02}-"))),
                    )
                })
                .collect(),
        },
    ]
}

/// Assert one reported summary against what the row expects of it.
fn assert_summary(case: &str, expected: &ExpectedSummary, actual: &WireSummary, status: &str) {
    assert_eq!(
        actual.logical_id.as_deref(),
        Some(expected.logical_id.as_str()),
        "case {case:?}: expected the logical id {:?}, got {actual:?}",
        expected.logical_id
    );
    assert_eq!(
        actual.resource_type.as_deref(),
        Some(expected.resource_type),
        "case {case:?}: expected the declared type {:?}, got {actual:?}",
        expected.resource_type
    );
    match &expected.physical_id {
        PhysicalId::Exact(id) => assert_eq!(
            actual.physical_id.as_deref(),
            Some(id.as_str()),
            "case {case:?}: expected the physical id {id:?}, got {actual:?}"
        ),
        PhysicalId::Generated(prefix) => {
            let id = actual.physical_id.clone().unwrap_or_default();
            let unique = id.strip_prefix(prefix.as_str()).unwrap_or_else(|| {
                panic!("case {case:?}: expected a physical id starting {prefix:?}, got {id:?}")
            });
            assert!(
                unique.len() == 12 && unique.chars().all(|c| c.is_ascii_hexdigit()),
                "case {case:?}: expected a 12-character unique segment, got {unique:?}"
            );
        }
    }
    assert_eq!(
        actual.status.as_deref(),
        Some(status),
        "case {case:?}: expected the resource status {status:?}, got {actual:?}"
    );
    assert_eq!(
        actual.status_reason, None,
        "case {case:?}: nothing failed, so no summary carries a status reason, got {actual:?}"
    );
    let last_updated = actual.last_updated.clone().unwrap_or_default();
    assert!(
        is_iso8601_instant(&last_updated),
        "case {case:?}: expected the instant the status was updated, got {last_updated:?}"
    );
    assert_eq!(
        actual.drift_status.as_deref(),
        Some("NOT_CHECKED"),
        "case {case:?}: a resource never checked for drift is NOT_CHECKED, got {actual:?}"
    );
}

#[tokio::test]
async fn a_deployed_stack_lists_every_resource_its_template_declares() {
    let router = router();

    for case in list_cases() {
        let (status, body) = create_stack(&router, case.stack_name, &case.template, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
        let stack_id = xml_text(&body, "StackId")
            .unwrap_or_else(|| panic!("case {:?}: CreateStack answered no StackId", case.case));
        wait_for_the_transition().await;

        // Addressed by name and by stack id: both name the same stack, so both list the
        // same resources.
        for identifier in [case.stack_name.to_string(), stack_id.clone()] {
            let (status, body) = resources_of(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {:?}: ListStackResources({identifier}) must succeed, got {status} {body}",
                case.case
            );
            assert_eq!(
                logical_ids(&wire_summaries(&body)),
                case.expected
                    .iter()
                    .map(|expected| expected.logical_id.clone())
                    .collect::<Vec<String>>(),
                "case {:?}: expected one summary per declaration, by logical id, got {body}",
                case.case
            );
            let reported = wire_summaries(&body);
            for (expected, actual) in case.expected.iter().zip(reported.iter()) {
                assert_summary(
                    case.case,
                    expected,
                    actual,
                    resource_status::CREATE_COMPLETE,
                );
            }
            assert_eq!(
                xml_text(&body, "NextToken"),
                None,
                "case {:?}: a single-page answer carries no NextToken, got {body}",
                case.case
            );
        }
    }
}

/// `ListStackResources` and `DescribeStackResources` are two reads of ONE deployment, so
/// they agree resource for resource on the stack they both name.
#[tokio::test]
async fn the_listed_and_described_resources_of_one_stack_agree() {
    for case in list_cases() {
        // A router per row, so each row's stack owns the physical resources its template
        // names outright rather than sharing them with the row above.
        let router = router();
        let name = case.stack_name;
        let (status, body) = create_stack(&router, name, &case.template, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let (listed_status, listed) = resources_of(&router, name).await;
        let (described_status, described) =
            call(&router, "DescribeStackResources", &[member("StackName", name)]).await;
        assert_eq!(
            (listed_status, described_status),
            (200, 200),
            "case {:?}: both reads must succeed, got {listed_status} / {described_status}",
            case.case
        );

        // Compared member by member, over the members both shapes carry: the described
        // view adds the stack's own name and id, which the summary shape does not model.
        let listed_view: Vec<(String, String, String)> = wire_summaries(&listed)
            .iter()
            .map(|summary| {
                (
                    summary.logical_id.clone().unwrap_or_default(),
                    summary.physical_id.clone().unwrap_or_default(),
                    summary.status.clone().unwrap_or_default(),
                )
            })
            .collect();
        // The described view is read with the `DescribeStackResources` suite's own reader,
        // projected onto the same triple.
        let described_view: Vec<(String, String, String)> = wire_resources(&described)
            .iter()
            .map(|resource| {
                (
                    resource.logical_id.clone().unwrap_or_default(),
                    resource.physical_id.clone().unwrap_or_default(),
                    resource.status.clone().unwrap_or_default(),
                )
            })
            .collect();
        // Both readers answer an empty vector when they cannot find their list block, so
        // the row's own count is what keeps the comparison below from passing vacuously.
        assert_eq!(
            listed_view.len(),
            case.expected.len(),
            "case {:?}: expected {} resource(s) to compare, got {listed}",
            case.case,
            case.expected.len()
        );
        assert_eq!(
            listed_view, described_view,
            "case {:?}: the two reads of one deployment must agree, got {listed} / {described}",
            case.case
        );
    }
}

/// A template declaring `count` unnamed resources, named `R000`.. so the logical-id order
/// the answer is sorted by is also the numeric order.
fn template_with(count: usize) -> String {
    let declarations: Vec<String> = (0..count)
        .map(|index| format!(r#""R{index:03}":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}"#))
        .collect();
    format!(r#"{{"Resources":{{{}}}}}"#, declarations.join(","))
}

#[tokio::test]
async fn the_pages_of_a_large_stack_enumerate_every_resource_exactly_once() {
    let router = router();
    // Two full pages and a partial one, so the case spans a page boundary in both
    // directions: a page with more behind it, and the last page.
    let total = PAGE_SIZE * 2 + 1;
    let (status, body) = create_stack(&router, "list-paged", &template_with(total), &[]).await;
    assert_eq!(
        status, 200,
        "case \"a stack spanning three pages\": expected CreateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let mut token: Option<String> = None;
    let mut seen: Vec<String> = Vec::new();
    let mut page_sizes: Vec<usize> = Vec::new();
    loop {
        let mut members = vec![member("StackName", "list-paged")];
        if let Some(token) = &token {
            members.push(member("NextToken", token));
        }
        let (status, body) = list_stack_resources(&router, &members).await;
        assert_eq!(
            status, 200,
            "case \"a stack spanning three pages\": page {} must succeed, got {status} {body}",
            page_sizes.len() + 1
        );
        let page = logical_ids(&wire_summaries(&body));
        assert!(
            page.len() <= PAGE_SIZE,
            "case \"a stack spanning three pages\": a page carries at most {PAGE_SIZE} items, got {}",
            page.len()
        );
        page_sizes.push(page.len());
        seen.extend(page);
        token = xml_text(&body, "NextToken");
        if token.is_none() {
            break;
        }
        assert!(
            page_sizes.len() <= 4,
            "case \"a stack spanning three pages\": paging must terminate, got pages {page_sizes:?}"
        );
    }

    assert_eq!(
        page_sizes,
        vec![PAGE_SIZE, PAGE_SIZE, 1],
        "case \"a stack spanning three pages\": expected full pages then the remainder, got \
         {page_sizes:?}"
    );
    let expected: Vec<String> = (0..total).map(|index| format!("R{index:03}")).collect();
    assert_eq!(
        seen, expected,
        "case \"a stack spanning three pages\": the pages enumerate every resource once, in \
         logical-id order"
    );
    let distinct: BTreeSet<&String> = seen.iter().collect();
    assert_eq!(
        distinct.len(),
        total,
        "case \"a stack spanning three pages\": no resource appears on two pages, got {} of {total}",
        distinct.len()
    );
}

#[tokio::test]
async fn a_next_token_this_operation_never_issued_is_refused() {
    let router = router();
    let (_, created) = create_stack(&router, "list-token", &template_with(3), &[]).await;
    let stack_id = xml_text(&created, "StackId").expect("CreateStack answers a StackId");
    wait_for_the_transition().await;

    // A cursor past the end of the stack's resources is not one this operation handed out
    // for it, and neither is one issued for a different collection, nor an opaque string
    // this service never minted.
    let cases: Vec<(&str, String)> = vec![
        (
            "a cursor past the end",
            state::encode_next_token(state::PageOf::StackResourceSummaries, 4),
        ),
        (
            "a cursor issued for another collection",
            state::encode_next_token(state::PageOf::StackSummaries, 0),
        ),
        (
            "a string this service never issued",
            "not-a-token-we-issued".to_string(),
        ),
    ];

    for (case, token) in cases {
        let (status, body) = list_stack_resources(
            &router,
            &[member("StackName", "list-token"), member("NextToken", &token)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }

    // The cursor at the start of THIS stack's resources IS accepted, so the refusals above
    // are about the token and not about the operation: the token this read hands out is
    // scoped to the stack whose resources it pages.
    let first = state::encode_scoped_next_token(
        state::PageOf::StackResourceSummaries,
        state::PageScope(&stack_id),
        0,
    );
    let (status, body) = list_stack_resources(
        &router,
        &[member("StackName", "list-token"), member("NextToken", &first)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a cursor at the start of this collection\": accepted, got {status} {body}"
    );
    assert_eq!(
        logical_ids(&wire_summaries(&body)),
        vec!["R000".to_string(), "R001".to_string(), "R002".to_string()],
        "case \"a cursor at the start of this collection\": the whole stack, got {body}"
    );
}

#[tokio::test]
async fn a_stack_name_that_names_no_stack_is_refused() {
    let router = router();
    create_stack(&router, "list-live", &template_with(2), &[]).await;
    wait_for_the_transition().await;

    // (case, the StackName asked for)
    let cases: [(&str, &str); 2] = [
        ("a name no stack has", "list-absent"),
        (
            "a name differing only in case from a live stack",
            "LIST-LIVE",
        ),
    ];
    for (case, name) in cases {
        let (status, body) = resources_of(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: an unaddressable stack is refused, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected ValidationError, got {body}"
        );
    }
}

/// "For deleted stacks, ListStackResources returns resource information for up to 90 days
/// after the stack has been deleted" — and a deleted stack is addressable only by its
/// unique stack id, so its NAME stops naming it.
#[tokio::test]
async fn a_deleted_stack_lists_its_resources_by_stack_id() {
    let router = router();
    let (status, body) = create_stack(&router, "list-deleted", &template_with(2), &[]).await;
    assert_eq!(
        status, 200,
        "case \"a deleted stack\": expected CreateStack to succeed, got {status} {body}"
    );
    let stack_id = xml_text(&body, "StackId").expect("CreateStack answers a StackId");
    wait_for_the_transition().await;

    let (status, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "list-deleted")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a deleted stack\": expected DeleteStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (status, body) = resources_of(&router, &stack_id).await;
    assert_eq!(
        status, 200,
        "case \"a deleted stack by stack id\": still addressable, got {status} {body}"
    );
    let summaries = wire_summaries(&body);
    assert_eq!(
        logical_ids(&summaries),
        vec!["R000".to_string(), "R001".to_string()],
        "case \"a deleted stack by stack id\": its resources are still reported, got {body}"
    );
    for summary in &summaries {
        assert_eq!(
            summary.status.as_deref(),
            Some(resource_status::DELETE_COMPLETE),
            "case \"a deleted stack by stack id\": the deletion completed against every \
             resource, got {summary:?}"
        );
    }

    let (status, body) = resources_of(&router, "list-deleted").await;
    assert_eq!(
        status, 400,
        "case \"a deleted stack by name\": a deleted stack is named only by its id, got \
         {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case \"a deleted stack by name\": expected ValidationError, got {body}"
    );
}

/// The status a summary carries is the one the stack's last COMPLETED operation left that
/// resource in, which is what makes an update observable through this read.
#[tokio::test]
async fn an_update_moves_the_status_of_the_resources_it_touched() {
    let router = router();
    let before = concat!(
        "Resources:\n",
        "  Keep:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: list-keep\n",
        "  Change:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: list-change-before\n",
    );
    let after = concat!(
        "Resources:\n",
        "  Keep:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: list-keep\n",
        "  Change:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: list-change-after\n",
        "  Added:\n",
        "    Type: AWS::Logs::LogGroup\n",
        "    Properties:\n",
        "      LogGroupName: list-added\n",
    );

    create_stack(&router, "list-updated", before, &[]).await;
    wait_for_the_transition().await;
    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "list-updated"),
            member("TemplateBody", after),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"an updated stack\": expected UpdateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (status, body) = resources_of(&router, "list-updated").await;
    assert_eq!(
        status, 200,
        "case \"an updated stack\": the read must succeed, got {status} {body}"
    );
    let summaries = wire_summaries(&body);
    assert_eq!(
        logical_ids(&summaries),
        vec![
            "Added".to_string(),
            "Change".to_string(),
            "Keep".to_string()
        ],
        "case \"an updated stack\": the update's template is what the stack lists, got {body}"
    );

    // (case, logical id, the status that resource must carry)
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a resource the update left exactly as it was",
            "Keep",
            resource_status::CREATE_COMPLETE,
        ),
        (
            "a resource the update changed",
            "Change",
            resource_status::UPDATE_COMPLETE,
        ),
        (
            "a resource the update added",
            "Added",
            resource_status::CREATE_COMPLETE,
        ),
    ];
    for (case, logical_id, status) in rows {
        let summary = summaries
            .iter()
            .find(|summary| summary.logical_id.as_deref() == Some(logical_id))
            .unwrap_or_else(|| panic!("case {case:?}: no summary for {logical_id:?}: {body}"));
        assert_eq!(
            summary.status.as_deref(),
            Some(status),
            "case {case:?}: expected {status}, got {summary:?}"
        );
    }
}
