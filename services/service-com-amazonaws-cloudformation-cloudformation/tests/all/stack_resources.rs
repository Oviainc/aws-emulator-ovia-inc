// SPDX-License-Identifier: CC0-1.0

//! `DescribeStackResources`, end to end over the awsQuery wire.
//!
//! The rule under test: `DescribeStackResources` reports one `StackResource` per resource
//! the selected stack's last COMPLETED deployment materialised — carrying the logical id
//! and type the template declared, the physical id CloudFormation's documented naming
//! rules gave it, the stack's own name and id, and the resource-level status that
//! operation left the resource in with the instant it reached that status. The stack is
//! selected by `StackName` or by the `PhysicalResourceId` of one of its resources, never
//! by both and never by neither, and `LogicalResourceId` filters the result.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out)
//! through the wire harness `stack_lifecycle.rs` builds, so what it asserts is what a
//! client calling `DescribeStackResources` sees. The table spans the class the rule
//! generalises over: both template spellings, a template that declares no resources at
//! all, resource types the return-value catalogue carries and one it does not, named and
//! generated physical ids, the declaration order of the resources and their count.

use std::time::Instant;

use crate::stack_lifecycle::{
    call, create_stack, member, router, wait_for_the_transition, xml_text, xml_unescape, ACCOUNT_ID,
    REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    resource_status, MAX_DESCRIBED_STACK_RESOURCES, STATUS_TRANSITION_DELAY,
    TEMPLATE_BODY_MAX_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

/// One `StackResource` as the response carries it.
///
/// Read member by member rather than tag by tag, so each resource's logical id stays
/// paired with its own physical id, status and timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WireResource {
    stack_name: Option<String>,
    stack_id: Option<String>,
    pub(crate) logical_id: Option<String>,
    pub(crate) physical_id: Option<String>,
    pub(crate) resource_type: Option<String>,
    pub(crate) status: Option<String>,
    status_reason: Option<String>,
    timestamp: Option<String>,
    drift_status: Option<String>,
}

/// Every `StackResource` in a `DescribeStackResources` response, in wire order.
///
/// Shared with `ecs_elbv2_stack_identifiers.rs`, which reads the physical id a stack
/// reports for a resource out of the same response.
pub(crate) fn wire_resources(xml: &str) -> Vec<WireResource> {
    let Some(start) = xml.find("<StackResources>") else {
        return Vec::new();
    };
    let after = &xml[start + "<StackResources>".len()..];
    let end = after.find("</StackResources>").unwrap_or(after.len());
    let block = &after[..end];

    let mut resources = Vec::new();
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
        resources.push(WireResource {
            stack_name: element(entry, "StackName"),
            stack_id: element(entry, "StackId"),
            logical_id: element(entry, "LogicalResourceId"),
            physical_id: element(entry, "PhysicalResourceId"),
            resource_type: element(entry, "ResourceType"),
            status: element(entry, "ResourceStatus"),
            status_reason: element(entry, "ResourceStatusReason"),
            timestamp: element(entry, "Timestamp"),
            drift_status: element(entry, "StackResourceDriftStatus"),
        });
        rest = &after_open[close + "</member>".len()..];
    }
    resources
}

/// One element's text inside a single reported resource.
///
/// Shared with `stack_resource_detail.rs`, which reads a `StackResourceDetail` block the
/// same way.
pub(crate) fn element(entry: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = entry.find(&open)?;
    let after = &entry[start + open.len()..];
    let end = after.find(&close)?;
    Some(xml_unescape(&after[..end]))
}

async fn describe_stack_resources(
    router: &MultiServiceRouter,
    members: &[(String, String)],
) -> (u16, String) {
    call(router, "DescribeStackResources", members).await
}

/// `DescribeStackResources` for one stack, the common case.
pub(crate) async fn resources_of(router: &MultiServiceRouter, name_or_id: &str) -> (u16, String) {
    describe_stack_resources(router, &[member("StackName", name_or_id)]).await
}

