// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-signer-wallabyservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! `override_operation` is also where this service states the LENGTH of an answer
//! that carries no body: the operations whose modelled output is `Unit` are served
//! wrapped in [`StatedEmptyLength`], at the one place every response of theirs
//! passes through rather than once per operation. See [`EMPTY_BODY_LENGTH`].
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service,
};
use std::sync::Arc;

/// The length an answer with no body states.
///
/// A response that carries no body still has a length, and the recorded AWS
/// answer to a Signer call whose output carries none states it: a client (or an
/// intermediary) that frames a bodyless response by its stated length rather
/// than by the status alone has nothing to read otherwise. The value is a fact
/// about the body — it has no bytes — so it is stated for every such answer
/// rather than per operation.
///
/// It is the LAST word only up to the transport: the HTTP/1.1 writer this
/// emulator is served by removes `content-length` from a `204` (RFC 9110
/// §15.3.5 forbids it there), and that writer is a dependency of the vendored
/// HTTP layer rather than anything this crate decides — so a caller on the
/// in-process front door (the inter-service callback path and the SDK
/// connector, which both serve `dispatch`'s response unchanged) reads it, while
/// one on a socket reads the status and the empty body alone.
const EMPTY_BODY_LENGTH: &str = "0";

/// The response header that states it.
const CONTENT_LENGTH: &str = "content-length";

/// The operations whose modelled output carries NO body, and whose answers
/// therefore state [`EMPTY_BODY_LENGTH`].
///
/// Source: the three operations this model binds to `smithy.api#Unit`
/// (`smithy_query(query="digest", model_path="services/\
/// service-com-amazonaws-signer-wallabyservice/smithy-model/\
/// com-amazonaws-signer-wallabyservice.json")` — `CancelSigningProfile`,
/// `RevokeSignature` and `RevokeSigningProfile` are the ones whose `output` is
/// `Unit`). Every other operation of this service answers a modelled structure,
/// so its body is a document and its length is the document's.
///
/// The wrapper decides on the ANSWER rather than on the operation, so a response
/// of one of these that does carry a body (a refusal's error document) keeps the
/// length its renderer stated.
macro_rules! frame_empty_answers {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(StatedEmptyLength::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// One modelled operation, served with the length its empty answer has stated on
/// it.
///
/// Everything else about the operation is the operation's: the request walk, the
/// status, the body and every other header come from the wrapped
/// `impl Operation`, and the auth schemes are the wrapped operation's own —
/// wrapping must not change which schemes sign a request.
struct StatedEmptyLength<O> {
    operation: O,
}

impl<O> StatedEmptyLength<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for StatedEmptyLength<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { with_stated_length(self.operation.run(svc, req, ctx).await) })
    }

    /// The wrapped operation's modelled auth schemes.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// `resp` with the length of an EMPTY body stated on it, and anything else left
/// exactly as the operation answered it.
///
/// A body with bytes in it states the length its renderer stated, a header
/// already present is left alone (whoever wrote it first is its one writer), and
/// a streamed body — whose length is not known here — is left to the transport.
/// The insert is FALLIBLE: a header the HTTP layer cannot represent is dropped
/// rather than turned into a transport failure.
fn with_stated_length(mut resp: RtResponse) -> RtResponse {
    let empty_body = resp.body().bytes().is_some_and(<[u8]>::is_empty);
    if empty_body && resp.headers().get(CONTENT_LENGTH).is_none() {
        let _ = resp
            .headers_mut()
            .try_insert(CONTENT_LENGTH, EMPTY_BODY_LENGTH);
    }
    resp
}

/// The operation `GET /revocations` addresses, as the generated routing table keys
/// it (the operation's own name).
const GET_REVOCATION_STATUS: &str = "GetRevocationStatus";

/// The method that route is bound to.
const REVOCATIONS_METHOD: &str = "GET";

/// Its path, as the single path segment it consists of. Compared the way the router
/// compares it: `route_lookup` trims the path's leading and trailing `/` and splits
/// the rest into segments, so `/revocations`, `/revocations/` and `//revocations//`
/// are all that one segment and anything else is another route.
const REVOCATIONS_PATH_SEGMENT: &str = "revocations";

/// Whether `req` addresses the `GET /revocations` route, whatever query parameters
/// it carries.
///
/// Parsed with `http::Uri` rather than by hand, so an absolute-form URI, an
/// origin-form one, and the userinfo/port/fragment variants all yield the same path
/// the router reads. A URI that does not parse addresses no route.
fn addresses_revocation_status(req: &aws_smithy_runtime_api::http::Request) -> bool {
    if req.method() != REVOCATIONS_METHOD {
        return false;
    }
    let Ok(uri) = req.uri().parse::<http::Uri>() else {
        return false;
    };
    uri.path().trim_matches('/') == REVOCATIONS_PATH_SEGMENT
}

/// The `com-amazonaws-signer-wallabyservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The operations whose modelled output carries no body, each wrapped in
    /// [`StatedEmptyLength`] so their empty answer states its length. Keyed by
    /// the routing key the protocol resolves — the key the generated table uses
    /// — and built from the SAME shared engine, so a wrapped operation reads and
    /// writes the one store.
    empty_answers: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut empty_answers = OperationRoutingTable::new();
        frame_empty_answers!(
            empty_answers,
            engine,
            cancel_signing_profile::CancelSigningProfile,
            revoke_signature::RevokeSignature,
            revoke_signing_profile::RevokeSigningProfile,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            empty_answers,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // AWS Signer and the Signer data plane are two emulated services behind ONE
        // SigV4 signing name (`signer`), which the model cannot express. The router
        // resolves a request's routing key from the credential scope FIRST and falls
        // back to the `Host` label, so the key names an ENDPOINT: `signer` for this
        // service (whose endpoint prefix IS the shared signing name, and which every
        // signed request to either endpoint therefore resolves to) and `data-signer`
        // for the peer's own endpoint, which only a request addressed at that host
        // without a credential scope resolves to.
        //
        // So this service claims the signing name's traffic WHOLE, its own
        // `GetRevocationStatus` (`GET /revocations`) included: it holds the records
        // that answer it — `RevokeSigningProfile` and `RevokeSignature` write them —
        // and a request signed for `signer` is one this service serves whichever of
        // the two endpoint hosts it was sent to. The peer
        // (`services/service-com-amazonaws-signerdata-signerdataplane`) claims the key
        // of its OWN endpoint and no longer the shared signing name, so the two claims
        // are disjoint by key and no request has the two claimants the router answers
        // `500 AmbiguousService` for.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // `GetRevocationStatus` binds `GET /revocations` with ALL FIVE of its input
        // members `@required` + `@httpQuery`
        // (`smithy_query(query="operation",operation="GetRevocationStatus")`), so the
        // REST router's required-binding check matches that route only for a request
        // that carries every one of those query parameters. A request leaving one out
        // resolves to NO operation, and the protocol default for that is 501
        // `NotImplemented` — which says nobody implemented the operation, when what
        // actually happened is that the request left a required parameter out.
        //
        // Naming the operation here hands such a request to the operation it
        // addressed, so the centralized walk's own boundary validator refuses it with
        // the modelled 400 naming each absent member. A request that carries them all
        // resolves to this same operation through the routing table, so nothing else
        // changes.
        if addresses_revocation_status(req) {
            return self.entry.routing_table().get(GET_REVOCATION_STATUS);
        }

        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `StatedEmptyLength` so an answer with no body states its length. A key
        // no wrapped operation holds resolves to `None` here, so every other
        // request is served by the generated table unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.empty_answers.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
