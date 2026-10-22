// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `CheckNoPublicAccess`, driven over the wire.
//!
//! Every case goes through the real router (the shared wire harness in
//! `tests/all/harness.rs`): restJson1 routing, boundary validation, the handler and the
//! response serialization all run exactly as they do in the emulator binary.
//!
//! The rule under test: a resource policy grants PUBLIC access when a statement whose
//! `Effect` is `Allow` admits every principal — `"Principal": "*"` / `{"AWS": "*"}`, or an
//! `Allow` written with `NotPrincipal` — and no `Condition` binds the request to a known
//! principal, account, organization, calling service or network path; the operation then
//! answers `FAIL` with one reason per such statement, and `PASS` otherwise. The tables
//! below span that class (principal form, effect, condition operator, condition key,
//! condition value, statement count, statement form, resource type), and the two recorded
//! cases — a public policy and a private one on `AWS::S3::Bucket` — are two rows.
//!
//! The document itself is read by the crate's shared IAM policy parser, so the refusal
//! suites below also pin what that parser owns: the grammar, the input ceilings it
//! publishes (`MAX_DOCUMENT_BYTES`, `MAX_STATEMENTS`) and the condition operators it does
//! not model, which are answered with the modelled 422 rather than a guessed verdict.

use serde_json::{json, Value};
use service_com_amazonaws_accessanalyzer_accessanalyzer::ops::check_no_new_access::policy::{
    MAX_DOCUMENT_BYTES, MAX_STATEMENTS,
};
use service_com_amazonaws_accessanalyzer_accessanalyzer::ops::types::ACCESS_CHECK_RESOURCE_TYPE_VALUES;

use crate::harness::{Emulator, Reply};

/// The operation's HTTP binding, from
/// `smithy_query(query="operation", operation="CheckNoPublicAccess")`.
const CHECK_PATH: &str = "/policy/check-no-public-access";

/// A resource type the recorded cases use, and one they do not — both drawn from the
/// model's `AccessCheckResourceType` enum (`ACCESS_CHECK_RESOURCE_TYPE_VALUES`).
const RECORDED_RESOURCE_TYPE: &str = "AWS::S3::Bucket";
const UNCITED_RESOURCE_TYPE: &str = "AWS::SQS::Queue";

/// An account this test treats as known, so a policy naming it is not public.
const KNOWN_ACCOUNT: &str = "111122223333";

/// The canonical finding every `CheckNoPublicAccess` reason reports, from the model's own
/// `smithy.api#examples` for the operation (verify with
/// `grep 'Public access granted in the following statement' smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json`).
const PUBLIC_ACCESS_FINDING: &str = "Public access granted in the following statement";

/// Call `CheckNoPublicAccess` with `policy` serialized into the `policyDocument` string
/// member, as a client does.
async fn check(emulator: &Emulator, policy: &Value, resource_type: &str) -> Reply {
    check_raw(emulator, &policy.to_string(), resource_type).await
}

/// Same, for a `policyDocument` that is not necessarily valid JSON.
async fn check_raw(emulator: &Emulator, policy_document: &str, resource_type: &str) -> Reply {
    emulator
        .send(
            "POST",
            CHECK_PATH,
            Some(json!({"policyDocument": policy_document, "resourceType": resource_type})),
        )
        .await
}

