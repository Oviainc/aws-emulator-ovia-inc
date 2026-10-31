// SPDX-License-Identifier: CC0-1.0

//! Whether the resource-based policy a read REPORTS grants public access, which that read
//! states beside the document itself.
//!
//! # The rule
//!
//! A resource-based policy that allows a principal of `*` lets anyone invoke the resource,
//! which is what Lambda's block-public-access control exists to stop; so a read of a
//! function's policy reports whether the document it just handed back is such a policy, in
//! `PublicAccessAllowed`. AWS's own definition of a public policy is the one applied here:
//! a statement grants public access when it ALLOWS a wildcard principal and carries no
//! condition that narrows who may use it to a particular account, organization or source
//! resource. A policy is public when any of its statements is, so a document whose
//! statements all name a principal, or whose wildcard grant is narrowed by such a
//! condition, is reported as not public.
//!
//! The value is therefore a property of the DOCUMENT, computed from the policy the read
//! reports and from nothing else: two reads of the same policy report the same flag, and
//! the flag changes exactly when a grant that makes the policy public is added or removed.
//!
//! # Why it is stated here
//!
//! `GetPolicyResponse` models `Policy` and `RevisionId` only
//! (`smithy_query(query="operation",operation="GetPolicy",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")`),
//! while AWS's recorded answer carries the flag as well, so there is no output member for a
//! handler to set: the member is named on the rendered response instead, at the same seam
//! that names the class of a fault on an error body ([`crate::service::fault_class`]) — the
//! per-operation wrapper every modelled operation of this service is served behind.
//!
//! It is keyed by the operation that reports it. `GetPolicy` is the function-policy read
//! AWS records it on; the resource-policy read (`GetResourcePolicy`) reports the document
//! and its revision alone, so nothing is added there.

use aws_smithy_runtime_api::http::Response as RtResponse;
use aws_smithy_types::body::SdkBody;
use serde_json::Value;

/// The operation whose answer states the flag.
const REPORTED_BY: &str = "GetPolicy";

/// The member the flag travels in.
const PUBLIC_ACCESS_MEMBER: &str = "PublicAccessAllowed";

/// The member of the answer that carries the policy document the flag is computed from.
const POLICY_MEMBER: &str = "Policy";

/// The response header a renderer uses to advertise its body's length.
const CONTENT_LENGTH: &str = "content-length";

/// The policy element that says whether a statement allows or denies.
const EFFECT_ELEMENT: &str = "Effect";

/// The `Effect` of a statement that GRANTS; any other effect takes nothing away from the
/// public.
const EFFECT_ALLOW: &str = "Allow";

/// The policy element naming who a statement grants to.
const PRINCIPAL_ELEMENT: &str = "Principal";

/// The element carrying a statement's conditions.
const CONDITION_ELEMENT: &str = "Condition";

/// The member a `Principal` object names an account, user or role principal under, which
/// is where a wildcard is written when the principal is written as an object.
const PRINCIPAL_AWS_MEMBER: &str = "AWS";

/// The principal that is everyone.
const WILDCARD_PRINCIPAL: &str = "*";

/// The statements of a policy document, as the policy language writes them.
const STATEMENT_ELEMENT: &str = "Statement";

/// The condition keys that narrow a grant to a particular caller, organization or source
/// resource — so a wildcard principal restricted by one of them is not public access.
///
/// They are the global condition keys a resource policy restricts an invoker with: the
/// calling account or principal, the organization the caller belongs to, and the resource,
/// account or organization the call comes from. Compared case-insensitively, because IAM
/// condition keys are not case-sensitive.
const NARROWING_CONDITION_KEYS: &[&str] = &[
    "aws:principalaccount",
    "aws:principalarn",
    "aws:principalorgid",
    "aws:principalorgpaths",
    "aws:sourcearn",
    "aws:sourceaccount",
    "aws:sourceowner",
    "aws:sourceorgid",
    "aws:sourceorgpaths",
];

/// Whether `document` — one resource-based policy — grants public access.
#[must_use]
pub fn grants_public_access(document: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<Value>(document) else {
        return false;
    };
    statements(&parsed).iter().any(|statement| {
        is_allow(statement) && allows_everyone(statement) && !narrowed_by_condition(statement)
    })
}

/// The statements `document` declares: the entries of its `Statement` list, or the one
/// statement written as the object itself. Empty for anything else.
fn statements(document: &Value) -> Vec<&Value> {
    match document.get(STATEMENT_ELEMENT) {
        Some(Value::Array(statements)) => statements.iter().collect(),
        Some(single) => vec![single],
        None => Vec::new(),
    }
}

