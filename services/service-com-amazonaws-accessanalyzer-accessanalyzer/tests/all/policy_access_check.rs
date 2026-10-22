// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `CheckAccessNotGranted`, driven over the wire.
//!
//! Every case goes through the real router: an `http::Request` is dispatched to the
//! mounted service, so restJson1 routing, boundary validation, the handler and response
//! serialization all run exactly as they do in the emulator binary.
//!
//! The rule under test: the check answers `FAIL` when at least one statement of the
//! policy may grant one of the requested (action, resource) permissions and no
//! unconditional `Deny` covering the whole request rules it out, and `PASS` otherwise.
//! The tables below span what that rule generalises over — which dimension the request
//! names, wildcards on either side, case, the positive and negated forms of each
//! statement member, `Deny` and `Condition`, both policy types, and statement count —
//! and the recorded `s3:GetObject` identity-policy case is one row.

use serde_json::{json, Value};

// The wire harness is shared by every suite in this target; see `tests/all/harness.rs`.
use crate::harness::{Emulator, Reply};

/// The URI this operation is bound to, from
/// `smithy_query(query="operation", operation="CheckAccessNotGranted")`'s
/// `http: {method: POST, uri: /policy/check-access-not-granted}`.
const CHECK_PATH: &str = "/policy/check-access-not-granted";

/// The `message` AWS reports with each `result`: ONE fixed sentence per result, naming
/// the dimensions the request's `access` object probed (see [`probed_phrase`]) and
/// nothing else from the request. The recorded response to an actions-only request says
/// "the listed actions"; the model's `smithy.api#examples` for this operation say "the
/// listed actions or resources". The two sentences are deliberately not parallel (only
/// the FAIL wording says "one or more of"). Verify with
/// `grep 'one or more of the listed actions or resources' smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json`.
fn message_fail(probed: &str) -> String {
    format!("The policy document grants access to perform one or more of the listed {probed}.")
}

/// See [`message_fail`].
fn message_pass(probed: &str) -> String {
    format!("The policy document does not grant access to perform the listed {probed}.")
}

/// The noun phrase the answer to this `access` list names its probed dimensions with: the
/// three modes the `access` member's own documentation defines ("If only actions are
/// specified … If only resources are specified … If both actions and resources are
/// specified …", `smithy_query(query="operation", operation="CheckAccessNotGranted")`).
fn probed_phrase(access: &Value) -> &'static str {
    let named = |member: &str| {
        access.as_array().is_some_and(|objects| {
            objects.iter().any(|object| {
                object
                    .get(member)
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.is_empty())
            })
        })
    };
    match (named("actions"), named("resources")) {
        (true, false) => "actions",
        (false, true) => "resources",
        // Naming neither is refused rather than answered, so the remaining case is a
        // request that named both.
        _ => "actions or resources",
    }
}

/// The `reasons[].description` AWS reports for a statement that may grant the access: the
/// canonical finding for the probed dimensions, then the statement named by its `Sid` when
/// it carries one and by its index when it does not — the `sid` form is the model's
/// examples' (`grep 'in the statement with sid: ' smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json`),
/// the `index` form the recorded response's.
fn expected_description(probed: &str, statement_id: Option<&str>, statement_index: i64) -> String {
    let reference = match statement_id {
        Some(sid) => format!("with sid: {sid}"),
        None => format!("with index: {statement_index}"),
    };
    format!("One or more of the listed {probed} in the statement {reference}.")
}

/// Actions used by the sweeps. Any string usable as an action in an IAM policy is a
/// legal value — see the `actions` member's doc in
/// `smithy_query(query="shape", shape="Access")` — so these are six unrelated ones, from
/// services the issue's recording never mentions as well as the one it does.
const ACTION_CATALOG: &[&str] = &[
    "s3:GetObject",
    "s3:PutObject",
    "sqs:SendMessage",
    "kms:Decrypt",
    "iam:PassRole",
    "dynamodb:Query",
];

