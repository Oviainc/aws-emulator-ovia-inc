// SPDX-License-Identifier: CC0-1.0

//! The tags an `AWS::Lambda::Function` a stack declares carries in AWS Lambda itself.
//!
//! RULE: a resource CloudFormation provisions carries, in the service that owns it, the
//! stack-level `Tags` the stack operation supplied, the resource's own declared `Tags`
//! (which win a key collision, being the more specific statement about that one resource),
//! and the three tags CloudFormation records itself — `aws:cloudformation:stack-name`,
//! `aws:cloudformation:stack-id` and `aws:cloudformation:logical-id`. Those three sit in the
//! `aws:` namespace AWS keeps for itself, and that is the point: the namespace binds a
//! CALLER, while the service provisioning the resource is what SETS such a tag, so they
//! reach the owning service's own tag store and `lambda:ListTags` on the ARN the stack
//! published answers with the whole set. A key in that namespace the TEMPLATE merely spells
//! is a caller's and does not travel, and the deployment still completes.
//!
//! The rows differ along what the rule generalises over: how many stack-level tags there are
//! (none, one, several), how many functions one stack declares (so the `logical-id` tag
//! differs per resource while the other two do not), whether the function declares `Tags` of
//! its own, whether one of those keys COLLIDES with a stack tag or with one of
//! CloudFormation's own three, the FORM a declared tag value takes (text, a number, a
//! boolean), and a declared key in the reserved namespace that is NOT CloudFormation's.
//!
//! Every expected value is computed from what the row declares plus the stack id
//! `DescribeStacks` reports — never read back out of the tag answer being checked. The tags
//! are read through AWS LAMBDA's own wire, not out of CloudFormation's record of them.

use std::collections::BTreeMap;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, create_stack, describe_stack, member, router, wait_for_the_transition,
    xml_text, REGION,
};
use crate::stack_resource_provisioning::output_value;
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// The tag keys CloudFormation records on every resource it provisions. Source: the
/// `aws:cloudformation:*` tags CloudFormation's own documentation names
/// (`crate::state::provisioning`'s `STACK_NAME_TAG` / `STACK_ID_TAG` / `LOGICAL_ID_TAG`).
const STACK_NAME_TAG: &str = "aws:cloudformation:stack-name";
const STACK_ID_TAG: &str = "aws:cloudformation:stack-id";
const LOGICAL_ID_TAG: &str = "aws:cloudformation:logical-id";

/// Call one AWS Lambda operation over its restJson1 wire, signed for this suite's Region —
/// the Region the function the stack creates belongs to.
async fn lambda(router: &MultiServiceRouter, method: &str, uri: &str) -> (u16, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://lambda.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed Lambda request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, answered)
}

/// The tags AWS Lambda holds for the function `arn`, through its own `ListTags`, with the
/// status it answered.
async fn function_tags(
    router: &MultiServiceRouter,
    arn: &str,
) -> (u16, BTreeMap<String, String>) {
    let (status, answered) = lambda(
        router,
        "GET",
        &format!("/2017-03-31/tags/{}", percent_encoded(arn)),
    )
    .await;
    let held = answered
        .get("Tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    (status, held)
}

/// An ARN as a non-greedy `@httpLabel` is sent: every character outside the unreserved set
/// is percent-encoded, which for an ARN is its colons.
fn percent_encoded(label: &str) -> String {
    label
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
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

/// One function a row's stack declares, and the tags its OWN declaration asks for as they
/// must read back.
struct Declared<'a> {
    logical_id: &'a str,
    output_key: &'a str,
    /// The tags the declaration asks for that AWS Lambda is to STORE — so a key the
    /// declaration spells in the reserved namespace is not among them.
    stored: &'a [(&'a str, &'a str)],
}

/// One row of the table.
struct Case<'a> {
    case: &'a str,
    stack_name: &'a str,
    template: &'a str,
    /// The stack-level `Tags` the `CreateStack` call supplies.
    stack_tags: &'a [(&'a str, &'a str)],
    functions: &'a [Declared<'a>],
}

