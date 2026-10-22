// SPDX-License-Identifier: CC0-1.0

//! The TEXT the three policy checks report, driven over the wire.
//!
//! The rule under test: a policy check reports its finding as the service's fixed short
//! sentence for that check — `CheckAccessNotGranted` naming the dimensions the request's
//! `access` object probed, which are the three modes its own documentation defines — and
//! each `reasons[].description` is that sentence followed by a reference to the offending
//! statement: `with sid: <Sid>` when the statement carries one and `with index:
//! <statementIndex>` when it does not. No part of the text is generated prose about what
//! the statement contains.
//!
//! The sentences are AWS's own: the `sid` form and the "actions or resources" mode come
//! from the model's `smithy.api#examples` for `CheckAccessNotGranted` and
//! `CheckNoPublicAccess` (`grep 'in the statement with sid: '
//! smithy-model/com-amazonaws-accessanalyzer-accessanalyzer.json`), the `index` form and
//! the actions-only mode from the recorded responses. The table below spans what the rule
//! generalises over: which check answers, which dimensions the request probes, whether the
//! blamed statement carries a `Sid`, which position it holds, how many statements are
//! blamed at once, and both results — the recorded actions-only/no-`Sid` case is one row.

use serde_json::{json, Value};

use crate::harness::{Emulator, Reply};

/// The three routes, from `smithy_query(query="digest")`'s `http` bindings.
const CHECK_ACCESS_NOT_GRANTED: &str = "/policy/check-access-not-granted";
const CHECK_NO_NEW_ACCESS: &str = "/policy/check-no-new-access";
const CHECK_NO_PUBLIC_ACCESS: &str = "/policy/check-no-public-access";

/// A resource type from the model's `AccessCheckResourceType` enum.
const RESOURCE_TYPE: &str = "AWS::S3::Bucket";

/// How every policy-check reason names the statement it blames.
fn statement_reference(statement_id: Option<&str>, statement_index: i64) -> String {
    match statement_id {
        Some(sid) => format!("with sid: {sid}"),
        None => format!("with index: {statement_index}"),
    }
}

/// `CheckAccessNotGranted`'s reason: the listed permissions in the probed dimensions, and
/// the statement that may grant one of them.
fn access_not_granted_reason(probed: &str, sid: Option<&str>, index: i64) -> String {
    format!(
        "One or more of the listed {probed} in the statement {}.",
        statement_reference(sid, index)
    )
}

/// `CheckAccessNotGranted`'s `message` for each result, naming the probed dimensions.
fn access_not_granted_message(probed: &str, result: &str) -> String {
    if result == "FAIL" {
        format!("The policy document grants access to perform one or more of the listed {probed}.")
    } else {
        format!("The policy document does not grant access to perform the listed {probed}.")
    }
}

/// `CheckNoNewAccess`'s reason.
fn no_new_access_reason(sid: Option<&str>, index: i64) -> String {
    format!(
        "New access in the statement {}.",
        statement_reference(sid, index)
    )
}

/// `CheckNoPublicAccess`'s reason.
fn no_public_access_reason(sid: Option<&str>, index: i64) -> String {
    format!(
        "Public access granted in the following statement {}.",
        statement_reference(sid, index)
    )
}

/// One reason the answer must carry: the statement it names, and the exact text.
#[derive(Debug)]
struct ExpectedReason {
    index: i64,
    sid: Option<&'static str>,
    description: String,
}

/// One row of the table: a request, and the text the rule demands of the answer.
#[derive(Debug)]
struct Row {
    case: &'static str,
    path: &'static str,
    request: Value,
    /// The `message`, where the rule fixes its wording for this check; `None` asserts
    /// only that the required sentence is there.
    message: Option<String>,
    /// Every reason, in report order. Empty means a `PASS`, which blames no statement.
    reasons: Vec<ExpectedReason>,
}