/// The logical ids a response reports, in wire order.
fn logical_ids(resources: &[WireResource]) -> Vec<String> {
    resources
        .iter()
        .map(|resource| resource.logical_id.clone().unwrap_or_default())
        .collect()
}

pub(crate) fn sns_arn(name: &str) -> String {
    format!("arn:aws:sns:{REGION}:{ACCOUNT_ID}:{name}")
}


/// Whether `value` is the ISO-8601 instant the query protocol renders a timestamp as
/// (`2026-01-01T00:00:00Z`, with optional fractional seconds).
pub(crate) fn is_iso8601_instant(value: &str) -> bool {
    let bytes = value.as_bytes();
    value.len() >= 20
        && value.ends_with('Z')
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | ':' | '.' | 'T' | 'Z'))
}

/// What the physical resource id of one reported resource must be.
#[derive(Debug, Clone)]
pub(crate) enum PhysicalId {
    /// Exactly this id: the template NAMED the resource, so the documented `Ref` value of
    /// its type is built from that name.
    Exact(String),
    /// The id of a resource the template does not name, built from CloudFormation's
    /// generated `<stack name>-<logical id>-<unique id>` name — so it is this prefix plus
    /// a 12-character unique segment.
    Generated(String),
}

/// What one declared resource must be reported as.
#[derive(Debug, Clone)]
struct ExpectedResource {
    logical_id: String,
    resource_type: &'static str,
    physical_id: PhysicalId,
}

/// One row of the deployed-resources table: a stack, and what reading its resources back
/// must report.
struct ResourceCase {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    /// In the order the response must carry them — by logical id, which is what makes one
    /// answer the same on every build.
    expected: Vec<ExpectedResource>,
}

fn expect(
    logical_id: &str,
    resource_type: &'static str,
    physical_id: PhysicalId,
) -> ExpectedResource {
    ExpectedResource {
        logical_id: logical_id.to_string(),
        resource_type,
        physical_id,
    }
}

