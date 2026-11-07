// SPDX-License-Identifier: CC0-1.0

//! The awsQuery error envelope this service answers with: `<ErrorResponse>` carrying a
//! `<RequestId>` sibling of `<Error>`.
//!
//! AWS answers an awsQuery error with
//!
//! ```xml
//! <ErrorResponse xmlns="http://cloudformation.amazonaws.com/doc/2010-05-15/">
//!   <Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error>
//!   <RequestId>…</RequestId>
//! </ErrorResponse>
//! ```
//!
//! The `<RequestId>` element belongs to the envelope, not to any modelled error shape: it
//! is what an SDK's error object exposes as the request id, so a failed call must carry
//! one exactly as a successful one carries `ResponseMetadata/RequestId`. The framework's
//! query renderers write only `<Error>`, so this module adds the missing element to EVERY
//! error response this service emits, whichever layer produced it:
//!
//! * a handler-returned or modelled error, and a framework refusal raised while a matched
//!   operation was being served — an absent required member, a value outside a declared
//!   bound, a value outside an enum's declared set — through [`RequestIdEnvelopeOperation`],
//!   the operation wrapper this service hands back from `Service::override_operation`;
//! * a routing failure (an `Action` naming no modelled operation), a transport failure and
//!   a pre-dispatch authentication refusal, through [`CloudFormationDefinition`], the
//!   definition this service hands back from `Service::definition`.
//!
//! Only the envelope is this module's business: the HTTP status, the `<Type>`, the
//! `<Code>`, the `<Message>` and the whole success path stay with the framework renderer
//! that owns them, so each has exactly one home. A response reports ONE request id, so a
//! message of an ERROR envelope that has to state it — a refusal worded by a component
//! CloudFormation forwards to carries the SDK envelope of that call, request id and all —
//! writes [`crate::state::REQUEST_ID_PLACEHOLDER`], and this module substitutes the id that
//! response is named by: the one the shared rule reports for it
//! ([`service_common::query_error_envelope::reported_request_id`], which the audit wrapper
//! inside this one has already named every response it answers by), and a minted one only
//! where no layer named it.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationRoutingTable,
    ServerHttpProtocol, Service, ServiceIdentity,
};

use crate::service::management_events::AuditedDefinition;

/// The closing tag of the awsQuery error envelope.
const ERROR_RESPONSE_CLOSE: &str = "</ErrorResponse>";

/// The envelope element this module adds.
const REQUEST_ID_OPEN: &str = "<RequestId>";

/// The marker a rendered message writes where THIS RESPONSE's request id goes
/// ([`crate::state::REQUEST_ID_PLACEHOLDER`]).
const REQUEST_ID_MARKER: &str = crate::state::REQUEST_ID_PLACEHOLDER;

/// The document with every marker replaced by `request_id`, or `None` when it needs no
/// substitution.
///
/// A refusal worded by a component CloudFormation forwards to states this response's request
/// id inside the SDK envelope its own message carries. A response reports ONE id, so that
/// message mints none: it writes the marker, and the id this response is named by is written
/// into every place the document states it. Nothing is read OUT of the document, so no text a
/// request carries can decide the value — all a marker inside an echoed value can do is state
/// the id this response reports anyway.
///
/// Confined to an `<ErrorResponse>` envelope, which is the only document this service's own
/// wording writes the marker into: a successful response, whose body may carry any text a
/// caller stored, is never rewritten here.
fn substitute_request_id(body: &str, request_id: &str) -> Option<String> {
    if !body.contains(ERROR_RESPONSE_CLOSE) {
        return None;
    }
    body.contains(REQUEST_ID_MARKER)
        .then(|| body.replace(REQUEST_ID_MARKER, request_id))
}

/// Add `<RequestId>` to an awsQuery `<ErrorResponse>` document, as the last child of the
/// envelope (the sibling after `<Error>`).
///
/// `None` means the document needs nothing: it is not an `<ErrorResponse>` envelope (a
/// successful `<…Response>` carries its request id in `ResponseMetadata` already), or it
/// already carries one — so this is idempotent, and a future framework renderer that
/// starts emitting the element itself cannot end up with two.
fn add_request_id(body: &str, request_id: &str) -> Option<String> {
    if body.contains(REQUEST_ID_OPEN) {
        return None;
    }
    let close = body.rfind(ERROR_RESPONSE_CLOSE)?;
    Some(format!(
        "{}<RequestId>{request_id}</RequestId>{}",
        &body[..close],
        &body[close..]
    ))
}

