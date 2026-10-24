// SPDX-License-Identifier: CC0-1.0

//! The tags a stack's resources carry in the services that OWN them, end to end.
//!
//! RULE: a resource CloudFormation provisions carries, in its owning service, the
//! stack-level `Tags` the stack operation supplied PLUS the three tags CloudFormation
//! records itself — `aws:cloudformation:stack-name`, `aws:cloudformation:stack-id` and
//! `aws:cloudformation:logical-id` — and a key the resource's own `Tags` property states
//! keeps the value that property gives it. So the owning service's own tag read on the
//! handle the stack publishes for the resource (`Ref`/`Fn::GetAtt`) answers with that set,
//! which is what tag-based cost allocation and tag-based access control key off.
//!
//! The rows differ along what the rule generalises over: how many stack-level tags there
//! are (none, one, several), how many resources one stack declares (so the `logical-id` tag
//! differs per resource while the other two do not), whether the resource declares `Tags`
//! of its own, whether one of those keys COLLIDES with a stack tag, the FORM a declared tag
//! value takes (text, a number, a boolean), and the resource TYPE — an `AWS::SNS::Topic`,
//! whose tags ride on the `CreateTopic` call itself, and an `AWS::S3::Bucket`, whose create
//! takes no tags at all so they are a `PutBucketTagging` of its own. Both halves of the
//! rule are read through the OWNING service's wire (`sns:ListTagsForResource`,
//! `s3:GetBucketTagging`), never out of CloudFormation's own record.
//!
//! Every expected value is computed from what the row declares plus the stack id
//! `DescribeStacks` reports — never read back out of the tag answer being checked.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    create_stack, describe_stack, member, router, sns, wait_for_the_transition, xml_blocks,
    xml_text,
};
use crate::stack_resource_provisioning::{output_value, s3};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The tag keys CloudFormation records on every resource it provisions. Source: the
/// `aws:cloudformation:*` tags CloudFormation's own documentation names
/// (`crate::state::provisioning`'s `STACK_NAME_TAG` / `STACK_ID_TAG` / `LOGICAL_ID_TAG`).
const STACK_NAME_TAG: &str = "aws:cloudformation:stack-name";
const STACK_ID_TAG: &str = "aws:cloudformation:stack-id";
const LOGICAL_ID_TAG: &str = "aws:cloudformation:logical-id";

/// The `{Key, Value}` pairs inside every `<block>…</block>` of an XML answer, as a map.
///
/// Both tag readings this suite makes report a list of two-member structures — SNS's
/// `<Tags><member><Key/><Value/></member></Tags>` and S3's
/// `<TagSet><Tag><Key/><Value/></Tag></TagSet>` — so one reader serves both, and it is the
/// harness's own block scanner ([`crate::stack_lifecycle::xml_blocks`]), which tolerates the
/// `xmlns` the query serializer puts on a list's first member.
fn tag_pairs(xml: &str, block: &str) -> BTreeMap<String, String> {
    xml_blocks(xml, block)
        .iter()
        .filter_map(|entry| {
            let key = xml_text(entry, "Key")?;
            Some((key, xml_text(entry, "Value").unwrap_or_default()))
        })
        .collect()
}

/// The tags Amazon SNS holds for the topic `arn`, with the status it answered.
async fn topic_tags(router: &MultiServiceRouter, arn: &str) -> (u16, BTreeMap<String, String>) {
    let (status, body) = sns(router, "ListTagsForResource", &[("ResourceArn", arn)]).await;
    (status, tag_pairs(&body, "member"))
}

/// The tags Amazon S3 holds for `bucket`, with the status it answered.
async fn bucket_tags(
    router: &MultiServiceRouter,
    bucket: &str,
) -> (u16, BTreeMap<String, String>) {
    let answer = s3(router, "GET", &format!("/{bucket}?tagging"), b"").await;
    (answer.status, tag_pairs(&answer.body, "Tag"))
}