/// Call `CheckAccessNotGranted` with `policy` as its `policyDocument` (serialized the
/// way a client sends it: a JSON document inside a string member).
async fn check(emulator: &Emulator, policy_type: &str, policy: &Value, access: Value) -> Reply {
    let body = json!({
        "policyDocument": policy.to_string(),
        "policyType": policy_type,
        "access": access,
    });
    emulator.send("POST", CHECK_PATH, Some(body)).await
}

/// The same call with the `policyDocument` passed through verbatim, for the rows whose
/// document is not valid JSON at all.
async fn check_raw(emulator: &Emulator, policy_type: &str, policy: &str, access: Value) -> Reply {
    let body = json!({
        "policyDocument": policy,
        "policyType": policy_type,
        "access": access,
    });
    emulator.send("POST", CHECK_PATH, Some(body)).await
}

/// The `statementIndex` of every reason the reply carries, in the order reported.
fn reported_statements(reply: &Reply) -> Vec<i64> {
    reply
        .member("reasons")
        .and_then(Value::as_array)
        .map(|reasons| {
            reasons
                .iter()
                .map(|reason| {
                    reason
                        .get("statementIndex")
                        .and_then(Value::as_i64)
                        .unwrap_or(-1)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the evaluation table.
#[derive(Debug)]
struct Case {
    /// What this row varies, named in every assertion.
    case: &'static str,
    policy_type: &'static str,
    policy: Value,
    access: Value,
    /// The `result` the check must answer.
    result: &'static str,
    /// The `statementIndex` of every statement the reply must point at, in order. Empty
    /// for a `PASS`, which points at none.
    reasons: Vec<i64>,
}

/// An identity policy allowing exactly one action on one resource — the recorded case's
/// document, and the base several rows vary from.
fn allow_get_object() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Action": ["s3:GetObject"],
            "Resource": ["arn:aws:s3:::demo/*"],
        }],
    })
}