/// The class the rule covers. The issue's own case — a template deployed through
/// `CreateStack` whose resources a client then reads back — is every row.
fn resource_cases() -> Vec<ResourceCase> {
    vec![
        ResourceCase {
            // A template may declare no resources at all, which is an empty list and not
            // a refusal.
            case: "a json template with no Resources section",
            stack_name: "no-resources",
            template: "{}".to_string(),
            expected: vec![],
        },
        ResourceCase {
            case: "one resource the template names, of a type the catalogue carries",
            stack_name: "named-topic",
            template:
                r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"probe-topic"}}}}"#
                    .to_string(),
            expected: vec![expect(
                "Topic",
                "AWS::SNS::Topic",
                PhysicalId::Exact(sns_arn("probe-topic")),
            )],
        },
        ResourceCase {
            case: "one resource the yaml template does not name",
            stack_name: "unnamed",
            template: "Resources:\n  Unnamed:\n    Type: AWS::SNS::Topic\n".to_string(),
            expected: vec![expect(
                "Unnamed",
                "AWS::SNS::Topic",
                PhysicalId::Generated(sns_arn("unnamed-Unnamed-")),
            )],
        },
        ResourceCase {
            // The ORDER dimension: declared Topic, Bucket, Alpha and reported by logical
            // id. Alpha is also a type the return-value catalogue does not carry, which
            // reports its physical NAME as its physical resource id, while the topic's
            // documented `Ref` is its ARN rather than its name. The topic is really
            // created in Amazon SNS, which this crate's harness mounts beside
            // CloudFormation; what the stack and SNS must agree about is
            // `core/router/tests/all/cloudformation_provisions_sns_topics.rs`.
            case: "three resources of three types, declared out of logical-id order",
            stack_name: "mixed",
            template: concat!(
                "Resources:\n",
                "  Topic:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: probe-topic\n",
                "  Bucket:\n",
                "    Type: AWS::S3::Bucket\n",
                "    Properties:\n",
                "      BucketName: probe-bucket\n",
                "  Alpha:\n",
                "    Type: AWS::CloudFormation::WaitConditionHandle\n",
            )
            .to_string(),
            expected: vec![
                expect(
                    "Alpha",
                    "AWS::CloudFormation::WaitConditionHandle",
                    PhysicalId::Generated("mixed-Alpha-".to_string()),
                ),
                expect(
                    "Bucket",
                    "AWS::S3::Bucket",
                    PhysicalId::Exact("probe-bucket".to_string()),
                ),
                expect(
                    "Topic",
                    "AWS::SNS::Topic",
                    PhysicalId::Exact(sns_arn("probe-topic")),
                ),
            ],
        },
        ResourceCase {
            // Two more types whose documented `Ref` value is the resource's own name.
            case: "two named resources whose ids are their names",
            stack_name: "catalogued",
            template: r#"{"Resources":{
                 "Table":{"Type":"AWS::DynamoDB::Table","Properties":{
                   "TableName":"probe-table","BillingMode":"PAY_PER_REQUEST",
                   "KeySchema":[{"AttributeName":"pk","KeyType":"HASH"}],
                   "AttributeDefinitions":[{"AttributeName":"pk","AttributeType":"S"}]}},
                 "Group":{"Type":"AWS::Logs::LogGroup","Properties":{"LogGroupName":"probe-logs"}}}}"#
                .to_string(),
            expected: vec![
                expect(
                    "Group",
                    "AWS::Logs::LogGroup",
                    PhysicalId::Exact("probe-logs".to_string()),
                ),
                expect(
                    "Table",
                    "AWS::DynamoDB::Table",
                    PhysicalId::Exact("probe-table".to_string()),
                ),
            ],
        },
        ResourceCase {
            // The COUNT dimension: ten declared resources, every one reported.
            case: "ten unnamed resources",
            stack_name: "ten",
            template: {
                let declarations: Vec<String> = (0..10)
                    .map(|index| {
                        format!(r#""R{index:02}":{{"Type":"AWS::SNS::Topic"}}"#)
                    })
                    .collect();
                format!(r#"{{"Resources":{{{}}}}}"#, declarations.join(","))
            },
            expected: (0..10)
                .map(|index| {
                    expect(
                        &format!("R{index:02}"),
                        "AWS::SNS::Topic",
                        PhysicalId::Generated(sns_arn(&format!("ten-R{index:02}-"))),
                    )
                })
                .collect(),
        },
    ]
}

/// Assert a reported physical resource id against what the row expects of it: the exact
/// id a NAMED resource takes, or the prefix plus unique segment a generated name takes.
///
/// Shared with `stack_resource_detail.rs`: `DescribeStackResource` reports the same
/// physical id for the same resource, so both suites hold it to the same rule.
pub(crate) fn assert_physical_id(case: &str, expected: &PhysicalId, actual: Option<&str>) {
    match expected {
        PhysicalId::Exact(id) => assert_eq!(
            actual,
            Some(id.as_str()),
            "case {case:?}: expected the physical id {id:?}, got {actual:?}"
        ),
        PhysicalId::Generated(prefix) => {
            let id = actual.unwrap_or_default().to_string();
            let unique = id.strip_prefix(prefix.as_str()).unwrap_or_else(|| {
                panic!("case {case:?}: expected a physical id starting {prefix:?}, got {id:?}")
            });
            assert!(
                unique.len() == 12 && unique.chars().all(|c| c.is_ascii_hexdigit()),
                "case {case:?}: expected a 12-character unique segment, got {unique:?}"
            );
        }
    }
}

