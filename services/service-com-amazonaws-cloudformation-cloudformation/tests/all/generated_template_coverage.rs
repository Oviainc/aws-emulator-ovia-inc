// SPDX-License-Identifier: CC0-1.0

//! What a generation COVERS, end to end over the awsQuery wire: the resource rows
//! `CreateGeneratedTemplate` persists for one record, as `DescribeGeneratedTemplate` reports
//! them, and the identifier the create mints for that record.
//!
//! RULE: an accepted `CreateGeneratedTemplate` persists ONE generated-template record, with
//! its own `GeneratedTemplateId`, whose covered resources are the base stack's managed
//! resources together with the `Resources` the request listed — each reported as one
//! `ResourceDetail` carrying that resource's type, the logical id it has in the final
//! generated template, and, for a resource the request listed, the `ResourceIdentifier`
//! property pairs the request supplied for it — one row per distinct resource of the final
//! template, so a listed resource that re-states one the base stack already contributes
//! refines that row rather than adding a second one under the same logical id.
//!
//! Two suites, one per face of that rule:
//!
//! 1. **The rows.** A table whose rows differ along what the rule generalises over: how many
//!    resources a generation covers (one and four), where each comes from (the base stack,
//!    the request's own list, and both), how many property pairs identify a listed resource
//!    (one, and the pair a type whose primary identifier is composite needs — the modelled
//!    `ResourceDefinition$ResourceIdentifier` is a map of 1..256 pairs), and a listed
//!    resource that re-states a base-stack resource. Each row is checked
//!    resource-by-resource — type, logical id and the identifier pairs of THAT row — rather
//!    than against the response's values pooled together.
//! 2. **The record each id addresses.** A whole in-flight set's worth of creates mint
//!    pairwise distinct ids, and each id addresses the record of its own call: its name and
//!    its own covered resources.
//!
//! `generated_templates.rs` pins the surrounding rules (the statuses, the in-flight cap and
//! the refusals) and `generated_template_reads.rs` pins which record an address resolves;
//! this suite is about WHAT one accepted create wrote.

use service_com_amazonaws_cloudformation_cloudformation::state::generated_template as generated;
use smithy_server_runtime::MultiServiceRouter;

use crate::generated_templates::{
    assert_generated_template_id_shape, create_generated_template, deploy,
    describe_generated_template, wait_for_the_generation,
};
use crate::stack_lifecycle::{list_members, member, router, xml_text, xml_texts};

/// A base stack declaring one resource, under the logical id `Topic`.
const ONE_RESOURCE_STACK: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// A base stack declaring two resources of two types.
const TWO_RESOURCE_STACK: &str = r#"{"Resources":{
    "Bucket":{"Type":"AWS::S3::Bucket"},
    "Table":{"Type":"AWS::DynamoDB::Table"}
}}"#;

/// One resource the request lists itself, as `ResourceDefinition` models it: the type, the
/// logical id it is to have in the generated template, and the property pairs that identify
/// it.
struct ListedResource {
    resource_type: &'static str,
    logical_resource_id: &'static str,
    /// `ResourceDefinition$ResourceIdentifier`, a map of 1..256 pairs keyed by the property
    /// name holding one of the type's primary identifiers.
    identifier: &'static [(&'static str, &'static str)],
}

/// One resource the generated template must report, as `ResourceDetail` carries it.
type ExpectedResource = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// One row of the coverage table.
struct CoverageCase {
    case: &'static str,
    /// The base stack's template, or `None` for a request that names no base stack.
    template: Option<&'static str>,
    listed: &'static [ListedResource],
    /// The resources the record must report: type, logical id and identifier pairs.
    expected: &'static [ExpectedResource],
}

/// The identifier pairs of an `AWS::SNS::Topic` and an `AWS::S3::Bucket` the request lists.
const TOPIC_ARN: &str = "arn:aws:sns:us-east-1:000000000000:coverage-topic";
const SERVICE_ARN: &str =
    "arn:aws:ecs:us-east-1:000000000000:service/coverage-cluster/coverage-service";