#[tokio::test]
async fn the_check_answers_from_the_policy_document() {
    let cases = vec![
        Case {
            case: "the recorded case: the requested action is allowed",
            policy_type: "IDENTITY_POLICY",
            policy: allow_get_object(),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "the recorded case: the requested action is absent",
            policy_type: "IDENTITY_POLICY",
            policy: allow_get_object(),
            access: json!([{"actions": ["s3:DeleteObject"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "a policy wildcard covers the requested action",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "Resource": "*"},
            ]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "a requested wildcard meets a narrower policy action",
            policy_type: "IDENTITY_POLICY",
            policy: allow_get_object(),
            access: json!([{"actions": ["s3:*"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "a single-character wildcard in the policy action",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:GetObjec?", "Resource": "*"},
            ]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "actions are matched case-insensitively",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:getobject", "Resource": "*"},
            ]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "an action of another service entirely",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "Resource": "*"},
            ]}),
            access: json!([{"actions": ["sqs:SendMessage"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "a requested action wildcard that meets nothing in the policy",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "sqs:*", "Resource": "*"},
            ]}),
            access: json!([{"actions": ["s3:*"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "resources only: the resource is in the policy",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "Resource": "arn:aws:s3:::demo/*"},
            ]}),
            access: json!([{"resources": ["arn:aws:s3:::demo/key"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "resources only: a resource outside the policy",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "Resource": "arn:aws:s3:::demo/*"},
            ]}),
            access: json!([{"resources": ["arn:aws:s3:::other/key"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "resource ARNs are matched case-sensitively",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "Resource": "arn:aws:s3:::Demo/*"},
            ]}),
            access: json!([{"resources": ["arn:aws:s3:::demo/key"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "both dimensions match",
            policy_type: "IDENTITY_POLICY",
            policy: allow_get_object(),
            access: json!([{
                "actions": ["s3:GetObject"],
                "resources": ["arn:aws:s3:::demo/key"],
            }]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "both dimensions named, only the action matches",
            policy_type: "IDENTITY_POLICY",
            policy: allow_get_object(),
            access: json!([{
                "actions": ["s3:GetObject"],
                "resources": ["arn:aws:s3:::other/key"],
            }]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "NotAction excludes the requested action",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "NotAction": "s3:GetObject", "Resource": "*"},
            ]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "NotAction leaves every other action allowed",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "NotAction": "s3:GetObject", "Resource": "*"},
            ]}),
            access: json!([{"actions": ["s3:PutObject"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "NotResource excludes the requested resource",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "NotResource": "arn:aws:s3:::demo/*"},
            ]}),
            access: json!([{"resources": ["arn:aws:s3:::demo/key"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "NotResource leaves every other resource allowed",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "NotResource": "arn:aws:s3:::demo/*"},
            ]}),
            access: json!([{"resources": ["arn:aws:s3:::other/key"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "an unconditional Deny rules the allowed access out",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "Resource": "*"},
                {"Effect": "Deny", "Action": "s3:GetObject", "Resource": "*"},
            ]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "a conditional Deny does not rule the access out",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "Resource": "*"},
                {
                    "Effect": "Deny",
                    "Action": "s3:GetObject",
                    "Resource": "*",
                    "Condition": {"Bool": {"aws:SecureTransport": "false"}},
                },
            ]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "a Deny narrower than the request does not rule it out",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "Resource": "*"},
                {"Effect": "Deny", "Action": "s3:GetObject", "Resource": "arn:aws:s3:::demo/secret*"},
            ]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "a conditional Allow may still grant the access",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [{
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "Resource": "*",
                "Condition": {"StringEquals": {"aws:PrincipalTag/team": "admins"}},
            }]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "a policy whose only statement is a Deny grants nothing",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Deny", "Action": "s3:*", "Resource": "*"},
            ]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "only the granting statement among several is reported",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Sid": "AllowQueues", "Effect": "Allow", "Action": "sqs:*", "Resource": "*"},
                {"Sid": "AllowRead", "Effect": "Allow", "Action": "s3:Get*", "Resource": "*"},
                {"Sid": "AllowKeys", "Effect": "Allow", "Action": "kms:Decrypt", "Resource": "*"},
            ]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "FAIL",
            reasons: vec![1],
        },
        Case {
            case: "two granting statements are both reported, in document order",
            policy_type: "IDENTITY_POLICY",
            policy: json!({"Statement": [
                {"Effect": "Allow", "Action": "s3:*", "Resource": "*"},
                {"Effect": "Allow", "Action": "s3:GetObject", "Resource": "arn:aws:s3:::demo/*"},
            ]}),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "FAIL",
            reasons: vec![0, 1],
        },
        Case {
            case: "a single statement given as an object rather than a list",
            policy_type: "IDENTITY_POLICY",
            policy: json!({
                "Statement": {"Effect": "Allow", "Action": "s3:GetObject", "Resource": "*"},
            }),
            access: json!([{"actions": ["s3:GetObject"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "a resource policy naming no Resource is about the attached resource",
            policy_type: "RESOURCE_POLICY",
            policy: json!({"Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": "*"},
                "Action": "sqs:SendMessage",
            }]}),
            access: json!([{
                "actions": ["sqs:SendMessage"],
                "resources": ["arn:aws:sqs:us-east-1:111122223333:queue1"],
            }]),
            result: "FAIL",
            reasons: vec![0],
        },
        Case {
            case: "a resource policy whose Resource is a different queue",
            policy_type: "RESOURCE_POLICY",
            policy: json!({"Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                "Action": "sqs:*",
                "Resource": "arn:aws:sqs:us-east-1:111122223333:queue1",
            }]}),
            access: json!([{"resources": ["arn:aws:sqs:us-east-1:111122223333:queue2"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "a resource policy Deny naming no Resource rules the access out",
            policy_type: "RESOURCE_POLICY",
            policy: json!({"Statement": [
                {
                    "Effect": "Allow",
                    "Principal": {"AWS": "*"},
                    "Action": "sqs:*",
                    "Resource": "arn:aws:sqs:us-east-1:111122223333:queue1",
                },
                {
                    "Effect": "Deny",
                    "Principal": {"AWS": "*"},
                    "Action": "sqs:SendMessage",
                },
            ]}),
            access: json!([{"actions": ["sqs:SendMessage"]}]),
            result: "PASS",
            reasons: vec![],
        },
        Case {
            case: "a resource policy that grants the requested action",
            policy_type: "RESOURCE_POLICY",
            policy: json!({"Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                "Action": "sqs:*",
                "Resource": "arn:aws:sqs:us-east-1:111122223333:queue1",
            }]}),
            access: json!([{"actions": ["sqs:ReceiveMessage"]}]),
            result: "FAIL",
            reasons: vec![0],
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let reply = check(
            &emulator,
            case.policy_type,
            &case.policy,
            case.access.clone(),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected HTTP 200, got {} ({:?})",
            case.case, reply.status, reply.body
        );
        let result = reply.member("result").and_then(Value::as_str);
        assert_eq!(
            result,
            Some(case.result),
            "case {:?}: expected result {:?}, got {:?}",
            case.case,
            case.result,
            reply.body
        );
        let probed = probed_phrase(&case.access);
        let expected_message = if case.result == "FAIL" {
            message_fail(probed)
        } else {
            message_pass(probed)
        };
        assert_eq!(
            reply.member("message").and_then(Value::as_str),
            Some(expected_message.as_str()),
            "case {:?}: expected the documented {} message, got {:?}",
            case.case,
            case.result,
            reply.member("message")
        );
        // Every reason reports the canonical finding and the statement it blames — never
        // prose about what that statement contains.
        for reason in reply
            .member("reasons")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
        {
            let statement_index = reason
                .get("statementIndex")
                .and_then(Value::as_i64)
                .unwrap_or(-1);
            let statement_id = reason.get("statementId").and_then(Value::as_str);
            assert_eq!(
                reason.get("description").and_then(Value::as_str),
                Some(expected_description(probed, statement_id, statement_index).as_str()),
                "case {:?}: expected the canonical reason description, got {reason:?}",
                case.case
            );
        }
        assert_eq!(
            reported_statements(&reply),
            case.reasons,
            "case {:?}: expected reasons for statements {:?}, got {:?}",
            case.case,
            case.reasons,
            reply.member("reasons")
        );
    }
}

