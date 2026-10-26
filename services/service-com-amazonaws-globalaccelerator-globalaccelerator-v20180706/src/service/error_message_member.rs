// SPDX-License-Identifier: CC0-1.0

//! Which member of an error body this service reports the failure's text in: `Message`.
//!
//! # The rule
//!
//! On the awsJson protocols a structure member's wire name IS its Smithy member name (the
//! codec reads `@jsonName` only on restJson1), so the member a client reads an error's text
//! out of is the one the ERROR SHAPE declares. Every error shape in this service's model
//! declares exactly one member, `Message`, targeting `ErrorMessage`, and none carries a
//! `@jsonName`
//! (`smithy_query(query="shape",shape="InvalidArgumentException,AcceleratorNotFoundException,AcceleratorNotDisabledException,AccessDeniedException,AssociatedEndpointGroupFoundException,AssociatedListenerFoundException,AttachmentNotFoundException,ByoipCidrNotFoundException,ConflictException,EndpointAlreadyExistsException,EndpointGroupAlreadyExistsException,EndpointGroupNotFoundException,EndpointNotFoundException,IncorrectCidrStateException,InternalServiceErrorException,InvalidNextTokenException,InvalidPortRangeException,LimitExceededException,ListenerNotFoundException,TransactionInProgressException",model_path="services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/smithy-model/com-amazonaws-globalaccelerator-globalaccelerator-v20180706.json")`),
//! so `Message` is where this service's text goes on EVERY error body:
//!
//! ```json
//! HTTP/1.1 400 Bad Request
//! content-type: application/x-amz-json-1.1
//!
//! {"__type":"InvalidArgumentException","Message":"1 validation error detected: …"}
//! ```
//!
//! An error a HANDLER returns is serialized from the model's own shape, so it already
//! carries that member. The error bodies the FRAMEWORK renders do not: the pre-handler
//! refusal of a value outside a member's declared bound, and the scaffold's 501, are built
//! from synthetic shapes whose own member is spelled `message`, and the awsJson error
//! envelope writes that spelling. A client — and any SDK deserializing the modelled
//! exception — then finds no `Message` member at all and surfaces the failure with no text,
//! even though the status and the error code are right.
//!
//! This module therefore reports that text in `Message` on the error bodies of this
//! service, wherever it was produced. The status, the `__type` discriminator, every other
//! member, every header and the whole success path stay with the renderer that owns them;
//! the body is re-serialized through the same JSON serializer it was written with, so its
//! member names and values survive while its byte-level layout (member order, number
//! formatting) is that serializer's. A failure answered before any operation is served —
//! a request naming no modelled operation, a transport-level failure, a pre-dispatch
//! authentication refusal — is not reached from here, because this layer is an operation
//! wrapper.
//!
//! The same mechanism, for the same reason, is stated in
//! `service-com-amazonaws-connecthealth-connecthealth` (`src/service/error_detail.rs`),
//! `service-com-amazonaws-ssoadmin-swbexternalservice` (`src/service/refusal.rs`) and
//! `service-com-amazonaws-appconfig-amazonappconfig` (`src/service.rs`); only the member
//! name and the service it is wired into differ. The structural home for it is the error
//! renderer itself, which writes the synthetic shape's `message` for every service — but
//! that renderer is in the agent-immutable `vendor/` tree, which a fix may not change, so
//! each service states it at the one seam that can see a whole rendered response.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Service,
};

/// The body member every error shape in this service's model reports its text in — see the
/// module header for the `smithy_query` call this comes from.
pub const MODELLED_MESSAGE_MEMBER: &str = "Message";

/// The body member the FRAMEWORK's own renderers write that text in: the member name of
/// the synthetic `smithy.framework#ValidationException` / `NotImplemented` shapes the
/// pre-handler refusal and the scaffold's 501 are built from.
const RENDERED_MESSAGE_MEMBER: &str = "message";

/// The response header a renderer uses to advertise its body's length.
const CONTENT_LENGTH: &str = "content-length";