/// Assert one reported resource against what the row expects of it.
fn assert_resource(
    case: &str,
    expected: &ExpectedResource,
    actual: &WireResource,
    stack_name: &str,
    stack_id: &str,
    status: &str,
) {
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
    assert_physical_id(case, &expected.physical_id, actual.physical_id.as_deref());
    assert_eq!(
        actual.stack_name.as_deref(),
        Some(stack_name),
        "case {case:?}: a resource must name the stack it belongs to, got {actual:?}"
    );
    assert_eq!(
        actual.stack_id.as_deref(),
        Some(stack_id),
        "case {case:?}: a resource must carry its stack's id, got {actual:?}"
    );
    assert_eq!(
        actual.status.as_deref(),
        Some(status),
        "case {case:?}: expected the resource status {status:?}, got {actual:?}"
    );
    assert_eq!(
        actual.status_reason, None,
        "case {case:?}: nothing failed, so no resource carries a status reason, got {actual:?}"
    );
    let timestamp = actual.timestamp.clone().unwrap_or_default();
    assert!(
        is_iso8601_instant(&timestamp),
        "case {case:?}: expected the instant the status was set, got {timestamp:?}"
    );
    assert_eq!(
        actual.drift_status.as_deref(),
        Some("NOT_CHECKED"),
        "case {case:?}: a resource never checked for drift is NOT_CHECKED, got {actual:?}"
    );
}

#[tokio::test]
async fn a_deployed_stack_reports_every_resource_its_template_declares() {
    let router = router();

    for case in resource_cases() {
        let (status, body) = create_stack(&router, case.stack_name, &case.template, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to succeed, got {status} {body}",
            case.case
        );
        let stack_id = xml_text(&body, "StackId")
            .unwrap_or_else(|| panic!("case {:?}: CreateStack answered no StackId", case.case));
        wait_for_the_transition().await;

        // Addressed by name and by stack id: both name the same stack, so both report the
        // same resources.
        for identifier in [case.stack_name.to_string(), stack_id.clone()] {
            let (status, body) = resources_of(&router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {:?}: DescribeStackResources({identifier}) must succeed, got {status} {body}",
                case.case
            );
            let reported = wire_resources(&body);
            assert_eq!(
                logical_ids(&reported),
                case.expected
                    .iter()
                    .map(|expected| expected.logical_id.clone())
                    .collect::<Vec<String>>(),
                "case {:?}: expected one resource per declaration, by logical id, got {body}",
                case.case
            );
            for (expected, actual) in case.expected.iter().zip(reported.iter()) {
                assert_resource(
                    case.case,
                    expected,
                    actual,
                    case.stack_name,
                    &stack_id,
                    resource_status::CREATE_COMPLETE,
                );
            }
        }
    }
}

/// `LogicalResourceId` filters the stack's resources: it is a filter, so one that matches
/// nothing leaves an empty list rather than refusing the request.
#[tokio::test]
async fn logical_resource_id_filters_the_reported_resources() {
    let router = router();
    let template = concat!(
        "Resources:\n",
        "  Alpha:\n",
        "    Type: AWS::SNS::Topic\n",
        "  Beta:\n",
        "    Type: AWS::S3::Bucket\n",
        "    Properties:\n",
        "      BucketName: filter-bucket\n",
        "  Gamma:\n",
        "    Type: AWS::CloudFormation::WaitConditionHandle\n",
    );
    create_stack(&router, "filtered", template, &[]).await;
    wait_for_the_transition().await;

    // (case, the LogicalResourceId asked for, the logical ids the answer must carry)
    let cases: Vec<(&str, Option<&str>, Vec<&str>)> = vec![
        ("no filter", None, vec!["Alpha", "Beta", "Gamma"]),
        ("the first resource", Some("Alpha"), vec!["Alpha"]),
        ("a middle resource", Some("Beta"), vec!["Beta"]),
        ("the last resource", Some("Gamma"), vec!["Gamma"]),
        ("a logical id the template never declared", Some("Delta"), vec![]),
        ("a logical id differing only in case", Some("alpha"), vec![]),
    ];

    for (case, logical_id, expected) in cases {
        let mut members = vec![member("StackName", "filtered")];
        if let Some(logical_id) = logical_id {
            members.push(member("LogicalResourceId", logical_id));
        }
        let (status, body) = describe_stack_resources(&router, &members).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a filter is not a refusal, got {status} {body}"
        );
        assert_eq!(
            logical_ids(&wire_resources(&body)),
            expected
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected exactly {expected:?}, got {body}"
        );
    }
}