/// One row of the refusal table.
#[derive(Debug)]
struct RefusalCase {
    case: &'static str,
    policy_type: &'static str,
    /// The `policyDocument` exactly as the request carries it.
    policy: &'static str,
    access: Value,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_document_that_is_not_an_iam_policy_is_refused() {
    // A document that is not JSON is a malformed parameter (400
    // `InvalidParameterException`); one that is JSON but breaks the IAM policy grammar is
    // an entity the check cannot process (422 `UnprocessableEntityException`); a request
    // asking about no permission at all is a field refusal (400 `ValidationException`).
    let cases = vec![
        RefusalCase {
            case: "the document is not JSON",
            policy_type: "IDENTITY_POLICY",
            policy: "this is not a policy",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "the document is truncated JSON",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Statement\": [",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 400,
            code: "InvalidParameterException",
        },
        RefusalCase {
            case: "the document is a JSON array rather than an object",
            policy_type: "IDENTITY_POLICY",
            policy: "[]",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "the document carries no Statement",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Version\": \"2012-10-17\"}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "a statement carries no Effect",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Statement\": [{\"Action\": \"s3:*\", \"Resource\": \"*\"}]}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "a statement's Effect is neither Allow nor Deny",
            policy_type: "IDENTITY_POLICY",
            policy:
                "{\"Statement\": [{\"Effect\": \"Permit\", \"Action\": \"s3:*\", \"Resource\": \"*\"}]}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "a statement names neither Action nor NotAction",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Statement\": [{\"Effect\": \"Allow\", \"Resource\": \"*\"}]}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "a statement names both Action and NotAction",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Statement\": [{\"Effect\": \"Allow\", \"Action\": \"s3:*\", \"NotAction\": \"s3:GetObject\", \"Resource\": \"*\"}]}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "an identity policy statement names no resource",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Statement\": [{\"Effect\": \"Allow\", \"Action\": \"s3:*\"}]}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "an identity policy statement names a principal",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Statement\": [{\"Effect\": \"Allow\", \"Principal\": {\"AWS\": \"*\"}, \"Action\": \"s3:*\", \"Resource\": \"*\"}]}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "a statement's Action is neither a string nor a list of strings",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Statement\": [{\"Effect\": \"Allow\", \"Action\": 7, \"Resource\": \"*\"}]}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "a policy whose Statement list is empty",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Version\": \"2012-10-17\", \"Statement\": []}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "a statement's Action list is empty",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Statement\": [{\"Effect\": \"Allow\", \"Action\": [], \"Resource\": \"*\"}]}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "a statement's Resource list is empty",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Statement\": [{\"Effect\": \"Allow\", \"Action\": \"s3:*\", \"Resource\": []}]}",
            access: json!([{"actions": ["s3:GetObject"]}]),
            status: 422,
            code: "UnprocessableEntityException",
        },
        RefusalCase {
            case: "the access names neither actions nor resources",
            policy_type: "IDENTITY_POLICY",
            policy: "{\"Statement\": [{\"Effect\": \"Allow\", \"Action\": \"s3:*\", \"Resource\": \"*\"}]}",
            access: json!([{}]),
            status: 400,
            code: "ValidationException",
        },
    ];

    let emulator = Emulator::new();
    for case in &cases {
        let reply = check_raw(
            &emulator,
            case.policy_type,
            case.policy,
            case.access.clone(),
        )
        .await;
        assert_refused(&reply, case.case, case.status, case.code);
    }
}

/// Every status/code pair asserted below is one the model declares for this operation,
/// per `smithy_query(query="operation", operation="CheckAccessNotGranted")`.
fn assert_refused(reply: &Reply, case: &str, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} ({:?})",
        reply.status, reply.body
    );
    assert_eq!(
        reply.code(),
        code,
        "case {case:?}: expected error code {code:?}, got {:?}",
        reply.body
    );
    let message = reply
        .member("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !message.is_empty(),
        "case {case:?}: expected a message saying what to fix, got {:?}",
        reply.body
    );
}