/// One resource of a row: the logical id the template declares it under, the output key
/// that publishes its handle, and the tags the owning service must hold for it beside the
/// stack's own and the `aws:cloudformation:*` ones.
struct Provisioned<'a> {
    logical_id: &'a str,
    output_key: &'a str,
    /// The tags the resource's OWN declaration asks for, as they must read back.
    declared: &'a [(&'a str, &'a str)],
}

/// One row of the propagation table.
struct TagCase<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    /// The stack-level `Tags` the `CreateStack` call supplies.
    stack_tags: &'a [(&'a str, &'a str)],
    topics: &'a [Provisioned<'a>],
    buckets: &'a [Provisioned<'a>],
}

/// The `Tags.member.N.Key`/`Value` request members a stack-level tag list is.
fn tag_members(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut members = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        members.push(member(&format!("Tags.member.{position}.Key"), key));
        members.push(member(&format!("Tags.member.{position}.Value"), value));
    }
    members
}

/// The whole tag set a provisioned resource must carry: the stack's own tags, the resource's
/// own declaration winning any key collision, and the three CloudFormation records — which
/// are CloudFormation's to set, so they are applied LAST and no declaration displaces them.
fn expected_tags(
    stack_tags: &[(&str, &str)],
    declared: &[(&str, &str)],
    stack_name: &str,
    stack_id: &str,
    logical_id: &str,
) -> BTreeMap<String, String> {
    let mut expected: BTreeMap<String, String> = stack_tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    for (key, value) in declared {
        expected.insert((*key).to_string(), (*value).to_string());
    }
    expected.insert(STACK_NAME_TAG.to_string(), stack_name.to_string());
    expected.insert(STACK_ID_TAG.to_string(), stack_id.to_string());
    expected.insert(LOGICAL_ID_TAG.to_string(), logical_id.to_string());
    expected
}

