// SPDX-License-Identifier: CC0-1.0

//! Where this service reports the detail text of a failure: the `Message` member of the
//! error body.
//!
//! ConnectHealth answers a refused call with the status, the error-type discriminator and
//! the exception's detail text in a member spelled `Message`:
//!
//! ```json
//! HTTP/1.1 404 Not Found
//! content-type: application/json
//! x-amzn-errortype: ResourceNotFoundException
//!
//! {"Message":"Subscription not found: dom-0123456789abcdef0123/sub-000000000000000000000"}
//! ```
//!
//! Every exception this service's model declares carries exactly one member, `message`
//! targeting `String`, and NO `@jsonName`
//! (`smithy_query(query="shape",shape="ResourceNotFoundException,ValidationException,AccessDeniedException,InternalServerException,ServiceQuotaExceededException,ThrottlingException,ConflictException",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
//! so the framework's restJson1 renderers — which fall back to the Smithy member name when
//! a member declares no `@jsonName` — write that text under the key `message`, and the
//! `Message` member a ConnectHealth client reads is absent from the body entirely. The same
//! holds for the two error bodies the FRAMEWORK renders rather than a handler: the boundary
//! refusal raised for a value outside a member's declared bound, and the scaffold's 501,
//! both of which spell the member `message` in their own synthetic shapes.
//!
//! This module therefore renames that one member on the error bodies of this service, so
//! the detail text is readable wherever it was produced. Everything else about the response
//! is left exactly as the renderer wrote it: the status, the discriminator (`__type` and the
//! `x-amzn-errortype` header), every other member, every header, and the whole success
//! path. A failure answered before any operation is served — a request naming no modelled
//! operation, a transport-level failure, a pre-dispatch authentication refusal — is not
//! reached from here, because this layer is an operation wrapper.
//!
//! It also renders, once, the text AWS gives a constraint violation a handler enforces
//! itself ([`range_violation_message`]) — through the framework's own
//! `Violation`/`envelope` renderer, so a page size this service refuses rather than
//! clamps reads exactly like the bound the boundary validator refuses. The bounds on a
//! `TagMap`'s entries, which that walk cannot reach at all, are enforced in
//! [`crate::service::tag_bounds`] by driving the walk itself.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The body member ConnectHealth reports an exception's detail text in.
pub const DETAIL_MEMBER: &str = "Message";

/// The body member the framework's renderers write that text in: the Smithy member name
/// of every exception shape in this model (`message`), which is also the name the
/// synthetic framework shapes use.
const RENDERED_DETAIL_MEMBER: &str = "message";

/// The error body with its detail member renamed to [`DETAIL_MEMBER`], or `None` when
/// there is nothing to rename: the body is not a JSON object, or it carries no
/// `message` member.
///
/// Idempotent, and non-destructive: a body that already carries a `Message` member keeps
/// that value, so a modelled member of the error shape always wins over the renamed one.
fn with_detail_member(body: &[u8]) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    let detail = object.remove(RENDERED_DETAIL_MEMBER)?;
    object.entry(DETAIL_MEMBER).or_insert(detail);
    serde_json::to_vec(&document).ok()
}