/// A document or request larger than the operation evaluates is refused BEFORE the
/// matching starts, so no single request can grow the emulator's work without bound.
///
/// The caps are the model's own where it has them (`access` at 1 entry, `actions` and
/// `resources` at 100 each) and the emulator's own, named constants where it has none
/// (document bytes, pattern characters, statement count) — each row asks for exactly one
/// more than the cap allows.
#[tokio::test]
async fn work_past_the_evaluation_bounds_is_refused() {
    let emulator = Emulator::new();
    let valid = "{\"Statement\": [{\"Effect\": \"Allow\", \"Action\": \"s3:*\", \"Resource\": \"*\"}]}";
    // The ceilings, as the code names them: `intersect::MAX_PATTERN_CHARS` (1024) in this
    // operation, and the shared parser's `MAX_STATEMENTS` (256), `MAX_ELEMENT_VALUES`
    // (512) and `MAX_DOCUMENT_BYTES` (65536) in
    // `crate::ops::check_no_new_access::policy`.
    let long_pattern = format!("s3:{}", "a".repeat(1022));
    let many_actions: Vec<String> = (0..=100).map(|n| format!("s3:Action{n}")).collect();
    let many_statements: Vec<Value> = (0..=256)
        .map(|n| json!({"Effect": "Allow", "Action": format!("s3:Action{n}"), "Resource": "*"}))
        .collect();
    let padded_sid = "p".repeat(65_536);

    let cases: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            "the access list is empty",
            valid.to_string(),
            json!([]),
            400,
            "ValidationException",
        ),
        (
            "more access objects than the model allows",
            valid.to_string(),
            json!([{"actions": ["s3:GetObject"]}, {"actions": ["s3:PutObject"]}]),
            400,
            "ValidationException",
        ),
        (
            "more actions than the model allows",
            valid.to_string(),
            json!([{"actions": many_actions}]),
            400,
            "ValidationException",
        ),
        (
            "a requested action longer than the matcher evaluates",
            valid.to_string(),
            json!([{"actions": [format!("{long_pattern}x")]}]),
            400,
            "ValidationException",
        ),
        (
            "a requested resource longer than the matcher evaluates",
            valid.to_string(),
            json!([{"resources": [format!("{long_pattern}x")]}]),
            400,
            "ValidationException",
        ),
        (
            "a policy pattern longer than the matcher evaluates",
            json!({"Statement": [
                {"Effect": "Allow", "Action": format!("{long_pattern}x"), "Resource": "*"},
            ]})
            .to_string(),
            json!([{"actions": ["s3:GetObject"]}]),
            422,
            "UnprocessableEntityException",
        ),
        (
            "more statements than the parser evaluates",
            json!({"Statement": many_statements}).to_string(),
            json!([{"actions": ["s3:GetObject"]}]),
            422,
            "UnprocessableEntityException",
        ),
        (
            "a statement naming more values than the parser evaluates",
            json!({"Statement": [{
                "Effect": "Allow",
                "Action": (0..=512).map(|n| format!("s3:Action{n}")).collect::<Vec<String>>(),
                "Resource": "*",
            }]})
            .to_string(),
            json!([{"actions": ["s3:GetObject"]}]),
            422,
            "UnprocessableEntityException",
        ),
        (
            "a document larger than the parser evaluates",
            json!({"Statement": [
                {"Sid": padded_sid, "Effect": "Allow", "Action": "s3:*", "Resource": "*"},
            ]})
            .to_string(),
            json!([{"actions": ["s3:GetObject"]}]),
            422,
            "UnprocessableEntityException",
        ),
    ];

    for (case, policy, access, status, code) in &cases {
        let reply = check_raw(&emulator, "IDENTITY_POLICY", policy, access.clone()).await;
        assert_refused(&reply, case, *status, code);
    }

    // A request AT the caps is still answered, so the bounds refuse only what is past
    // them: 100 actions and 100 resources of the length real ones have, against a
    // document of 100 statements whose resource patterns are full ARNs, with the granting
    // action last so the whole document is evaluated. (The statement count is well under
    // the parser's ceiling to keep the suite quick; the matching budget is sized for the
    // full product, see `intersect::MAX_MATCH_CELLS`.)
    let at_the_cap: Vec<String> = (0..100).map(|n| format!("s3:Action{n}")).collect();
    let resources: Vec<String> = (0..100)
        .map(|n| format!("arn:aws:s3:us-east-1:111122223333:demo-bucket-{n}/some/longer/key/{n}"))
        .collect();
    let statements: Vec<Value> = (0..99)
        .map(|n| {
            json!({
                "Effect": "Allow",
                "Action": format!("sqs:Action{n}"),
                "Resource": format!("arn:aws:sqs:us-east-1:111122223333:demo-queue-{n}/some/longer/path/{n}"),
            })
        })
        .chain(std::iter::once(json!({
            "Effect": "Allow",
            "Action": "s3:Action99",
            "Resource": "arn:aws:s3:us-east-1:111122223333:other-bucket-99/some/longer/key/99",
        })))
        .collect();
    let reply = check_raw(
        &emulator,
        "IDENTITY_POLICY",
        &json!({"Statement": statements}).to_string(),
        json!([{"actions": at_the_cap, "resources": resources}]),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected HTTP 200, got {} ({:?})",
        "a request at every cap", reply.status, reply.body
    );
    assert_eq!(
        reply.member("result").and_then(Value::as_str),
        Some("PASS"),
        "case {:?}: the statements grant none of the requested permissions, got {:?}",
        "a request at every cap",
        reply.body
    );
}