/// A `PhysicalResourceId` names the stack the resource belongs to, and that stack's whole
/// resource list is what comes back — "You can pass the EC2 InstanceId to
/// DescribeStackResources to find which stack the instance belongs to and what other
/// resources are part of the stack."
#[tokio::test]
async fn a_physical_resource_id_resolves_the_stack_it_belongs_to() {
    let router = router();
    let first = concat!(
        "Resources:\n",
        "  Topic:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: owner-topic\n",
        "  Handle:\n",
        "    Type: AWS::CloudFormation::WaitConditionHandle\n",
    );
    let second = r#"{"Resources":{"Other":{"Type":"AWS::S3::Bucket","Properties":{"BucketName":"other-bucket"}}}}"#;
    let (_, created) = create_stack(&router, "owner", first, &[]).await;
    let owner_id = xml_text(&created, "StackId").expect("a stack id");
    create_stack(&router, "other", second, &[]).await;
    wait_for_the_transition().await;

    // The physical id of each of the owner stack's resources, as the stack itself reports
    // them: a generated one and a named one, so the lookup is not keyed to either form.
    let (_, body) = resources_of(&router, "owner").await;
    let owned = wire_resources(&body);
    assert_eq!(
        logical_ids(&owned),
        vec!["Handle".to_string(), "Topic".to_string()],
        "the owner stack must report both its resources, got {body}"
    );

    for resource in &owned {
        let physical_id = resource.physical_id.clone().expect("a physical id");
        let case = format!("the physical id of {:?}", resource.logical_id);
        let (status, body) =
            describe_stack_resources(&router, &[member("PhysicalResourceId", &physical_id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the owning stack's resources, got {status} {body}"
        );
        let reported = wire_resources(&body);
        assert_eq!(
            logical_ids(&reported),
            vec!["Handle".to_string(), "Topic".to_string()],
            "case {case:?}: a physical id must answer every resource of ITS stack, got {body}"
        );
        assert!(
            reported
                .iter()
                .all(|resource| resource.stack_id.as_deref() == Some(owner_id.as_str())),
            "case {case:?}: every resource must belong to the owner stack, got {body}"
        );

        // Combined with the filter, which narrows the same stack's list.
        let (status, body) = describe_stack_resources(
            &router,
            &[
                member("PhysicalResourceId", &physical_id),
                member("LogicalResourceId", "Topic"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a filtered lookup must succeed, got {status} {body}"
        );
        assert_eq!(
            logical_ids(&wire_resources(&body)),
            vec!["Topic".to_string()],
            "case {case:?}: LogicalResourceId must narrow the stack's list, got {body}"
        );
    }

    // A physical id no stack has deployed names no stack, which is a refusal and not an
    // empty list.
    for (case, physical_id) in [
        ("an id nothing deployed", "i-00000000000000000"),
        ("the owner stack's own id", owner_id.as_str()),
        ("a resource name of another stack's resource, misspelled", "other-bucket-x"),
    ] {
        let (status, body) =
            describe_stack_resources(&router, &[member("PhysicalResourceId", physical_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(format!("Stack for {physical_id} does not exist").as_str()),
            "case {case:?}: the refusal must name what it could not find, got {body}"
        );
    }
}

/// "You must specify either StackName or PhysicalResourceId, but not both": each spelling
/// outside that rule is refused with CloudFormation's own `ValidationError`, and a
/// `StackName` that names no stack is refused the way every other read of a stack refuses
/// it.
#[tokio::test]
async fn selecting_no_stack_or_two_ways_at_once_is_refused() {
    let router = router();
    let (_, created) = create_stack(
        &router,
        "selectable",
        r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"selectable-topic"}}}}"#,
        &[],
    )
    .await;
    let stack_id = xml_text(&created, "StackId").expect("a stack id");
    wait_for_the_transition().await;
    let physical_id = sns_arn("selectable-topic");

    /// One request outside the either-or rule, and the member names its refusal must
    /// name so the caller can fix the request from the message alone.
    struct RefusalCase {
        case: &'static str,
        members: Vec<(String, String)>,
        names: Vec<&'static str>,
    }
    let both = vec!["StackName", "PhysicalResourceId"];
    let cases = vec![
        RefusalCase {
            case: "neither selector",
            members: vec![],
            names: both.clone(),
        },
        RefusalCase {
            case: "only a LogicalResourceId, which selects no stack",
            members: vec![member("LogicalResourceId", "Topic")],
            names: both.clone(),
        },
        RefusalCase {
            case: "both selectors, by stack name",
            members: vec![
                member("StackName", "selectable"),
                member("PhysicalResourceId", &physical_id),
            ],
            names: both.clone(),
        },
        RefusalCase {
            case: "both selectors, by stack id",
            members: vec![
                member("StackName", &stack_id),
                member("PhysicalResourceId", &physical_id),
            ],
            names: both.clone(),
        },
    ];

    for RefusalCase {
        case,
        members,
        names,
    } in cases
    {
        let (status, body) = describe_stack_resources(&router, &members).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {body}"
        );
        let message = xml_text(&body, "Message").unwrap_or_default();
        for name in names {
            assert!(
                message.contains(name),
                "case {case:?}: the refusal must name {name:?}, got {message:?}"
            );
        }
    }

    // A stack name that resolves to nothing: the same refusal DescribeStacks answers.
    for (case, identifier) in [
        ("a name never created", "no-such-stack".to_string()),
        (
            "a stack id no stack carries",
            format!("arn:aws:cloudformation:{REGION}:{ACCOUNT_ID}:stack/ghost/00000000-0000-0000-0000-000000000000"),
        ),
    ] {
        let (status, body) = resources_of(&router, &identifier).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} {body}"
        );
        assert_eq!(
            xml_text(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {case:?}: expected a ValidationError, got {body}"
        );
        assert_eq!(
            xml_text(&body, "Message").as_deref(),
            Some(format!("Stack with id {identifier} does not exist").as_str()),
            "case {case:?}: the refusal must name the stack it could not find, got {body}"
        );
    }
}