/// The `(statementIndex, statementId)` of every reported reason, in report order, after
/// asserting each reason carries the CANONICAL description: the finding, then the
/// statement named by its `Sid` when it carries one and by its index when it does not.
fn reported_reasons(reply: &Reply, case: &str) -> Vec<(i64, Option<String>)> {
    reply
        .member("reasons")
        .and_then(Value::as_array)
        .map(|reasons| {
            reasons
                .iter()
                .map(|reason| {
                    let statement_index = reason
                        .get("statementIndex")
                        .and_then(Value::as_i64)
                        .unwrap_or(-1);
                    let statement_id = reason
                        .get("statementId")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let expected = match statement_id.as_deref() {
                        Some(sid) => format!("{PUBLIC_ACCESS_FINDING} with sid: {sid}."),
                        None => format!("{PUBLIC_ACCESS_FINDING} with index: {statement_index}."),
                    };
                    assert_eq!(
                        reason.get("description").and_then(Value::as_str),
                        Some(expected.as_str()),
                        "case {case:?}: expected the canonical reason {expected:?}, got {reason:?}"
                    );
                    (statement_index, statement_id)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the evaluation table.
#[derive(Debug)]
struct PolicyCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The policy document submitted.
    policy: Value,
    /// The `(statementIndex, statementId)` of every reason the answer must carry, in
    /// order. EMPTY means the policy grants no public access, so the result is `PASS`.
    expected_reasons: Vec<(i64, Option<&'static str>)>,
}

/// Every statement form the rule generalises over, answered from the document itself.
#[tokio::test]
async fn public_access_is_decided_per_statement_from_the_policy() {
    let emulator = Emulator::new();
    let cases = vec![
        PolicyCase {
            case: "the recorded public policy: bare wildcard principal",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": "*",
                    "Action": "s3:GetObject",
                    "Resource": "arn:aws:s3:::example-bucket/*"
                }]
            }),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "the recorded private policy: a single named account principal",
            policy: json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": format!("arn:aws:iam::{KNOWN_ACCOUNT}:root")},
                    "Action": "s3:GetObject",
                    "Resource": "arn:aws:s3:::example-bucket/*"
                }]
            }),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "wildcard in the AWS principal map, with a Sid",
            policy: json!({
                "Statement": [{
                    "Sid": "PublicRead",
                    "Effect": "Allow",
                    "Principal": {"AWS": "*"},
                    "Action": "sqs:SendMessage",
                    "Resource": "*"
                }]
            }),
            expected_reasons: vec![(0, Some("PublicRead"))],
        },
        PolicyCase {
            case: "wildcard alongside a named account in one principal list",
            policy: json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"AWS": ["*", format!("arn:aws:iam::{KNOWN_ACCOUNT}:root")]},
                    "Action": "sqs:SendMessage"
                }]
            }),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "a service principal is a named principal, not the public",
            policy: json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Service": "lambda.amazonaws.com"},
                    "Action": "sqs:SendMessage"
                }]
            }),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "a federated identity provider is a named principal",
            policy: json!({
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": {"Federated": "cognito-identity.amazonaws.com"},
                    "Action": "sts:AssumeRoleWithWebIdentity"
                }]
            }),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "Deny with a wildcard principal grants nothing",
            policy: json!({
                "Statement": [{
                    "Effect": "Deny",
                    "Principal": "*",
                    "Action": "s3:*",
                    "Resource": "*"
                }]
            }),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "Allow with NotPrincipal admits every principal it does not list",
            policy: json!({
                "Statement": [{
                    "Sid": "AllButOne",
                    "Effect": "Allow",
                    "NotPrincipal": {"AWS": format!("arn:aws:iam::{KNOWN_ACCOUNT}:root")},
                    "Action": "s3:GetObject",
                    "Resource": "*"
                }]
            }),
            expected_reasons: vec![(0, Some("AllButOne"))],
        },
        PolicyCase {
            case: "a single statement object rather than an array",
            policy: json!({
                "Statement": {"Effect": "Allow", "Principal": "*", "Action": "kms:Decrypt"}
            }),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "the public statement is the second of three",
            policy: json!({
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": {"AWS": format!("arn:aws:iam::{KNOWN_ACCOUNT}:root")},
                        "Action": "s3:GetObject"
                    },
                    {
                        "Sid": "Open",
                        "Effect": "Allow",
                        "Principal": "*",
                        "Action": "s3:ListBucket"
                    },
                    {"Effect": "Deny", "Principal": "*", "Action": "s3:DeleteObject"}
                ]
            }),
            expected_reasons: vec![(1, Some("Open"))],
        },
        PolicyCase {
            case: "two public statements report two reasons, in document order",
            policy: json!({
                "Statement": [
                    {"Sid": "First", "Effect": "Allow", "Principal": "*", "Action": "s3:GetObject"},
                    {"Sid": "Second", "Effect": "Allow", "Principal": {"AWS": "*"}, "Action": "s3:PutObject"}
                ]
            }),
            expected_reasons: vec![(0, Some("First")), (1, Some("Second"))],
        },
        // The condition rows: the same wildcard-principal statement, scoped and not.
        PolicyCase {
            case: "scoped by aws:PrincipalOrgID",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:PrincipalOrgID": "o-exampleorgid"}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "scoped by aws:PrincipalAccount",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:PrincipalAccount": KNOWN_ACCOUNT}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "scoped by aws:SourceArn under ArnLike",
            policy: wildcard_with_condition(json!({
                "ArnLike": {"aws:SourceArn": format!("arn:aws:sns:us-east-1:{KNOWN_ACCOUNT}:*")}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "scoped by the aws:PrincipalTag/<key> family",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:PrincipalTag/team": "payments"}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "scoped by aws:PrincipalIsAWSService written as a JSON boolean",
            policy: wildcard_with_condition(json!({
                "Bool": {"aws:PrincipalIsAWSService": true}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            // The opposite polarity excludes only service principals, leaving every
            // other caller — the anonymous one included — allowed.
            case: "aws:PrincipalIsAWSService false scopes nothing",
            policy: wildcard_with_condition(json!({
                "Bool": {"aws:PrincipalIsAWSService": "false"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            // `Anonymous` is one of aws:PrincipalType's values, so a condition on that
            // key can pin a statement TO unauthenticated callers.
            case: "a condition on aws:PrincipalType scopes nothing",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:PrincipalType": "Anonymous"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            // aws:ResourceAccount describes the account that OWNS the resource, not the
            // caller, so every caller satisfies it — anonymous ones included.
            case: "a condition on aws:ResourceAccount scopes nothing",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:ResourceAccount": KNOWN_ACCOUNT}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "scoped by kms:CallerAccount, with the key name in another case",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"kms:calleraccount": KNOWN_ACCOUNT}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "scoped by Null demanding aws:PrincipalAccount be present",
            policy: wildcard_with_condition(json!({
                "Null": {"aws:PrincipalAccount": "false"}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "Null demanding aws:PrincipalAccount be ABSENT scopes nothing",
            policy: wildcard_with_condition(json!({
                "Null": {"aws:PrincipalAccount": "true"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "a negated operator on a scoping key scopes nothing",
            policy: wildcard_with_condition(json!({
                "StringNotEquals": {"aws:PrincipalAccount": KNOWN_ACCOUNT}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "a wildcard condition value scopes nothing",
            policy: wildcard_with_condition(json!({
                "StringLike": {"aws:PrincipalArn": "*"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "a value list that admits the wildcard scopes nothing",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:PrincipalAccount": [KNOWN_ACCOUNT, "*"]}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "a condition on a key that scopes no identity",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"s3:prefix": "public/"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            // A condition key is caller-controlled text: a multi-byte character where a
            // scoping-key prefix would end must be read, not indexed into.
            case: "a non-ASCII condition key is no scoping key",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:PrincipalTag\u{20ac}x": "payments"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "a non-ASCII tag name inside the scoping-key family still scopes",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:PrincipalTag/équipe": "payments"}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "an IfExists operator scopes nothing",
            policy: wildcard_with_condition(json!({
                "StringEqualsIfExists": {"aws:PrincipalAccount": KNOWN_ACCOUNT}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "scoped by aws:SourceVpce",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:SourceVpce": "vpce-1a2b3c4d"}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "a negation spelled in another case is still a negation",
            policy: wildcard_with_condition(json!({
                "stringnotequals": {"aws:PrincipalAccount": KNOWN_ACCOUNT}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "an ARN whose account segment is a wildcard scopes nothing",
            policy: wildcard_with_condition(json!({
                "ArnLike": {"aws:PrincipalArn": "arn:aws:iam::*:role/*"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "an accountless ARN naming one resource still scopes",
            policy: wildcard_with_condition(json!({
                "ArnEquals": {"aws:SourceArn": "arn:aws:s3:::example-bucket"}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "a non-ARN value carrying a wildcard scopes nothing",
            policy: wildcard_with_condition(json!({
                "StringLike": {"aws:PrincipalOrgID": "o-*"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "scoped by aws:SourceIp to one network",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:SourceIp": "203.0.113.0/24"}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "aws:SourceIp 0.0.0.0/0 is every address, so it scopes nothing",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:SourceIp": "0.0.0.0/0"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "aws:SourceIp ::/0 is every address, so it scopes nothing",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:SourceIp": "::/0"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            // Judged by the CIDR prefix length, so another spelling of the same
            // any-address range is caught too.
            case: "aws:SourceIp 0:0:0:0:0:0:0:0/0 is every address",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:SourceIp": "0:0:0:0:0:0:0:0/0"}
            })),
            expected_reasons: vec![(0, None)],
        },
        PolicyCase {
            case: "scoped by aws:SourceIp to a single address, with no prefix length",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"aws:SourceIp": "198.51.100.7"}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "one scoping condition among several entries is enough",
            policy: wildcard_with_condition(json!({
                "StringEquals": {"s3:prefix": "public/"},
                "ArnEquals": {"aws:PrincipalArn": format!("arn:aws:iam::{KNOWN_ACCOUNT}:role/app")}
            })),
            expected_reasons: Vec::new(),
        },
        PolicyCase {
            case: "a Deny statement written with NotAction grants nothing",
            policy: json!({"Version": "2012-10-17", "Statement": [{
                "Effect": "Deny",
                "Principal": "*",
                "NotAction": "s3:GetObject"
            }]}),
            expected_reasons: Vec::new(),
        },
    ];

    for case in cases {
        let reply = check(&emulator, &case.policy, RECORDED_RESOURCE_TYPE).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected HTTP 200, got {} {:?}",
            case.case, reply.status, reply.body
        );

        let expected_result = if case.expected_reasons.is_empty() {
            "PASS"
        } else {
            "FAIL"
        };
        let result = reply.member("result").and_then(Value::as_str);
        assert_eq!(
            result,
            Some(expected_result),
            "case {:?}: expected result {expected_result}, got {:?}",
            case.case,
            reply.body
        );

        // The message states the finding the result encodes, so the two never disagree.
        let message = reply
            .member("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            message.contains("does not grant public access"),
            expected_result == "PASS",
            "case {:?}: the message must agree with result {expected_result}, got {message:?}",
            case.case
        );

        let expected_reasons: Vec<(i64, Option<String>)> = case
            .expected_reasons
            .iter()
            .map(|(index, sid)| (*index, sid.map(str::to_string)))
            .collect();
        let reported = reported_reasons(&reply, case.case);
        assert_eq!(
            reported, expected_reasons,
            "case {:?}: expected reasons {expected_reasons:?}, got {:?}",
            case.case, reply.body
        );

        if expected_result == "PASS" {
            assert!(
                reply.member("reasons").is_none(),
                "case {:?}: a policy granting no public access carries no reasons, got {:?}",
                case.case,
                reply.body
            );
        }
    }
}

/// The wildcard-principal statement the condition rows scope, with `condition` attached.
fn wildcard_with_condition(condition: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": "s3:GetObject",
            "Resource": "arn:aws:s3:::example-bucket/*",
            "Condition": condition
        }]
    })
}

/// The answer depends on the policy, not on which supported resource type is named: the
/// whole `AccessCheckResourceType` catalog is exercised, including entries no recorded
/// case cites.
#[tokio::test]
async fn every_supported_resource_type_is_answered_from_the_policy() {
    let emulator = Emulator::new();
    let public = json!({
        "Statement": [{"Effect": "Allow", "Principal": "*", "Action": "*", "Resource": "*"}]
    });
    let private = json!({
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": format!("arn:aws:iam::{KNOWN_ACCOUNT}:root")},
            "Action": "*",
            "Resource": "*"
        }]
    });

    // The catalog is the model's own enum, re-exported by this crate's `ops::types`.
    assert!(
        ACCESS_CHECK_RESOURCE_TYPE_VALUES.contains(&UNCITED_RESOURCE_TYPE),
        "the uncited resource type must be a modelled value, got {UNCITED_RESOURCE_TYPE}"
    );
    for resource_type in ACCESS_CHECK_RESOURCE_TYPE_VALUES {
        for (policy, expected) in [(&public, "FAIL"), (&private, "PASS")] {
            let reply = check(&emulator, policy, resource_type).await;
            assert_eq!(
                reply.status, 200,
                "case {resource_type:?}: expected HTTP 200, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.member("result").and_then(Value::as_str),
                Some(expected),
                "case {resource_type:?}: expected result {expected}, got {:?}",
                reply.body
            );
        }
    }
}

/// A `resourceType` the model does not declare is refused, not answered.
#[tokio::test]
async fn an_unsupported_resource_type_is_refused() {
    let emulator = Emulator::new();
    let policy = json!({
        "Statement": [{"Effect": "Allow", "Principal": "*", "Action": "*"}]
    });
    for resource_type in [
        "AWS::EC2::Instance",
        "AWS::S3::bucket",
        "",
        "AWS::RDS::DBSnapshot",
    ] {
        let reply = check(&emulator, &policy, resource_type).await;
        assert_eq!(
            reply.status, 400,
            "case {resource_type:?}: expected HTTP 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {resource_type:?}: expected ValidationException, got {:?}",
            reply.body
        );
    }
}

/// A document the shared parser cannot read is refused with the modelled
/// `ValidationException`, never answered `PASS`.
#[tokio::test]
async fn an_unevaluable_policy_document_is_refused() {
    let emulator = Emulator::new();
    let cases: Vec<(&str, String)> = vec![
        ("not JSON at all", "this is not a policy".to_string()),
        ("an empty document", String::new()),
        ("truncated JSON", "{\"Statement\": [".to_string()),
        (
            "a JSON array rather than an object",
            json!([{"Effect": "Allow"}]).to_string(),
        ),
        ("a JSON string", json!("Allow").to_string()),
        (
            "no Statement element",
            json!({"Version": "2012-10-17"}).to_string(),
        ),
        (
            "a Statement that is a string",
            json!({"Statement": "Allow everything"}).to_string(),
        ),
        (
            "a statement that is not an object",
            json!({"Statement": ["Allow"]}).to_string(),
        ),
        (
            "a statement with no Effect",
            json!({"Statement": [{"Principal": "*", "Action": "*"}]}).to_string(),
        ),
        (
            "an Effect that is neither Allow nor Deny",
            json!({"Statement": [{"Effect": "Permit", "Principal": "*"}]}).to_string(),
        ),
        (
            "an Effect in the wrong case",
            json!({"Statement": [{"Effect": "allow", "Principal": "*"}]}).to_string(),
        ),
        (
            "an Effect that is not a string",
            json!({"Statement": [{"Effect": true, "Principal": "*"}]}).to_string(),
        ),
        (
            "an empty statement list",
            json!({"Version": "2012-10-17", "Statement": []}).to_string(),
        ),
        (
            "a resource policy statement naming no principal",
            json!({"Statement": [{"Effect": "Allow", "Action": "s3:GetObject"}]}).to_string(),
        ),
        (
            "a statement naming no action",
            json!({"Statement": [{"Effect": "Allow", "Principal": "*"}]}).to_string(),
        ),
        (
            "a Sid that is not a string",
            json!({"Statement": [{
                "Sid": 7,
                "Effect": "Allow",
                "Principal": "*",
                "Action": "s3:GetObject"
            }]})
            .to_string(),
        ),
        (
            "a Condition block that is not an object",
            json!({"Statement": [{
                "Effect": "Allow",
                "Principal": "*",
                "Action": "s3:GetObject",
                "Condition": "StringEquals"
            }]})
            .to_string(),
        ),
        (
            "a statement carrying both Action and NotAction",
            json!({"Statement": [{
                "Effect": "Allow",
                "Principal": "*",
                "Action": "s3:GetObject",
                "NotAction": "s3:PutObject"
            }]})
            .to_string(),
        ),
        (
            // The ceilings belong to the shared parser, which publishes them.
            "a document past MAX_DOCUMENT_BYTES",
            json!({"Statement": [{
                "Effect": "Allow",
                "Principal": "*",
                "Action": format!("s3:Get{}", "x".repeat(MAX_DOCUMENT_BYTES))
            }]})
            .to_string(),
        ),
        (
            "a document past MAX_STATEMENTS",
            json!({
                "Statement": (0..=MAX_STATEMENTS)
                    .map(|index| json!({
                        "Sid": format!("s{index}"),
                        "Effect": "Allow",
                        "Principal": {"AWS": format!("arn:aws:iam::{KNOWN_ACCOUNT}:root")},
                        "Action": "s3:GetObject"
                    }))
                    .collect::<Vec<Value>>()
            })
            .to_string(),
        ),
        (
            "a statement carrying both Principal and NotPrincipal",
            json!({"Statement": [{
                "Effect": "Allow",
                "Principal": "*",
                "NotPrincipal": {"AWS": "arn:aws:iam::111122223333:root"}
            }]})
            .to_string(),
        ),
    ];

    for (case, document) in cases {
        let reply = check_raw(&emulator, &document, UNCITED_RESOURCE_TYPE).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            reply.body
        );
        assert!(
            reply
                .member("message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("policyDocument")),
            "case {case:?}: the refusal must name the parameter that is wrong, got {:?}",
            reply.body
        );
    }
}

/// A condition operator the shared engine does not model is answered with the modelled
/// 422 `UnprocessableEntityException` — whether such a clause binds the caller is exactly
/// what this check would otherwise have to guess — never with a `PASS` or a `FAIL`.
#[tokio::test]
async fn an_unmodelled_condition_operator_is_not_guessed() {
    let emulator = Emulator::new();
    for condition in [
        json!({"IpAddress": {"aws:SourceIp": "203.0.113.0/24"}}),
        json!({"NotIpAddress": {"aws:SourceIp": "203.0.113.0/24"}}),
        json!({"BinaryEquals": {"aws:PrincipalAccount": "MTExMTIyMjIzMzMz"}}),
        json!({"ForAllValues:StringEquals": {"aws:PrincipalAccount": "111122223333"}}),
        json!({"ForAnyValue:StringEquals": {"aws:PrincipalOrgPaths": "o-exampleorgid/r-ab12/"}}),
    ] {
        let case = condition.to_string();
        let reply = check(
            &emulator,
            &wildcard_with_condition(condition),
            RECORDED_RESOURCE_TYPE,
        )
        .await;
        assert_eq!(
            reply.status, 422,
            "case {case:?}: expected HTTP 422, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.code(),
            "UnprocessableEntityException",
            "case {case:?}: expected UnprocessableEntityException, got {:?}",
            reply.body
        );
        assert!(
            reply.member("result").is_none(),
            "case {case:?}: a refusal carries no verdict, got {:?}",
            reply.body
        );
    }
}

/// The check reports; it changes nothing. Repeating it answers identically, and nothing
/// a later call reads has moved.
#[tokio::test]
async fn the_check_reports_without_changing_anything() {
    let emulator = Emulator::new();
    let policy = json!({
        "Statement": [{"Sid": "Open", "Effect": "Allow", "Principal": "*", "Action": "*"}]
    });

    let before = emulator.send("GET", "/analyzer", None).await;
    let first = check(&emulator, &policy, RECORDED_RESOURCE_TYPE).await;
    let second = check(&emulator, &policy, RECORDED_RESOURCE_TYPE).await;
    let after = emulator.send("GET", "/analyzer", None).await;

    assert_eq!(
        first.body, second.body,
        "case {:?}: two identical checks must answer identically, got {:?} then {:?}",
        "repeated check", first.body, second.body
    );
    assert_eq!(
        before.body, after.body,
        "case {:?}: a check must leave the service's state alone, got {:?} then {:?}",
        "listing around a check", before.body, after.body
    );
}