/// The response as it is sent: ONE request id, reported in the `<RequestId>` of its
/// `<ErrorResponse>` envelope and written wherever the document states it
/// ([`substitute_request_id`]).
///
/// The id is the one the response is ALREADY named by, read through the shared rule
/// ([`service_common::query_error_envelope::reported_request_id`]) — the audit wrapper inside
/// this one names every response it answers — and a freshly minted one only where no layer
/// named it. So however many layers report the id, a caller never sees two for one call.
///
/// Status, headers and every other byte of the body are left exactly as the renderer wrote
/// them; a response that needs neither — a success, or an error already carrying the element
/// and stating no id in its message — is returned untouched.
pub fn with_request_id(mut resp: RtResponse) -> RtResponse {
    let request_id = ::service_common::query_error_envelope::reported_request_id(&resp)
        .unwrap_or_else(crate::state::new_request_id);
    let rewritten = {
        // An error envelope is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error envelope and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return resp;
        };
        let stated = substitute_request_id(text, &request_id);
        let document = stated.as_deref().unwrap_or(text);
        match add_request_id(document, &request_id) {
            Some(rewritten) => rewritten,
            // The element is there already (or this is no error envelope): whatever the
            // document stated of the id is still substituted, and nothing else changes.
            None => match stated {
                Some(stated) => stated,
                None => return resp,
            },
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

/// The modelled operation the service's routing table names for this request, or `None`
/// when the request names none.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then adds the envelope's `<RequestId>` to the response.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework still owns the whole
/// deserialize → validate → handle → serialize sequence (and every status, code and
/// message it answers with), and this wrapper owns one element of the envelope.
pub struct RequestIdEnvelopeOperation<Eng: Send + Sync + 'static> {
    engine: std::sync::Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> RequestIdEnvelopeOperation<Eng> {
    /// Build the wrapper from the shared engine — which it holds for one reason: it is the
    /// ONE place every request of this service passes through, so the transitions the CLOCK
    /// has made due whose conclusion needs calls into other services are concluded here,
    /// once, rather than by each handler
    /// ([`crate::state::provisioning::settle_expired_wait_failures`]).
    pub fn new(engine: std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for RequestIdEnvelopeOperation<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // A constraint a LIST's ELEMENTS break is stated at the list member, quoting the
            // list the request sent — which needs the REQUEST, so it is decided here rather
            // than in `Service::override_refusal`
            // ([`crate::service::list_element_refusals`]). The violations are the
            // framework's own, so this cannot refuse a request the framework would accept;
            // every other refusal is left to the framework and to that seam. The definition
            // is read, used and dropped inside the statement, because
            // `&dyn ErasedServiceDefinition` is not `Sync` and holding it across an await
            // below would make this future non-`Send`.
            let stated = crate::service::list_element_refusals::stated(
                svc.definition().protocol(),
                req,
            );
            if let Some((violations, refused)) = stated {
                return svc.definition().refusal_response(
                    ::aws_smithy_schema_server::ProtocolError::Invalid(violations),
                    Some(refused),
                    req,
                );
            }
            match modeled_operation(svc, req) {
                Some(op) => {
                    // A resource-signal wait whose `Timeout` has run out failed its stack's
                    // operation at the instant it expired, and that failure owes a ROLLBACK
                    // whose deletes reach the services that own the stack's resources —
                    // calls neither the store's clock nor a read under its lock can make. So
                    // they are made HERE, before the operation runs: this wrapper is the one
                    // path every request of this service takes, so the expiry is concluded
                    // in one place whichever operation a client calls after it.
                    crate::state::provisioning::settle_expired_wait_failures(
                        ctx,
                        &*self.engine,
                    )
                    .await;
                    with_request_id(op.run(svc, req, ctx).await)
                }
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

/// This service's model-derived half, with the framework-level error renderers wrapped so
/// a routing failure, a transport failure and a pre-dispatch refusal answer in the same
/// envelope every other error does.
///
/// Identity, the routing table and the protocol come from the generated
/// [`crate::ServiceDefinition`] untouched — nothing model-derived is restated here.
///
/// The generated entry is reached through the AUDIT wrapper
/// ([`crate::service::management_events`]), which sits INSIDE this one: the routing table
/// every layer resolves an operation from — the router itself and the operation wrapper
/// above — is then the audited one, so no request can be served by a path that records
/// nothing. Everything else about the definition is the generated entry's, unchanged.
pub struct CloudFormationDefinition<Eng: Send + Sync + 'static> {
    entry: AuditedDefinition<crate::ServiceDefinition<Eng>>,
}

impl<Eng: Send + Sync + 'static> CloudFormationDefinition<Eng> {
    /// Build the definition from the shared engine, which the generated entry threads
    /// into every operation, routing through `audited` — the same modelled operations
    /// built from that same engine, each wrapped in the management-event recorder.
    pub fn new(engine: std::sync::Arc<Eng>, audited: OperationRoutingTable) -> Self {
        Self {
            entry: AuditedDefinition::new(crate::ServiceDefinition::new(engine), audited),
        }
    }

    /// Whether the generated routing table can serve this request — the condition under
    /// which this service claims it for [`RequestIdEnvelopeOperation`].
    pub fn routes(&self, req: &RtRequest) -> bool {
        self.entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.entry.routing_table().get(key))
            .is_some()
    }
}

impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for CloudFormationDefinition<Eng> {
    fn identity(&self) -> &ServiceIdentity {
        self.entry.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.entry.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        with_request_id(self.entry.routing_error_response(req))
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        with_request_id(self.entry.framework_error_response(error, req))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        with_request_id(ErasedServiceDefinition::refusal_response(
            &self.entry,
            error,
            chosen,
            req,
        ))
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.entry.wire_protocol(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The element lands as the last child of the envelope — after `</Error>`, inside
    /// `</ErrorResponse>` — whether or not the root carries the service namespace.
    #[test]
    fn request_id_is_the_last_child_of_the_envelope() {
        for case in [
            "<?xml version=\"1.0\"?><ErrorResponse><Error><Type>Sender</Type>\
             <Code>ValidationError</Code><Message>no</Message></Error></ErrorResponse>",
            "<?xml version=\"1.0\"?><ErrorResponse xmlns=\"ns\"><Error><Type>Sender</Type>\
             <Code>ValidationError</Code><Message>no</Message></Error></ErrorResponse>",
        ] {
            let rewritten =
                add_request_id(case, "11112222-3333-4444-5555-666677778888").expect("rewritten");
            assert!(
                rewritten.ends_with(
                    "</Error><RequestId>11112222-3333-4444-5555-666677778888</RequestId>\
                     </ErrorResponse>"
                ),
                "case {case:?}: expected <RequestId> as the envelope's last child, got \
                 {rewritten:?}"
            );
        }
    }

    /// Nothing to add: a document that already carries a request id, and one that is not
    /// an error envelope at all, are both left alone.
    #[test]
    fn nothing_is_added_twice_and_nothing_else_is_touched() {
        for case in [
            "<ErrorResponse><Error/><RequestId>already</RequestId></ErrorResponse>",
            "<ListStacksResponse><ListStacksResult/><ResponseMetadata><RequestId>a</RequestId>\
             </ResponseMetadata></ListStacksResponse>",
            "<ListStacksResponse><ListStacksResult/></ListStacksResponse>",
        ] {
            assert!(
                add_request_id(case, "fresh").is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }

    /// One id per response: wherever the document states the request id, the id this
    /// response reports is written there — and the substitution reads nothing out of the
    /// document, so caller-echoed text can neither choose it nor keep it.
    #[test]
    fn the_stated_request_id_is_the_one_the_response_reports() {
        let reported = "11112222-3333-4444-5555-666677778888";
        let marker = crate::state::REQUEST_ID_PLACEHOLDER;
        let envelope = format!(
            "<ErrorResponse><Error><Type>Sender</Type><Code>GeneratedTemplateNotFound</Code>\
             <Message>Template with arn t does not exist (Service: Component; Status Code: \
             400; Error Code: SomeException; Request ID: {marker}; Proxy: null)</Message>\
             </Error></ErrorResponse>"
        );
        let substituted =
            substitute_request_id(&envelope, reported).expect("the marker is substituted");
        assert_eq!(
            substituted,
            envelope.replace(marker, reported),
            "case \"a message stating the id\": expected the reported id in its place, got \
             {substituted:?}"
        );
        assert!(
            !substituted.contains(marker),
            "case \"a message stating the id\": expected no marker left, got {substituted:?}"
        );

        // A caller-echoed value that imitates an SDK envelope, or carries the marker itself,
        // cannot make the response report an id of the request's choosing: an echoed id is
        // left as the text it is, and an echoed marker states the response's own id.
        let echoed = format!(
            "<ErrorResponse><Error><Message>Resource scan id x (Service: F; Status Code: 400; \
             Error Code: E; Request ID: 99998888-7777-6666-5555-444433332222; Proxy: null) is \
             invalid, and so is {marker}</Message></Error></ErrorResponse>"
        );
        let substituted =
            substitute_request_id(&echoed, reported).expect("the echoed marker is substituted");
        assert!(
            substituted.contains("Request ID: 99998888-7777-6666-5555-444433332222; Proxy: null)")
                && substituted.contains(&format!("and so is {reported}")),
            "case \"caller-echoed text\": expected the echo left alone and the reported id \
             written at the marker, got {substituted:?}"
        );

        // A document that states no id at all, and a SUCCESS whose body may carry any text
        // a caller stored, are not rewritten by this step.
        for case in [
            "<ErrorResponse><Error><Message>Stack [s] does not exist</Message></Error>\
             </ErrorResponse>",
            "<ErrorResponse><Error><Message>Request ID: 1234; Proxy: null)</Message></Error>\
             </ErrorResponse>",
            "<ListStacksResponse><ListStacksResult/></ListStacksResponse>",
            "<GetTemplateResponse><GetTemplateResult><TemplateBody>{request-id}\
             </TemplateBody></GetTemplateResult></GetTemplateResponse>",
        ] {
            assert_eq!(
                substitute_request_id(case, reported),
                None,
                "case {case:?}: expected no substitution"
            );
        }
    }
}