#[tokio::test]
async fn a_failing_check_points_at_the_statement_that_grants_the_access() {
    let emulator = Emulator::new();
    let policy = json!({"Statement": [
        {"Sid": "AllowQueues", "Effect": "Allow", "Action": "sqs:*", "Resource": "*"},
        {"Sid": "AllowRead", "Effect": "Allow", "Action": "s3:Get*", "Resource": "arn:aws:s3:::demo/*"},
    ]});

    let reply = check(
        &emulator,
        "IDENTITY_POLICY",
        &policy,
        json!([{"actions": ["s3:GetObject"], "resources": ["arn:aws:s3:::demo/key"]}]),
    )
    .await;
    let reasons = reply
        .member("reasons")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        reasons.len(),
        1,
        "case {:?}: expected exactly one reason, got {:?}",
        "the second statement grants the access",
        reply.body
    );
    let reason = &reasons[0];
    assert_eq!(
        reason.get("statementIndex").and_then(Value::as_i64),
        Some(1),
        "case {:?}: expected statementIndex 1, got {reason:?}",
        "the second statement grants the access"
    );
    assert_eq!(
        reason.get("statementId").and_then(Value::as_str),
        Some("AllowRead"),
        "case {:?}: expected the statement's Sid as statementId, got {reason:?}",
        "the second statement grants the access"
    );
    let description = reason
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert_eq!(
        description,
        expected_description("actions or resources", Some("AllowRead"), 1),
        "case {:?}: expected the canonical reason naming the statement's Sid, got {description:?}",
        "the second statement grants the access"
    );

    // A statement without a `Sid` has no identifier to report, and a PASS points at no
    // statement at all.
    let anonymous = json!({"Statement": [
        {"Effect": "Allow", "Action": "s3:Get*", "Resource": "*"},
    ]});
    let reply = check(
        &emulator,
        "IDENTITY_POLICY",
        &anonymous,
        json!([{"actions": ["s3:GetObject"]}]),
    )
    .await;
    let reason = reply
        .member("reasons")
        .and_then(Value::as_array)
        .and_then(|reasons| reasons.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert!(
        reason
            .get("statementId")
            .is_none_or(|statement_id| statement_id.is_null()),
        "case {:?}: expected no statementId, got {reason:?}",
        "a granting statement without a Sid"
    );
    // With no `Sid` to name, the reason names the statement's index — the recorded form.
    assert_eq!(
        reason.get("description").and_then(Value::as_str),
        Some(expected_description("actions", None, 0).as_str()),
        "case {:?}: expected the canonical reason naming the statement's index, got {reason:?}",
        "a granting statement without a Sid"
    );

    let reply = check(
        &emulator,
        "IDENTITY_POLICY",
        &anonymous,
        json!([{"actions": ["s3:PutObject"]}]),
    )
    .await;
    assert!(
        reply.member("reasons").is_none(),
        "case {:?}: expected no reasons on a PASS, got {:?}",
        "nothing grants the requested access",
        reply.body
    );
}

