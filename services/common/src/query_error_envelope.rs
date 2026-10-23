// SPDX-License-Identifier: CC0-1.0

//! The `<RequestId>` element an `awsQuery` `<ErrorResponse>` carries beside its `<Error>`,
//! for every emulated service whose model speaks that protocol.
//!
//! AWS answers an `awsQuery` error with
//!
//! ```xml
//! <ErrorResponse>
//!   <Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error>
//!   <RequestId>…</RequestId>
//! </ErrorResponse>
//! ```
//!
//! The `<RequestId>` belongs to the ENVELOPE, not to any modelled error shape: no model
//! declares it, and it is what an SDK exposes as the failed call's request id
//! (`ResponseMetadata.RequestId` in botocore), the counterpart of the
//! `ResponseMetadata/RequestId` the framework's SUCCESS renderer already writes. The
//! framework's error renderers write `<Error>` alone
//! (`aws_smithy_schema_server::query_error_body` and the `awsQuery` `serialize_error`), so
//! a service that wants the whole envelope adds the element here — through the two seams
//! below, which cover every layer that can answer an error:
//!
//! * [`RequestIdEnvelopeOperation`], the operation wrapper a service hands back from
//!   `Service::override_operation`, covers a handler-returned modelled error and a
//!   framework refusal raised while a matched operation was being served (an absent
//!   required member, a value outside a declared bound);
//! * [`RequestIdEnvelopeDefinition`], the definition wrapper a service hands back from
//!   `Service::definition`, covers a routing failure (an `Action` naming no modelled
//!   operation), a transport failure detected before any operation was named, and a
//!   pre-dispatch authentication refusal;
//! * a response a service renders ITSELF (an API-version refusal on a shared endpoint,
//!   say) is passed through [`with_request_id`] directly.
//!
//! Only the envelope is this module's business: the HTTP status, the `<Type>`, the
//! `<Code>`, the `<Message>` and the entire success path stay with the framework renderer
//! that owns them, so each has exactly one home. Nothing here is protocol-specific beyond
//! the one element: a document that is not an `awsQuery` `<ErrorResponse>` is returned
//! untouched, so a service of any other protocol can hold the wrapper harmlessly.
//!
//! This is the shared home the service-local copies of this rule record as their migration
//! target — `services/service-com-amazonaws-rds-amazonrdsv19`,
//! `services/service-com-amazonaws-docdb-amazonrdsv19`,
//! `services/service-com-amazonaws-cloudformation-cloudformation`,
//! `services/service-com-amazonaws-sns-amazonsimplenotificationservice`,
//! `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01`,
//! `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice` and
//! `services/service-com-amazonaws-redshift-redshiftserviceversion20121201`, each in that
//! service's own `src/service/query_error_envelope.rs`. This module carries the most
//! complete of those behaviours — the `x-amzn-requestid` header and the sibling-scoped
//! idempotence check that the latest of them (autoscaling, redshift) have, which the
//! earlier ones (rds, docdb, cloudformation, sns) do not — so a service migrating onto it
//! gains those two and loses nothing; performing those migrations belongs to those
//! services' issues, so they are left where they are rather than moved by a change that
//! does not own them. Nothing NEW should state the rule again — call this module.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationRoutingTable,
    ServerHttpProtocol, Service, ServiceIdentity,
};

/// The closing tag of the `awsQuery` error envelope: the point the element is inserted
/// before, and the one marker that tells an error document from a successful one (whose
/// own request id already sits in `<ResponseMetadata>`).
const ERROR_RESPONSE_CLOSE: &str = "</ErrorResponse>";

/// The closing tag of the element the request id is a SIBLING of: what sits between it and
/// [`ERROR_RESPONSE_CLOSE`] is the only place a `<RequestId>` of this ENVELOPE can be.
const ERROR_ELEMENT_CLOSE: &str = "</Error>";

/// The element this module adds.
const REQUEST_ID_OPEN: &str = "<RequestId>";