/// A resource reaches a status only when an operation COMPLETES against it: an update
/// reports the resources it created and the ones it changed as of its own completion, and
/// leaves the status and timestamp of a resource it did not touch exactly where they were.
#[tokio::test]
async fn an_update_reports_the_status_each_resource_reached() {
    let router = router();
    let before = r#"{"Resources":{
         "Keep":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
         "Change":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"before-topic"}}}}"#;
    let after = r#"{"Resources":{
         "Keep":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
         "Change":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"after-topic"}},
         "Added":{"Type":"AWS::S3::Bucket","Properties":{"BucketName":"added-bucket"}}}}"#;

    create_stack(&router, "updated", before, &[]).await;
    wait_for_the_transition().await;
    let (_, body) = resources_of(&router, "updated").await;
    let created = wire_resources(&body);
    let read = |resources: &[WireResource], logical_id: &str| -> WireResource {
        resources
            .iter()
            .find(|resource| resource.logical_id.as_deref() == Some(logical_id))
            .cloned()
            .unwrap_or_else(|| panic!("expected a resource {logical_id:?}, got {resources:?}"))
    };
    let keep_before = read(&created, "Keep");
    assert_eq!(
        keep_before.status.as_deref(),
        Some(resource_status::CREATE_COMPLETE),
        "the create must leave its resources CREATE_COMPLETE, got {body}"
    );

    let (status, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "updated"),
            member("TemplateBody", after),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected UpdateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (_, body) = resources_of(&router, "updated").await;
    let updated = wire_resources(&body);
    assert_eq!(
        logical_ids(&updated),
        vec![
            "Added".to_string(),
            "Change".to_string(),
            "Keep".to_string()
        ],
        "the update's template is what the stack reports resources for, got {body}"
    );

    // (case, logical id, the status the resource must carry, whether the update moved its
    // timestamp)
    let rows: Vec<(&str, &str, &str, bool)> = vec![
        (
            "a resource the update left exactly as it was",
            "Keep",
            resource_status::CREATE_COMPLETE,
            false,
        ),
        (
            "a resource the update changed",
            "Change",
            resource_status::UPDATE_COMPLETE,
            true,
        ),
        (
            "a resource the update added",
            "Added",
            resource_status::CREATE_COMPLETE,
            true,
        ),
    ];
    for (case, logical_id, status, moved) in rows {
        let resource = read(&updated, logical_id);
        assert_eq!(
            resource.status.as_deref(),
            Some(status),
            "case {case:?}: expected {status}, got {resource:?}"
        );
        let timestamp = resource.timestamp.clone().unwrap_or_default();
        assert!(
            is_iso8601_instant(&timestamp),
            "case {case:?}: expected the instant the status was set, got {timestamp:?}"
        );
        let unchanged_timestamp = keep_before.timestamp.clone().unwrap_or_default();
        if moved {
            assert_ne!(
                timestamp, unchanged_timestamp,
                "case {case:?}: a resource this operation touched reports its completion"
            );
        } else {
            assert_eq!(
                timestamp, unchanged_timestamp,
                "case {case:?}: an untouched resource's status was not updated, so its \
                 timestamp must not move"
            );
        }
    }

    // The changed resource's physical id follows the name the new template gives it, while
    // the untouched one keeps the id it already had.
    assert_eq!(
        read(&updated, "Change").physical_id,
        Some(sns_arn("after-topic")),
        "the update's template is what names the resource, got {body}"
    );
    assert_eq!(
        read(&updated, "Keep").physical_id,
        keep_before.physical_id,
        "a resource the update did not replace keeps its physical id, got {body}"
    );
}