#[tokio::test]
async fn a_provisioned_resource_carries_the_stacks_tags_and_the_cloudformation_ones() {
    let cases: &[TagCase<'_>] = &[
        TagCase {
            // The issue's own shape: one stack tag, one topic published by `!Ref`.
            case: "one stack tag on a topic that declares none of its own",
            stack_name: "sim2real-xsvc-probe",
            template: concat!(
                "Resources:\n",
                "  ProbeTopic:\n",
                "    Type: AWS::SNS::Topic\n",
                "    Properties:\n",
                "      TopicName: sim2real-xsvc-probe-topic\n",
                "Outputs:\n",
                "  TopicArn:\n",
                "    Value: !Ref ProbeTopic\n",
            ),
            stack_tags: &[("sim2real:managed", "true")],
            topics: &[Provisioned {
                logical_id: "ProbeTopic",
                output_key: "TopicArn",
                declared: &[],
            }],
            buckets: &[],
        },
        TagCase {
            case: "no stack tags at all: the three CloudFormation tags alone",
            stack_name: "untagged-stack",
            template: concat!(
                "Resources:\n",
                "  Events:\n",
                "    Type: AWS::SNS::Topic\n",
                "Outputs:\n",
                "  TopicArn:\n",
                "    Value: !Ref Events\n",
            ),
            stack_tags: &[],
            topics: &[Provisioned {
                logical_id: "Events",
                output_key: "TopicArn",
                declared: &[],
            }],
            buckets: &[],
        },
        TagCase {
            case: "three stack tags over two topics, so only the logical-id tag differs",
            stack_name: "two-topic-stack",
            template: concat!(
                "Resources:\n",
                "  Alarms:\n",
                "    Type: AWS::SNS::Topic\n",
                "  Notices:\n",
                "    Type: AWS::SNS::Topic\n",
                "Outputs:\n",
                "  AlarmsArn:\n",
                "    Value: !Ref Alarms\n",
                "  NoticesArn:\n",
                "    Value: !Ref Notices\n",
            ),
            stack_tags: &[
                ("team", "platform"),
                ("cost-centre", "4711"),
                ("sim2real:prefix", "sim2real-xsvc"),
            ],
            topics: &[
                Provisioned {
                    logical_id: "Alarms",
                    output_key: "AlarmsArn",
                    declared: &[],
                },
                Provisioned {
                    logical_id: "Notices",
                    output_key: "NoticesArn",
                    declared: &[],
                },
            ],
            buckets: &[],
        },
        TagCase {
            // A resource-level tag beside the stack's, and one key declared on BOTH: the
            // declaration is the more specific statement about that resource, so it wins.
            case: "a declared tag, and a key the stack and the resource both name",
            stack_name: "collide-stack",
            template: r#"{"Resources":{"Audit":{"Type":"AWS::SNS::Topic","Properties":{
                 "Tags":[{"Key":"owner","Value":"the-resource"},
                         {"Key":"retention-days","Value":7},
                         {"Key":"encrypted","Value":true}]}}},
                 "Outputs":{"TopicArn":{"Value":{"Ref":"Audit"}}}}"#,
            stack_tags: &[("owner", "the-stack"), ("team", "platform")],
            topics: &[Provisioned {
                logical_id: "Audit",
                output_key: "TopicArn",
                // A number and a boolean are tag VALUES as the text they are.
                declared: &[
                    ("owner", "the-resource"),
                    ("retention-days", "7"),
                    ("encrypted", "true"),
                ],
            }],
            buckets: &[],
        },
        TagCase {
            // A declaration that spells one of CloudFormation's OWN three keys does not
            // displace it: those three are CloudFormation's to set, wherever they are
            // written.
            case: "a declaration that spells aws:cloudformation:logical-id itself",
            stack_name: "reserved-key-stack",
            template: r#"{"Resources":{"Claimed":{"Type":"AWS::SNS::Topic","Properties":{
                 "Tags":[{"Key":"aws:cloudformation:logical-id","Value":"NotThisOne"},
                         {"Key":"owner","Value":"the-resource"}]}}},
                 "Outputs":{"TopicArn":{"Value":{"Ref":"Claimed"}}}}"#,
            stack_tags: &[("team", "platform")],
            topics: &[Provisioned {
                logical_id: "Claimed",
                output_key: "TopicArn",
                // Only the key that is CloudFormation's own is overridden; the ordinary
                // declared tag stands. `expected_tags` sets the three itself, so naming
                // the claimed key here is not what makes the assertion pass.
                declared: &[("owner", "the-resource")],
            }],
            buckets: &[],
        },
        TagCase {
            // Two resource TYPES in one stack: a topic, whose tags ride on `CreateTopic`,
            // and a bucket, whose create takes none so they are a `PutBucketTagging`.
            case: "a topic and a bucket in one stack",
            stack_name: "mixed-type-stack",
            template: concat!(
                "Resources:\n",
                "  Artifacts:\n",
                "    Type: AWS::S3::Bucket\n",
                "  Signals:\n",
                "    Type: AWS::SNS::Topic\n",
                "Outputs:\n",
                "  BucketName:\n",
                "    Value: !Ref Artifacts\n",
                "  TopicArn:\n",
                "    Value: !Ref Signals\n",
            ),
            stack_tags: &[("sim2real:managed", "true"), ("team", "platform")],
            topics: &[Provisioned {
                logical_id: "Signals",
                output_key: "TopicArn",
                declared: &[],
            }],
            buckets: &[Provisioned {
                logical_id: "Artifacts",
                output_key: "BucketName",
                declared: &[],
            }],
        },
    ];

    for case in cases {
        let router = router();
        let (status, created) = create_stack(
            &router,
            case.stack_name,
            case.template,
            &tag_members(case.stack_tags),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateStack must answer 200, got {status} {created}",
            case.case
        );
        wait_for_the_transition().await;

        let (status, described) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: DescribeStacks must answer 200, got {status} {described}",
            case.case
        );
        assert_eq!(
            xml_text(&described, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: the stack must reach CREATE_COMPLETE, got {described}",
            case.case
        );
        let stack_id =
            xml_text(&described, "StackId").unwrap_or_else(|| panic!("case {:?}: the stack must report a StackId, got {described}", case.case));

        for topic in case.topics {
            let arn = output_value(&described, topic.output_key);
            let expected = expected_tags(
                case.stack_tags,
                topic.declared,
                case.stack_name,
                &stack_id,
                topic.logical_id,
            );
            let (status, held) = topic_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {:?}: sns:ListTagsForResource({arn:?}) must answer 200, got {status}",
                case.case
            );
            assert_eq!(
                held, expected,
                "case {:?}: the topic {:?} ({arn:?}) must carry {expected:?}, got {held:?}",
                case.case, topic.logical_id
            );
        }

        for bucket in case.buckets {
            let name = output_value(&described, bucket.output_key);
            let expected = expected_tags(
                case.stack_tags,
                bucket.declared,
                case.stack_name,
                &stack_id,
                bucket.logical_id,
            );
            let (status, held) = bucket_tags(&router, &name).await;
            assert_eq!(
                status, 200,
                "case {:?}: s3:GetBucketTagging({name:?}) must answer 200, got {status}",
                case.case
            );
            assert_eq!(
                held, expected,
                "case {:?}: the bucket {:?} ({name:?}) must carry {expected:?}, got {held:?}",
                case.case, bucket.logical_id
            );
        }
    }
}