/// An identity policy whose single statement allows `action` on `resource`, with an
/// optional `Sid`.
fn identity_allow(sid: Option<&str>, action: &str, resource: &str) -> Value {
    let mut statement = json!({"Effect": "Allow", "Action": action, "Resource": resource});
    if let Some(sid) = sid {
        statement["Sid"] = json!(sid);
    }
    json!({"Version": "2012-10-17", "Statement": [statement]})
}

/// A `CheckAccessNotGranted` request body.
fn access_request(policy: &Value, access: Value) -> Value {
    json!({
        "policyDocument": policy.to_string(),
        "policyType": "IDENTITY_POLICY",
        "access": access,
    })
}

/// A `CheckNoNewAccess` request body.
fn new_access_request(existing: &Value, updated: &Value) -> Value {
    json!({
        "existingPolicyDocument": existing.to_string(),
        "newPolicyDocument": updated.to_string(),
        "policyType": "IDENTITY_POLICY",
    })
}

/// A `CheckNoPublicAccess` request body.
fn public_access_request(policy: &Value) -> Value {
    json!({"policyDocument": policy.to_string(), "resourceType": RESOURCE_TYPE})
}

/// A resource-policy statement admitting every principal, with an optional `Sid`.
fn public_statement(sid: Option<&str>, action: &str) -> Value {
    let mut statement = json!({"Effect": "Allow", "Principal": "*", "Action": action});
    if let Some(sid) = sid {
        statement["Sid"] = json!(sid);
    }
    statement
}

/// Assert one row end to end.
async fn assert_row(emulator: &Emulator, row: &Row) {
    let reply: Reply = emulator
        .send("POST", row.path, Some(row.request.clone()))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected HTTP 200, got {} with body {:?}",
        row.case, reply.status, reply.body
    );
    // The result and the reasons never disagree: a check that blames a statement fails.
    let expected_result = if row.reasons.is_empty() { "PASS" } else { "FAIL" };
    assert_eq!(
        reply.member("result").and_then(Value::as_str),
        Some(expected_result),
        "case {:?}: expected result {expected_result:?}, got {:?}",
        row.case,
        reply.body
    );
    let message = reply
        .member("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match &row.message {
        Some(expected) => assert_eq!(
            &message, expected,
            "case {:?}: expected the message {expected:?}, got {message:?}",
            row.case
        ),
        None => assert!(
            !message.is_empty(),
            "case {:?}: expected a message stating the finding, got {:?}",
            row.case,
            reply.body
        ),
    }
    let reasons = reply
        .member("reasons")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if row.reasons.is_empty() {
        assert!(
            reply.member("reasons").is_none(),
            "case {:?}: a check that blames no statement carries no reasons, got {:?}",
            row.case,
            reply.body
        );
        return;
    }
    assert_eq!(
        reasons.len(),
        row.reasons.len(),
        "case {:?}: expected {} reason(s), got {:?}",
        row.case,
        row.reasons.len(),
        reply.body
    );
    for (reason, expected) in reasons.iter().zip(&row.reasons) {
        assert_eq!(
            reason.get("statementIndex").and_then(Value::as_i64),
            Some(expected.index),
            "case {:?}: expected statementIndex {}, got {reason:?}",
            row.case,
            expected.index
        );
        assert_eq!(
            reason.get("statementId").and_then(Value::as_str),
            expected.sid,
            "case {:?}: expected statementId {:?}, got {reason:?}",
            row.case,
            expected.sid
        );
        assert_eq!(
            reason.get("description").and_then(Value::as_str),
            Some(expected.description.as_str()),
            "case {:?}: expected the description {:?}, got {reason:?}",
            row.case,
            expected.description
        );
    }
}