/// The header AWS reports the same id in, so a client correlating on the header and one
/// reading the document see one value.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// Mint the id one response reports as its own.
///
/// A correlation id, not a security token, and per-response rather than per-resource: two
/// responses must be distinguishable, which is what a client logs and what an SDK's retry
/// and telemetry layers read. A fresh v4 UUID from the maintained `uuid` crate — the same
/// thing the framework's own renderers put in a successful response's
/// `ResponseMetadata/RequestId` (`vendor/smithy-server-runtime/src/pre_dispatch.rs`), so a
/// client cannot tell the two apart. It needs no state, so a request refused before any
/// store is reached still answers with one.
pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form every AWS request id carries — what
/// [`new_request_id`] mints, and the only shape this module will report as one.
fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        let hex = group.len() == width
            && group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !hex {
            return false;
        }
    }
    groups.next().is_none()
}

/// Where the envelope ends, and the span its OWN children occupy.
///
/// The `<RequestId>` belongs to the ENVELOPE, so only the envelope's own children count: a
/// `<RequestId>` a modelled error shape carries inside its `<Error>`, and the text of a
/// `<Message>` the renderer built from the request, are not the envelope's. So the span is
/// what follows the last `</Error>` — and the whole envelope when there is no closing
/// `</Error>` to anchor on (a self-closing `<Error/>`). One reader for both the idempotence
/// decision and the id, so the two cannot disagree about where that span starts.
///
/// `None` for a document that is not an `awsQuery` error envelope at all.
fn envelope_children(body: &str) -> Option<(usize, &str)> {
    let close = body.rfind(ERROR_RESPONSE_CLOSE)?;
    let siblings = match body[..close].rfind(ERROR_ELEMENT_CLOSE) {
        Some(error_end) => &body[error_end..close],
        None => &body[..close],
    };
    Some((close, siblings))
}

/// The `<RequestId>` the envelope ALREADY states, read from the sibling span.
///
/// `None` for a document that is not an `awsQuery` error envelope, for one that states no
/// id of its own, and for a "stated id" that does not have the FORM of one: the span is
/// located textually and renderer-built text is the one part of an error document a
/// request can influence, so nothing but a well-formed id is ever reported as the
/// response's id.
fn envelope_request_id(body: &str) -> Option<&str> {
    let (_, siblings) = envelope_children(body)?;
    let value = siblings.find(REQUEST_ID_OPEN)? + REQUEST_ID_OPEN.len();
    let end = siblings[value..].find("</RequestId>")?;
    let stated = &siblings[value..value + end];
    is_request_id(stated).then_some(stated)
}

/// Add `<RequestId>` to an `awsQuery` `<ErrorResponse>` document, as the last child of the
/// envelope (the sibling after `<Error>`).
///
/// `None` means the document needs nothing: it is not an `<ErrorResponse>` envelope (a
/// successful `<…Response>` carries its request id in `ResponseMetadata` already), or the
/// envelope's own children already hold one — so this is idempotent, and a future framework
/// renderer that starts emitting the element itself cannot end up with two.
fn add_request_id(body: &str, request_id: &str) -> Option<String> {
    let (close, siblings) = envelope_children(body)?;
    if siblings.contains(REQUEST_ID_OPEN) {
        return None;
    }
    Some(format!(
        "{}<RequestId>{request_id}</RequestId>{}",
        &body[..close],
        &body[close..]
    ))
}

/// The response with a freshly minted `<RequestId>` in its `<ErrorResponse>` envelope, and
/// the same id in the header AWS reports it in.
///
/// Status, headers and every other byte of the body are left exactly as the renderer wrote
/// them; a response that is not an `awsQuery` error envelope is returned untouched (and
/// mints nothing), so the success path cannot be affected. An envelope that already holds
/// an id keeps its document, and the header reports THAT id — the two can never name
/// different ids — or carries none when what the document holds is not a well-formed id.
pub fn with_request_id(mut resp: RtResponse) -> RtResponse {
    let (rewritten, request_id) = {
        // An error envelope is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error envelope and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return resp;
        };
        // Not an `awsQuery` error envelope at all: not this module's business, and no id
        // is minted for it.
        if envelope_children(text).is_none() {
            return resp;
        }
        let minted = new_request_id();
        match add_request_id(text, &minted) {
            Some(rewritten) => (Some(rewritten), Some(minted)),
            // The envelope already holds an id: the document stands, and the header
            // reports that same id.
            None => (None, envelope_request_id(text).map(str::to_string)),
        }
    };
    if let Some(rewritten) = rewritten {
        // Only a renderer that advertised a length needs it corrected; the framework's
        // error renderers set `content-type` alone and leave framing to the transport.
        if resp.headers().get("content-length").is_some() {
            let length = rewritten.len().to_string();
            let _ = resp.headers_mut().try_insert("content-length", length);
        }
        *resp.body_mut() = SdkBody::from(rewritten);
    }
    if let Some(request_id) = request_id {
        // Dropped rather than fatal if the HTTP layer cannot carry it: the document already
        // states the id, and a header is not worth failing a modelled error for.
        let _ = resp.headers_mut().try_insert(REQUEST_ID_HEADER, request_id);
    }
    resp
}