/// The tags a stack's resource carries survive the resource being carried forward by a
/// later deployment: the stack re-declares it unchanged, nothing is created again, and the
/// owning service still holds the tags the create applied.
#[tokio::test]
async fn an_update_that_keeps_a_resource_leaves_the_tags_its_create_applied() {
    let router = router();
    let before = concat!(
        "Resources:\n",
        "  Kept:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: tag-kept-topic\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref Kept\n",
    );
    // The same topic, declared beside one the update adds.
    let after = concat!(
        "Resources:\n",
        "  Kept:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: tag-kept-topic\n",
        "  Added:\n",
        "    Type: AWS::SNS::Topic\n",
        "    Properties:\n",
        "      TopicName: tag-added-topic\n",
        "Outputs:\n",
        "  TopicArn:\n",
        "    Value: !Ref Kept\n",
        "  AddedArn:\n",
        "    Value: !Ref Added\n",
    );
    let stack_tags: &[(&str, &str)] = &[("sim2real:managed", "true")];

    let (status, created) =
        create_stack(&router, "tagged-update", before, &tag_members(stack_tags)).await;
    assert_eq!(
        status, 200,
        "CreateStack must answer 200, got {status} {created}"
    );
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "tagged-update").await;
    let stack_id = xml_text(&described, "StackId").expect("the stack reports a StackId");

    let (status, updated) = crate::stack_lifecycle::call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "tagged-update"),
            member("TemplateBody", after),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "UpdateStack must answer 200, got {status} {updated}"
    );
    wait_for_the_transition().await;

    let (_, described) = describe_stack(&router, "tagged-update").await;
    assert_eq!(
        xml_text(&described, "StackStatus").as_deref(),
        Some(status::UPDATE_COMPLETE),
        "the update must complete, got {described}"
    );

    // (case, the output that publishes the topic, its logical id)
    for (case, output_key, logical_id) in [
        ("the topic both templates declare", "TopicArn", "Kept"),
        ("the topic the update added", "AddedArn", "Added"),
    ] {
        let arn = output_value(&described, output_key);
        let expected = expected_tags(stack_tags, &[], "tagged-update", &stack_id, logical_id);
        let (status, held) = topic_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: sns:ListTagsForResource({arn:?}) must answer 200, got {status}"
        );
        assert_eq!(
            held, expected,
            "case {case:?}: the topic {arn:?} must carry {expected:?}, got {held:?}"
        );
    }
}