/// Whether `statement` grants rather than denies. A statement that names no effect at all
/// grants nothing, so it cannot make a policy public.
fn is_allow(statement: &Value) -> bool {
    statement
        .get(EFFECT_ELEMENT)
        .and_then(Value::as_str)
        .is_some_and(|effect| effect.eq_ignore_ascii_case(EFFECT_ALLOW))
}

/// Whether `statement` names the wildcard principal, in any of the forms the policy
/// language writes a principal in: the bare string, a list of them, or the object mapping a
/// principal type to either.
fn allows_everyone(statement: &Value) -> bool {
    let Some(principal) = statement.get(PRINCIPAL_ELEMENT) else {
        return false;
    };
    match principal {
        Value::Object(members) => members
            .get(PRINCIPAL_AWS_MEMBER)
            .is_some_and(names_wildcard),
        other => names_wildcard(other),
    }
}

/// Whether a policy element names the wildcard: written as the value itself or as one entry
/// of a list of values.
fn names_wildcard(element: &Value) -> bool {
    match element {
        Value::String(value) => value == WILDCARD_PRINCIPAL,
        Value::Array(values) => values.iter().any(names_wildcard),
        _ => false,
    }
}

/// Whether `statement` carries a condition that narrows who may use the grant.
fn narrowed_by_condition(statement: &Value) -> bool {
    let Some(Value::Object(operators)) = statement.get(CONDITION_ELEMENT) else {
        return false;
    };
    operators.values().any(|keys| match keys {
        Value::Object(keys) => keys.keys().any(|key| {
            NARROWING_CONDITION_KEYS
                .iter()
                .any(|narrowing| key.eq_ignore_ascii_case(narrowing))
        }),
        _ => false,
    })
}

/// The answer with the flag named on it, or `None` when there is nothing to name it on:
/// the body is not a JSON object, carries no policy document, or already states the flag.
fn naming_public_access(body: &[u8]) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    if object.contains_key(PUBLIC_ACCESS_MEMBER) {
        return None;
    }
    let policy = object.get(POLICY_MEMBER).and_then(Value::as_str)?;
    let allowed = grants_public_access(policy);
    object.insert(PUBLIC_ACCESS_MEMBER.to_string(), Value::Bool(allowed));
    serde_json::to_vec(&document).ok()
}

