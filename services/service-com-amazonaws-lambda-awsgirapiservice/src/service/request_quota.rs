// SPDX-License-Identifier: CC0-1.0

//! The size a REQUEST to one operation of this service may reach, and the refusal for one
//! that passes it.
//!
//! # The rule
//!
//! A quota on the size of the request itself is separate from any quota on what the request
//! would store. `AddPermission` is where the two meet: it binds
//! `PolicyLengthExceededException` for a grant that would push the function's
//! resource-based policy past the policy quota, and the model also declares
//! `RequestTooLargeException` — "The request payload exceeded the … request body … quota",
//! `@error("client")` with `@httpError(413)`
//! (`smithy_query(query="shape",shape="RequestTooLargeException",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")`)
//! — for a request whose own body is too big to accept at all. The two are different
//! conditions and are answered at different statuses, because a caller acts on them
//! differently: a 413 says to send less in ONE call, a 400 policy-length refusal says the
//! resulting policy is too big however it is sent.
//!
//! So a request whose body exceeds the quota published for the operation it addresses is
//! refused `413 RequestTooLargeException`, stating that quota and that operation:
//! `Request must be smaller than <quota> bytes for the <operation> operation`. Both parts
//! are variables of one sentence, which is why the quota is a per-operation table rather
//! than a constant: each row is the quota AWS publishes for that operation, and an
//! operation with no row has no body quota this emulator knows of and is never refused
//! here.
//!
//! # Why it is stated here
//!
//! The check is on the request's BODY, before it is read as that operation's input: a body
//! past the quota is refused whatever it holds, so no handler — which is only ever handed
//! an input the framework already deserialized and validated — can answer for it. The one
//! seam that sees the whole request as it arrived is the per-operation wrapper every
//! modelled operation of this service is served behind ([`crate::service`]), so the rule
//! lives beside it and is applied to every operation alike.
//!
//! The answer is this service's error envelope, built here rather than from the operation's
//! modelled error enum, because `AddPermission` does not BIND `RequestTooLargeException` in
//! the on-disk model while AWS answers it there: the status and the error code come from the
//! error shape the model declares, and the message member is the one AWS's recorded body
//! carries it in. The class of the fault is named on it by
//! [`crate::service::fault_class`], as it is on every other error body of this service.

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;

/// The error code a request past its operation's quota is refused as on the WIRE, and so
/// the `x-amzn-errortype` the refusal carries.
///
/// It is the code AWS's recorded answer names, which is not the name of the modelled shape:
/// the model declares `RequestTooLargeException` — `@error("client")` with
/// `@httpError(413)`
/// (`smithy_query(query="shape",shape="RequestTooLargeException",model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")`),
/// which is how an SDK models the refusal — while the answer on the wire names the entity
/// that was too large (corpus `simparity-wire-aws-v1`, fixture
/// `lam-err-policy-length-exceeded`).
const REQUEST_TOO_LARGE: &str = "RequestEntityTooLargeException";

/// The status that shape declares (`@httpError(413)`).
const REQUEST_TOO_LARGE_STATUS: u16 = 413;

/// The member AWS's recorded body for this refusal carries its sentence in.
const MESSAGE_MEMBER: &str = "Message";

/// The member every error body of this service names its error shape in.
const SHAPE_MEMBER: &str = "__type";

/// The largest request body one operation of this service accepts.
struct BodyQuota {
    /// The operation, as the model names it — the name the routing table is keyed by.
    operation: &'static str,
    /// The quota AWS publishes for that operation's request, in bytes.
    max_bytes: usize,
}

/// Every operation of this service with a published request-body quota, one row each.
///
/// A row is the quota for that operation and nothing else: the sentence names both, so two
/// operations with different quotas are refused with their own. An operation absent from
/// this table is never refused for its size here.
const BODY_QUOTAS: &[BodyQuota] = &[
    // AddPermission: 5 KB. Recorded behaviour (corpus `simparity-wire-aws-v1`, fixture
    // `lam-err-policy-length-exceeded`): a request over it is answered 413, naming this
    // quota, while the resource policy's own 20480-byte quota is answered 400.
    BodyQuota {
        operation: "AddPermission",
        max_bytes: 5_120,
    },
];

/// The sentence a request past `max_bytes` for `operation` is refused with.
fn too_large_message(operation: &str, max_bytes: usize) -> String {
    format!("Request must be smaller than {max_bytes} bytes for the {operation} operation")
}