/// The response as ConnectHealth answers it: an error body whose detail text is in its
/// `Message` member.
///
/// A response that is not a failure, or whose body is not an error body carrying a
/// rendered detail member (see [`with_detail_member`]), is returned untouched — status,
/// headers and every byte of the body exactly as the renderer wrote them.
pub fn with_service_detail_member(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < 400 {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match with_detail_member(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        let _ = resp.headers_mut().try_insert("content-length", length);
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

/// The text a constraint violation is reported with: the FRAMEWORK's own renderer.
///
/// `aws_smithy_schema_server` owns both halves of that sentence — [`Violation::message`]
/// renders the clause (`Value '<value>' at '<member path>' failed to satisfy constraint:
/// <requirement>`, with the requirement coming from the [`ViolationKind`]) and
/// [`envelope`] wraps it (`1 validation error detected: <clause>`) — and it is what the
/// boundary walk answers with when it refuses a request before a handler runs. A bound a
/// HANDLER has to enforce itself therefore renders through the same code rather than
/// restating the words, so the two cannot drift.
///
/// `violations` is never empty at a call site (a violation is what brought us here);
/// `envelope` is the one place that decides what no violation reads as, and an empty
/// slice could only mean the caller found nothing to refuse.
fn rendered_violation(violations: &[Violation]) -> String {
    envelope(violations).unwrap_or_default()
}

/// AWS's constraint-violation text for `value` outside the `@range` of the member at
/// `member_path`.
///
/// Rendered by the framework (see [`rendered_violation`]), so a bound a handler enforces
/// itself — a page size it refuses rather than clamps — reads exactly like the one the
/// boundary validator refuses. `@range`'s bounds are held as the text they display with,
/// which is why they are passed through as strings.
pub fn range_violation_message(member_path: &str, value: i32, min: i32, max: i32) -> String {
    let kind = if value < min {
        ViolationKind::MinValue(min.to_string())
    } else {
        ViolationKind::MaxValue(max.to_string())
    };
    rendered_violation(&[Violation {
        path: member_path.to_string(),
        kind,
        value: Some(value.to_string()),
    }])
}

/// The detail text of the `ResourceNotFoundException` answered when a domain cannot be
/// resolved — because no domain of that id is held, or because the one held is already
/// on its way out.
///
/// Built from the REQUEST: the requested `domainId`, verbatim, is what tells one 404
/// from another. ConnectHealth words it `<resource kind> not found: <identifier>` — the
/// same form [`subscription_not_found_message`] carries for a subscription — so the
/// text is stated ONCE here rather than in each of the handlers that resolve a domain.
pub fn domain_not_found_message(domain_id: &str) -> String {
    format!("Domain not found: {domain_id}")
}

/// The detail text of the `ResourceNotFoundException` answered when a Medical Scribe
/// listening session cannot be resolved under the (`domainId`, `subscriptionId`) scope
/// the request named.
///
/// Built from the REQUEST, and in the form ConnectHealth reports for a scoped
/// identifier it cannot resolve: the member that could not be resolved and the value
/// the request gave it.
pub fn scribe_session_not_found_message(session_id: &str) -> String {
    format!("Resource not found: sessionId with ID {session_id} does not exist")
}

/// The detail text of the `ResourceNotFoundException` answered when a subscription cannot
/// be resolved — because the subscription is absent, or because its parent domain is.
///
/// Built from the REQUEST, and from both identifiers: the two are what tell one 404 from
/// another, since the same status answers a missing domain and a missing subscription. The
/// `<domainId>/<subscriptionId>` form is the one ConnectHealth itself reports, the same for
/// every pair of identifiers; stated once here so the operations that resolve a
/// subscription — the reader and the two status transitions — cannot word it three ways.
pub fn subscription_not_found_message(domain_id: &str, subscription_id: &str) -> String {
    format!("Subscription not found: {domain_id}/{subscription_id}")
}

/// The detail text of the `ResourceNotFoundException` answered when the ARN a tagging
/// call names resolves to no resource this service holds.
///
/// The same `<resource kind> not found: <identifier>` form as the two constructors
/// above, with the identifier the request used — here a resource ARN, since that is
/// what the tagging operations address a resource by.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource not found: {resource_arn}")
}

/// The operation the generated routing table names for this request.
fn modeled_operation<'s>(
    svc: &'s dyn Service,
    req: &'s RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// Whether the generated routing table can serve this request — the condition under which
/// this service claims it for [`ErrorDetailOperation`].
pub fn routes<Eng: Send + Sync + 'static>(
    entry: &crate::ServiceDefinition<Eng>,
    req: &RtRequest,
) -> bool {
    entry
        .protocol()
        .routing_key(req)
        .ok()
        .and_then(|key| entry.routing_table().get(key))
        .is_some()
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then reports the detail text of whatever it answered in this service's `Message` member.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework owns the whole request walk and every status,
/// code and text it answers with; this wrapper owns only which member that text is in.
pub struct ErrorDetailOperation;

impl ErasedOperation for ErrorDetailOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match modeled_operation(svc, req) {
                Some(op) => with_service_detail_member(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is
        // the operation this wrapper delegates to, so its model-declared schemes decide
        // and this wrapper cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a rewritten body for assertions.
    fn decoded(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).expect("the rewritten body is JSON")
    }

    /// One row per error body this service can answer with — every exception the model
    /// declares, at the status it declares, plus the two bodies the framework renders
    /// itself — so the rename is not keyed to the one the issue recorded.
    #[test]
    fn every_error_body_reports_its_detail_text_in_the_message_member() {
        for (code, detail) in [
            ("ResourceNotFoundException", "Subscription sub-x was not found in domain dom-y"),
            ("ValidationException", "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"),
            ("AccessDeniedException", "no credentials"),
            ("InternalServerException", "could not reach this service's store"),
            ("ServiceQuotaExceededException", "too many domains"),
            ("ThrottlingException", "slow down"),
            ("ConflictException", "a job with that token is running"),
            // The framework's own signals, rendered by the scaffold channel and the
            // boundary validator rather than by a handler.
            ("NotImplemented", "operation X is scaffolded but not implemented"),
        ] {
            let body = format!(r#"{{"__type":"{code}","message":"{detail}"}}"#);
            let rewritten = with_detail_member(body.as_bytes())
                .unwrap_or_else(|| panic!("case {code:?}: expected a rename, got no rewrite"));
            let document = decoded(&rewritten);
            assert_eq!(
                document.get(DETAIL_MEMBER).and_then(Value::as_str),
                Some(detail),
                "case {code:?}: expected the detail text in {DETAIL_MEMBER}, got {document}"
            );
            assert!(
                document.get(RENDERED_DETAIL_MEMBER).is_none(),
                "case {code:?}: the body must carry the text under ONE key, got {document}"
            );
            assert_eq!(
                document.get("__type").and_then(Value::as_str),
                Some(code),
                "case {code:?}: the discriminator must survive, got {document}"
            );
        }
    }

    /// Nothing to rename: a body that carries no rendered detail member, one that already
    /// spells it the way this service does, one that is not an object, and one that is not
    /// JSON at all, are all left alone.
    #[test]
    fn a_body_with_nothing_to_rename_is_left_alone() {
        for (label, body) in [
            ("no detail member", r#"{"__type":"ThrottlingException"}"#),
            // The front door's authorizer failure, which `DeleteDomain` answers for an
            // id with no allocation history: it carries no detail text at all, so there
            // is nothing to rename and nothing may be invented.
            (
                "the front door's authorizer failure",
                r#"{"__type":"AuthorizerConfigurationException"}"#,
            ),
            ("already renamed", r#"{"__type":"ThrottlingException","Message":"slow down"}"#),
            ("not an object", r#"["ResourceNotFoundException"]"#),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                with_detail_member(body.as_bytes()).is_none(),
                "case {label:?}: expected no rewrite"
            );
        }
    }

    /// A body carrying BOTH spellings keeps the one the error shape modelled.
    #[test]
    fn a_modelled_message_member_wins_over_the_renamed_one() {
        let body = r#"{"__type":"ValidationException","Message":"modelled","message":"rendered"}"#;
        let document = decoded(&with_detail_member(body.as_bytes()).expect("a rewrite"));
        assert_eq!(
            document.get(DETAIL_MEMBER).and_then(Value::as_str),
            Some("modelled"),
            "the modelled member must survive, got {document}"
        );
        assert!(
            document.get(RENDERED_DETAIL_MEMBER).is_none(),
            "the rendered member must not remain beside it, got {document}"
        );
    }

    /// A successful response is not an error body: the rename never reaches it.
    #[test]
    fn a_success_is_returned_untouched() {
        let body = r#"{"subscription":{"status":"ACTIVE","message":"not an error"}}"#;
        let resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(body),
        );
        let answered = with_service_detail_member(resp);
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            body.as_bytes(),
            "a success body must be byte-identical"
        );
    }

    /// The constraint sentence names the offending value, the member path and the bound
    /// that was violated — whichever end of the range it fell outside.
    #[test]
    fn the_range_violation_sentence_names_the_value_the_member_and_the_bound() {
        for (value, expected) in [
            (
                101,
                "1 validation error detected: Value '101' at 'maxResults' failed to satisfy \
                 constraint: Member must have value less than or equal to 100",
            ),
            (
                0,
                "1 validation error detected: Value '0' at 'maxResults' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1",
            ),
            (
                -7,
                "1 validation error detected: Value '-7' at 'maxResults' failed to satisfy \
                 constraint: Member must have value greater than or equal to 1",
            ),
        ] {
            let rendered = range_violation_message("maxResults", value, 1, 100);
            assert_eq!(
                rendered, expected,
                "case {value}: expected {expected:?}, got {rendered:?}"
            );
        }
    }
}