/// The error body with its text member renamed to [`MODELLED_MESSAGE_MEMBER`], or `None`
/// when there is nothing to rename: the body is not a JSON object, or it carries no
/// `message` member.
///
/// Idempotent, and non-destructive: a body that already carries a `Message` member keeps
/// that value, so the member a modelled error shape serialized always wins over the
/// renamed one.
fn with_modelled_message_member(body: &[u8]) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    let text = object.remove(RENDERED_MESSAGE_MEMBER)?;
    object.entry(MODELLED_MESSAGE_MEMBER).or_insert(text);
    serde_json::to_vec(&document).ok()
}

/// The response as this service answers it: an error body whose text is in its `Message`
/// member.
///
/// A response that is not a failure, or whose body is not an error body carrying a rendered
/// text member (see [`with_modelled_message_member`]), is returned untouched — status,
/// headers and every byte of the body exactly as the renderer wrote them.
pub fn with_service_message_member(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < 400 {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match with_modelled_message_member(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get(CONTENT_LENGTH).is_some() {
        let length = rewritten.len().to_string();
        // A decimal length is always a valid header value, so this cannot fail today; if
        // it ever did, the header is REMOVED rather than left describing the body before
        // the rename, so the two can never disagree.
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

/// The operation `definition`'s generated routing table names for this request, if any.
///
/// The ONE resolution in this module: [`routes`] (the condition under which the service
/// claims a request for the wrapper) and [`ErrorMessageMemberOperation::run`] (the
/// operation it then delegates to) are both expressed through it, so the claim and the
/// delegation target cannot drift apart.
fn routed<'s>(
    definition: &'s dyn ErasedServiceDefinition,
    req: &RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = definition.protocol().routing_key(req).ok()?;
    definition.routing_table().get(key)
}

/// Whether the generated routing table can serve this request — the condition under which
/// this service claims it for [`ErrorMessageMemberOperation`].
///
/// A request the table cannot serve is declined rather than claimed, so the router answers
/// it as the unroutable request it is instead of authenticating a request that names no
/// operation first.
pub fn routes(definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> bool {
    routed(definition, req).is_some()
}

/// Serves the modelled operation the routing table names for the request, exactly as that
/// table would, then reports the text of whatever it answered in this service's `Message`
/// member.
///
/// WHY a wrapper and not [`Service::override_refusal`]: that seam chooses the CODE and the
/// WORDS of a pre-handler refusal, not the body member they are serialized into — the
/// framework writes a framework-rendered error's text into its own synthetic shape's
/// `message`. A wrapper sees the whole response, so it can report that text where this
/// service's model says an error carries it.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`] — a wrapper held on the service cannot borrow from the entry it
/// is held beside, which is why the key is resolved here rather than carried in. So the
/// framework owns the whole deserialize → validate → handle → serialize walk and every
/// status, code and word it answers with; this wrapper owns only which member the text is
/// in.
#[derive(Debug, Clone, Copy)]
pub struct ErrorMessageMemberOperation;

impl ErasedOperation for ErrorMessageMemberOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match routed(svc.definition(), req) {
                Some(op) => with_service_message_member(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key ([`routes`]); answered
                // as the framework answers an unroutable request rather than guessed at.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is the
        // operation this wrapper delegates to, so its model-declared schemes decide and
        // this wrapper cannot weaken them.
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

    /// A response at `status` carrying `body`.
    fn response(status: u16, body: &str) -> RtResponse {
        RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    /// One row per class of error body this service can answer with — the modelled
    /// exceptions, at the status this service serializes each at, plus the two bodies the
    /// framework renders itself — so the rename is a property of the error envelope rather
    /// than of the one body the issue recorded.
    #[test]
    fn every_error_body_reports_its_text_in_the_modelled_message_member() {
        for (code, text) in [
            (
                "InvalidArgumentException",
                "1 validation error detected: Value '150' at 'trafficDialPercentage' failed to \
                 satisfy constraint: Member must have value less than or equal to 100",
            ),
            (
                "AcceleratorNotFoundException",
                "Accelerator arn:aws:globalaccelerator::000000000000:accelerator/0123abcd \
                 not found.",
            ),
            (
                "AcceleratorNotDisabledException",
                "Please disable accelerator arn:aws:globalaccelerator::000000000000:\
                 accelerator/0123abcd before deleting it.",
            ),
            ("AccessDeniedException", "You don't have access permission."),
            ("InvalidPortRangeException", "Port range 0-70000 is not valid."),
            ("LimitExceededException", "Accelerator limit reached."),
            ("InternalServiceErrorException", "this service's store is unreachable"),
            // The framework's own signals, rendered by the boundary validator and the
            // scaffold channel rather than by a handler.
            ("ValidationException", "invalid request"),
            (
                "NotImplemented",
                "operation TagResource is scaffolded but not implemented in this emulator",
            ),
        ] {
            let body = format!(r#"{{"__type":"{code}","message":"{text}"}}"#);
            let rewritten = with_modelled_message_member(body.as_bytes())
                .unwrap_or_else(|| panic!("case {code:?}: expected a rename, got no rewrite"));
            let document = decoded(&rewritten);
            assert_eq!(
                document.get(MODELLED_MESSAGE_MEMBER).and_then(Value::as_str),
                Some(text),
                "case {code:?}: expected the text in {MODELLED_MESSAGE_MEMBER}, got {document}"
            );
            assert!(
                document.get(RENDERED_MESSAGE_MEMBER).is_none(),
                "case {code:?}: the body must carry the text under ONE member, got {document}"
            );
            assert_eq!(
                document.get("__type").and_then(Value::as_str),
                Some(code),
                "case {code:?}: the discriminator must survive the rename, got {document}"
            );
        }
    }

    /// Nothing to rename: a body that carries no rendered text member, one that already
    /// spells it the way this service does, one that is not an object, and one that is not
    /// JSON at all, are all left alone.
    #[test]
    fn a_body_with_nothing_to_rename_is_left_alone() {
        for (case, body) in [
            ("no text member", r#"{"__type":"ConflictException"}"#),
            (
                "already the modelled member",
                r#"{"__type":"ConflictException","Message":"You can't use both of those options."}"#,
            ),
            ("not an object", r#"["AcceleratorNotFoundException"]"#),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                with_modelled_message_member(body.as_bytes()).is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }

    /// A body carrying BOTH spellings keeps the one the error shape modelled.
    #[test]
    fn a_modelled_message_member_wins_over_the_renamed_one() {
        let body =
            r#"{"__type":"InvalidArgumentException","Message":"modelled","message":"rendered"}"#;
        let document = decoded(&with_modelled_message_member(body.as_bytes()).expect("a rewrite"));
        assert_eq!(
            document.get(MODELLED_MESSAGE_MEMBER).and_then(Value::as_str),
            Some("modelled"),
            "the modelled member must survive, got {document}"
        );
        assert!(
            document.get(RENDERED_MESSAGE_MEMBER).is_none(),
            "the rendered member must not remain beside it, got {document}"
        );
    }

    /// A successful response is not an error body: the rename never reaches it, whatever
    /// its members are called.
    #[test]
    fn a_success_is_returned_untouched() {
        let body = r#"{"Accelerator":{"Status":"IN_PROGRESS","message":"not an error"}}"#;
        let answered = with_service_message_member(response(200, body));
        assert_eq!(
            answered.body().bytes().expect("an in-memory body"),
            body.as_bytes(),
            "a success body must be byte-identical"
        );
    }

    /// An error response is rewritten in place, and a length a renderer advertised is
    /// corrected to the body it now describes.
    #[test]
    fn an_error_response_carries_the_renamed_member_and_a_matching_length() {
        let body = r#"{"__type":"InvalidArgumentException","message":"Invalid parameter."}"#;
        let mut resp = response(400, body);
        resp.headers_mut()
            .insert(CONTENT_LENGTH, body.len().to_string());
        let answered = with_service_message_member(resp);
        let bytes = answered.body().bytes().expect("an in-memory body").to_vec();
        let document = decoded(&bytes);
        assert_eq!(
            document.get(MODELLED_MESSAGE_MEMBER).and_then(Value::as_str),
            Some("Invalid parameter."),
            "expected the text in {MODELLED_MESSAGE_MEMBER}, got {document}"
        );
        assert_eq!(
            answered.headers().get(CONTENT_LENGTH),
            Some(bytes.len().to_string().as_str()),
            "expected the advertised length to describe the rewritten body"
        );
    }
}