/// The rows: one covered resource and four; from the base stack, from the request's own
/// list, and both; a listed resource identified by one property pair and one identified by
/// two; and a listed resource that re-states a resource the base stack already contributes.
const COVERAGE_CASES: &[CoverageCase] = &[
    CoverageCase {
        case: "one resource from a base stack, nothing listed",
        template: Some(ONE_RESOURCE_STACK),
        listed: &[],
        // A base stack's resource is reported with no identifier: which property holds a
        // type's primary identifier is not something this emulator records, so the member
        // is absent rather than invented.
        expected: &[("AWS::SNS::Topic", "Topic", &[])],
    },
    CoverageCase {
        case: "no base stack: one listed resource identified by two property pairs",
        listed: &[ListedResource {
            // A type whose primary identifier is composite, so the row needs more than one
            // pair: `AWS::ECS::Service` is identified by its cluster and its service ARN.
            resource_type: "AWS::ECS::Service",
            logical_resource_id: "ListedService",
            identifier: &[("Cluster", "coverage-cluster"), ("ServiceArn", SERVICE_ARN)],
        }],
        template: None,
        expected: &[(
            "AWS::ECS::Service",
            "ListedService",
            &[("Cluster", "coverage-cluster"), ("ServiceArn", SERVICE_ARN)],
        )],
    },
    CoverageCase {
        case: "a base-stack resource the request re-states with its identifier",
        template: Some(ONE_RESOURCE_STACK),
        listed: &[ListedResource {
            resource_type: "AWS::SNS::Topic",
            logical_resource_id: "Topic",
            identifier: &[("TopicArn", TOPIC_ARN)],
        }],
        // ONE resource of the final template, now carrying the identifier the request
        // supplied for it — not a second row under the logical id the stack already used.
        expected: &[("AWS::SNS::Topic", "Topic", &[("TopicArn", TOPIC_ARN)])],
    },
    CoverageCase {
        case: "two resources from a base stack plus two the request lists itself",
        template: Some(TWO_RESOURCE_STACK),
        listed: &[
            ListedResource {
                resource_type: "AWS::Logs::LogGroup",
                logical_resource_id: "ListedGroup",
                identifier: &[("LogGroupName", "coverage-group")],
            },
            ListedResource {
                resource_type: "AWS::SNS::Topic",
                logical_resource_id: "ListedTopic",
                identifier: &[("TopicArn", TOPIC_ARN)],
            },
        ],
        expected: &[
            ("AWS::DynamoDB::Table", "Table", &[]),
            ("AWS::Logs::LogGroup", "ListedGroup", &[("LogGroupName", "coverage-group")]),
            ("AWS::S3::Bucket", "Bucket", &[]),
            ("AWS::SNS::Topic", "ListedTopic", &[("TopicArn", TOPIC_ARN)]),
        ],
    },
];

/// The request members that submit one `Resources` entry, in the awsQuery flattened form,
/// with however many `ResourceIdentifier` pairs the row carries.
fn listed_members(position: usize, listed: &ListedResource) -> Vec<(String, String)> {
    let index = position + 1;
    let mut members = vec![
        member(
            &format!("Resources.member.{index}.ResourceType"),
            listed.resource_type,
        ),
        member(
            &format!("Resources.member.{index}.LogicalResourceId"),
            listed.logical_resource_id,
        ),
    ];
    for (entry, (key, value)) in listed.identifier.iter().enumerate() {
        let entry = entry + 1;
        members.push(member(
            &format!("Resources.member.{index}.ResourceIdentifier.entry.{entry}.key"),
            key,
        ));
        members.push(member(
            &format!("Resources.member.{index}.ResourceIdentifier.entry.{entry}.value"),
            value,
        ));
    }
    members
}

/// One reported resource row: its type, its logical id and its identifier pairs, sorted so
/// two rows can be compared whatever order the wire carried them in.
type ReportedResource = (String, Option<String>, Vec<(String, String)>);

/// The `ResourceDetail` rows a described generation reports, each read from its OWN
/// `<member>` block so a sibling row's identifier cannot be swept into it, sorted by type
/// and logical id.
fn reported_resources(body: &str) -> Vec<ReportedResource> {
    let mut rows: Vec<ReportedResource> = list_members(body, "Resources")
        .into_iter()
        .map(|block| {
            let keys = xml_texts(&block, "key");
            let values = xml_texts(&block, "value");
            let mut identifier: Vec<(String, String)> = keys.into_iter().zip(values).collect();
            identifier.sort();
            (
                xml_text(&block, "ResourceType").unwrap_or_default(),
                xml_text(&block, "LogicalResourceId"),
                identifier,
            )
        })
        .collect();
    rows.sort();
    rows
}

/// The rows one case expects, in the same shape and order [`reported_resources`] answers.
fn expected_resources(expected: &[ExpectedResource]) -> Vec<ReportedResource> {
    let mut rows: Vec<ReportedResource> = expected
        .iter()
        .map(|(resource_type, logical_resource_id, identifier)| {
            let mut pairs: Vec<(String, String)> = identifier
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            pairs.sort();
            (
                resource_type.to_string(),
                Some(logical_resource_id.to_string()),
                pairs,
            )
        })
        .collect();
    rows.sort();
    rows
}