/// The modelled operation the service's routing table names for this request, or `None`
/// when the request names none.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let definition = svc.definition();
    let key = definition.protocol().routing_key(req).ok()?;
    definition.routing_table().get(key)
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then adds the envelope's `<RequestId>` to the response.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework still owns the whole
/// deserialize → validate → handle → serialize sequence (and every status, code and
/// message it answers with), and this wrapper owns one element of the envelope.
pub struct RequestIdEnvelopeOperation;

impl ErasedOperation for RequestIdEnvelopeOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match modeled_operation(svc, req) {
                Some(op) => with_request_id(op.run(svc, req, ctx).await),
                // Unreachable through a `Service::override_operation` that claims a
                // request only when the routing table holds its key
                // ([`RequestIdEnvelopeDefinition::routes`]); answered as the framework
                // answers an unroutable request rather than guessed at.
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

/// A service's model-derived half with the framework-level error renderers wrapped, so a
/// routing failure, a transport failure and a pre-dispatch refusal answer in the same
/// envelope every other error of that service does.
///
/// Identity, the routing table and the protocol come from the wrapped definition
/// untouched — nothing model-derived is restated here. Generic over that definition rather
/// than over an engine, so this crate names no service's generated type.
pub struct RequestIdEnvelopeDefinition<D> {
    inner: D,
}

impl<D: ErasedServiceDefinition> RequestIdEnvelopeDefinition<D> {
    /// Wrap a service's generated definition.
    pub fn new(inner: D) -> Self {
        Self { inner }
    }