#[tokio::test]
async fn every_policy_check_reports_the_canonical_finding_and_statement_reference() {
    let emulator = Emulator::new();
    let bucket_objects = "arn:aws:s3:::demo/*";
    let bucket_key = "arn:aws:s3:::demo/key";
    let unrelated_policy = identity_allow(
        None,
        "logs:DescribeLogGroups",
        "arn:aws:logs:us-east-1:111122223333:log-group:*",
    );

    let rows = vec![
        // ---- CheckAccessNotGranted: the probed dimensions name the finding. ----
        Row {
            case: "the recorded case: actions only, a statement with no Sid",
            path: CHECK_ACCESS_NOT_GRANTED,
            request: access_request(
                &identity_allow(None, "s3:GetObject", bucket_objects),
                json!([{"actions": ["s3:GetObject"]}]),
            ),
            message: Some(access_not_granted_message("actions", "FAIL")),
            reasons: vec![ExpectedReason {
                index: 0,
                sid: None,
                description: access_not_granted_reason("actions", None, 0),
            }],
        },
        Row {
            case: "actions only, the blamed statement carries a Sid",
            path: CHECK_ACCESS_NOT_GRANTED,
            request: access_request(
                &identity_allow(Some("AllowRead"), "s3:GetObject", bucket_objects),
                json!([{"actions": ["s3:GetObject"]}]),
            ),
            message: Some(access_not_granted_message("actions", "FAIL")),
            reasons: vec![ExpectedReason {
                index: 0,
                sid: Some("AllowRead"),
                description: access_not_granted_reason("actions", Some("AllowRead"), 0),
            }],
        },
        Row {
            case: "resources only",
            path: CHECK_ACCESS_NOT_GRANTED,
            request: access_request(
                &identity_allow(None, "s3:*", bucket_objects),
                json!([{"resources": [bucket_key]}]),
            ),
            message: Some(access_not_granted_message("resources", "FAIL")),
            reasons: vec![ExpectedReason {
                index: 0,
                sid: None,
                description: access_not_granted_reason("resources", None, 0),
            }],
        },
        Row {
            case: "both dimensions, with a Sid",
            path: CHECK_ACCESS_NOT_GRANTED,
            request: access_request(
                &identity_allow(Some("AllowReads"), "s3:Get*", bucket_objects),
                json!([{"actions": ["s3:GetObject"], "resources": [bucket_key]}]),
            ),
            message: Some(access_not_granted_message("actions or resources", "FAIL")),
            reasons: vec![ExpectedReason {
                index: 0,
                sid: Some("AllowReads"),
                description: access_not_granted_reason(
                    "actions or resources",
                    Some("AllowReads"),
                    0,
                ),
            }],
        },
        Row {
            case: "an empty Sid identifies nothing, so the statement is named by its index",
            path: CHECK_ACCESS_NOT_GRANTED,
            request: access_request(
                &identity_allow(Some(""), "s3:GetObject", bucket_objects),
                json!([{"actions": ["s3:GetObject"]}]),
            ),
            message: Some(access_not_granted_message("actions", "FAIL")),
            reasons: vec![ExpectedReason {
                index: 0,
                sid: None,
                description: access_not_granted_reason("actions", None, 0),
            }],
        },
        Row {
            case: "two statements blamed at once, only the second with a Sid",
            path: CHECK_ACCESS_NOT_GRANTED,
            request: access_request(
                &json!({"Statement": [
                    {"Effect": "Allow", "Action": "s3:*", "Resource": "*"},
                    {"Sid": "AlsoRead", "Effect": "Allow", "Action": "s3:GetObject", "Resource": bucket_objects},
                ]}),
                json!([{"actions": ["s3:GetObject"]}]),
            ),
            message: Some(access_not_granted_message("actions", "FAIL")),
            reasons: vec![
                ExpectedReason {
                    index: 0,
                    sid: None,
                    description: access_not_granted_reason("actions", None, 0),
                },
                ExpectedReason {
                    index: 1,
                    sid: Some("AlsoRead"),
                    description: access_not_granted_reason("actions", Some("AlsoRead"), 1),
                },
            ],
        },
        Row {
            case: "a PASS names the probed dimensions too: actions only",
            path: CHECK_ACCESS_NOT_GRANTED,
            request: access_request(
                &identity_allow(None, "s3:GetObject", bucket_objects),
                json!([{"actions": ["s3:DeleteObject"]}]),
            ),
            message: Some(access_not_granted_message("actions", "PASS")),
            reasons: Vec::new(),
        },
        Row {
            case: "a PASS for resources only",
            path: CHECK_ACCESS_NOT_GRANTED,
            request: access_request(
                &identity_allow(None, "s3:*", bucket_objects),
                json!([{"resources": ["arn:aws:s3:::other/key"]}]),
            ),
            message: Some(access_not_granted_message("resources", "PASS")),
            reasons: Vec::new(),
        },
        Row {
            case: "a PASS for both dimensions",
            path: CHECK_ACCESS_NOT_GRANTED,
            request: access_request(
                &identity_allow(None, "s3:GetObject", bucket_objects),
                json!([{"actions": ["s3:GetObject"], "resources": ["arn:aws:s3:::other/key"]}]),
            ),
            message: Some(access_not_granted_message("actions or resources", "PASS")),
            reasons: Vec::new(),
        },
        // ---- CheckNoPublicAccess ----
        Row {
            case: "the recorded case: a public statement with no Sid",
            path: CHECK_NO_PUBLIC_ACCESS,
            request: public_access_request(&json!({
                "Version": "2012-10-17",
                "Statement": [public_statement(None, "s3:GetObject")],
            })),
            message: None,
            reasons: vec![ExpectedReason {
                index: 0,
                sid: None,
                description: no_public_access_reason(None, 0),
            }],
        },
        Row {
            case: "a public statement carrying a Sid",
            path: CHECK_NO_PUBLIC_ACCESS,
            request: public_access_request(&json!({
                "Statement": [public_statement(Some("PublicRead"), "s3:GetObject")],
            })),
            message: None,
            reasons: vec![ExpectedReason {
                index: 0,
                sid: Some("PublicRead"),
                description: no_public_access_reason(Some("PublicRead"), 0),
            }],
        },
        Row {
            case: "a public statement whose Sid is empty is named by its index",
            path: CHECK_NO_PUBLIC_ACCESS,
            request: public_access_request(&json!({
                "Statement": [public_statement(Some(""), "s3:GetObject")],
            })),
            message: None,
            reasons: vec![ExpectedReason {
                index: 0,
                sid: None,
                description: no_public_access_reason(None, 0),
            }],
        },
        Row {
            case: "the public statement is the second one, and has no Sid",
            path: CHECK_NO_PUBLIC_ACCESS,
            request: public_access_request(&json!({"Statement": [
                {
                    "Sid": "OnlyOneAccount",
                    "Effect": "Allow",
                    "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                    "Action": "s3:GetObject",
                },
                public_statement(None, "s3:PutObject"),
            ]})),
            message: None,
            reasons: vec![ExpectedReason {
                index: 1,
                sid: None,
                description: no_public_access_reason(None, 1),
            }],
        },
        Row {
            case: "a NotPrincipal statement is reported the same way",
            path: CHECK_NO_PUBLIC_ACCESS,
            request: public_access_request(&json!({"Statement": [{
                "Sid": "AllButOne",
                "Effect": "Allow",
                "NotPrincipal": {"AWS": "arn:aws:iam::111122223333:root"},
                "Action": "s3:GetObject",
            }]})),
            message: None,
            reasons: vec![ExpectedReason {
                index: 0,
                sid: Some("AllButOne"),
                description: no_public_access_reason(Some("AllButOne"), 0),
            }],
        },
        Row {
            case: "a policy granting no public access blames no statement",
            path: CHECK_NO_PUBLIC_ACCESS,
            request: public_access_request(&json!({"Statement": [{
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::111122223333:root"},
                "Action": "s3:GetObject",
            }]})),
            message: None,
            reasons: Vec::new(),
        },
        // ---- CheckNoNewAccess: the same statement reference. ----
        Row {
            case: "new access from a statement with no Sid",
            path: CHECK_NO_NEW_ACCESS,
            request: new_access_request(
                &unrelated_policy,
                &identity_allow(None, "s3:GetObject", bucket_objects),
            ),
            message: None,
            reasons: vec![ExpectedReason {
                index: 0,
                sid: None,
                description: no_new_access_reason(None, 0),
            }],
        },
        Row {
            case: "new access from a statement carrying a Sid",
            path: CHECK_NO_NEW_ACCESS,
            request: new_access_request(
                &unrelated_policy,
                &identity_allow(Some("AllowReads"), "s3:GetObject", bucket_objects),
            ),
            message: None,
            reasons: vec![ExpectedReason {
                index: 0,
                sid: Some("AllowReads"),
                description: no_new_access_reason(Some("AllowReads"), 0),
            }],
        },
        Row {
            case: "no new access blames no statement",
            path: CHECK_NO_NEW_ACCESS,
            request: new_access_request(
                &identity_allow(None, "s3:GetObject", bucket_objects),
                &identity_allow(None, "s3:GetObject", bucket_objects),
            ),
            message: None,
            reasons: Vec::new(),
        },
    ];

    for row in &rows {
        assert_row(&emulator, row).await;
    }
}