/// Create one generation for a case and answer the `GeneratedTemplateId` it was given.
async fn start_generation(
    router: &MultiServiceRouter,
    index: usize,
    row: &CoverageCase,
    name: &str,
) -> String {
    let case = row.case;
    let mut extra: Vec<(String, String)> = Vec::new();
    if let Some(template) = row.template {
        let stack = format!("coverage-base-stack-{index}");
        deploy(router, &stack, template).await;
        extra.push(member("StackName", &stack));
    }
    for (position, listed) in row.listed.iter().enumerate() {
        extra.extend(listed_members(position, listed));
    }
    let (status, body) = create_generated_template(router, name, &extra).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateGeneratedTemplate answered {status} with {body}"
    );
    let id = xml_text(&body, "GeneratedTemplateId")
        .unwrap_or_else(|| panic!("case {case:?}: expected a GeneratedTemplateId, got {body}"));
    assert_generated_template_id_shape(case, &id);
    id
}

/// The resources a create persisted are the base stack's together with the ones the request
/// listed, one row per distinct resource of the final template, each carrying its own type,
/// logical id and identifier pairs.
#[tokio::test]
async fn a_generation_covers_the_base_stack_and_the_listed_resources() {
    let router = router();

    for (index, row) in COVERAGE_CASES.iter().enumerate() {
        let case = row.case;
        let name = format!("coverage-{index}");
        let id = start_generation(&router, index, row, &name).await;

        wait_for_the_generation().await;
        let (status, body) = describe_generated_template(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate answered {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "GeneratedTemplateId").as_deref(),
            Some(id.as_str()),
            "case {case:?}: expected the record the create minted, got {body}"
        );

        let reported = reported_resources(&body);
        let expected = expected_resources(row.expected);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the covered resources {expected:?}, got {reported:?} from \
             {body}"
        );
        // One row per distinct resource of the final generated template: a logical id
        // addresses one resource there, so no two rows may report the same one.
        let mut logical_ids: Vec<Option<String>> =
            reported.iter().map(|(_, logical, _)| logical.clone()).collect();
        let total_rows = logical_ids.len();
        logical_ids.sort();
        logical_ids.dedup();
        assert_eq!(
            logical_ids.len(),
            total_rows,
            "case {case:?}: expected one row per logical id, got {reported:?} from {body}"
        );
        assert_eq!(
            xml_text(&body, "ResourcesSucceeded"),
            Some(total_rows.to_string()),
            "case {case:?}: expected the covered resources counted, got {body}"
        );
        assert_eq!(
            xml_texts(&body, "ResourceStatus"),
            vec![generated::resource_status::COMPLETE.to_string(); total_rows],
            "case {case:?}: expected every covered resource complete, got {body}"
        );
    }
}

/// Every accepted create mints a record of its own: the ids a whole in-flight set's worth of
/// calls answer are pairwise distinct, and each id addresses the record of ITS call — its
/// name and its own covered resources.
#[tokio::test]
async fn every_accepted_create_mints_its_own_record() {
    let router = router();
    // One create per case, repeated until the in-flight cap is full, so the suite spans a
    // whole metered set rather than a single call.
    let rounds = generated::MAX_CONCURRENT_GENERATIONS.div_ceil(COVERAGE_CASES.len());
    let mut created: Vec<(String, String, &CoverageCase)> = Vec::new();

    for round in 0..rounds {
        for (index, row) in COVERAGE_CASES.iter().enumerate() {
            let position = round * COVERAGE_CASES.len() + index;
            if position >= generated::MAX_CONCURRENT_GENERATIONS {
                break;
            }
            let name = format!("minted-{position}");
            let id = start_generation(&router, position, row, &name).await;
            created.push((name, id, row));
        }
        // The cap meters the generations still in flight, so the set is drained between
        // rounds rather than relying on how long a round took.
        wait_for_the_generation().await;
    }

    let mut ids: Vec<&str> = created.iter().map(|(_, id, _)| id.as_str()).collect();
    let minted = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        minted,
        "case \"a full in-flight set of creates\": expected {minted} distinct ids, got {:?}",
        created.iter().map(|(_, id, _)| id).collect::<Vec<_>>()
    );

    wait_for_the_generation().await;
    for (name, id, row) in &created {
        let case = format!("{:?} created as {name:?}", row.case);
        let (status, body) = describe_generated_template(&router, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeGeneratedTemplate({id:?}) answered {status} with {body}"
        );
        assert_eq!(
            xml_text(&body, "GeneratedTemplateName").as_deref(),
            Some(name.as_str()),
            "case {case:?}: expected the id to address its own call's record, got {body}"
        );
        let reported = reported_resources(&body);
        let expected = expected_resources(row.expected);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the resources of its own call {expected:?}, got \
             {reported:?} from {body}"
        );
    }
}