/// "For deleted stacks, DescribeStackResources returns resource information for up to 90
/// days after the stack has been deleted": a completed deletion leaves the resources on
/// the record, in `DELETE_COMPLETE`, addressable by the stack's id.
#[tokio::test]
async fn a_deleted_stacks_resources_are_reported_as_delete_complete() {
    let router = router();
    let template = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"doomed-topic"}}}}"#;
    let (_, created) = create_stack(&router, "doomed", template, &[]).await;
    let stack_id = xml_text(&created, "StackId").expect("a stack id");
    wait_for_the_transition().await;
    let (_, body) = resources_of(&router, "doomed").await;
    let before = wire_resources(&body);
    let created_timestamp = before[0].timestamp.clone().expect("a timestamp");

    let (status, body) = call(&router, "DeleteStack", &[member("StackName", "doomed")]).await;
    assert_eq!(
        status, 200,
        "expected DeleteStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (status, body) = resources_of(&router, &stack_id).await;
    assert_eq!(
        status, 200,
        "case \"by stack id\": a deleted stack stays addressable, got {status} {body}"
    );
    let reported = wire_resources(&body);
    assert_eq!(
        logical_ids(&reported),
        vec!["Topic".to_string()],
        "case \"by stack id\": the deleted stack's resources are still reported, got {body}"
    );
    assert_eq!(
        reported[0].status.as_deref(),
        Some(resource_status::DELETE_COMPLETE),
        "case \"by stack id\": the deletion completed against the resource, got {body}"
    );
    assert_ne!(
        reported[0].timestamp.clone().unwrap_or_default(),
        created_timestamp,
        "case \"by stack id\": the deletion updated the resource's status, so its \
         timestamp moved"
    );

    // The NAME of a deleted stack is not addressable, here as everywhere else.
    let (status, body) = resources_of(&router, "doomed").await;
    assert_eq!(
        status, 400,
        "case \"by name\": a deleted stack's name names nothing, got {status} {body}"
    );
    assert_eq!(
        xml_text(&body, "Code").as_deref(),
        Some("ValidationError"),
        "case \"by name\": expected a ValidationError, got {body}"
    );
}