/// The whole tag set a provisioned function must carry: the stack's own tags, the function's
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

/// The execution role every template below declares, so the role the function RUNS AS is one
/// this account's IAM really holds — CloudFormation resolves it before it creates the
/// function.
const ROLE: &str = r#""HandlerRole":{"Type":"AWS::IAM::Role","Properties":{
     "AssumeRolePolicyDocument":{"Version":"2012-10-17","Statement":[{
       "Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},
       "Action":"sts:AssumeRole"}]}}}"#;

#[tokio::test]
async fn a_provisioned_function_carries_the_stacks_tags_and_the_cloudformation_ones() {
    let cases: &[Case<'_>] = &[
        Case {
            // The issue's own shape: one stack tag, one function published by `Fn::GetAtt`.
            case: "one stack tag on a function that declares none of its own",
            stack_name: "tagged-functions",
            template: &format!(
                r#"{{"Resources":{{{ROLE},
                     "Tagged":{{"Type":"AWS::Lambda::Function","Properties":{{
                        "Runtime":"python3.12","Handler":"index.handler",
                        "Role":{{"Fn::GetAtt":["HandlerRole","Arn"]}},
                        "Code":{{"ZipFile":"def handler(event, context):\n    return {{}}\n"}}}}}}}},
                   "Outputs":{{"TaggedArn":{{"Value":{{"Fn::GetAtt":["Tagged","Arn"]}}}}}}}}"#
            ),
            stack_tags: &[("lambda-console:blueprint", "true")],
            functions: &[Declared {
                logical_id: "Tagged",
                output_key: "TaggedArn",
                stored: &[],
            }],
        },
        Case {
            case: "no stack tags at all: the three CloudFormation tags alone",
            stack_name: "untagged-function-stack",
            template: &format!(
                r#"{{"Resources":{{{ROLE},
                     "Plain":{{"Type":"AWS::Lambda::Function","Properties":{{
                        "Runtime":"python3.12","Handler":"index.handler",
                        "Role":{{"Fn::GetAtt":["HandlerRole","Arn"]}},
                        "Code":{{"ZipFile":"def handler(event, context):\n    return {{}}\n"}}}}}}}},
                   "Outputs":{{"PlainArn":{{"Value":{{"Fn::GetAtt":["Plain","Arn"]}}}}}}}}"#
            ),
            stack_tags: &[],
            functions: &[Declared {
                logical_id: "Plain",
                output_key: "PlainArn",
                stored: &[],
            }],
        },
        Case {
            case: "three stack tags over two functions, so only the logical-id tag differs",
            stack_name: "two-function-stack",
            template: &format!(
                r#"{{"Resources":{{{ROLE},
                     "Reader":{{"Type":"AWS::Lambda::Function","Properties":{{
                        "Runtime":"python3.12","Handler":"index.handler",
                        "Role":{{"Fn::GetAtt":["HandlerRole","Arn"]}},
                        "Code":{{"ZipFile":"def handler(event, context):\n    return {{}}\n"}}}}}},
                     "Writer":{{"Type":"AWS::Lambda::Function","Properties":{{
                        "Runtime":"python3.12","Handler":"index.handler",
                        "Role":{{"Fn::GetAtt":["HandlerRole","Arn"]}},
                        "Code":{{"ZipFile":"def handler(event, context):\n    return {{}}\n"}}}}}}}},
                   "Outputs":{{"ReaderArn":{{"Value":{{"Fn::GetAtt":["Reader","Arn"]}}}},
                               "WriterArn":{{"Value":{{"Fn::GetAtt":["Writer","Arn"]}}}}}}}}"#
            ),
            stack_tags: &[
                ("team", "platform"),
                ("cost-centre", "4711"),
                ("sim2real:prefix", "sim2real-scn"),
            ],
            functions: &[
                Declared {
                    logical_id: "Reader",
                    output_key: "ReaderArn",
                    stored: &[],
                },
                Declared {
                    logical_id: "Writer",
                    output_key: "WriterArn",
                    stored: &[],
                },
            ],
        },
        Case {
            // A declared tag beside the stack's, one key declared on BOTH, and a value that
            // is a number and one that is a boolean — a tag value is the text it is.
            case: "declared tags, a key the stack and the function both name, and non-text values",
            stack_name: "collide-function-stack",
            template: &format!(
                r#"{{"Resources":{{{ROLE},
                     "Audit":{{"Type":"AWS::Lambda::Function","Properties":{{
                        "Runtime":"python3.12","Handler":"index.handler",
                        "Role":{{"Fn::GetAtt":["HandlerRole","Arn"]}},
                        "Code":{{"ZipFile":"def handler(event, context):\n    return {{}}\n"}},
                        "Tags":[{{"Key":"owner","Value":"the-resource"}},
                                {{"Key":"retention-days","Value":7}},
                                {{"Key":"encrypted","Value":true}},
                                {{"Key":"aws:cloudformation:logical-id","Value":"NotThisOne"}}]}}}}}},
                   "Outputs":{{"AuditArn":{{"Value":{{"Fn::GetAtt":["Audit","Arn"]}}}}}}}}"#
            ),
            stack_tags: &[("owner", "the-stack"), ("team", "platform")],
            functions: &[Declared {
                logical_id: "Audit",
                output_key: "AuditArn",
                // Only CloudFormation's own key is overridden; the ordinary declared tags
                // stand. `expected_tags` sets the three itself, so naming the claimed key
                // here is not what makes the assertion pass.
                stored: &[
                    ("owner", "the-resource"),
                    ("retention-days", "7"),
                    ("encrypted", "true"),
                ],
            }],
        },
        Case {
            // A declared key in the reserved namespace that is NOT one of CloudFormation's
            // three is a caller's to have written, and AWS Lambda refuses a caller that
            // namespace — so it does not travel, and the deployment still completes.
            case: "a declared key in the reserved namespace that is not CloudFormation's own",
            stack_name: "reserved-declared-stack",
            template: &format!(
                r#"{{"Resources":{{{ROLE},
                     "Claimed":{{"Type":"AWS::Lambda::Function","Properties":{{
                        "Runtime":"python3.12","Handler":"index.handler",
                        "Role":{{"Fn::GetAtt":["HandlerRole","Arn"]}},
                        "Code":{{"ZipFile":"def handler(event, context):\n    return {{}}\n"}},
                        "Tags":[{{"Key":"aws:servicecatalog:provisioningPrincipalArn",
                                  "Value":"someone"}},
                                {{"Key":"stage","Value":"prod"}}]}}}}}},
                   "Outputs":{{"ClaimedArn":{{"Value":{{"Fn::GetAtt":["Claimed","Arn"]}}}}}}}}"#
            ),
            stack_tags: &[("team", "platform")],
            functions: &[Declared {
                logical_id: "Claimed",
                output_key: "ClaimedArn",
                stored: &[("stage", "prod")],
            }],
        },
    ];

    for case in cases {
        let router = router();
        let mut members = acknowledging_iam(&[]);
        members.extend(tag_members(case.stack_tags));
        let (status, created) =
            create_stack(&router, case.stack_name, case.template, &members).await;
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
        let stack_id = xml_text(&described, "StackId").unwrap_or_else(|| {
            panic!(
                "case {:?}: the stack must report a StackId, got {described}",
                case.case
            )
        });

        for function in case.functions {
            let arn = output_value(&described, function.output_key);
            let expected = expected_tags(
                case.stack_tags,
                function.stored,
                case.stack_name,
                &stack_id,
                function.logical_id,
            );
            let (status, held) = function_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {:?}: lambda:ListTags({arn:?}) must answer 200, got {status}",
                case.case
            );
            assert_eq!(
                held, expected,
                "case {:?}: the function {:?} ({arn:?}) must carry {expected:?}, got {held:?}",
                case.case, function.logical_id
            );
        }
    }
}