#[tokio::test]
async fn the_reason_text_names_the_statement_and_nothing_else_from_the_request() {
    // The invariant behind the rule: the description is a function of the check, the
    // probed dimensions and the statement's identity ALONE — so two requests that differ
    // only in document content the reference does not name (which action, which resource,
    // how the statement is spelled) are answered with the SAME text, and that text never
    // quotes the request.
    let emulator = Emulator::new();
    let probes: &[(&str, Value, Value)] = &[
        (
            "s3:GetObject on a prefix",
            identity_allow(None, "s3:GetObject", "arn:aws:s3:::demo/*"),
            json!([{"actions": ["s3:GetObject"]}]),
        ),
        (
            "sqs:SendMessage on a queue",
            identity_allow(
                None,
                "sqs:SendMessage",
                "arn:aws:sqs:us-east-1:111122223333:jobs",
            ),
            json!([{"actions": ["sqs:SendMessage"]}]),
        ),
        (
            "a wildcard action on every resource",
            identity_allow(None, "*", "*"),
            json!([{"actions": ["dynamodb:Query"]}]),
        ),
        (
            "a wildcard asked about through a wildcard",
            identity_allow(None, "kms:*", "*"),
            json!([{"actions": ["kms:Decrypt"]}]),
        ),
    ];

    let mut texts: Vec<String> = Vec::new();
    for (case, policy, access) in probes {
        let reply = emulator
            .send(
                "POST",
                CHECK_ACCESS_NOT_GRANTED,
                Some(access_request(policy, access.clone())),
            )
            .await;
        let description = reply
            .member("reasons")
            .and_then(Value::as_array)
            .and_then(|reasons| reasons.first())
            .and_then(|reason| reason.get("description"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // Nothing the request carried appears in the text: not the action asked about,
        // not the resource, not the policy's own values.
        for quoted in [
            "s3:GetObject",
            "sqs:SendMessage",
            "dynamodb:Query",
            "kms:Decrypt",
            "arn:aws:",
            "Principal",
        ] {
            assert!(
                !description.contains(quoted),
                "case {case:?}: the description must not quote the request value {quoted:?}, \
                 got {description:?}"
            );
        }
        texts.push(description);
    }
    let expected = access_not_granted_reason("actions", None, 0);
    for (case, text) in probes.iter().map(|(case, _, _)| *case).zip(&texts) {
        assert_eq!(
            text, &expected,
            "case {case:?}: expected the one canonical text {expected:?}, got {text:?}"
        );
    }
}