#[tokio::test]
async fn a_policy_allowing_one_action_fails_for_that_action_alone() {
    // The invariant, swept over the catalog: a policy whose single statement allows
    // exactly one action answers FAIL for that action and PASS for every other action in
    // the catalog — so the answer tracks the document, not the fact that a policy was
    // supplied. Each action is also asked about through a wildcard that covers only it.
    let emulator = Emulator::new();
    for allowed in ACTION_CATALOG {
        let policy = json!({"Statement": [
            {"Effect": "Allow", "Action": *allowed, "Resource": "*"},
        ]});
        for requested in ACTION_CATALOG {
            let expected = if requested == allowed { "FAIL" } else { "PASS" };
            let reply = check(
                &emulator,
                "IDENTITY_POLICY",
                &policy,
                json!([{"actions": [*requested]}]),
            )
            .await;
            assert_eq!(
                reply.member("result").and_then(Value::as_str),
                Some(expected),
                "case {:?}: expected {expected}, got {:?}",
                format!("allow {allowed}, ask about {requested}"),
                reply.body
            );
        }

        // `<action>*` covers the allowed action and nothing else in the catalog, so the
        // wildcard request must answer exactly as the exact one did.
        let reply = check(
            &emulator,
            "IDENTITY_POLICY",
            &policy,
            json!([{"actions": [format!("{allowed}*")]}]),
        )
        .await;
        assert_eq!(
            reply.member("result").and_then(Value::as_str),
            Some("FAIL"),
            "case {:?}: expected FAIL, got {:?}",
            format!("allow {allowed}, ask about {allowed}*"),
            reply.body
        );
    }
}