/// "Only the first 100 resources will be returned. If your stack has more resources than
/// this, you should use ListStackResources instead."
#[tokio::test]
async fn only_the_documented_cap_of_resources_is_reported() {
    let router = router();
    let declared = MAX_DESCRIBED_STACK_RESOURCES + 20;
    let declarations: Vec<String> = (0..declared)
        .map(|index| format!(r#""R{index:03}":{{"Type":"AWS::CloudFormation::WaitConditionHandle"}}"#))
        .collect();
    let template = format!(r#"{{"Resources":{{{}}}}}"#, declarations.join(","));
    assert!(
        template.len() < TEMPLATE_BODY_MAX_BYTES,
        "the fixture must be a body a caller may send, got {} bytes",
        template.len()
    );

    let (status, body) = create_stack(&router, "capped", &template, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (status, body) = resources_of(&router, "capped").await;
    assert_eq!(
        status, 200,
        "expected DescribeStackResources to succeed, got {status} {body}"
    );
    let reported = wire_resources(&body);
    assert_eq!(
        reported.len(),
        MAX_DESCRIBED_STACK_RESOURCES,
        "a stack declaring {declared} resources reports only the documented cap, got {}",
        reported.len()
    );
    assert_eq!(
        logical_ids(&reported),
        (0..MAX_DESCRIBED_STACK_RESOURCES)
            .map(|index| format!("R{index:03}"))
            .collect::<Vec<String>>(),
        "the cap keeps the FIRST resources of a deterministic order, got {body}"
    );

    // The filter reaches a resource past the cap: it narrows the stack's resources before
    // the cap applies.
    let past_the_cap = format!("R{:03}", declared - 1);
    let (status, body) = describe_stack_resources(
        &router,
        &[
            member("StackName", "capped"),
            member("LogicalResourceId", &past_the_cap),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the filtered read to succeed, got {status} {body}"
    );
    assert_eq!(
        logical_ids(&wire_resources(&body)),
        vec![past_the_cap.clone()],
        "case {past_the_cap:?}: a filter reaches a resource past the cap, got {body}"
    );
}

/// A stack reports the resources of the deployment that FINISHED: a create still in
/// progress has materialised nothing yet.
#[tokio::test]
async fn a_create_still_in_progress_reports_no_resources_yet() {
    let router = router();
    let template = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"pending-topic"}}}}"#;
    let started = Instant::now();
    let (status, body) = create_stack(&router, "pending", template, &[]).await;
    assert_eq!(
        status, 200,
        "expected CreateStack to succeed, got {status} {body}"
    );

    let (status, body) = resources_of(&router, "pending").await;
    assert_eq!(
        status, 200,
        "expected DescribeStackResources to succeed, got {status} {body}"
    );
    // The guard is what keeps the case honest on a loaded machine: the emulator owes the
    // in-progress answer only until the create's terminal status is due.
    if started.elapsed() < STATUS_TRANSITION_DELAY {
        assert!(
            wire_resources(&body).is_empty(),
            "case \"while the create is in progress\": nothing has been deployed yet, got {body}"
        );
    }

    wait_for_the_transition().await;
    let (_, body) = resources_of(&router, "pending").await;
    assert_eq!(
        logical_ids(&wire_resources(&body)),
        vec!["Topic".to_string()],
        "case \"once the create has completed\": the deployed resource is reported, got {body}"
    );
}