/// How this service refuses a request to `operation` whose body is `body_bytes` long, or
/// `None` when that body is within the quota published for it — which includes every
/// operation this emulator knows no request quota for.
pub(super) fn refusing_body(operation: &str, body_bytes: usize) -> Option<RtResponse> {
    let quota = BODY_QUOTAS
        .iter()
        .find(|quota| quota.operation == operation)?;
    if body_bytes <= quota.max_bytes {
        return None;
    }
    Some(too_large_response(&too_large_message(
        operation,
        quota.max_bytes,
    )))
}

/// The refusal itself: this service's JSON error envelope, at the status the error shape
/// declares.
fn too_large_response(message: &str) -> RtResponse {
    let mut document = ::serde_json::Map::new();
    document.insert(
        SHAPE_MEMBER.to_string(),
        ::serde_json::Value::String(REQUEST_TOO_LARGE.to_string()),
    );
    document.insert(
        MESSAGE_MEMBER.to_string(),
        ::serde_json::Value::String(message.to_string()),
    );
    let body = ::serde_json::Value::Object(document).to_string();
    let status = StatusCode::try_from(REQUEST_TOO_LARGE_STATUS)
        .expect("the status the model declares is a valid one");
    let mut response = RtResponse::new(status, SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", "application/json");
    response
        .headers_mut()
        .insert("x-amzn-errortype", REQUEST_TOO_LARGE);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decoded body of a refusal.
    fn decoded(response: &RtResponse) -> ::serde_json::Value {
        ::serde_json::from_slice(response.body().bytes().expect("an in-memory body"))
            .expect("the refusal body is JSON")
    }

    /// One row per size class, at and around every quota this table holds: the quota itself
    /// and one byte under it are accepted, one byte over it and far over it are refused.
    #[test]
    fn a_body_is_refused_exactly_past_its_operations_quota() {
        for quota in BODY_QUOTAS {
            let cases: &[(&str, usize, bool)] = &[
                ("an empty body", 0, false),
                ("one byte under the quota", quota.max_bytes - 1, false),
                ("exactly the quota", quota.max_bytes, false),
                ("one byte over the quota", quota.max_bytes + 1, true),
                ("far over the quota", quota.max_bytes * 4, true),
            ];
            for (case, body_bytes, refused) in cases {
                let stated = refusing_body(quota.operation, *body_bytes);
                assert_eq!(
                    stated.is_some(),
                    *refused,
                    "case {case:?} on {}: expected refused {refused}, got {:?}",
                    quota.operation,
                    stated.map(|response| response.status().as_u16())
                );
            }
        }
    }

    /// The refusal names the status, the error shape and the quota of the operation it
    /// refused — the whole wire answer, so a caller can tell it from the policy-size one.
    #[test]
    fn the_refusal_names_the_shape_the_quota_and_the_operation() {
        for quota in BODY_QUOTAS {
            let response = refusing_body(quota.operation, quota.max_bytes + 1)
                .unwrap_or_else(|| panic!("case {}: expected a refusal", quota.operation));
            assert_eq!(
                response.status().as_u16(),
                REQUEST_TOO_LARGE_STATUS,
                "case {}: expected the status the model declares, got {}",
                quota.operation,
                response.status().as_u16()
            );
            assert_eq!(
                response.headers().get("x-amzn-errortype"),
                Some(REQUEST_TOO_LARGE),
                "case {}: expected the error shape in the header, got {:?}",
                quota.operation,
                response.headers().get("x-amzn-errortype")
            );
            let document = decoded(&response);
            let expected = format!(
                "Request must be smaller than {} bytes for the {} operation",
                quota.max_bytes, quota.operation
            );
            assert_eq!(
                document
                    .get(MESSAGE_MEMBER)
                    .and_then(::serde_json::Value::as_str),
                Some(expected.as_str()),
                "case {}: expected {expected:?}, got {document}",
                quota.operation
            );
        }
    }

    /// An operation this emulator knows no request quota for is never refused for its
    /// size, however large the body.
    #[test]
    fn an_operation_with_no_quota_is_never_refused_here() {
        for case in ["GetPolicy", "PutResourcePolicy", "Invoke", ""] {
            let stated = refusing_body(case, 10 * 1_024 * 1_024);
            assert!(
                stated.is_none(),
                "case {case:?}: expected no size refusal, got {:?}",
                stated.map(|response| response.status().as_u16())
            );
        }
    }
}