/// `resp`, the answer `operation` gave, as this service reports it: a policy read also
/// states whether the policy it reports grants public access.
///
/// Anything else is returned untouched — another operation, a failure (which carries no
/// policy to judge), and a body that is not a JSON object naming one.
#[must_use]
pub fn with_public_access_allowed(operation: &str, mut resp: RtResponse) -> RtResponse {
    if operation != REPORTED_BY || !resp.status().is_success() {
        return resp;
    }
    let rewritten = {
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match naming_public_access(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; a length that cannot be
    // written is REMOVED rather than left describing the body before the member was added.
    if resp.headers().get(CONTENT_LENGTH).is_some() {
        let length = rewritten.len().to_string();
        if resp
            .headers_mut()
            .try_insert(CONTENT_LENGTH, length)
            .is_err()
        {
            resp.headers_mut().remove(CONTENT_LENGTH);
        }
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A statement granting `principal`, with `condition` written after it when there is
    /// one.
    fn statement(principal: &str, condition: Option<&str>) -> String {
        let conditioned = condition
            .map(|condition| format!(",\"Condition\":{condition}"))
            .unwrap_or_default();
        format!(
            "{{\"Sid\":\"one\",\"Effect\":\"Allow\",\"Principal\":{principal},\
             \"Action\":\"lambda:InvokeFunction\",\
             \"Resource\":\"arn:aws:lambda:us-east-1:000000000000:function:probe\"\
             {conditioned}}}"
        )
    }

    /// A policy document carrying `statements`.
    fn policy(statements: &str) -> String {
        format!("{{\"Version\":\"2012-10-17\",\"Id\":\"default\",\"Statement\":[{statements}]}}")
    }

    /// One row per way a document can and cannot grant public access: the form the
    /// principal is written in, the effect, whether a condition narrows the grant and which
    /// key it narrows with, how many statements there are, and documents that are no policy
    /// at all.
    #[test]
    fn a_policy_is_public_when_a_grant_allows_everyone_unconditionally() {
        let cases: &[(&str, String, bool)] = &[
            (
                "a wildcard principal written as the bare string",
                policy(&statement("\"*\"", None)),
                true,
            ),
            (
                "a wildcard principal written under AWS",
                policy(&statement("{\"AWS\":\"*\"}", None)),
                true,
            ),
            (
                "a wildcard among a list of principals",
                policy(&statement(
                    "{\"AWS\":[\"arn:aws:iam::000000000000:root\",\"*\"]}",
                    None,
                )),
                true,
            ),
            (
                "one statement written as the object itself",
                format!(
                    "{{\"Version\":\"2012-10-17\",\"Statement\":{}}}",
                    statement("\"*\"", None)
                ),
                true,
            ),
            (
                "a public grant beside a narrowed one",
                policy(&format!(
                    "{},{}",
                    statement(
                        "{\"AWS\":\"arn:aws:iam::000000000000:root\"}",
                        None
                    ),
                    statement("\"*\"", None)
                )),
                true,
            ),
            (
                "a wildcard grant carrying a condition that narrows nothing",
                policy(&statement(
                    "\"*\"",
                    Some("{\"Bool\":{\"lambda:InvokedViaFunctionUrl\":\"true\"}}"),
                )),
                true,
            ),
            (
                "a named account principal",
                policy(&statement(
                    "{\"AWS\":\"arn:aws:iam::000000000000:root\"}",
                    None,
                )),
                false,
            ),
            (
                "a service principal",
                policy(&statement("{\"Service\":\"s3.amazonaws.com\"}", None)),
                false,
            ),
            (
                "a wildcard grant narrowed to one source account",
                policy(&statement(
                    "\"*\"",
                    Some("{\"StringEquals\":{\"AWS:SourceAccount\":\"000000000000\"}}"),
                )),
                false,
            ),
            (
                "a wildcard grant narrowed to one organization",
                policy(&statement(
                    "{\"AWS\":\"*\"}",
                    Some("{\"StringEquals\":{\"aws:PrincipalOrgID\":\"o-abcdefghij\"}}"),
                )),
                false,
            ),
            (
                "a wildcard grant narrowed to one source ARN",
                policy(&statement(
                    "\"*\"",
                    Some(
                        "{\"ArnLike\":{\"AWS:SourceArn\":\
                         \"arn:aws:s3:::probe-bucket\"}}",
                    ),
                )),
                false,
            ),
            (
                "a wildcard DENY, which grants nothing",
                policy(
                    &statement("\"*\"", None).replace("\"Effect\":\"Allow\"", "\"Effect\":\"Deny\""),
                ),
                false,
            ),
            (
                "a policy with no statement at all",
                "{\"Version\":\"2012-10-17\",\"Id\":\"default\"}".to_string(),
                false,
            ),
            (
                "text that is not a policy document",
                "not a policy".to_string(),
                false,
            ),
        ];
        for (case, document, expected) in cases {
            let actual = grants_public_access(document);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected public access {expected}, got {actual} for {document}"
            );
        }
    }

    /// The flag is named on the answer of the read that reports it, carrying what the
    /// document says, and on nothing else.
    #[test]
    fn the_read_that_reports_a_policy_states_the_flag() {
        let public = policy(&statement("\"*\"", None));
        let private = policy(&statement("{\"AWS\":\"arn:aws:iam::000000000000:root\"}", None));
        let cases: &[(&str, &str, u16, &str, Option<bool>)] = &[
            ("a policy nobody public may use", REPORTED_BY, 200, &private, Some(false)),
            ("a policy everyone may use", REPORTED_BY, 200, &public, Some(true)),
            // Another operation's answer, and a failure, are not policy reads.
            ("another operation", "GetResourcePolicy", 200, &private, None),
        ];
        for (case, operation, status, document, expected) in cases {
            let body = serde_json::json!({
                "Policy": document,
                "RevisionId": "7b9f0e8a-1c2d-4e5f-8a9b-0c1d2e3f4a5b",
            })
            .to_string();
            let answered = with_public_access_allowed(
                operation,
                RtResponse::new(
                    aws_smithy_runtime_api::http::StatusCode::try_from(*status)
                        .expect("a valid status"),
                    SdkBody::from(body),
                ),
            );
            let decoded: Value =
                serde_json::from_slice(answered.body().bytes().expect("an in-memory body"))
                    .expect("the answer is JSON");
            assert_eq!(
                decoded.get(PUBLIC_ACCESS_MEMBER).and_then(Value::as_bool),
                *expected,
                "case {case:?}: expected the flag {expected:?}, got {decoded}"
            );
            assert_eq!(
                decoded.get(POLICY_MEMBER).and_then(Value::as_str),
                Some(*document),
                "case {case:?}: the reported document must survive, got {decoded}"
            );
        }
    }

    /// An error body carries no policy, so a refusal of the same read is left exactly as
    /// its renderer wrote it.
    #[test]
    fn a_refusal_is_left_alone() {
        let body = r#"{"__type":"ResourceNotFoundException","Message":"The resource you requested does not exist."}"#;
        let answered = with_public_access_allowed(
            REPORTED_BY,
            RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(404).expect("a valid status"),
                SdkBody::from(body),
            ),
        );
        assert_eq!(
            answered.body().bytes().expect("an in-memory body"),
            body.as_bytes(),
            "expected a refusal body untouched"
        );
    }
}