    /// Whether the generated routing table can serve this request — the condition under
    /// which a service claims it for [`RequestIdEnvelopeOperation`], so an `Action` naming
    /// no modelled operation stays the framework's (or the service's own) routing failure.
    pub fn routes(&self, req: &RtRequest) -> bool {
        self.inner
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.inner.routing_table().get(key))
            .is_some()
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for RequestIdEnvelopeDefinition<D> {
    fn identity(&self) -> &ServiceIdentity {
        self.inner.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.inner.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.inner.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        with_request_id(self.inner.routing_error_response(req))
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
    ) -> RtResponse {
        with_request_id(self.inner.framework_error_response(error))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
    ) -> RtResponse {
        with_request_id(self.inner.refusal_response(error, chosen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The element lands as the last child of the envelope — after `</Error>`, inside
    /// `</ErrorResponse>` — whether or not the root carries a service namespace.
    #[test]
    fn request_id_is_the_last_child_of_the_envelope() {
        for case in [
            "<?xml version=\"1.0\"?><ErrorResponse><Error><Type>Sender</Type>\
             <Code>TargetGroupNotFound</Code><Message>no</Message></Error></ErrorResponse>",
            "<?xml version=\"1.0\"?><ErrorResponse xmlns=\"ns\"><Error><Type>Sender</Type>\
             <Code>TargetGroupNotFound</Code><Message>no</Message></Error></ErrorResponse>",
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

    /// The envelope's own children decide whether an id is there already: an `<Error>`
    /// whose `<Message>` reproduces the element's name — the renderer builds that message
    /// from the request's own action and version — still gets the envelope's id.
    #[test]
    fn text_inside_the_error_does_not_suppress_the_envelope_id() {
        for case in [
            "<ErrorResponse><Error><Type>Sender</Type><Code>InvalidAction</Code>\
             <Message>Could not find operation <RequestId>x</RequestId> for version 1\
             </Message></Error></ErrorResponse>",
            "<ErrorResponse><Error><Code>TargetGroupNotFound</Code>\
             <RequestId>a modelled member</RequestId></Error></ErrorResponse>",
        ] {
            let rewritten =
                add_request_id(case, "11112222-3333-4444-5555-666677778888").expect("rewritten");
            assert!(
                rewritten.ends_with(
                    "</Error><RequestId>11112222-3333-4444-5555-666677778888</RequestId>\
                     </ErrorResponse>"
                ),
                "case {case:?}: expected the envelope's own <RequestId> to be added, got \
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
            "<DescribeTargetGroupsResponse><DescribeTargetGroupsResult/><ResponseMetadata>\
             <RequestId>a</RequestId></ResponseMetadata></DescribeTargetGroupsResponse>",
            "<DescribeTargetGroupsResponse><DescribeTargetGroupsResult/>\
             </DescribeTargetGroupsResponse>",
        ] {
            assert!(
                add_request_id(case, "fresh").is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }

    /// The id the envelope already states is read from the SIBLING position, so the header
    /// written beside such a document reports that id rather than a second one — and text
    /// inside `<Error>` is not mistaken for it.
    #[test]
    fn the_envelopes_own_request_id_is_read_from_the_sibling_position() {
        for (case, body, expected) in [
            (
                "an envelope that states its own id",
                "<ErrorResponse><Error><Code>c</Code></Error>\
                 <RequestId>11112222-3333-4444-5555-666677778888</RequestId></ErrorResponse>",
                Some("11112222-3333-4444-5555-666677778888"),
            ),
            (
                "a sibling value that is not a request id",
                "<ErrorResponse><Error><Code>c</Code></Error><RequestId>not an id</RequestId>\
                 </ErrorResponse>",
                None,
            ),
            (
                "an id inside the error, which is not the envelope's",
                "<ErrorResponse><Error><Code>c</Code><RequestId>a member</RequestId></Error>\
                 </ErrorResponse>",
                None,
            ),
            (
                "a successful document, which is not this module's business",
                "<DescribeTargetGroupsResponse><ResponseMetadata><RequestId>s</RequestId>\
                 </ResponseMetadata></DescribeTargetGroupsResponse>",
                None,
            ),
        ] {
            assert_eq!(
                envelope_request_id(body),
                expected,
                "case {case:?}: expected {expected:?} as the envelope's own id"
            );
        }
    }

    /// The minted id has the shape of an AWS request id, and two of them differ — the
    /// invariant a correlation id carries.
    #[test]
    fn minted_ids_are_well_formed_and_distinct() {
        let ids: Vec<String> = (0..8).map(|_| new_request_id()).collect();
        for id in &ids {
            assert!(
                is_request_id(id),
                "case {id:?}: expected the 8-4-4-4-12 lowercase-hex form of a request id"
            );
        }
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "expected 8 distinct request ids, got {ids:?}"
        );
    }

    /// What the form test accepts and refuses: only the 8-4-4-4-12 lowercase-hex shape,
    /// which is what keeps anything else out of the response's request id.
    #[test]
    fn only_the_request_id_form_is_accepted() {
        for (case, id, expected) in [
            ("the minted form", "11112222-3333-4444-5555-666677778888", true),
            ("every hex digit", "abcdef01-2345-6789-abcd-ef0123456789", true),
            ("uppercase hex", "AAAA2222-3333-4444-5555-666677778888", false),
            ("a non-hex letter", "1111222g-3333-4444-5555-666677778888", false),
            ("a group too short", "1111222-3333-4444-5555-666677778888", false),
            ("a group too long", "111122223-3333-4444-5555-66667777888", false),
            ("a sixth group", "11112222-3333-4444-5555-666677778888-9999", false),
            ("no groups at all", "111122223333444455556666777788889999", false),
            ("prose", "not an id", false),
            ("nothing", "", false),
        ] {
            assert_eq!(
                is_request_id(id),
                expected,
                "case {case:?}: expected is_request_id({id:?}) == {expected}"
            );
        }
    }
}
